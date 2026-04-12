use std::path::{Path, PathBuf};

use anyhow::Context;
use color_print::cformat;
use worktrunk::HookType;
use worktrunk::config::CommandConfig;
use worktrunk::git::WorktrunkError;
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{
    eprintln, error_message, format_bash_with_gutter, progress_message, warning_message,
};

use super::command_executor::{
    CommandContext, PreparedCommand, PreparedStep, expand_shell_template, prepare_steps,
};
use crate::commands::process::{HookLog, spawn_detached_exec};
use crate::output::execute_shell_command;
use worktrunk::shell_exec::DIRECTIVE_FILE_ENV_VAR;

/// Short summary name: "user:name" for named commands, "user" otherwise.
pub(crate) fn command_summary_name(name: Option<&str>, source: HookSource) -> String {
    match name {
        Some(n) => format!("{source}:{n}"),
        None => source.to_string(),
    }
}

/// Announce a hook command before execution.
///
/// Format: "Running pre-merge user:foo" for named, "Running pre-start user hook" for unnamed.
/// When `display_path` is set, appends "@ path" to show where the command runs.
fn announce_hook_command(
    cmd: &PreparedCommand,
    source: HookSource,
    hook_type: HookType,
    display_path: Option<&Path>,
) {
    let summary = command_summary_name(cmd.name.as_deref(), source);
    let full_label = match &cmd.name {
        Some(_) => crate::commands::format_command_label(&hook_type.to_string(), Some(&summary)),
        None => format!("Running {hook_type} {summary} hook"),
    };
    let message = match display_path {
        Some(path) => {
            let path_display = format_path_for_display(path);
            cformat!("{full_label} @ <bold>{path_display}</>")
        }
        None => full_label,
    };
    eprintln!("{}", progress_message(message));
    eprintln!("{}", format_bash_with_gutter(&cmd.expanded));
}

/// Controls how hook execution should respond to failures.
#[derive(Clone, Copy)]
pub enum HookFailureStrategy {
    /// Stop on first failure and surface a `HookCommandFailed` error.
    FailFast,
    /// Log warnings and continue executing remaining commands.
    Warn,
}

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

        let steps = prepare_steps(config, ctx, extra_vars, hook_type, source)?;
        for step in steps {
            if let Some(filtered) = filter_step_by_name(step, &parsed_filters) {
                result.push(SourcedStep {
                    step: filtered,
                    source,
                    hook_type,
                    display_path: display_path.clone(),
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

/// Announce and spawn background hooks for one or more hook types.
///
/// Displays a single combined summary line covering all hook types, then
/// spawns each source group as an independent pipeline. Use this instead
/// of calling `spawn_hook_pipeline` directly when multiple hook types
/// fire together (e.g., post-switch + post-start on create).
///
/// Each pipeline carries its own `CommandContext` so that different hook types
/// can use different contexts (e.g., post-remove uses the removed branch while
/// post-switch uses the destination branch).
///
/// When `show_branch` is true, includes the branch name for disambiguation in batch
/// contexts (e.g., prune removing multiple worktrees):
/// `Running post-remove for feature: docs; post-switch for feature: zellij-tab`
///
/// Without `show_branch`: `Running post-switch: zellij-tab; post-start: deps, assets, docs`
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

    // Build combined summary, merging groups with the same hook type:
    // "post-switch: zellij-tab; post-start: deps, assets, docs"
    let display_path = non_empty
        .iter()
        .flat_map(|(_, g)| g.iter())
        .find_map(|s| s.display_path.as_ref());

    // Merge summaries by hook type so user+project for the same type
    // shows "post-start: user_bg, project" not "post-start: user_bg; post-start: project".
    let mut type_summaries: Vec<(HookType, Vec<String>)> = Vec::new();
    for (_, group) in &non_empty {
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
        non_empty
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
    eprintln!("{}", progress_message(message));

    for (ctx, group) in non_empty {
        spawn_hook_pipeline_quiet(&ctx, group)?;
    }

    Ok(())
}

/// Spawn a hook pipeline as a background `wt hook run-pipeline` process.
///
/// Displays a summary line and spawns the pipeline. For multiple hook types
/// that should share a single display line, use `announce_and_spawn_background_hooks`.
pub fn spawn_hook_pipeline(ctx: &CommandContext, steps: Vec<SourcedStep>) -> anyhow::Result<()> {
    if steps.is_empty() {
        return Ok(());
    }

    let hook_type = steps[0].hook_type;
    let display_path = steps[0].display_path.as_ref();
    let summary = format_pipeline_summary(&steps);
    let message = match display_path {
        Some(path) => {
            let path_display = format_path_for_display(path);
            cformat!("Running {hook_type}: {summary} @ <bold>{path_display}</>")
        }
        None => format!("Running {hook_type}: {summary}"),
    };
    eprintln!("{}", progress_message(message));

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
            warning_message(format!("Failed to spawn pipeline: {err}"))
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
    failure_strategy: HookFailureStrategy,
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

    // Foreground hooks have the same trust profile as aliases — the hook body
    // is already arbitrary shell. Pass the directive file through so inner `wt`
    // invocations (e.g. `wt switch --create` in a pre-start hook) can write
    // shell directives back to the parent shell.
    let directive_file: Option<PathBuf> =
        std::env::var_os(DIRECTIVE_FILE_ENV_VAR).map(PathBuf::from);
    let directive_file_ref = directive_file.as_deref();

    // Foreground hooks always execute serially, even when the prepared step is
    // `Concurrent`. The documented contract is "for pre-* hooks, commands in a
    // table run sequentially" (`src/cli/mod.rs`). Concurrent execution is
    // reserved for the background pipeline runner (`run_pipeline.rs`).
    for sourced in sourced_steps {
        let display_path_ref = sourced.display_path.as_deref();
        for cmd in &sourced.step.into_commands() {
            announce_hook_command(cmd, sourced.source, sourced.hook_type, display_path_ref);
            execute_one_hook_command(
                ctx,
                cmd,
                sourced.source,
                sourced.hook_type,
                directive_file_ref,
                failure_strategy,
            )?;
        }
    }

    Ok(())
}

/// Execute a single prepared hook command (caller has already announced it).
fn execute_one_hook_command(
    ctx: &CommandContext,
    cmd: &PreparedCommand,
    source: HookSource,
    hook_type: HookType,
    directive_file: Option<&Path>,
    failure_strategy: HookFailureStrategy,
) -> anyhow::Result<()> {
    let summary = command_summary_name(cmd.name.as_deref(), source);

    let lazy_expanded;
    let command_str = if let Some(template) = &cmd.lazy_template {
        let context: std::collections::HashMap<String, String> =
            serde_json::from_str(&cmd.context_json)
                .context("failed to deserialize context_json")?;
        lazy_expanded = expand_shell_template(template, &context, ctx.repo, &summary)?;
        &lazy_expanded
    } else {
        &cmd.expanded
    };

    let log_label = format!("{hook_type} {summary}");

    let Err(err) = execute_shell_command(
        ctx.worktree_path,
        command_str,
        Some(&cmd.context_json),
        Some(&log_label),
        directive_file,
    ) else {
        return Ok(());
    };

    let (err_msg, exit_code) = if let Some(wt_err) = err.downcast_ref::<WorktrunkError>() {
        match wt_err {
            WorktrunkError::ChildProcessExited { message, code } => (message.clone(), Some(*code)),
            _ => (err.to_string(), None),
        }
    } else {
        (err.to_string(), None)
    };

    match failure_strategy {
        HookFailureStrategy::FailFast => Err(WorktrunkError::HookCommandFailed {
            hook_type,
            command_name: cmd.name.clone(),
            error: err_msg,
            exit_code,
        }
        .into()),
        HookFailureStrategy::Warn => {
            let message = match &cmd.name {
                Some(name) => cformat!("Command <bold>{name}</> failed: {err_msg}"),
                None => format!("Command failed: {err_msg}"),
            };
            eprintln!("{}", error_message(message));
            Ok(())
        }
    }
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
    failure_strategy: HookFailureStrategy,
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
    fn test_hook_failure_strategy_copy() {
        let strategy = HookFailureStrategy::FailFast;
        let copied = strategy; // Copy trait
        assert!(matches!(copied, HookFailureStrategy::FailFast));

        let warn = HookFailureStrategy::Warn;
        let copied_warn = warn;
        assert!(matches!(copied_warn, HookFailureStrategy::Warn));
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
