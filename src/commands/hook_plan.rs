//! The frozen, approved command plan that closes the approval-boundary TOCTOU.
//!
//! # The invariant
//!
//! `CLAUDE.md` → "Project Commands Run Only After Approval". Project-defined
//! hook commands are arbitrary code shipped in a repo the user may have just
//! cloned; they run only after the approval gate clears them.
//!
//! # Why this type exists
//!
//! Selection (*which* `(source, hook_type, name, template)` tuples run),
//! authorization (project templates ∈ [`Approvals`]) and rendering
//! (template → shell string, needs live git) are three separate concerns.
//! Operation-driven hooks (`pre-merge`, `post-merge`, `pre-remove`,
//! `post-remove`, `post-switch`, `pre-start`, `post-start`) are gated *before*
//! a state mutation (auto-rebase rewrites the feature `.config/wt.toml`; a
//! merge moves the target ref; a removal scrubs the worktree; `git worktree
//! add` materializes a `--create` worktree) and executed *after* it. When
//! selection runs a second time at execution (`load_project_config()` again),
//! the mutated on-disk config can yield an *unapproved* command. On a fresh
//! clone that is remote code execution.
//!
//! The structural fix: **the gate performs selection exactly once and freezes
//! it into an [`ApprovedHookPlan`]. Executors consume only that value.** They
//! hold no [`ProjectConfig`] and call no `load_project_config()` for the
//! covered hook types — re-derivation is a compile error, not a review check.
//! The `Repository` an executor still holds is used only for *rendering*
//! ([`render_planned`] takes the frozen [`CommandConfig`] list, never config),
//! so it cannot re-select.
//!
//! The frozen unit is the *selected, source-tagged [`CommandConfig`] list* per
//! `(HookType, anchor)` — the anchor being the worktree the hook is *about*
//! (its config source). Rendering stays deferred (post-`*` hooks legitimately
//! need post-operation context like the merge commit), but it consumes
//! `&[(HookSource, CommandConfig)]`, so it cannot change the set. The
//! authorization-relevant artifact is the template set, which is exactly what
//! [`Approvals`] stores and the prompt shows.
//!
//! `ApprovedHookPlan` is obtainable *only* via [`HookPlan::approve`] (the
//! interactive / `--yes` gate) or `HookPlan::approve_readonly` (the picker,
//! which can't prompt mid-render; `#[cfg(unix)]`). [`HookPlanBuilder::add`] — the sole
//! config→commands step for the covered hooks — is the only place
//! `load_project_config()`'s result is selected from.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use worktrunk::HookType;
use worktrunk::config::{Approvals, Command, CommandConfig, ProjectConfig, UserConfig};
use worktrunk::git::add_hook_skip_hint;

use super::command_approval::approve_command_batch;
use super::command_executor::{
    CommandContext, FailureStrategy, PipelineKind, execute_pipeline_foreground, prepare_steps,
};
use super::hook_announcement::SourcedStep;
use super::hook_filter::HookSource;
use super::hooks::{
    HookAnnouncer, into_source_groups, lookup_hook_configs, sourced_steps_to_foreground,
};
use super::project_config::{ApprovableCommand, Phase};

/// One `(hook_type, anchor)`'s frozen, source-tagged selection.
///
/// `User` entries precede `Project` so the flat render order matches the
/// existing background grouping (a user-hook failure must not abort project
/// hooks). Each `CommandConfig` is an owned clone — frozen data, no config
/// handle to re-resolve from.
type Selection = Vec<(HookSource, CommandConfig)>;

/// One frozen entry: the hook type, the anchor worktree (the `.config/wt.toml`
/// the hook reads — stored as the caller provides it; the gate and executor
/// both derive it from the same value, so equality holds without
/// canonicalization), and that pair's source-tagged selection.
struct PlanEntry {
    hook_type: HookType,
    anchor: PathBuf,
    selection: Selection,
}

/// A selected-but-not-yet-authorized plan. Built only by [`HookPlanBuilder`].
pub struct HookPlan {
    entries: Vec<PlanEntry>,
}

/// Accumulates per-anchor selections from each worktree's resolved config.
///
/// `add` is the only place `load_project_config()`'s result feeds command
/// selection for the covered hook types — `pub(crate)` and called only from
/// command gates.
pub struct HookPlanBuilder {
    entries: Vec<PlanEntry>,
}

impl HookPlanBuilder {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Select `hook_types` anchored at `anchor`, from the already-resolved
    /// `project_config` (the gate snapshots / `git show`s it) plus user config.
    ///
    /// `project_id` scopes the user-config hook lookup. Source identity is
    /// preserved so source-scoped behavior survives into execution.
    pub fn add(
        &mut self,
        anchor: &Path,
        hook_types: &[HookType],
        project_config: Option<&ProjectConfig>,
        user: &UserConfig,
        project_id: Option<&str>,
    ) -> &mut Self {
        let user_hooks = user.hooks(project_id);
        for &hook_type in hook_types {
            let (user_cfg, proj_cfg) = lookup_hook_configs(&user_hooks, project_config, hook_type);
            let mut selection: Selection = Vec::new();
            if let Some(cfg) = user_cfg {
                selection.push((HookSource::User, cfg.clone()));
            }
            if let Some(cfg) = proj_cfg {
                selection.push((HookSource::Project, cfg.clone()));
            }
            if selection.is_empty() {
                continue;
            }
            match self
                .entries
                .iter_mut()
                .find(|e| e.hook_type == hook_type && e.anchor == anchor)
            {
                Some(e) => {
                    // A repeated `(hook_type, anchor)` add must not interleave
                    // sources: `into_source_groups` requires User entries
                    // contiguous before Project ones. Stable sort by
                    // `HookSource` (User < Project) keeps that structurally,
                    // not by caller convention.
                    e.selection.extend(selection);
                    e.selection.sort_by_key(|(source, _)| *source);
                }
                None => self.entries.push(PlanEntry {
                    hook_type,
                    anchor: anchor.to_path_buf(),
                    selection,
                }),
            }
        }
        self
    }

    pub fn finish(self) -> HookPlan {
        HookPlan {
            entries: self.entries,
        }
    }
}

impl Default for HookPlanBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl HookPlan {
    /// The project-source templates the prompt must show, deduped by template
    /// so the same command across several anchors prompts once (the common
    /// case: every removal in a batch lands in the same primary worktree).
    fn approvable(&self) -> Vec<ApprovableCommand> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for entry in &self.entries {
            for (source, cfg) in &entry.selection {
                if *source != HookSource::Project {
                    continue;
                }
                for cmd in cfg.commands() {
                    if seen.insert(cmd.template.clone()) {
                        out.push(ApprovableCommand {
                            phase: Phase::Hook(entry.hook_type),
                            command: Command::new(cmd.name.clone(), cmd.template.clone()),
                        });
                    }
                }
            }
        }
        out
    }

    /// Interactive / `--yes` gate. Reuses [`approve_command_batch`] so the
    /// prompt, the `--yes` path, and the saved approvals are byte-identical to
    /// before. `Ok(None)` means the user declined — the caller prints its own
    /// "continuing without hooks" message and proceeds with
    /// [`ApprovedHookPlan::empty`].
    ///
    /// When no project-source command needs the gate (no project config, or
    /// only user hooks), this returns the frozen plan **without** loading
    /// `Approvals` or requiring `project_id`. That keeps a malformed
    /// `approvals.toml` or an unresolvable project identifier from aborting a
    /// command that has nothing to authorize, matching the pre-plan behaviour
    /// where the empty-batch fast path ran before any approval state was
    /// touched. `project_id` is therefore `Option`: it is consulted only when
    /// there is something to approve, where it must be present.
    pub fn approve(
        self,
        project_id: Option<&str>,
        yes: bool,
    ) -> anyhow::Result<Option<ApprovedHookPlan>> {
        let approvable = self.approvable();
        if approvable.is_empty() {
            return Ok(Some(ApprovedHookPlan {
                entries: self.entries,
            }));
        }
        let project_id =
            project_id.context("project identifier is required to approve project commands")?;
        let approvals = Approvals::load().context("Failed to load approvals")?;
        let approved = approve_command_batch(&approvable, project_id, &approvals, yes, false)?;
        if !approved {
            return Ok(None);
        }
        Ok(Some(ApprovedHookPlan {
            entries: self.entries,
        }))
    }

    /// The picker can't prompt mid-render, so it runs only the already-approved
    /// project pipelines and silently drops the rest (keeping user pipelines).
    /// Strictly the CLAUDE.md "consult the approval state read-only and run
    /// only the already-approved commands, skipping the rest" rule. An absent
    /// `project_id` (unresolvable identifier) drops every project pipeline —
    /// fail-closed, never run unapproved.
    #[cfg(unix)]
    pub fn approve_readonly(
        self,
        approvals: &Approvals,
        project_id: Option<&str>,
    ) -> ApprovedHookPlan {
        let mut entries = self.entries;
        for entry in &mut entries {
            entry.selection.retain(|(source, cfg)| {
                *source != HookSource::Project
                    || project_id.is_some_and(|pid| {
                        cfg.commands()
                            .all(|c| approvals.is_command_approved(pid, &c.template))
                    })
            });
        }
        entries.retain(|e| !e.selection.is_empty());
        ApprovedHookPlan { entries }
    }
}

/// The only value executors accept. Constructible solely via
/// [`HookPlan::approve`] / `HookPlan::approve_readonly` / [`Self::empty`].
/// Holds no `Repository` / `ProjectConfig`, only the frozen selection.
pub struct ApprovedHookPlan {
    entries: Vec<PlanEntry>,
}

impl ApprovedHookPlan {
    /// No hooks: `--no-hooks`, declined approval, or no project config. Every
    /// covered executor consumes this and runs nothing.
    pub fn empty() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// The frozen selection for `(hook_type, anchor)`, by exact match.
    ///
    /// Every covered gate anchors at the identical path its executor passes:
    /// the `RemoveResult` worktree/destination path threaded unchanged, the
    /// `SwitchPlan`/`SwitchResult` destination, or the merge feature root
    /// (`current_worktree().root()` on the same `Repository`, cached, so the
    /// gate and `finish_after_merge` produce the same `PathBuf`). The invariant
    /// is gate-anchor == executor-anchor; this method does not paper over a
    /// violation with a "sole entry" guess that would silently run a different
    /// worktree's hook. A miss returns `&[]` — no hooks run, which surfaces
    /// visibly rather than as a wrong-worktree execution.
    fn lookup(&self, hook_type: HookType, anchor: &Path) -> &[(HookSource, CommandConfig)] {
        self.entries
            .iter()
            .find(|e| e.hook_type == hook_type && e.anchor == anchor)
            .map(|e| e.selection.as_slice())
            .unwrap_or(&[])
    }
}

/// Render the frozen selection into source-tagged steps. Takes the frozen
/// `CommandConfig` list, never config — re-selection is impossible. `ctx`'s
/// `Repository` is used only for template expansion.
fn render_planned(
    entries: &[(HookSource, CommandConfig)],
    ctx: &CommandContext<'_>,
    extra_vars: &[(&str, &str)],
    hook_type: HookType,
) -> anyhow::Result<Vec<SourcedStep>> {
    let mut out = Vec::new();
    for (source, cfg) in entries {
        let is_pipeline = cfg.is_pipeline();
        for step in prepare_steps(cfg, ctx, extra_vars, hook_type, *source)? {
            out.push(SourcedStep {
                step,
                source: *source,
                is_pipeline,
            });
        }
    }
    Ok(out)
}

/// Foreground execution of a covered hook from the approved plan.
///
/// Replaces `execute_hook` for `pre-merge` / `pre-remove` / `pre-start`. The
/// signature carries no config — only the plan, the render context, and the
/// failure strategy.
///
/// `anchor` is the worktree the hook's config was selected from at the gate
/// (usually `ctx.worktree_path`, but for `post-remove` the removed worktree
/// while `ctx` runs in the destination). It is *not* a config handle — just
/// the lookup key into the frozen plan.
pub fn execute_planned_hook(
    plan: &ApprovedHookPlan,
    anchor: &Path,
    ctx: &CommandContext<'_>,
    hook_type: HookType,
    extra_vars: &[(&str, &str)],
    failure_strategy: FailureStrategy,
    display_path: Option<&Path>,
) -> anyhow::Result<()> {
    let sourced = render_planned(plan.lookup(hook_type, anchor), ctx, extra_vars, hook_type)?;
    if sourced.is_empty() {
        return Ok(());
    }
    let kind = PipelineKind::Hook {
        hook_type,
        display_path: display_path.map(Path::to_path_buf),
    };
    let foreground = sourced_steps_to_foreground(sourced, &kind);
    execute_pipeline_foreground(&foreground, ctx.repo, ctx.worktree_path, failure_strategy)
        .map_err(add_hook_skip_hint)
}

/// Register a covered background hook (`post-merge` / `post-remove` /
/// `post-switch` / `post-start`) from the approved plan onto `announcer`.
///
/// The plan-backed counterpart of `HookAnnouncer::register`: steps are
/// rendered from the frozen selection and added via the announcer's existing
/// config-free `extend`, with no `load_project_config()` at execution.
///
/// `anchor` is the gate's config-source worktree (see [`execute_planned_hook`]).
pub fn register_planned(
    announcer: &mut HookAnnouncer<'_>,
    plan: &ApprovedHookPlan,
    anchor: &Path,
    ctx: &CommandContext<'_>,
    hook_type: HookType,
    extra_vars: &[(&str, &str)],
    display_path: Option<&Path>,
) -> anyhow::Result<()> {
    let sourced = render_planned(plan.lookup(hook_type, anchor), ctx, extra_vars, hook_type)
        .context("failed to render planned hooks")?;
    let dp = display_path.map(Path::to_path_buf);
    announcer.extend(
        into_source_groups(sourced)
            .into_iter()
            .map(|g| (*ctx, hook_type, dp.clone(), g)),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project_cfg(toml: &str) -> ProjectConfig {
        toml::from_str(toml).unwrap()
    }

    /// The plan freezes the selection at build time: once approved, the
    /// authorized command set is fixed and the executor's `lookup` returns it
    /// from the frozen value alone — there is no `Repository`/`ProjectConfig`
    /// on `ApprovedHookPlan` to re-derive a (mutated) command from. This is the
    /// structural property that closes the TOCTOU class.
    #[test]
    fn approved_plan_lookup_is_frozen_and_anchor_scoped() {
        let user = UserConfig::default();
        let gate_cfg = project_cfg(r#"post-merge = "echo approved""#);

        let mut builder = HookPlanBuilder::new();
        builder.add(
            Path::new("/dest"),
            &[HookType::PostMerge],
            Some(&gate_cfg),
            &user,
            None,
        );
        // `--yes` ⇒ approved without writing approvals.
        let plan = builder
            .finish()
            .approve(Some("proj"), true)
            .unwrap()
            .expect("yes-approval never declines");

        // The frozen selection is exactly the gate config's command. A later
        // on-disk mutation is unrepresentable here: `lookup` consults only the
        // plan (no config handle in scope).
        let sel = plan.lookup(HookType::PostMerge, Path::new("/dest"));
        assert_eq!(sel.len(), 1);
        let (source, cfg) = &sel[0];
        assert_eq!(*source, HookSource::Project);
        let templates: Vec<_> = cfg.commands().map(|c| c.template.as_str()).collect();
        assert_eq!(templates, vec!["echo approved"]);

        // A hook type that was never planned yields nothing — the executor
        // cannot conjure one from config it doesn't hold.
        assert!(
            plan.lookup(HookType::PreMerge, Path::new("/dest"))
                .is_empty()
        );
    }

    /// The picker's read-only gate keeps user pipelines but drops project
    /// pipelines whose templates aren't already approved (no prompt).
    #[cfg(unix)]
    #[test]
    fn approve_readonly_drops_unapproved_project_keeps_user() {
        let user = UserConfig {
            hooks: toml::from_str(r#"pre-remove = "echo user-hook""#).unwrap(),
            ..UserConfig::default()
        };
        let proj = project_cfg(r#"pre-remove = "echo project-hook""#);

        let mut builder = HookPlanBuilder::new();
        builder.add(
            Path::new("/wt"),
            &[HookType::PreRemove],
            Some(&proj),
            &user,
            None,
        );
        let plan = builder
            .finish()
            // Empty approvals ⇒ the project pipeline is unapproved.
            .approve_readonly(&Approvals::default(), Some("proj"));

        let sel = plan.lookup(HookType::PreRemove, Path::new("/wt"));
        assert_eq!(
            sel.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            vec![HookSource::User],
            "unapproved project pipeline dropped, user pipeline kept"
        );

        // With the project command approved, it survives the read-only gate.
        // A tempdir-backed approvals path keeps the write off the real user
        // config dir, which the nix build sandbox makes unwritable.
        let temp_dir = tempfile::tempdir().unwrap();
        let approvals_path = temp_dir.path().join("approvals.toml");
        let mut approvals = Approvals::default();
        approvals
            .approve_command(
                "proj".to_string(),
                "echo project-hook".to_string(),
                Some(&approvals_path),
            )
            .unwrap();
        let mut builder = HookPlanBuilder::new();
        builder.add(
            Path::new("/wt"),
            &[HookType::PreRemove],
            Some(&proj),
            &user,
            None,
        );
        let plan = builder.finish().approve_readonly(&approvals, Some("proj"));
        let sel = plan.lookup(HookType::PreRemove, Path::new("/wt"));
        assert_eq!(
            sel.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            vec![HookSource::User, HookSource::Project],
        );
    }

    /// Repeated `(hook_type, anchor)` adds must keep User entries contiguous
    /// before Project ones — `into_source_groups` (a user-hook failure must
    /// not abort project hooks) depends on it. Structural via the stable sort
    /// in `add`, not caller convention.
    #[test]
    fn duplicate_add_keeps_sources_grouped() {
        let user = UserConfig {
            hooks: toml::from_str(r#"post-merge = "echo u""#).unwrap(),
            ..UserConfig::default()
        };
        let proj = project_cfg(r#"post-merge = "echo p""#);

        let mut builder = HookPlanBuilder::new();
        // Same (PostMerge, /dest) added twice — the interleave-prone path.
        builder.add(
            Path::new("/dest"),
            &[HookType::PostMerge],
            Some(&proj),
            &user,
            None,
        );
        builder.add(
            Path::new("/dest"),
            &[HookType::PostMerge],
            Some(&proj),
            &user,
            None,
        );
        let plan = builder
            .finish()
            .approve(Some("proj"), true)
            .unwrap()
            .unwrap();
        let sources: Vec<_> = plan
            .lookup(HookType::PostMerge, Path::new("/dest"))
            .iter()
            .map(|(s, _)| *s)
            .collect();
        // All User before all Project, never interleaved.
        let first_project = sources.iter().position(|s| *s == HookSource::Project);
        assert!(
            first_project.is_none_or(|i| sources[i..].iter().all(|s| *s == HookSource::Project)),
            "sources interleaved: {sources:?}"
        );
    }
}
