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
    CommandContext, CommandOrigin, FailureStrategy, ForegroundStep, PreparedCommand, PreparedStep,
    execute_pipeline_foreground, prepare_steps,
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
pub fn prepare_sourced_steps(
    ctx: &CommandContext,
    spec: HookCommandSpec<'_, '_, '_, '_>,
) -> anyhow::Result<Vec<SourcedStep>> {
    let HookCommandSpec {
        user_config,
        project_config,
        hook_type,
        extra_vars,
        name_filters,
        display_path,
    } = spec;

    let parsed_filters: Vec<ParsedFilter<'_>> = name_filters
        .iter()
        .map(|f| ParsedFilter::parse(f))
        .collect();

    let display_path = display_path.map(|p| p.to_path_buf());
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
                    hook_type,
                    display_path: display_path.clone(),
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
pub(crate) fn count_sourced_commands(steps: &[SourcedStep]) -> usize {
    steps
        .iter()
        .map(|s| match &s.step {
            PreparedStep::Single(_) => 1,
            PreparedStep::Concurrent(cmds) => cmds.len(),
        })
        .sum()
}

/// A pipeline step with source information, for pipeline-aware execution.
pub struct SourcedStep {
    pub step: PreparedStep,
    pub source: HookSource,
    pub hook_type: HookType,
    pub display_path: Option<PathBuf>,
    /// Whether this step came from a pipeline config (`[[hook]]` blocks).
    /// Pipeline `Concurrent` steps run concurrently; non-pipeline `Concurrent`
    /// steps (deprecated single-table form) run serially.
    pub is_pipeline: bool,
}

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

/// Format a pipeline summary from per-step command names.
///
/// `step_names[i]` is the list of commands in step `i`; `Some(name)` for named
/// commands, `None` for unnamed. Serial steps are joined by `;`, concurrent
/// commands within a step by `,`. Contiguous runs of unnamed commands (across
/// steps, until the next named command) are collapsed into a single
/// `label_unnamed(count)` entry; return `None` from that closure to drop
/// unnamed commands entirely.
///
/// Shared by hook announcements (where unnamed commands collapse to
/// `user ×N`) and alias announcements (which skip unnamed commands since
/// aliases have no natural fallback label).
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
            parts.push(named.join(", "));
        }
    }

    // Flush trailing unnamed count.
    if unnamed_count > 0
        && let Some(s) = label_unnamed(unnamed_count)
    {
        parts.push(s);
    }

    parts.join("; ")
}

/// Format a summary description of a hook pipeline for display.
///
/// Named steps show as `source:name`; unnamed steps are collapsed into a single
/// `source ×N` count. Serial steps are separated by `;`, concurrent steps by `,`.
/// Example: "user:install; user:build, user:lint"
///
/// TODO: The `source:` prefix on named steps may be too verbose when only one
/// source is present (e.g., `user:bg` vs just `bg`). Consider prefixing only
/// when both user and project hooks exist for the same hook type.
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

    format_pipeline_summary_from_names(
        &step_names,
        |name| cformat!("<bold>{source_label}:{name}</>"),
        |count| {
            Some(if count == 1 {
                cformat!("<bold>{source_label}</>")
            } else {
                cformat!("<bold>{source_label}</> ×{count}")
            })
        },
    )
}

/// Coordinates background hook announcements within a single `wt` command.
///
/// The principle: one `◎ Running …` line per command. Sites that would have
/// individually spawned background hooks instead `register` their pipelines
/// here, and the command `flush`es once after all phases have been resolved.
/// All registered hook types end up on a single combined line, joined by `;`.
///
/// Just-in-time honesty: registration happens at each phase's normal moment,
/// so if an earlier phase fails (e.g. pre-merge errors before post-merge is
/// queued), the announce only describes hooks that actually ran. The line is
/// emitted at `flush`, not at the first registration.
pub struct HookAnnouncer<'a> {
    pending: Vec<PendingPipeline>,
    repo: &'a Repository,
    /// Shared config used to rebuild `CommandContext`s at flush time.
    config: &'a UserConfig,
    show_branch: bool,
}

/// Owned spawn data for a registered pipeline. Stores enough to rebuild a
/// `CommandContext` at flush time so the announcer can outlive any short-lived
/// contexts at the registration site.
struct PendingPipeline {
    worktree_path: PathBuf,
    branch: Option<String>,
    steps: Vec<SourcedStep>,
}

impl<'a> HookAnnouncer<'a> {
    /// `show_branch=true` includes the branch name for disambiguation in batch
    /// contexts (e.g., prune removing multiple worktrees):
    /// `Running post-remove for feature: docs; post-switch for feature: zellij-tab`
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
    /// Pipelines come from `prepare_background_hooks` / its callers, which
    /// already filter out empty source groups — empty `steps` is unreachable.
    pub fn extend<'b, I>(&mut self, pipelines: I)
    where
        I: IntoIterator<Item = (CommandContext<'b>, Vec<SourcedStep>)>,
    {
        for (ctx, steps) in pipelines {
            self.pending.push(PendingPipeline {
                worktree_path: ctx.worktree_path.to_path_buf(),
                branch: ctx.branch.map(String::from),
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
        let groups = prepare_background_hooks(ctx, hook_type, extra_vars, display_path)?;
        self.extend(groups.into_iter().map(|g| (*ctx, g)));
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
                    std::mem::take(&mut p.steps),
                )
            })
            .collect();

        announce_and_spawn_pipelines(pipelines, self.show_branch)
    }
}

impl<'a> Drop for HookAnnouncer<'a> {
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

/// Announce and spawn background hooks for one or more hook types.
///
/// Convenience wrapper around `HookAnnouncer` for sites that produce all
/// pipelines in one place (e.g., post-switch + post-start on create). For
/// commands that fire hooks at multiple moments, use `HookAnnouncer` directly
/// so the whole command shares one announce line.
///
/// Each pipeline carries its own `CommandContext` so that different hook types
/// can use different contexts (e.g., post-remove uses the removed branch while
/// post-switch uses the destination branch).
pub fn announce_and_spawn_background_hooks(
    pipelines: Vec<(CommandContext<'_>, Vec<SourcedStep>)>,
    show_branch: bool,
) -> anyhow::Result<()> {
    let non_empty: Vec<_> = pipelines
        .into_iter()
        .filter(|(_, steps)| !steps.is_empty())
        .collect();
    if non_empty.is_empty() {
        return Ok(());
    }
    announce_and_spawn_pipelines(non_empty, show_branch)
}

/// Format the combined `Running …` line for `pipelines`, print it, and spawn
/// each source group as an independent pipeline. Caller is responsible for
/// filtering out empty groups before calling.
fn announce_and_spawn_pipelines(
    pipelines: Vec<(CommandContext<'_>, Vec<SourcedStep>)>,
    show_branch: bool,
) -> anyhow::Result<()> {
    // Build combined summary, merging groups with the same hook type:
    // "post-switch: zellij-tab; post-start: deps, assets, docs"
    let display_path = pipelines
        .iter()
        .flat_map(|(_, g)| g.iter())
        .find_map(|s| s.display_path.as_ref());

    // Merge summaries by hook type so user+project for the same type
    // shows "post-start: user_bg, project" not "post-start: user_bg; post-start: project".
    let mut type_summaries: Vec<(HookType, Vec<String>)> = Vec::new();
    for (_, group) in &pipelines {
        let hook_type = group[0].hook_type;
        let summary = format_pipeline_summary(group);
        if let Some(entry) = type_summaries.iter_mut().find(|(ht, _)| *ht == hook_type) {
            entry.1.push(summary);
        } else {
            type_summaries.push((hook_type, vec![summary]));
        }
    }

    // In batch contexts (prune), use the first pipeline's branch for disambiguation.
    // This is the removed branch — it identifies the triggering event even for
    // post-switch hooks that fire as a consequence of the removal.
    let branch_suffix = if show_branch {
        pipelines
            .first()
            .and_then(|(ctx, _)| ctx.branch)
            .map(|b| cformat!(" for <bold>{b}</>"))
    } else {
        None
    };

    let combined: String = type_summaries
        .iter()
        .map(|(ht, summaries)| {
            let suffix = branch_suffix.as_deref().unwrap_or("");
            format!("{ht}{suffix}: {}", summaries.join(", "))
        })
        .collect::<Vec<_>>()
        .join("; ");
    let message = match display_path {
        Some(path) => {
            let path_display = format_path_for_display(path);
            cformat!("Running {combined} @ <bold>{path_display}</>")
        }
        None => format!("Running {combined}"),
    };
    if verbosity() >= 1 {
        print_background_variable_tables(&pipelines);
    }
    eprintln!("{}", progress_message(message));

    for (ctx, group) in pipelines {
        spawn_hook_pipeline_quiet(&ctx, group)?;
    }

    Ok(())
}

/// Emit a `template variables:` block per distinct hook type in `pipelines`.
///
/// Background hooks don't flow through `announce_command` (which prints the
/// table in the foreground path), so this is the symmetric entry point: once
/// per hook type, using the first command's context. `hook_name` within a
/// table reflects that first command — the combined announce line above
/// already enumerates the rest.
fn print_background_variable_tables(pipelines: &[(CommandContext<'_>, Vec<SourcedStep>)]) {
    let mut seen: Vec<HookType> = Vec::new();
    for (_, group) in pipelines {
        for sourced in group {
            if seen.contains(&sourced.hook_type) {
                continue;
            }
            let cmd = match &sourced.step {
                PreparedStep::Single(cmd) => cmd,
                PreparedStep::Concurrent(cmds) => &cmds[0],
            };
            let ctx: HashMap<String, String> = serde_json::from_str(&cmd.context_json)
                .expect("context_json is always serialized from a HashMap<String, String>");
            eprintln!("{}", info_message("template variables:"));
            eprintln!(
                "{}",
                format_with_gutter(&format_hook_variables(sourced.hook_type, &ctx), None)
            );
            seen.push(sourced.hook_type);
        }
    }
}

/// Prepare and spawn all source-group pipelines for a single hook type.
///
/// Wraps `prepare_background_hooks` + `announce_and_spawn_background_hooks` so
/// callers produce exactly one `Running {hook}: …` announce line even when
/// both user and project configs contribute pipelines. Iterating the prepared
/// groups and calling `spawn_hook_pipeline` per group is a footgun — it prints
/// one announce line per source.
pub fn spawn_background_hooks(
    ctx: &CommandContext,
    hook_type: HookType,
    extra_vars: &[(&str, &str)],
    display_path: Option<&Path>,
) -> anyhow::Result<()> {
    let pipelines: Vec<_> = prepare_background_hooks(ctx, hook_type, extra_vars, display_path)?
        .into_iter()
        .map(|g| (*ctx, g))
        .collect();
    announce_and_spawn_background_hooks(pipelines, false)
}

/// Spawn a filter-matched hook pipeline as a background `wt hook run-pipeline`.
///
/// The name-filter path merges user + project matches into one pipeline (vs.
/// the source-grouped path that produces one pipeline per source). For the
/// source-grouped path, use `spawn_background_hooks` instead.
///
/// `check_name_filter_matched` must have run first — it guarantees `steps` is
/// non-empty. The filter path always calls with `display_path: None`, so no
/// path annotation is rendered.
pub fn spawn_hook_pipeline(ctx: &CommandContext, steps: Vec<SourcedStep>) -> anyhow::Result<()> {
    let hook_type = steps[0].hook_type;
    let summary = format_pipeline_summary(&steps);
    eprintln!(
        "{}",
        progress_message(format!("Running {hook_type}: {summary}"))
    );
    spawn_hook_pipeline_quiet(ctx, steps)
}

/// Spawn a hook pipeline without displaying a summary line.
///
/// Used by `announce_and_spawn_background_hooks` which handles display separately.
fn spawn_hook_pipeline_quiet(ctx: &CommandContext, steps: Vec<SourcedStep>) -> anyhow::Result<()> {
    use super::pipeline_spec::{PipelineCommandSpec, PipelineSpec, PipelineStepSpec};

    let hook_type = steps[0].hook_type;
    let source = steps[0].source;

    // Extract base context from the first command. All steps share the same base context,
    // but per-step metadata (hook_name) is stripped — it gets injected per-step by the
    // background runner.
    let mut context: std::collections::HashMap<String, String> = steps
        .iter()
        .find_map(|s| match &s.step {
            PreparedStep::Single(cmd) => Some(&cmd.context_json),
            PreparedStep::Concurrent(cmds) => cmds.first().map(|c| &c.context_json),
        })
        .map(|json| serde_json::from_str(json).context("failed to deserialize context_json"))
        .transpose()?
        .unwrap_or_default();
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
pub(crate) fn check_name_filter_matched(
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

/// Run user and project hooks for a given hook type.
///
/// This is the canonical implementation for running hooks from both sources.
/// Runs user hooks first, then project hooks sequentially. Handles name filtering
/// and returns an error if a name filter was provided but no matching command found.
///
/// `display_path`: Pass `ctx.hooks_display_path()` for automatic detection, or
/// explicit `Some(path)` when hooks run somewhere the user won't be cd'd to.
pub fn run_hook_with_filter(
    ctx: &CommandContext,
    spec: HookCommandSpec<'_, '_, '_, '_>,
    failure_strategy: FailureStrategy,
) -> anyhow::Result<()> {
    let sourced_steps = prepare_sourced_steps(ctx, spec)?;
    let HookCommandSpec {
        user_config,
        project_config,
        name_filters,
        ..
    } = spec;

    check_name_filter_matched(
        name_filters,
        count_sourced_commands(&sourced_steps),
        user_config,
        project_config,
    )?;

    if sourced_steps.is_empty() {
        return Ok(());
    }

    let directives = DirectivePassthrough::inherit_from_env();

    // Convert SourcedSteps → ForegroundSteps for the shared executor.
    // Pipeline configs (`[[hook]]` blocks) get concurrent execution within each
    // block. Non-pipeline configs (deprecated single-table `[hook]` form) run
    // their commands serially.
    let foreground_steps: Vec<ForegroundStep> = sourced_steps
        .into_iter()
        .map(|sourced| ForegroundStep {
            concurrent: sourced.is_pipeline,
            step: sourced.step,
            origin: CommandOrigin::Hook {
                source: sourced.source,
                hook_type: sourced.hook_type,
                display_path: sourced.display_path,
            },
        })
        .collect();

    execute_pipeline_foreground(
        &foreground_steps,
        ctx.repo,
        ctx.worktree_path,
        &directives,
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

/// Run a hook type with automatic config lookup.
///
/// This is a convenience wrapper that:
/// 1. Loads project config from the repository
/// 2. Looks up user hooks from the config
/// 3. Calls `run_hook_with_filter` with the appropriate hook configs
/// 4. Adds the hook skip hint to errors
///
/// Use this instead of manually looking up configs and calling `run_hook_with_filter`.
pub fn execute_hook(
    ctx: &CommandContext,
    hook_type: HookType,
    extra_vars: &[(&str, &str)],
    failure_strategy: FailureStrategy,
    name_filters: &[String],
    display_path: Option<&Path>,
) -> anyhow::Result<()> {
    let project_config = ctx.repo.load_project_config()?;
    let user_hooks = ctx.config.hooks(ctx.project_id().as_deref());
    let (user_config, proj_config) =
        lookup_hook_configs(&user_hooks, project_config.as_ref(), hook_type);

    run_hook_with_filter(
        ctx,
        HookCommandSpec {
            user_config,
            project_config: proj_config,
            hook_type,
            extra_vars,
            name_filters,
            display_path,
        },
        failure_strategy,
    )
    .map_err(worktrunk::git::add_hook_skip_hint)
}

/// Prepare background hooks with automatic config lookup.
///
/// Returns pipeline steps grouped by source — one group per source that has
/// hooks configured. Each group should be spawned as an independent pipeline
/// so that user and project hooks remain independent (a user hook failure
/// doesn't abort project hooks).
pub(crate) fn prepare_background_hooks(
    ctx: &CommandContext,
    hook_type: HookType,
    extra_vars: &[(&str, &str)],
    display_path: Option<&Path>,
) -> anyhow::Result<Vec<Vec<SourcedStep>>> {
    let project_config = ctx.repo.load_project_config()?;
    let user_hooks = ctx.config.hooks(ctx.project_id().as_deref());
    let (user_config, proj_config) =
        lookup_hook_configs(&user_hooks, project_config.as_ref(), hook_type);

    let display_path = display_path.map(|p| p.to_path_buf());
    let mut groups = Vec::new();

    let sources = [
        (HookSource::User, user_config),
        (HookSource::Project, proj_config),
    ];

    for (source, config) in sources {
        let Some(config) = config else { continue };
        let is_pipeline = config.is_pipeline();
        let steps = prepare_steps(config, ctx, extra_vars, hook_type, source)?;
        if steps.is_empty() {
            continue;
        }
        groups.push(
            steps
                .into_iter()
                .map(|step| SourcedStep {
                    step,
                    source,
                    hook_type,
                    display_path: display_path.clone(),
                    is_pipeline,
                })
                .collect(),
        );
    }

    Ok(groups)
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
            hook_type: worktrunk::HookType::PostStart,
            display_path: None,
            is_pipeline: false,
        }
    }

    fn make_cmd(name: Option<&str>, expanded: &str) -> PreparedCommand {
        PreparedCommand {
            name: name.map(String::from),
            expanded: expanded.to_string(),
            context_json: "{}".to_string(),
            lazy_template: None,
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
        assert_snapshot!(summary.ansi_strip(), @"user:install; user:build, user:lint");
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
        assert_snapshot!(summary.ansi_strip(), @"user; user:bg");
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
        assert_snapshot!(summary.ansi_strip(), @"user:install, user:setup; user:build, user:lint");
    }

    #[test]
    fn test_is_pipeline() {
        use worktrunk::config::CommandConfig;

        let single = CommandConfig::single("npm install");
        assert!(!single.is_pipeline());
    }
}
