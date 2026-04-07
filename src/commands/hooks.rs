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
    CommandContext, PreparedCommand, PreparedStep, prepare_commands, prepare_steps,
};
use crate::commands::process::{HookLog, spawn_detached_exec};
use crate::output::execute_command_in_worktree;

/// A prepared command with its source information.
pub struct SourcedCommand {
    pub prepared: PreparedCommand,
    pub source: HookSource,
    pub hook_type: HookType,
    /// Path to display in announcement, if different from user's current directory.
    /// When `Some`, shows "@ path" suffix to clarify where the command runs.
    pub display_path: Option<PathBuf>,
}

impl SourcedCommand {
    /// Short name for summary display: "user:name" or just "user" if unnamed.
    fn summary_name(&self) -> String {
        match &self.prepared.name {
            Some(n) => format!("{}:{}", self.source, n),
            None => self.source.to_string(),
        }
    }

    /// Announce this command before execution.
    ///
    /// Format: "Running pre-merge user:foo" for named, "Running pre-start user hook" for unnamed
    /// When display_path is set, appends "@ path" to show where the command runs.
    fn announce(&self) -> anyhow::Result<()> {
        // Named: "Running post-switch user:foo" with "user:foo" bold
        // Unnamed: "Running post-switch user hook" with no bold
        let full_label = match &self.prepared.name {
            Some(n) => {
                let display_name = format!("{}:{}", self.source, n);
                crate::commands::format_command_label(
                    &self.hook_type.to_string(),
                    Some(&display_name),
                )
            }
            None => format!("Running {} {} hook", self.hook_type, self.source),
        };
        let message = match &self.display_path {
            Some(path) => {
                let path_display = format_path_for_display(path);
                cformat!("{full_label} @ <bold>{path_display}</>")
            }
            None => full_label,
        };
        eprintln!("{}", progress_message(message));
        eprintln!("{}", format_bash_with_gutter(&self.prepared.expanded));
        Ok(())
    }
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
    pub name_filter: Option<&'name str>,
    pub display_path: Option<&'path Path>,
}

/// Prepare hook commands from both user and project configs.
///
/// Collects commands from user config first, then project config, applying the name filter.
/// The filter supports source prefixes: `user:foo` or `project:foo` to run only from one source.
/// Returns a flat list of commands with source information for execution.
///
/// `display_path`: When `Some`, the path is shown in hook announcements (e.g., "@ ~/repo").
/// Use this when commands run in a different directory than where the user invoked the command.
pub fn prepare_hook_commands(
    ctx: &CommandContext,
    spec: HookCommandSpec<'_, '_, '_, '_>,
) -> anyhow::Result<Vec<SourcedCommand>> {
    let HookCommandSpec {
        user_config,
        project_config,
        hook_type,
        extra_vars,
        name_filter,
        display_path,
    } = spec;

    let parsed_filter = name_filter.map(ParsedFilter::parse);
    let mut commands = Vec::new();

    let display_path = display_path.map(|p| p.to_path_buf());

    // Process user config first, then project config (execution order)
    let sources = [
        (HookSource::User, user_config),
        (HookSource::Project, project_config),
    ];

    for (source, config) in sources {
        let Some(config) = config else { continue };

        // Skip if filter specifies a different source
        if !parsed_filter
            .as_ref()
            .is_none_or(|f| f.matches_source(source))
        {
            continue;
        }

        let prepared = prepare_commands(config, ctx, extra_vars, hook_type, source)?;
        let filtered = filter_by_name(prepared, parsed_filter.as_ref().map(|f| f.name));
        commands.extend(filtered.into_iter().map(|p| SourcedCommand {
            prepared: p,
            source,
            hook_type,
            display_path: display_path.clone(),
        }));
    }

    Ok(commands)
}

/// Filter commands by name (returns empty vec if name not found).
/// Empty name matches all commands (supports `user:` to mean "all user hooks").
fn filter_by_name(
    commands: Vec<PreparedCommand>,
    name_filter: Option<&str>,
) -> Vec<PreparedCommand> {
    match name_filter {
        Some(name) if !name.is_empty() => commands
            .into_iter()
            .filter(|cmd| cmd.name.as_deref() == Some(name))
            .collect(),
        _ => commands, // None or empty = match all
    }
}

/// A pipeline step with source information, for pipeline-aware execution.
pub struct SourcedStep {
    pub step: PreparedStep,
    pub source: HookSource,
    pub hook_type: HookType,
    pub display_path: Option<PathBuf>,
}

/// Format a summary description of a pipeline for display.
///
/// Shows step names/counts with `→` separating serial steps.
/// Named steps show their name; unnamed steps show their source (`user`/`project`).
/// Example: "install → build, lint"
///
/// TODO: Rethink hook display presentation. Current issues:
/// - Arrows (`→`) add visual noise and aren't obviously meaningful to users
/// - Multiple unnamed steps from the same source repeat the label (`user → user`)
/// - Source prefix was dropped for named steps (`user:bg` → `bg`) — less
///   informative when both user and project hooks are present
fn format_pipeline_summary(steps: &[SourcedStep]) -> String {
    let mut parts = Vec::new();
    for step in steps {
        let source_label = step.source.to_string();
        match &step.step {
            PreparedStep::Single(cmd) => {
                let label = cmd.name.as_deref().unwrap_or(&source_label);
                parts.push(cformat!("<bold>{}</>", label));
            }
            PreparedStep::Concurrent(cmds) => {
                let names: Vec<String> = cmds
                    .iter()
                    .map(|c| {
                        let label = c.name.as_deref().unwrap_or(&source_label);
                        cformat!("<bold>{}</>", label)
                    })
                    .collect();
                parts.push(names.join(", "));
            }
        }
    }
    parts.join(" → ")
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
/// Example output: `Running post-switch: zellij-tab; post-start: deps, assets, docs`
pub fn announce_and_spawn_background_hooks(
    pipelines: Vec<(CommandContext<'_>, Vec<SourcedStep>)>,
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

    let combined: String = type_summaries
        .iter()
        .map(|(ht, summaries)| format!("{ht}: {}", summaries.join(", ")))
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

/// Check if a name filter was provided but no commands matched.
/// Returns an error listing available command names if so.
pub(crate) fn check_name_filter_matched(
    name_filter: Option<&str>,
    total_commands_run: usize,
    user_config: Option<&CommandConfig>,
    project_config: Option<&CommandConfig>,
) -> anyhow::Result<()> {
    if let Some(filter_str) = name_filter
        && total_commands_run == 0
    {
        let parsed = ParsedFilter::parse(filter_str);
        let mut available = Vec::new();

        // Collect available commands from sources that match the filter
        let sources = [
            (HookSource::User, user_config),
            (HookSource::Project, project_config),
        ];
        for (source, config) in sources {
            let Some(config) = config else { continue };
            if !parsed.matches_source(source) {
                continue;
            }
            available.extend(
                config
                    .commands()
                    .filter_map(|c| c.name.as_ref().map(|n| format!("{source}:{n}"))),
            );
        }

        return Err(worktrunk::git::GitError::HookCommandNotFound {
            name: filter_str.to_string(),
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
    let commands = prepare_hook_commands(ctx, spec)?;
    let HookCommandSpec {
        user_config,
        project_config,
        hook_type,
        name_filter,
        ..
    } = spec;

    check_name_filter_matched(name_filter, commands.len(), user_config, project_config)?;

    if commands.is_empty() {
        return Ok(());
    }

    // Track first failure's exit code for Warn strategy (to propagate after all commands run)
    for cmd in commands {
        cmd.announce()?;

        // Lazy commands (referencing vars.) are expanded just before execution
        // so that vars set by earlier commands in the pipeline are available.
        let expanded = if let Some(ref template) = cmd.prepared.lazy_template {
            let name = cmd.summary_name();
            expand_lazy_template(template, &cmd.prepared.context_json, ctx.repo, &name)?
        } else {
            cmd.prepared.expanded.clone()
        };

        let log_label = format!("{} {}", cmd.hook_type, cmd.summary_name());
        if let Err(err) = execute_command_in_worktree(
            ctx.worktree_path,
            &expanded,
            Some(&cmd.prepared.context_json),
            Some(&log_label),
        ) {
            // Extract raw message and exit code from error
            let (err_msg, exit_code) = if let Some(wt_err) = err.downcast_ref::<WorktrunkError>() {
                match wt_err {
                    WorktrunkError::ChildProcessExited { message, code } => {
                        (message.clone(), Some(*code))
                    }
                    _ => (err.to_string(), None),
                }
            } else {
                (err.to_string(), None)
            };

            match &failure_strategy {
                HookFailureStrategy::FailFast => {
                    return Err(WorktrunkError::HookCommandFailed {
                        hook_type,
                        command_name: cmd.prepared.name.clone(),
                        error: err_msg,
                        exit_code,
                    }
                    .into());
                }
                HookFailureStrategy::Warn => {
                    let message = match &cmd.prepared.name {
                        Some(name) => cformat!("Command <bold>{name}</> failed: {err_msg}"),
                        None => format!("Command failed: {err_msg}"),
                    };
                    eprintln!("{}", error_message(message));
                }
            }
        }
    }

    Ok(())
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
    name_filter: Option<&str>,
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
            name_filter,
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

/// Expand a lazy template using its command's context JSON.
///
/// Used by `run_hook_with_filter` (foreground) to expand templates that
/// reference `vars.*` at execution time. Background hooks handle lazy
/// expansion inside the pipeline runner process instead.
fn expand_lazy_template(
    template: &str,
    context_json: &str,
    repo: &worktrunk::git::Repository,
    label: &str,
) -> anyhow::Result<String> {
    let context_map: std::collections::HashMap<String, String> =
        serde_json::from_str(context_json).context("failed to deserialize context_json")?;
    let vars: std::collections::HashMap<&str, &str> = context_map
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    Ok(worktrunk::config::expand_template(
        template, &vars, true, repo, label,
    )?)
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
        // Contains arrow and names (stripped of ANSI for assertion)
        assert!(summary.contains("→"));
        assert!(summary.contains("install"));
        assert!(summary.contains("build"));
        assert!(summary.contains("lint"));
    }

    #[test]
    fn test_format_pipeline_summary_unnamed() {
        let steps = vec![
            make_sourced_step(PreparedStep::Single(make_cmd(None, "npm install"))),
            make_sourced_step(PreparedStep::Single(make_cmd(None, "npm run build"))),
        ];
        let summary = format_pipeline_summary(&steps);
        // Unnamed steps show source name ("user" from make_sourced_step)
        assert!(summary.contains("user"));
        assert!(summary.contains("→"));
    }

    #[test]
    fn test_is_pipeline() {
        use worktrunk::config::CommandConfig;

        let single = CommandConfig::single("npm install");
        assert!(!single.is_pipeline());
    }
}
