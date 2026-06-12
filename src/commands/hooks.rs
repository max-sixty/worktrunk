//! Hook config loading, step preparation, and execution.
//!
//! See [`super::hook_announcement`] for the announcement format / grammar that
//! the background path emits before spawning pipelines.
//!
//! # Which `.config/wt.toml` a hook reads
//!
//! Every hook resolves its commands from the **invoking** worktree's
//! `.config/wt.toml` — the worktree `wt` ran in, read from its working tree.
//! That holds regardless of which worktree the hook is *about*: `post-merge`
//! runs in the merge target and `post-start` in the newly created worktree,
//! but both select their commands from the invoking worktree's config — the
//! same file `wt config show` reads.
//!
//! Two execution models split on whether a state mutation separates the
//! approval gate from execution.
//!
//! **Plan-backed (the TOCTOU-covered set):** `pre-merge`, `post-merge`,
//! `pre-remove`, `post-remove`, `post-switch`, `pre-start`, `post-start`. A
//! merge, rebase, removal, or `git worktree add` runs between the gate and
//! these hooks; a rebase can even rewrite the invoking worktree's own
//! `.config/wt.toml`, so a second config read could select a command the user
//! never approved. Each command gate calls `load_project_config()` on the
//! invoking worktree once, selects the commands, and freezes them into a
//! [`super::hook_plan::ApprovedHookPlan`]; the executor renders and runs only
//! that frozen value via [`super::hook_plan::execute_planned_hook`] /
//! [`super::hook_plan::register_planned`], holding no `ProjectConfig` to
//! re-derive from. See the [`super::hook_plan`] module spec.
//!
//! | Plan-backed hook | Runs in (the anchor) | Gate |
//! |---|---|---|
//! | `pre-merge`, `pre-remove`, `post-remove` | the feature/removed worktree | `merge::approve_merge_plan`, `main.rs`'s `approve_remove`, `step::prune::approve_prune_hooks` |
//! | `post-merge`, `post-switch` (after a removal) | the merge/removal destination | the same gates |
//! | `pre-start`, `post-start`, `post-switch` (on switch) | the new/destination worktree | `worktree::switch::approve_switch_hooks` |
//!
//! "Runs in" is the *anchor* — the executor's plan lookup key and render root,
//! not a config source. A `pre-start`'s new worktree need not exist when the
//! gate runs; the config came from the invoking worktree regardless.
//!
//! **Invocation-resolved (no gate→exec mutation):** `pre-commit`,
//! `post-commit`, `pre-switch`, `wt hook <type>`, aliases. They resolve config
//! from `ctx.repo.load_project_config()` at invocation via [`execute_hook`] /
//! [`HookAnnouncer::register`]. Two facts make that re-read safe, and a new
//! call site must preserve **both**: (1) nothing between the gate and the
//! executor mutates the worktree `.config/wt.toml`; (2) the executor reads
//! through the **same `Repository` instance** the gate used —
//! `load_project_config` reads the working-tree file and memoizes it in a
//! never-invalidated `OnceCell` (`RepoCache::project_config`), so the
//! executor's call is a cache hit returning the gate's exact bytes. A
//! refactor that runs an uncovered executor through a fresh `Repository::at()`
//! (empty cache) breaks (2) and silently reintroduces the TOCTOU even if (1)
//! still holds — there is no compile-time guard here, unlike the plan-backed
//! set. (Aliases get the property structurally instead: the body is frozen
//! into `AliasEntry` before the gate, like `ApprovedHookPlan`.)
//!
//! `ctx.repo` is the invoking worktree — except `wt step commit --branch <b>`
//! and `wt -C <path>` re-root the whole command (the commit, its hooks, and
//! `ctx.repo` are all `<b>`), so "the invoking worktree" follows them.
//!
//! A present-but-malformed config aborts the operation rather than silently
//! running something else. `WORKTRUNK_PROJECT_CONFIG_PATH` overrides the path
//! (test isolation); user config (`~/.config/worktrunk/config.toml`) is global
//! and unaffected.

use std::path::{Path, PathBuf};

use anyhow::Context;
use color_print::cformat;
use worktrunk::HookType;
use worktrunk::config::{CommandConfig, format_hook_variables};
use worktrunk::git::{Repository, add_hook_skip_hint};
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{
    eprintln, format_with_gutter, info_message, progress_message, verbosity, warning_message,
};

use super::command_executor::{
    CommandContext, FailureStrategy, ForegroundStep, PipelineKind, PreparedCommand, PreparedStep,
    alias_error_wrapper, execute_pipeline_foreground, hook_error_wrapper, prepare_steps,
};
use super::hook_announcement::{SourcedStep, format_pipeline_summary};
use crate::commands::process::{HookLog, spawn_detached_exec};
use crate::output::DirectivePassthrough;

// Re-export for backward compatibility with existing imports
pub use super::hook_filter::{HookSource, ParsedFilter};

/// Prepare hook steps from both user and project configs, preserving pipeline
/// structure, and verify any name filter matched at least one command.
/// Returns the steps (possibly empty if no hooks are configured).
///
/// Collects steps from user config first, then project config, applying the
/// name filter to individual commands within each step. The filter supports
/// source prefixes: `user:foo` or `project:foo` to run only from one source.
///
/// Shared by [`run_hooks_foreground`] and the dry-run branch of `run_hook`
/// (in `hook_commands.rs`) so both paths produce the same "no commands
/// matched" error when a filter mismatches. `run_post_hook`'s filter path
/// additionally relies on the error as a guarantee: `Ok` under a non-empty
/// filter implies non-empty steps, which [`HookAnnouncer::add_groups`]
/// requires.
pub(crate) fn prepare_and_check(
    ctx: &CommandContext,
    user_config: Option<&CommandConfig>,
    project_config: Option<&CommandConfig>,
    hook_type: HookType,
    extra_vars: &[(&str, &str)],
    name_filters: &[String],
) -> anyhow::Result<Vec<SourcedStep>> {
    let parsed_filters: Vec<ParsedFilter<'_>> = name_filters
        .iter()
        .map(|f| ParsedFilter::parse(f))
        .collect();

    let mut result = Vec::new();

    let sources = [
        (HookSource::User, user_config),
        (HookSource::Project, project_config),
    ];

    for (source, config) in sources {
        let Some(config) = config else { continue };

        if !parsed_filters.is_empty() && !parsed_filters.iter().any(|f| f.matches_source(source)) {
            continue;
        }

        let is_pipeline = config.is_pipeline();
        let steps = prepare_steps(config, ctx, extra_vars, hook_type, source)?;
        for step in steps {
            if let Some(filtered) = filter_step_by_name(step, source, &parsed_filters) {
                result.push(SourcedStep {
                    step: filtered,
                    source,
                    is_pipeline,
                });
            }
        }
    }

    // Every surviving step keeps at least one command, so an empty result
    // under a non-empty filter means nothing matched.
    if !name_filters.is_empty() && result.is_empty() {
        return Err(no_matching_commands_error(
            name_filters,
            user_config,
            project_config,
        ));
    }

    Ok(result)
}

/// Filter commands within a step by name. Returns `None` if all commands were
/// filtered out. A `Concurrent` group reduced to one command collapses to `Single`.
fn filter_step_by_name(
    step: PreparedStep,
    source: HookSource,
    parsed_filters: &[ParsedFilter<'_>],
) -> Option<PreparedStep> {
    if parsed_filters.is_empty() {
        return Some(step);
    }

    let matches = |cmd: &PreparedCommand| {
        parsed_filters
            .iter()
            .any(|f| f.matches_command(source, cmd.name.as_deref()))
    };

    match step {
        PreparedStep::Single(cmd) => matches(&cmd).then_some(PreparedStep::Single(cmd)),
        PreparedStep::Concurrent(cmds) => {
            let mut kept: Vec<_> = cmds.into_iter().filter(matches).collect();
            match kept.len() {
                0 => None,
                1 => Some(PreparedStep::Single(kept.pop().unwrap())),
                _ => Some(PreparedStep::Concurrent(kept)),
            }
        }
    }
}

/// Build the error for a name filter that matched no commands, listing the
/// available command names across the filters' source scopes.
fn no_matching_commands_error(
    name_filters: &[String],
    user_config: Option<&CommandConfig>,
    project_config: Option<&CommandConfig>,
) -> anyhow::Error {
    // Show the combined filter string in the error
    let filter_display = name_filters.join(", ");

    let parsed_filters: Vec<ParsedFilter<'_>> = name_filters
        .iter()
        .map(|f| ParsedFilter::parse(f))
        .collect();
    let mut available = Vec::new();

    let sources = [
        (HookSource::User, user_config),
        (HookSource::Project, project_config),
    ];
    for (source, config) in sources {
        let Some(config) = config else { continue };
        // Include this source if any filter matches it
        if !parsed_filters.iter().any(|f| f.matches_source(source)) {
            continue;
        }
        available.extend(
            config
                .commands()
                .filter_map(|c| c.name.as_ref().map(|n| format!("{source}:{n}"))),
        );
    }

    worktrunk::git::GitError::HookCommandNotFound {
        name: filter_display,
        available,
    }
    .into()
}

/// Coordinates background hook announcements within a single `wt` command.
///
/// The principle: one `◎ Running …` line per command. Sites that would have
/// individually spawned background hooks instead `register` their pipelines
/// here, and the command `flush`es once after all phases have been resolved —
/// every registered hook type ends up on the same combined announce line, in
/// registration order, with `;` between clauses (see module spec).
///
/// Just-in-time honesty: registration happens at each phase's normal moment,
/// so if an earlier phase fails (e.g. pre-merge errors before post-merge is
/// queued), the announce only describes hooks that actually ran. The line is
/// emitted at `flush`, not at registration time.
pub struct HookAnnouncer<'a> {
    pending: Vec<PendingPipeline>,
    repo: &'a Repository,
    show_branch: bool,
}

/// One registered background hook pipeline, owned so the announcer can outlive
/// any short-lived `CommandContext`s at the registration site. Background flow
/// only ever sees hook pipelines, so `hook_type` and `display_path` live on
/// this struct directly rather than wrapped in a `PipelineKind` variant.
/// `steps` is non-empty by construction (see [`HookAnnouncer::add_groups`]).
struct PendingPipeline {
    worktree_path: PathBuf,
    branch: Option<String>,
    hook_type: HookType,
    display_path: Option<PathBuf>,
    steps: Vec<SourcedStep>,
}

impl<'a> HookAnnouncer<'a> {
    /// `show_branch=true` includes the branch name for disambiguation in batch
    /// contexts (e.g., prune removing multiple worktrees):
    /// `Running post-remove for feature: docs (user); post-switch for feature: zellij-tab (user)`
    pub fn new(repo: &'a Repository, show_branch: bool) -> Self {
        Self {
            pending: Vec::new(),
            repo,
            show_branch,
        }
    }

    /// Add one pipeline per step group, all sharing `hook_type` and
    /// `display_path` and running in `ctx`'s worktree.
    ///
    /// Groups come from [`into_source_groups`] (one pipeline per source — the
    /// canonical shape) or as a single merged group (`wt hook`'s filter path,
    /// where the user cherry-picked names across sources). Callers never pass
    /// empty groups.
    pub fn add_groups(
        &mut self,
        ctx: &CommandContext<'_>,
        hook_type: HookType,
        display_path: Option<&Path>,
        groups: Vec<Vec<SourcedStep>>,
    ) {
        for steps in groups {
            debug_assert!(!steps.is_empty(), "add_groups: empty step group");
            self.pending.push(PendingPipeline {
                worktree_path: ctx.worktree_path.to_path_buf(),
                branch: ctx.branch.map(String::from),
                hook_type,
                display_path: display_path.map(Path::to_path_buf),
                steps,
            });
        }
    }

    /// Prepare and add user+project pipelines for a single hook type, reading
    /// the project config from `ctx.repo.load_project_config()` — the worktree
    /// this hook acts on. Used by the invocation-resolved hooks
    /// (`pre-commit` / `post-commit` and `wt hook <type>`'s no-filter path),
    /// where nothing mutates config between approval and execution. The
    /// plan-backed hooks use [`super::hook_plan::register_planned`] instead.
    ///
    /// Each source's steps become an independent pipeline so a user hook
    /// failure doesn't abort project hooks.
    pub fn register(
        &mut self,
        ctx: &CommandContext<'_>,
        hook_type: HookType,
        extra_vars: &[(&str, &str)],
        display_path: Option<&Path>,
    ) -> anyhow::Result<()> {
        let project_config = ctx.repo.load_project_config()?;
        let user_hooks = ctx.config.hooks(ctx.project_id().as_deref());
        let flat = prepare_and_check(
            ctx,
            user_hooks.get(hook_type),
            project_config.as_ref().and_then(|c| c.hooks.get(hook_type)),
            hook_type,
            extra_vars,
            &[],
        )?;
        self.add_groups(ctx, hook_type, display_path, into_source_groups(flat));
        Ok(())
    }

    /// Emit the combined announce line and spawn all registered pipelines.
    ///
    /// No-op when nothing was registered. Drains `pending` so the announcer
    /// can be reused (though one-per-command is the intended pattern).
    pub fn flush(&mut self) -> anyhow::Result<()> {
        let pending = std::mem::take(&mut self.pending);
        if pending.is_empty() {
            return Ok(());
        }
        run_hooks_background(self.repo, pending, self.show_branch)
    }
}

impl Drop for HookAnnouncer<'_> {
    /// Best-effort flush on drop so hooks registered before an early-return
    /// error still spawn. Without this, a `register` error in a later phase
    /// would silently swallow earlier-registered pipelines — a regression
    /// from the prior fire-and-forget pattern. On the success path,
    /// `flush()` runs explicitly first and `pending` is empty here.
    fn drop(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        if let Err(err) = self.flush() {
            eprintln!(
                "{}",
                warning_message(format!("Failed to spawn pending hooks: {err:#}"))
            );
        }
    }
}

/// Announce and spawn background hook pipelines.
///
/// Module-private implementation of [`HookAnnouncer::flush`] — sites construct
/// a [`HookAnnouncer`] (one per command) and `register`/`add_groups` into it;
/// `flush` lands here. The function stays separate to keep the announce/spawn
/// formatting isolated from the announcer's pending-list bookkeeping.
///
/// Emits a single combined `Running ...` line covering every registered hook
/// type (see module-level grammar), with the path suffix at the end. Pipelines
/// may carry different worktrees (e.g., post-remove uses the removed branch
/// while post-switch uses the destination branch); the announce shares one
/// line because every bundled clause shares the same path.
///
/// When `show_branch` is true, the announce includes the branch name for
/// disambiguation in batch contexts (e.g., prune removing multiple worktrees):
/// `Running post-remove for feature: user: docs`.
fn run_hooks_background(
    repo: &Repository,
    pipelines: Vec<PendingPipeline>,
    show_branch: bool,
) -> anyhow::Result<()> {
    // Merge per-source summaries by hook type so user+project for the same
    // type render as one clause: `post-merge: sync, push (user); build (project)`.
    // Pull `display_path` off the first pipeline that has one — every hook
    // pipeline in this batch shares a path; the path slot is bundle-wide.
    let mut display_path: Option<&Path> = None;
    let mut type_summaries: Vec<(HookType, Vec<String>)> = Vec::new();
    for pipeline in &pipelines {
        if display_path.is_none() {
            display_path = pipeline.display_path.as_deref();
        }
        let summary = format_pipeline_summary(&pipeline.steps);
        if let Some(entry) = type_summaries
            .iter_mut()
            .find(|(ht, _)| *ht == pipeline.hook_type)
        {
            entry.1.push(summary);
        } else {
            type_summaries.push((pipeline.hook_type, vec![summary]));
        }
    }

    // In batch contexts (prune), use the first pipeline's branch for disambiguation.
    // This is the removed branch — it identifies the triggering event even for
    // post-switch hooks that fire as a consequence of the removal.
    let branch_suffix = if show_branch {
        pipelines
            .first()
            .and_then(|p| p.branch.as_deref())
            .map(|b| cformat!(" for <bold>{b}</>"))
    } else {
        None
    };

    if verbosity() >= 1 {
        for (ht, _) in &type_summaries {
            print_background_variable_table(&pipelines, *ht);
        }
    }
    let suffix = branch_suffix.as_deref().unwrap_or("");
    let combined: String = type_summaries
        .iter()
        .map(|(ht, summaries)| format!("{ht}{suffix}: {}", summaries.join("; ")))
        .collect::<Vec<_>>()
        .join("; ");
    let message = match display_path {
        Some(path) => {
            let path_display = format_path_for_display(path);
            cformat!("Running {combined} @ <bold>{path_display}</>")
        }
        None => format!("Running {combined}"),
    };
    eprintln!("{}", progress_message(message));

    for pipeline in &pipelines {
        spawn_hook_pipeline_quiet(repo, pipeline)?;
    }

    Ok(())
}

/// Group sourced steps into one Vec per source, preserving insertion order.
///
/// Used by background callers that want one pipeline per source (the canonical
/// shape — a user hook failure shouldn't abort project hooks). The flat input
/// already lists user steps before project steps, so contiguous chunks suffice.
pub(crate) fn into_source_groups(flat: Vec<SourcedStep>) -> Vec<Vec<SourcedStep>> {
    let mut groups: Vec<Vec<SourcedStep>> = Vec::new();
    for step in flat {
        match groups.last_mut() {
            Some(g) if g.last().is_some_and(|s| s.source == step.source) => g.push(step),
            _ => groups.push(vec![step]),
        }
    }
    groups
}

/// Emit a `template variables:` block for one hook type, using the first
/// matching pipeline's first step context.
///
/// Background hooks don't flow through `announce_command` (which prints the
/// table in the foreground path), so this is the symmetric entry point.
/// Called once per hook type from `run_hooks_background`, immediately before
/// that hook type's `Running ...` line, so each hook type reads as one block.
fn print_background_variable_table(pipelines: &[PendingPipeline], hook_type: HookType) {
    for pipeline in pipelines {
        if pipeline.hook_type != hook_type {
            continue;
        }
        // Pipelines carry non-empty steps by construction — `steps[0]` is safe.
        let cmd = match &pipeline.steps[0].step {
            PreparedStep::Single(cmd) => cmd,
            PreparedStep::Concurrent(cmds) => &cmds[0],
        };
        eprintln!("{}", info_message("template variables:"));
        eprintln!(
            "{}",
            format_with_gutter(&format_hook_variables(hook_type, &cmd.context), None)
        );
        return;
    }
}

/// Spawn a hook pipeline without displaying a summary line.
///
/// Used by `run_hooks_background` after the combined announcement is printed.
fn spawn_hook_pipeline_quiet(repo: &Repository, pipeline: &PendingPipeline) -> anyhow::Result<()> {
    use super::pipeline_spec::{PipelineCommandSpec, PipelineSpec, PipelineStepSpec};

    // Extract base context from the first command. Registration never adds an
    // empty step group (asserted in `add_groups`), so `steps[0]` is safe;
    // every step's first command carries the same base context (only
    // `hook_name` differs per step — strip it so the runner re-injects per
    // step).
    let steps = &pipeline.steps;
    let source = steps[0].source;
    let first_cmd = match &steps[0].step {
        PreparedStep::Single(cmd) => cmd,
        PreparedStep::Concurrent(cmds) => &cmds[0],
    };
    let mut context = first_cmd.context.clone();
    context.remove("hook_name");

    // Build pipeline spec from prepared steps. The runner renders each raw
    // template when its step runs (see `run_pipeline`'s "Template freshness").
    let spec_steps: Vec<PipelineStepSpec> = steps
        .iter()
        .map(|s| match &s.step {
            PreparedStep::Single(cmd) => PipelineStepSpec::Single {
                name: cmd.name.clone(),
                template_name: cmd.template_name.clone(),
                template: cmd.template.clone(),
            },
            PreparedStep::Concurrent(cmds) => PipelineStepSpec::Concurrent {
                commands: cmds
                    .iter()
                    .map(|c| PipelineCommandSpec {
                        name: c.name.clone(),
                        template_name: c.template_name.clone(),
                        template: c.template.clone(),
                    })
                    .collect(),
            },
        })
        .collect();

    // "HEAD" fallback matches `CommandContext::branch_or_head` for detached HEAD.
    let branch = pipeline.branch.as_deref().unwrap_or("HEAD");
    let hook_type = pipeline.hook_type;
    let spec = PipelineSpec {
        worktree_path: pipeline.worktree_path.clone(),
        branch: branch.to_string(),
        hook_type,
        source,
        context,
        steps: spec_steps,
        log_dir: repo.wt_logs_dir(),
    };

    let spec_json = serde_json::to_vec(&spec).context("failed to serialize pipeline spec")?;

    let wt_bin = std::env::current_exe().context("failed to resolve wt binary path")?;

    let hook_log = HookLog::hook(source, hook_type, "runner");
    let log_label = format!("{hook_type} {source} runner");

    if let Err(err) = spawn_detached_exec(
        repo,
        &pipeline.worktree_path,
        &wt_bin,
        &["hook", "run-pipeline"],
        branch,
        &hook_log,
        &spec_json,
    ) {
        eprintln!(
            "{}",
            warning_message(format!("Failed to spawn pipeline: {err:#}"))
        );
    } else {
        let cmd_display = format!("{} hook run-pipeline", wt_bin.display());
        worktrunk::command_log::log_command(&log_label, &cmd_display, None, None);
    }

    Ok(())
}

/// Convert source-tagged steps into foreground steps with pipeline-kind policy.
///
/// Shared between hook and alias dispatch. The `kind` argument supplies the
/// per-call-site policy (announce style, stdin handling, error wrapping) while
/// the `source` field on each step drives the per-step trust model
/// (`DirectivePassthrough`).
///
/// Trust model:
/// - User-source alias steps pass EXEC through. The body lives in the user's
///   own config, so a nested `wt switch --execute …` is no different from the
///   user typing it at the top level. See issue #2101.
/// - Project-source alias steps and all hook steps scrub EXEC (they can still
///   emit CD directives via `inherit_from_env`).
///
/// In a merged user+project alias body, the user's steps still get the
/// relaxation — the decision is per-step, so the project side scrubbing
/// doesn't bleed back into the user steps.
pub(crate) fn sourced_steps_to_foreground(
    sourced_steps: Vec<SourcedStep>,
    kind: &PipelineKind,
) -> Vec<ForegroundStep> {
    sourced_steps
        .into_iter()
        .map(|sourced| {
            let directives = match (kind, sourced.source) {
                (PipelineKind::Alias { .. }, HookSource::User) => {
                    DirectivePassthrough::inherit_from_env_with_exec()
                }
                _ => DirectivePassthrough::inherit_from_env(),
            };
            let (pipe_stdin, redirect_stdout_to_stderr, error_wrapper) = match kind {
                PipelineKind::Hook { hook_type, .. } => {
                    (true, true, hook_error_wrapper(*hook_type))
                }
                PipelineKind::Alias { name } => (false, false, alias_error_wrapper(name.clone())),
            };
            ForegroundStep {
                step: sourced.step,
                concurrent: sourced.is_pipeline,
                announce: kind.clone(),
                pipe_stdin,
                redirect_stdout_to_stderr,
                error_wrapper,
                directives,
            }
        })
        .collect()
}

/// Run user and project hooks for a given hook type in the foreground.
///
/// Used directly only by the `wt hook <type>` path, which intentionally
/// leaves errors unwrapped (the user explicitly asked for the hooks; the
/// `--no-hooks` hint would be misleading). Operation-driven callers should go
/// through [`execute_hook`] which auto-looks-up configs and adds the skip
/// hint.
///
/// The announcement shows the worktree path when commands run somewhere other
/// than the user's cwd ([`crate::output::pre_hook_display_path`] — pre-hooks
/// and manual `wt hook` invocations both run with the user still at cwd).
pub(crate) fn run_hooks_foreground(
    ctx: &CommandContext,
    user_config: Option<&CommandConfig>,
    project_config: Option<&CommandConfig>,
    hook_type: HookType,
    extra_vars: &[(&str, &str)],
    name_filters: &[String],
    failure_strategy: FailureStrategy,
) -> anyhow::Result<()> {
    let sourced_steps = prepare_and_check(
        ctx,
        user_config,
        project_config,
        hook_type,
        extra_vars,
        name_filters,
    )?;

    if sourced_steps.is_empty() {
        return Ok(());
    }

    let kind = PipelineKind::Hook {
        hook_type,
        display_path: crate::output::pre_hook_display_path(ctx.worktree_path)
            .map(Path::to_path_buf),
    };
    let foreground_steps = sourced_steps_to_foreground(sourced_steps, &kind);

    execute_pipeline_foreground(
        &foreground_steps,
        ctx.repo,
        ctx.worktree_path,
        failure_strategy,
    )
}

/// Run a single hook type in the foreground for an operation (merge, switch,
/// commit, remove, …).
///
/// Auto-loads project config, looks up the per-type configs, runs the hooks,
/// and tags failures with `add_hook_skip_hint` so the user sees the
/// `--no-hooks` reminder. This is the canonical operation-driven entry point;
/// the only path that should bypass it is `wt hook <type>` (which calls
/// [`run_hooks_foreground`] directly so failures don't carry the hint).
///
/// Project config comes from `ctx.repo.load_project_config()` — see the
/// module-level "Which `.config/wt.toml` a hook reads" docs for which worktree
/// `ctx.repo` is rooted at per hook type.
pub(crate) fn execute_hook(
    ctx: &CommandContext,
    hook_type: HookType,
    extra_vars: &[(&str, &str)],
    failure_strategy: FailureStrategy,
) -> anyhow::Result<()> {
    let project_config = ctx.repo.load_project_config()?;
    let user_hooks = ctx.config.hooks(ctx.project_id().as_deref());
    run_hooks_foreground(
        ctx,
        user_hooks.get(hook_type),
        project_config.as_ref().and_then(|c| c.hooks.get(hook_type)),
        hook_type,
        extra_vars,
        &[],
        failure_strategy,
    )
    .map_err(add_hook_skip_hint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hook_source_display() {
        assert_eq!(HookSource::User.to_string(), "user");
        assert_eq!(HookSource::Project.to_string(), "project");
    }

    #[test]
    fn test_failure_strategy_copy() {
        let strategy = FailureStrategy::FailFast;
        let copied = strategy; // Copy trait
        assert!(matches!(copied, FailureStrategy::FailFast));

        let warn = FailureStrategy::Warn;
        let copied_warn = warn;
        assert!(matches!(copied_warn, FailureStrategy::Warn));
    }

    #[test]
    fn test_parsed_filter() {
        // No prefix — matches all sources
        let f = ParsedFilter::parse("foo");
        assert!(f.source.is_none());
        assert_eq!(f.name, "foo");
        assert!(f.matches_source(HookSource::User));
        assert!(f.matches_source(HookSource::Project));

        // user: prefix
        let f = ParsedFilter::parse("user:foo");
        assert_eq!(f.source, Some(HookSource::User));
        assert_eq!(f.name, "foo");
        assert!(f.matches_source(HookSource::User));
        assert!(!f.matches_source(HookSource::Project));

        // project: prefix
        let f = ParsedFilter::parse("project:bar");
        assert_eq!(f.source, Some(HookSource::Project));
        assert_eq!(f.name, "bar");
        assert!(!f.matches_source(HookSource::User));
        assert!(f.matches_source(HookSource::Project));

        // Unknown prefix treated as name (colon in name)
        let f = ParsedFilter::parse("my:hook");
        assert!(f.source.is_none());
        assert_eq!(f.name, "my:hook");

        // Source-only (empty name matches all hooks from source)
        let f = ParsedFilter::parse("user:");
        assert_eq!(f.source, Some(HookSource::User));
        assert_eq!(f.name, "");
        let f = ParsedFilter::parse("project:");
        assert_eq!(f.source, Some(HookSource::Project));
        assert_eq!(f.name, "");
    }

    #[test]
    fn test_is_pipeline() {
        use worktrunk::config::CommandConfig;

        let single = CommandConfig::single("npm install");
        assert!(!single.is_pipeline());
    }
}
