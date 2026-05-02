//! # Hook announcement format
//!
//! Background hook execution emits a single `Running ...` line covering every
//! hook type that fires from one event. Separators form a precedence hierarchy
//! from tightest to loosest:
//!
//! ```text
//! &  : concurrent commands within a step          (tightest)
//! ,  : serial steps within a pipeline
//! ;  : source pipelines within a hook type, AND   (loosest)
//!      hook-type clauses within an announce
//! ```
//!
//! `;` is overloaded across the two outermost tiers. The reader disambiguates
//! by lookahead — `;` followed by a `<hook-type>:` (e.g. `post-start:`) is a
//! cross-type boundary; otherwise it's a cross-source boundary within the
//! current hook-type clause. In practice the two often coexist on one line.
//!
//! ## Grammar
//!
//! ```text
//! Running <clauses> [@ <path>]
//!
//! <clauses>     := <hook-clause> ("; " <hook-clause>)*
//! <hook-clause> := <hook-type> [" for " <branch>] ": " <pipelines>
//! <pipelines>   := <pipeline> | <labeled> ("; " <labeled>)*
//! <labeled>     := <pipeline> " (" <source> ")"       # all-named or mixed
//!               |  <source> [" ×" N]                  # all-unnamed (no parens)
//! <pipeline>    := <step> (", " <step>)*
//! <step>        := <command> (" & " <command>)*
//! <command>     := <name>                             # named
//!               |  "…"                                # unnamed run of 1
//!               |  "…×" N                             # unnamed run of N≥2
//! ```
//!
//! The source label appears **once per pipeline** as a suffix annotation
//! (`sync, push (user)`), not once per command. The all-unnamed degenerate
//! case has no body to attach the suffix to, so it stays bare (`user` or
//! `user ×N`).
//!
//! ## Examples
//!
//! | Pipeline shape (single source) | Output |
//! |---|---|
//! | one named command | `notify (user)` |
//! | two serial named | `sync, push (user)` |
//! | one concurrent step | `build & lint (user)` |
//! | serial then concurrent | `install, build & lint (user)` |
//! | mixed unnamed + named | `…, bg (user)` (or `…×2, bg (user)`) |
//! | all unnamed | `user ×2` |
//!
//! Multi-source for one hook type: `sync, push (user); build (project)`.
//!
//! Multi-type bundle (e.g., `wt merge` firing four hook types):
//! `Running post-commit: mark (user); post-remove: cleanup (user); post-switch: notify (user); post-merge: sync (user) @ ~/repo`.
//!
//! ## Implementation
//!
//! - [`format_pipeline_summary_from_names`] produces the bare `<pipeline>`
//!   body (handles `&` / `,` and unnamed-flush). Shared with alias announces.
//! - [`format_pipeline_summary`] wraps it with the per-pipeline source label,
//!   collapsing the all-unnamed case to the bare `<source>` / `<source> ×N`
//!   form.
//! - [`run_hooks_background`] joins source summaries with `;` within each
//!   hook-type clause, then joins clauses with `;` into one announce line
//!   with the path suffix at the end.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use color_print::cformat;
use worktrunk::HookType;
use worktrunk::config::{CommandConfig, UserConfig, format_hook_variables};
use worktrunk::git::Repository;
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{
    eprintln, format_with_gutter, info_message, progress_message, verbosity, warning_message,
};

use super::command_executor::{
    AnnouncePolicy, CommandContext, FailureStrategy, ForegroundStep, PipelineKind, PreparedCommand,
    PreparedStep, alias_error_wrapper, execute_pipeline_foreground, hook_error_wrapper,
    prepare_steps,
};
use crate::commands::process::{HookLog, spawn_detached_exec};
use crate::output::DirectivePassthrough;

// Re-export for backward compatibility with existing imports
pub use super::hook_filter::{HookSource, ParsedFilter};

/// Shared hook selection and rendering inputs for preparation/execution.
#[derive(Clone, Copy)]
pub struct HookCommandSpec<'cfg, 'vars, 'name, 'path> {
    pub user_config: Option<&'cfg CommandConfig>,
    pub project_config: Option<&'cfg CommandConfig>,
    pub hook_type: HookType,
    pub extra_vars: &'vars [(&'vars str, &'vars str)],
    pub name_filters: &'name [String],
    pub display_path: Option<&'path Path>,
}

/// Prepare hook steps from both user and project configs, preserving pipeline structure.
///
/// Collects steps from user config first, then project config, applying the name filter
/// to individual commands within each step. The filter supports source prefixes:
/// `user:foo` or `project:foo` to run only from one source.
///
/// `display_path`: When `Some`, the path is shown in hook announcements (e.g., "@ ~/repo").
/// Use this when commands run in a different directory than where the user invoked the command.
fn prepare_sourced_steps(
    ctx: &CommandContext,
    spec: HookCommandSpec<'_, '_, '_, '_>,
) -> anyhow::Result<Vec<SourcedStep>> {
    let HookCommandSpec {
        user_config,
        project_config,
        hook_type,
        extra_vars,
        name_filters,
        display_path: _,
    } = spec;

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
            if let Some(filtered) = filter_step_by_name(step, &parsed_filters) {
                result.push(SourcedStep {
                    step: filtered,
                    source,
                    is_pipeline,
                });
            }
        }
    }

    Ok(result)
}

/// Filter commands within a step by name. Returns `None` if all commands were
/// filtered out. A `Concurrent` group reduced to one command collapses to `Single`.
fn filter_step_by_name(
    step: PreparedStep,
    parsed_filters: &[ParsedFilter<'_>],
) -> Option<PreparedStep> {
    if parsed_filters.is_empty() {
        return Some(step);
    }
    let filter_names: Vec<&str> = parsed_filters
        .iter()
        .map(|f| f.name)
        .filter(|n| !n.is_empty())
        .collect();
    if filter_names.is_empty() {
        return Some(step);
    }

    let matches = |cmd: &PreparedCommand| {
        cmd.name
            .as_deref()
            .is_some_and(|n| filter_names.contains(&n))
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

/// Count total commands across all sourced steps (for `check_name_filter_matched`).
fn count_sourced_commands(steps: &[SourcedStep]) -> usize {
    steps
        .iter()
        .map(|s| match &s.step {
            PreparedStep::Single(_) => 1,
            PreparedStep::Concurrent(cmds) => cmds.len(),
        })
        .sum()
}

/// A pipeline step with source information, for pipeline-aware execution.
///
/// Used by both hook and alias dispatch as the source-tagged shape that feeds
/// `sourced_steps_to_foreground`. Per-pipeline metadata (`hook_type`,
/// `display_path` for hooks; `name` for aliases) lives on `PipelineKind`,
/// supplied at conversion time so this struct stays neutral. The two fields
/// here are genuinely per-step: alias flows mix steps from both sources into
/// one flat vec, and a config using `[[hook.x]]` on one side with `[hook.x]`
/// on the other can produce mixed `is_pipeline` values within a single hook
/// run.
pub struct SourcedStep {
    pub step: PreparedStep,
    pub source: HookSource,
    /// Whether `Concurrent` steps run concurrently. For hooks: derived from
    /// `is_pipeline()` (deprecated single-table form runs serially). For
    /// aliases: always true — no deprecated form.
    pub is_pipeline: bool,
}

/// One background hook pipeline, carrying its source group of steps with the
/// announce-time metadata (`hook_type`, `display_path`) already resolved.
/// Background flow only ever sees hooks, so the metadata is unwrapped here
/// rather than threaded through `PipelineKind`.
pub(crate) type BackgroundPipeline<'c> = (
    CommandContext<'c>,
    HookType,
    Option<PathBuf>,
    Vec<SourcedStep>,
);

/// Extract the per-step command name lists from a `CommandConfig`.
///
/// Shared by the formatters that describe alias / hook pipelines — `Single`
/// steps become one-element inner vecs, `Concurrent` steps become multi-element
/// vecs, each slot carrying the optional command name. Feeds directly into
/// [`format_pipeline_summary_from_names`].
pub(crate) fn step_names_from_config(
    cfg: &worktrunk::config::CommandConfig,
) -> Vec<Vec<Option<&str>>> {
    cfg.steps()
        .iter()
        .map(|step| match step {
            worktrunk::config::HookStep::Single(cmd) => vec![cmd.name.as_deref()],
            worktrunk::config::HookStep::Concurrent(cmds) => {
                cmds.iter().map(|c| c.name.as_deref()).collect()
            }
        })
        .collect()
}

/// Format the bare `<pipeline>` body from per-step command names — see the
/// module-level grammar.
///
/// `step_names[i]` is the list of commands in step `i`; `Some(name)` for named
/// commands, `None` for unnamed. Serial steps join with `, `; concurrent
/// commands within a step join with ` & `. Contiguous runs of unnamed commands
/// (across steps, until the next named command) collapse into a single
/// `label_unnamed(count)` entry; return `None` from that closure to drop
/// unnamed commands entirely.
///
/// Shared by hook and alias announcements. The caller wraps the body with any
/// surrounding context (source prefix for hooks, alias name for aliases).
///
/// Note: unnamed commands within a `Concurrent` step aren't reachable from
/// config today — TOML named tables always produce all-named commands, and
/// anonymous strings only appear as `Single` steps. The unnamed-flush logic
/// therefore only fires across step boundaries in practice.
pub(crate) fn format_pipeline_summary_from_names(
    step_names: &[Vec<Option<&str>>],
    label_named: impl Fn(&str) -> String,
    label_unnamed: impl Fn(usize) -> Option<String>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut unnamed_count: usize = 0;

    for step in step_names {
        let mut named = Vec::new();
        for entry in step {
            match entry {
                Some(name) => named.push(label_named(name)),
                None => unnamed_count += 1,
            }
        }

        if !named.is_empty() {
            // Flush any pending unnamed count before named labels.
            if unnamed_count > 0
                && let Some(s) = label_unnamed(unnamed_count)
            {
                parts.push(s);
            }
            unnamed_count = 0;
            parts.push(named.join(" & "));
        }
    }

    // Flush trailing unnamed count.
    if unnamed_count > 0
        && let Some(s) = label_unnamed(unnamed_count)
    {
        parts.push(s);
    }

    parts.join(", ")
}

/// Format a `<labeled>` source pipeline for the announce line — see the
/// module-level grammar.
///
/// Returns `<body> (<source>)` for named/mixed pipelines, or the bare
/// `<source>` / `<source> ×N` form when every command is unnamed (no body to
/// attach the suffix to). Unnamed runs inside a mixed pipeline render as `…`
/// (1) or `…×N` (≥2).
fn format_pipeline_summary(steps: &[SourcedStep]) -> String {
    // All steps in a group share the same source.
    let source_label = steps[0].source.to_string();

    let step_names: Vec<Vec<Option<&str>>> = steps
        .iter()
        .map(|step| match &step.step {
            PreparedStep::Single(cmd) => vec![cmd.name.as_deref()],
            PreparedStep::Concurrent(cmds) => cmds.iter().map(|c| c.name.as_deref()).collect(),
        })
        .collect();

    let total_unnamed: usize = step_names.iter().flatten().filter(|n| n.is_none()).count();
    let any_named = step_names.iter().flatten().any(|n| n.is_some());

    // All-unnamed degenerate case: no names to list, so skip the colon.
    if !any_named {
        return if total_unnamed == 1 {
            source_label
        } else {
            format!("{source_label} ×{total_unnamed}")
        };
    }

    let body = format_pipeline_summary_from_names(
        &step_names,
        |name| cformat!("<bold>{name}</>"),
        |count| {
            Some(if count == 1 {
                "…".to_string()
            } else {
                format!("…×{count}")
            })
        },
    );
    format!("{body} ({source_label})")
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
    /// Shared config used to rebuild `CommandContext`s at flush time.
    config: &'a UserConfig,
    show_branch: bool,
}

/// Owned spawn data for a registered pipeline. Stores enough to rebuild a
/// `CommandContext` at flush time so the announcer can outlive any short-lived
/// contexts at the registration site. Background flow only ever sees hook
/// pipelines, so `hook_type` and `display_path` live on this struct directly
/// rather than wrapped in a `PipelineKind` variant.
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
    pub fn new(repo: &'a Repository, config: &'a UserConfig, show_branch: bool) -> Self {
        Self {
            pending: Vec::new(),
            repo,
            config,
            show_branch,
        }
    }

    /// Add prepared pipelines to the announcer.
    ///
    /// Pipelines come from `prepare_background_pipelines` / its callers, which
    /// already filter out empty source groups — empty `steps` is unreachable.
    /// Each pipeline's hook type and display path are passed alongside the
    /// steps; background flow doesn't need `PipelineKind` since it only ever
    /// handles hooks (aliases run in the foreground).
    pub fn extend<'b, I>(&mut self, pipelines: I)
    where
        I: IntoIterator<Item = BackgroundPipeline<'b>>,
    {
        for (ctx, hook_type, display_path, steps) in pipelines {
            self.pending.push(PendingPipeline {
                worktree_path: ctx.worktree_path.to_path_buf(),
                branch: ctx.branch.map(String::from),
                hook_type,
                display_path,
                steps,
            });
        }
    }

    /// Prepare and add user+project pipelines for a single hook type.
    pub fn register(
        &mut self,
        ctx: &CommandContext<'_>,
        hook_type: HookType,
        extra_vars: &[(&str, &str)],
        display_path: Option<&Path>,
    ) -> anyhow::Result<()> {
        let pipelines = prepare_background_pipelines(ctx, hook_type, extra_vars, display_path)?;
        self.extend(pipelines);
        Ok(())
    }

    /// Emit the combined announce line and spawn all registered pipelines.
    ///
    /// No-op when nothing was registered. Drains `pending` so the announcer
    /// can be reused (though one-per-command is the intended pattern).
    pub fn flush(&mut self) -> anyhow::Result<()> {
        let mut pending = std::mem::take(&mut self.pending);
        if pending.is_empty() {
            return Ok(());
        }

        // Borrow `worktree_path` / `branch` from each `pending` slot for
        // CommandContext while moving `steps` out via `mem::take`. `pending`
        // outlives `pipelines`, so the borrow checker is satisfied.
        let pipelines: Vec<_> = pending
            .iter_mut()
            .map(|p| {
                (
                    CommandContext::new(
                        self.repo,
                        self.config,
                        p.branch.as_deref(),
                        &p.worktree_path,
                        false,
                    ),
                    p.hook_type,
                    p.display_path.clone(),
                    std::mem::take(&mut p.steps),
                )
            })
            .collect();

        run_hooks_background(pipelines, self.show_branch)
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
/// a [`HookAnnouncer`] (one per command) and `register`/`extend` into it;
/// `flush` lands here. The function stays separate to keep the announce/spawn
/// formatting isolated from the announcer's pending-list bookkeeping.
///
/// Emits a single combined `Running ...` line covering every registered hook
/// type (see module-level grammar), with the path suffix at the end. Pipelines
/// may carry different `CommandContext`s (e.g., post-remove uses the removed
/// branch while post-switch uses the destination branch); the announce shares
/// one line because every bundled clause shares the same path.
///
/// When `show_branch` is true, the announce includes the branch name for
/// disambiguation in batch contexts (e.g., prune removing multiple worktrees):
/// `Running post-remove for feature: user: docs`.
fn run_hooks_background(
    pipelines: Vec<BackgroundPipeline<'_>>,
    show_branch: bool,
) -> anyhow::Result<()> {
    let pipelines: Vec<_> = pipelines
        .into_iter()
        .filter(|(_, _, _, steps)| !steps.is_empty())
        .collect();
    if pipelines.is_empty() {
        return Ok(());
    }

    // Merge per-source summaries by hook type so user+project for the same
    // type render as one clause: `post-merge: sync, push (user); build (project)`.
    // Pull `display_path` off the first pipeline that has one — every hook
    // pipeline in this batch shares a path; the path slot is bundle-wide.
    let mut display_path: Option<&Path> = None;
    let mut type_summaries: Vec<(HookType, Vec<String>)> = Vec::new();
    for (_, hook_type, dp, group) in &pipelines {
        if display_path.is_none() {
            display_path = dp.as_deref();
        }
        let summary = format_pipeline_summary(group);
        if let Some(entry) = type_summaries.iter_mut().find(|(ht, _)| ht == hook_type) {
            entry.1.push(summary);
        } else {
            type_summaries.push((*hook_type, vec![summary]));
        }
    }

    // In batch contexts (prune), use the first pipeline's branch for disambiguation.
    // This is the removed branch — it identifies the triggering event even for
    // post-switch hooks that fire as a consequence of the removal.
    let branch_suffix = if show_branch {
        pipelines
            .first()
            .and_then(|(ctx, _, _, _)| ctx.branch)
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

    for (ctx, hook_type, _, group) in pipelines {
        spawn_hook_pipeline_quiet(&ctx, hook_type, group)?;
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

/// Prepare a single hook type's background pipelines for one context.
///
/// Looks up user/project configs, prepares + name-checks steps, and groups
/// them by source so each source spawns as an independent pipeline. Each
/// group is returned with its `(hook_type, display_path)` metadata.
pub(crate) fn prepare_background_pipelines<'c>(
    ctx: &CommandContext<'c>,
    hook_type: HookType,
    extra_vars: &[(&str, &str)],
    display_path: Option<&Path>,
) -> anyhow::Result<Vec<BackgroundPipeline<'c>>> {
    let project_config = ctx.repo.load_project_config()?;
    let user_hooks = ctx.config.hooks(ctx.project_id().as_deref());
    let (user_config, proj_config) =
        lookup_hook_configs(&user_hooks, project_config.as_ref(), hook_type);
    let flat = prepare_and_check(
        ctx,
        HookCommandSpec {
            user_config,
            project_config: proj_config,
            hook_type,
            extra_vars,
            name_filters: &[],
            display_path,
        },
    )?;
    let display_path_owned = display_path.map(|p| p.to_path_buf());
    Ok(into_source_groups(flat)
        .into_iter()
        .map(|g| (*ctx, hook_type, display_path_owned.clone(), g))
        .collect())
}

/// Emit a `template variables:` block for one hook type, using the first
/// matching pipeline's first step context.
///
/// Background hooks don't flow through `announce_command` (which prints the
/// table in the foreground path), so this is the symmetric entry point.
/// Called once per hook type from `run_hooks_background`, immediately before
/// that hook type's `Running ...` line, so each hook type reads as one block.
fn print_background_variable_table(pipelines: &[BackgroundPipeline<'_>], hook_type: HookType) {
    for (_, ht, _, group) in pipelines {
        if *ht != hook_type {
            continue;
        }
        // `into_source_groups` produces non-empty groups, and
        // `run_hooks_background` filters empties — `group[0]` is safe.
        let cmd = match &group[0].step {
            PreparedStep::Single(cmd) => cmd,
            PreparedStep::Concurrent(cmds) => &cmds[0],
        };
        let ctx: HashMap<String, String> = serde_json::from_str(&cmd.context_json)
            .expect("context_json is always serialized from a HashMap<String, String>");
        eprintln!("{}", info_message("template variables:"));
        eprintln!(
            "{}",
            format_with_gutter(&format_hook_variables(hook_type, &ctx), None)
        );
        return;
    }
}

/// Spawn a hook pipeline without displaying a summary line.
///
/// Used by `run_hooks_background` after the combined announcement is printed.
fn spawn_hook_pipeline_quiet(
    ctx: &CommandContext,
    hook_type: HookType,
    steps: Vec<SourcedStep>,
) -> anyhow::Result<()> {
    use super::pipeline_spec::{PipelineCommandSpec, PipelineSpec, PipelineStepSpec};

    let source = steps[0].source;

    // Extract base context from the first command. Both call sites
    // (`run_hooks_background` and `run_post_hook`'s filter path) skip empty
    // step lists before reaching here, so `steps[0]` is safe; every step's
    // first command carries the same base context (only `hook_name` differs
    // per step — strip it so the runner re-injects per step).
    debug_assert!(!steps.is_empty(), "spawn_hook_pipeline_quiet: empty steps");
    let first_cmd = match &steps[0].step {
        PreparedStep::Single(cmd) => cmd,
        PreparedStep::Concurrent(cmds) => &cmds[0],
    };
    let mut context: std::collections::HashMap<String, String> =
        serde_json::from_str(&first_cmd.context_json)
            .context("failed to deserialize context_json")?;
    context.remove("hook_name");

    // Build pipeline spec from prepared steps. Use the raw template for lazy
    // steps (vars-referencing) and the expanded command for eager steps.
    let spec_steps: Vec<PipelineStepSpec> = steps
        .iter()
        .map(|s| match &s.step {
            PreparedStep::Single(cmd) => PipelineStepSpec::Single {
                name: cmd.name.clone(),
                template: cmd.lazy_template.as_ref().unwrap_or(&cmd.expanded).clone(),
            },
            PreparedStep::Concurrent(cmds) => PipelineStepSpec::Concurrent {
                commands: cmds
                    .iter()
                    .map(|c| PipelineCommandSpec {
                        name: c.name.clone(),
                        template: c.lazy_template.as_ref().unwrap_or(&c.expanded).clone(),
                    })
                    .collect(),
            },
        })
        .collect();

    let spec = PipelineSpec {
        worktree_path: ctx.worktree_path.to_path_buf(),
        branch: ctx.branch_or_head().to_string(),
        hook_type,
        source,
        context,
        steps: spec_steps,
        log_dir: ctx.repo.wt_logs_dir(),
    };

    let spec_json = serde_json::to_vec(&spec).context("failed to serialize pipeline spec")?;

    let wt_bin = std::env::current_exe().context("failed to resolve wt binary path")?;

    let hook_log = HookLog::hook(source, hook_type, "runner");
    let log_label = format!("{hook_type} {source} runner");

    if let Err(err) = spawn_detached_exec(
        ctx.repo,
        ctx.worktree_path,
        &wt_bin,
        &["hook", "run-pipeline"],
        ctx.branch_or_head(),
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

/// Check if name filters were provided but no commands matched.
/// Returns an error listing available command names if so.
fn check_name_filter_matched(
    name_filters: &[String],
    total_commands_run: usize,
    user_config: Option<&CommandConfig>,
    project_config: Option<&CommandConfig>,
) -> anyhow::Result<()> {
    if !name_filters.is_empty() && total_commands_run == 0 {
        // Show the combined filter string in the error
        let filter_display = name_filters.join(", ");

        // Use the first filter to determine source scope for available commands,
        // but collect across all filters' source scopes
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

        return Err(worktrunk::git::GitError::HookCommandNotFound {
            name: filter_display,
            available,
        }
        .into());
    }
    Ok(())
}

/// Prepare sourced steps and verify any name filter matched at least one
/// command. Returns the steps (possibly empty if no hooks are configured).
///
/// Shared by [`run_hooks_foreground`] and the dry-run branch of `run_hook`
/// (in `hook_commands.rs`) so both paths produce the same "no commands
/// matched" error when a filter mismatches.
pub(crate) fn prepare_and_check(
    ctx: &CommandContext,
    spec: HookCommandSpec<'_, '_, '_, '_>,
) -> anyhow::Result<Vec<SourcedStep>> {
    let HookCommandSpec {
        user_config,
        project_config,
        name_filters,
        ..
    } = spec;
    let sourced_steps = prepare_sourced_steps(ctx, spec)?;
    check_name_filter_matched(
        name_filters,
        count_sourced_commands(&sourced_steps),
        user_config,
        project_config,
    )?;
    Ok(sourced_steps)
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
            let (announce, pipe_stdin, redirect_stdout_to_stderr, error_wrapper) = match kind {
                PipelineKind::Hook {
                    hook_type,
                    display_path,
                } => (
                    AnnouncePolicy::Hook {
                        hook_type: *hook_type,
                        display_path: display_path.clone(),
                    },
                    true,
                    true,
                    hook_error_wrapper(*hook_type),
                ),
                PipelineKind::Alias { name } => (
                    AnnouncePolicy::None,
                    false,
                    false,
                    alias_error_wrapper(name.clone()),
                ),
            };
            ForegroundStep {
                step: sourced.step,
                concurrent: sourced.is_pipeline,
                announce,
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
/// `display_path` (in the spec): pass `pre_hook_display_path(ctx.worktree_path)`
/// for automatic detection, or `Some(path)` when hooks run somewhere the user
/// won't be cd'd to.
pub(crate) fn run_hooks_foreground(
    ctx: &CommandContext,
    spec: HookCommandSpec<'_, '_, '_, '_>,
    failure_strategy: FailureStrategy,
) -> anyhow::Result<()> {
    let kind = PipelineKind::Hook {
        hook_type: spec.hook_type,
        display_path: spec.display_path.map(|p| p.to_path_buf()),
    };
    let sourced_steps = prepare_and_check(ctx, spec)?;

    if sourced_steps.is_empty() {
        return Ok(());
    }

    let foreground_steps = sourced_steps_to_foreground(sourced_steps, &kind);

    execute_pipeline_foreground(
        &foreground_steps,
        ctx.repo,
        ctx.worktree_path,
        failure_strategy,
    )
}

/// Look up user and project configs for a given hook type.
pub(crate) fn lookup_hook_configs<'a>(
    user_hooks: &'a worktrunk::config::HooksConfig,
    project_config: Option<&'a worktrunk::config::ProjectConfig>,
    hook_type: HookType,
) -> (Option<&'a CommandConfig>, Option<&'a CommandConfig>) {
    (
        user_hooks.get(hook_type),
        project_config.and_then(|c| c.hooks.get(hook_type)),
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
pub(crate) fn execute_hook(
    ctx: &CommandContext,
    hook_type: HookType,
    extra_vars: &[(&str, &str)],
    failure_strategy: FailureStrategy,
    display_path: Option<&Path>,
) -> anyhow::Result<()> {
    let project_config = ctx.repo.load_project_config()?;
    let user_hooks = ctx.config.hooks(ctx.project_id().as_deref());
    let (user_config, proj_config) =
        lookup_hook_configs(&user_hooks, project_config.as_ref(), hook_type);
    run_hooks_foreground(
        ctx,
        HookCommandSpec {
            user_config,
            project_config: proj_config,
            hook_type,
            extra_vars,
            name_filters: &[],
            display_path,
        },
        failure_strategy,
    )
    .map_err(worktrunk::git::add_hook_skip_hint)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ansi_str::AnsiStr;
    use insta::assert_snapshot;

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

    fn make_sourced_step(step: PreparedStep) -> SourcedStep {
        SourcedStep {
            step,
            source: HookSource::User,
            is_pipeline: false,
        }
    }

    fn make_cmd(name: Option<&str>, expanded: &str) -> PreparedCommand {
        let label = match name {
            Some(n) => format!("user:{n}"),
            None => "user".to_string(),
        };
        PreparedCommand {
            name: name.map(String::from),
            expanded: expanded.to_string(),
            context_json: "{}".to_string(),
            lazy_template: None,
            label,
            log_label: None,
        }
    }

    #[test]
    fn test_format_pipeline_summary_named() {
        let steps = vec![
            make_sourced_step(PreparedStep::Single(make_cmd(
                Some("install"),
                "npm install",
            ))),
            make_sourced_step(PreparedStep::Concurrent(vec![
                make_cmd(Some("build"), "npm run build"),
                make_cmd(Some("lint"), "npm run lint"),
            ])),
        ];
        let summary = format_pipeline_summary(&steps);
        assert_snapshot!(summary.ansi_strip(), @"install, build & lint (user)");
    }

    #[test]
    fn test_format_pipeline_summary_unnamed() {
        let steps = vec![
            make_sourced_step(PreparedStep::Single(make_cmd(None, "npm install"))),
            make_sourced_step(PreparedStep::Single(make_cmd(None, "npm run build"))),
        ];
        let summary = format_pipeline_summary(&steps);
        assert_snapshot!(summary.ansi_strip(), @"user ×2");
    }

    #[test]
    fn test_format_pipeline_summary_mixed_named_unnamed() {
        let steps = vec![
            make_sourced_step(PreparedStep::Single(make_cmd(None, "npm install"))),
            make_sourced_step(PreparedStep::Single(make_cmd(Some("bg"), "npm run dev"))),
        ];
        let summary = format_pipeline_summary(&steps);
        assert_snapshot!(summary.ansi_strip(), @"…, bg (user)");
    }

    #[test]
    fn test_format_pipeline_summary_single_unnamed() {
        let steps = vec![make_sourced_step(PreparedStep::Single(make_cmd(
            None,
            "npm install",
        )))];
        let summary = format_pipeline_summary(&steps);
        assert_snapshot!(summary.ansi_strip(), @"user");
    }

    #[test]
    fn test_format_pipeline_summary_concurrent_then_concurrent() {
        // The canonical pipeline: two concurrent groups in sequence.
        // post-start = [
        //     { install = "npm install", setup = "setup-db" },
        //     { build = "npm run build", lint = "npm run lint" },
        // ]
        let steps = vec![
            make_sourced_step(PreparedStep::Concurrent(vec![
                make_cmd(Some("install"), "npm install"),
                make_cmd(Some("setup"), "setup-db"),
            ])),
            make_sourced_step(PreparedStep::Concurrent(vec![
                make_cmd(Some("build"), "npm run build"),
                make_cmd(Some("lint"), "npm run lint"),
            ])),
        ];
        let summary = format_pipeline_summary(&steps);
        assert_snapshot!(summary.ansi_strip(), @"install & setup, build & lint (user)");
    }

    #[test]
    fn test_is_pipeline() {
        use worktrunk::config::CommandConfig;

        let single = CommandConfig::single("npm install");
        assert!(!single.is_pipeline());
    }
}
