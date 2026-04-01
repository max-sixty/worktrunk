use std::path::{Path, PathBuf};

use color_print::cformat;
use worktrunk::HookType;
use worktrunk::config::CommandConfig;
use worktrunk::git::WorktrunkError;
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{
    eprintln, error_message, format_bash_with_gutter, progress_message, verbosity, warning_message,
};

use super::command_executor::{
    CommandContext, PreparedCommand, PreparedStep, prepare_commands, prepare_steps,
};
use crate::commands::process::{HookLog, spawn_detached};
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

/// Spawn hook commands as background (detached) processes.
///
/// Used for post-start and post-switch hooks during normal worktree operations.
/// Commands are spawned and immediately detached - we don't wait for them.
///
/// By default, shows a single-line summary of all hooks being run, with support
/// for multiple hook types in a single message (e.g., "Running post-switch: user:foo; post-start: project:bar").
/// With `-v`, shows verbose per-hook output with command details.
pub fn spawn_background_hooks(
    ctx: &CommandContext,
    commands: Vec<SourcedCommand>,
) -> anyhow::Result<()> {
    if commands.is_empty() {
        return Ok(());
    }

    let verbose = verbosity();

    if verbose == 0 {
        // Group commands by hook type, preserving insertion order
        let groups = group_commands_by_hook_type(&commands);
        // All commands in a batch share the same display_path (set by prepare_hook_commands)
        let display_path = commands.first().and_then(|c| c.display_path.as_ref());

        // Format: "Running {type}: {names}[; {type}: {names}]... [@ {path}]"
        let type_segments: Vec<String> = groups
            .iter()
            .map(|(hook_type, cmds)| {
                let names: Vec<String> = cmds
                    .iter()
                    .map(|c| cformat!("<bold>{}</>", c.summary_name()))
                    .collect();
                format!("{hook_type}: {}", names.join(", "))
            })
            .collect();

        let message = match display_path {
            Some(path) => {
                let path_display = format_path_for_display(path);
                cformat!(
                    "Running {} @ <bold>{path_display}</>",
                    type_segments.join("; ")
                )
            }
            None => format!("Running {}", type_segments.join("; ")),
        };
        eprintln!("{}", progress_message(message));
    }

    // Track index for unnamed commands to prevent log collisions (per hook type)
    // Use a Vec since HookType doesn't implement Hash
    let mut unnamed_indices: Vec<(HookType, usize)> = Vec::new();

    for cmd in &commands {
        if verbose >= 1 {
            cmd.announce()?;
        }

        let name = match &cmd.prepared.name {
            Some(n) => n.clone(),
            None => {
                let idx = if let Some((_, count)) = unnamed_indices
                    .iter_mut()
                    .find(|(t, _)| *t == cmd.hook_type)
                {
                    let result = *count;
                    *count += 1;
                    result
                } else {
                    unnamed_indices.push((cmd.hook_type, 1));
                    0
                };
                format!("cmd-{idx}")
            }
        };
        // Use HookLog with the command's own hook_type for consistent log file naming
        let hook_log = HookLog::hook(cmd.source, cmd.hook_type, &name);

        let log_label = format!("{} {}", cmd.hook_type, cmd.summary_name());
        if let Err(err) = spawn_detached(
            ctx.repo,
            ctx.worktree_path,
            &cmd.prepared.expanded,
            ctx.branch_or_head(),
            &hook_log,
            Some(&cmd.prepared.context_json),
            &[],
        ) {
            let err_msg = err.to_string();
            let message = match &cmd.prepared.name {
                Some(name) => format!("Failed to spawn \"{name}\": {err_msg}"),
                None => format!("Failed to spawn command: {err_msg}"),
            };
            eprintln!("{}", warning_message(message));
        } else {
            // Background: outcome unknown, log with null exit/duration
            worktrunk::command_log::log_command(&log_label, &cmd.prepared.expanded, None, None);
        }
    }

    Ok(())
}

/// Prepared hooks ready for background spawning — either flat commands or a pipeline.
pub(crate) enum PreparedHooks {
    /// Traditional flat commands (string or map config) — spawned independently.
    Flat(Vec<SourcedCommand>),
    /// Pipeline steps (list config) — spawned as one compound shell command.
    Pipeline(Vec<SourcedStep>),
}

/// A pipeline step with source information, for pipeline-aware execution.
pub struct SourcedStep {
    pub step: PreparedStep,
    pub source: HookSource,
    pub hook_type: HookType,
    pub display_path: Option<PathBuf>,
}

/// Build a compound shell command from pipeline steps, collecting env vars for
/// lazy template expansion.
///
/// Serial steps are chained with `&&`. Concurrent groups use background
/// processes with `wait`. Commands referencing `vars.` are wrapped in
/// `eval "$(wt step eval --shell-escape ...)"` and their raw templates are
/// returned as env vars for the spawned process.
///
/// Returns `(shell_command, env_vars)`.
fn build_pipeline_command(steps: &[SourcedStep]) -> (String, Vec<(String, String)>) {
    let wt_bin = std::env::current_exe()
        .ok()
        .map(|p| shell_escape::escape(p.to_string_lossy()).into_owned())
        .unwrap_or_else(|| "wt".to_string());

    let mut parts = Vec::new();
    let mut env_vars: Vec<(String, String)> = Vec::new();

    for step in steps {
        match &step.step {
            PreparedStep::Single(cmd) => {
                // Wrap in braces to prevent operator interaction between steps.
                // Without this, `false || echo fallback && echo next` would
                // parse as `false || (echo fallback && echo next)`.
                parts.push(format!(
                    "{{ {}; }}",
                    format_cmd(cmd, &wt_bin, &mut env_vars)
                ));
            }
            PreparedStep::Concurrent(cmds) => {
                let bg: Vec<String> = cmds
                    .iter()
                    .map(|c| format!("{{ {}; }} &", format_cmd(c, &wt_bin, &mut env_vars)))
                    .collect();
                parts.push(format!("{{ {} wait; }}", bg.join(" ")));
            }
        }
    }
    (parts.join(" && "), env_vars)
}

/// Format a single command for the pipeline, using lazy eval wrapping for commands
/// with `lazy_template`.
fn format_cmd(cmd: &PreparedCommand, wt_bin: &str, env_vars: &mut Vec<(String, String)>) -> String {
    match &cmd.lazy_template {
        Some(template) => {
            let var_name = format!("__WT_TPL_{}", env_vars.len());
            env_vars.push((var_name.clone(), template.clone()));
            format!(r#"eval "$({wt_bin} step eval --shell-escape "${var_name}")""#)
        }
        None => cmd.expanded.clone(),
    }
}

/// Format a summary description of a pipeline for display.
///
/// Shows step names/counts with `→` separating serial steps.
/// Example: "install → build, lint"
fn format_pipeline_summary(steps: &[SourcedStep]) -> String {
    let mut unnamed_idx = 0usize;
    let mut parts = Vec::new();
    for step in steps {
        match &step.step {
            PreparedStep::Single(cmd) => {
                let name = match &cmd.name {
                    Some(n) => cformat!("<bold>{}</>", n),
                    None => {
                        let label = format!("cmd-{unnamed_idx}");
                        unnamed_idx += 1;
                        cformat!("<bold>{}</>", label)
                    }
                };
                parts.push(name);
            }
            PreparedStep::Concurrent(cmds) => {
                let names: Vec<String> = cmds
                    .iter()
                    .map(|c| match &c.name {
                        Some(n) => cformat!("<bold>{}</>", n),
                        None => {
                            let label = format!("cmd-{unnamed_idx}");
                            unnamed_idx += 1;
                            cformat!("<bold>{}</>", label)
                        }
                    })
                    .collect();
                parts.push(names.join(", "));
            }
        }
    }
    parts.join(" → ")
}

/// Spawn a hook pipeline as one background process.
///
/// Builds a compound shell command where serial steps are chained with `&&`
/// and concurrent groups use background processes. The entire pipeline runs
/// as one detached process.
///
/// Used for post-* hooks when the config uses a pipeline (list form).
pub fn spawn_hook_pipeline(ctx: &CommandContext, steps: Vec<SourcedStep>) -> anyhow::Result<()> {
    if steps.is_empty() {
        return Ok(());
    }

    let hook_type = steps[0].hook_type;
    let source = steps[0].source;
    let display_path = steps[0].display_path.as_ref();

    // Show summary: "Running post-start: install → build, lint"
    let summary = format_pipeline_summary(&steps);
    let message = match display_path {
        Some(path) => {
            let path_display = format_path_for_display(path);
            cformat!("Running {hook_type}: {summary} @ <bold>{path_display}</>")
        }
        None => format!("Running {hook_type}: {summary}"),
    };
    eprintln!("{}", progress_message(message));

    let (pipeline_cmd, lazy_env) = build_pipeline_command(&steps);
    let hook_log = HookLog::hook(source, hook_type, "pipeline");
    let log_label = format!("{hook_type} {source} pipeline");

    // Use the first command's context JSON for the whole pipeline.
    // All steps share the same base context (branch, worktree_path, etc.).
    let context_json = steps.iter().find_map(|s| match &s.step {
        PreparedStep::Single(cmd) => Some(cmd.context_json.as_str()),
        PreparedStep::Concurrent(cmds) => cmds.first().map(|c| c.context_json.as_str()),
    });

    let extra_env: Vec<(&str, &str)> = lazy_env
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    if let Err(err) = spawn_detached(
        ctx.repo,
        ctx.worktree_path,
        &pipeline_cmd,
        ctx.branch_or_head(),
        &hook_log,
        context_json,
        &extra_env,
    ) {
        eprintln!(
            "{}",
            warning_message(format!("Failed to spawn pipeline: {err}"))
        );
    } else {
        worktrunk::command_log::log_command(&log_label, &pipeline_cmd, None, None);
    }

    Ok(())
}

/// Prepare hook steps as a pipeline, preserving serial/concurrent structure.
///
/// Accepts pre-looked-up configs to avoid redundant loading.
pub(crate) fn prepare_pipeline_hooks_with_configs(
    ctx: &CommandContext,
    user_config: Option<&CommandConfig>,
    proj_config: Option<&CommandConfig>,
    hook_type: HookType,
    extra_vars: &[(&str, &str)],
    display_path: Option<&Path>,
) -> anyhow::Result<Vec<SourcedStep>> {
    let display_path = display_path.map(|p| p.to_path_buf());
    let mut all_steps = Vec::new();

    let sources = [
        (HookSource::User, user_config),
        (HookSource::Project, proj_config),
    ];

    for (source, config) in sources {
        let Some(config) = config else { continue };
        let steps = prepare_steps(config, ctx, extra_vars, hook_type, source)?;
        all_steps.extend(steps.into_iter().map(|step| SourcedStep {
            step,
            source,
            hook_type,
            display_path: display_path.clone(),
        }));
    }

    Ok(all_steps)
}

/// Group commands by hook type, preserving insertion order.
///
/// Returns a vector of (HookType, Vec<&SourcedCommand>) tuples.
/// This preserves the order in which hook types were first encountered
/// (e.g., post-switch before post-start).
fn group_commands_by_hook_type(
    commands: &[SourcedCommand],
) -> Vec<(HookType, Vec<&SourcedCommand>)> {
    let mut groups: Vec<(HookType, Vec<&SourcedCommand>)> = Vec::new();
    for cmd in commands {
        if let Some((_, vec)) = groups.iter_mut().find(|(t, _)| *t == cmd.hook_type) {
            vec.push(cmd);
        } else {
            groups.push((cmd.hook_type, vec![cmd]));
        }
    }
    groups
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
            let context_map: std::collections::HashMap<String, String> =
                serde_json::from_str(&cmd.prepared.context_json)
                    .expect("context_json round-trip should never fail");
            let vars: std::collections::HashMap<&str, &str> = context_map
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            let name = cmd.summary_name();
            worktrunk::config::expand_template(template, &vars, true, ctx.repo, &name)?
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
/// Returns `PreparedHooks::Pipeline` when any source uses a list config,
/// or `PreparedHooks::Flat` for traditional string/map configs.
/// Callers should use `spawn_prepared_hooks` to execute the result.
pub(crate) fn prepare_background_hooks(
    ctx: &CommandContext,
    hook_type: HookType,
    extra_vars: &[(&str, &str)],
    display_path: Option<&Path>,
) -> anyhow::Result<PreparedHooks> {
    let project_config = ctx.repo.load_project_config()?;
    let user_hooks = ctx.config.hooks(ctx.project_id().as_deref());
    let (user_config, proj_config) =
        lookup_hook_configs(&user_hooks, project_config.as_ref(), hook_type);

    if [user_config, proj_config]
        .iter()
        .any(|c| c.is_some_and(CommandConfig::is_pipeline))
    {
        let steps = prepare_pipeline_hooks_with_configs(
            ctx,
            user_config,
            proj_config,
            hook_type,
            extra_vars,
            display_path,
        )?;
        return Ok(PreparedHooks::Pipeline(steps));
    }

    let commands = prepare_hook_commands(
        ctx,
        HookCommandSpec {
            user_config,
            project_config: proj_config,
            hook_type,
            extra_vars,
            name_filter: None,
            display_path,
        },
    )?;
    Ok(PreparedHooks::Flat(commands))
}

/// Spawn prepared hooks (either flat or pipeline).
pub(crate) fn spawn_prepared_hooks(
    ctx: &CommandContext,
    hooks: PreparedHooks,
) -> anyhow::Result<()> {
    match hooks {
        PreparedHooks::Flat(commands) => spawn_background_hooks(ctx, commands),
        PreparedHooks::Pipeline(steps) => spawn_hook_pipeline(ctx, steps),
    }
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
    fn test_build_pipeline_command_serial_only() {
        let steps = vec![
            make_sourced_step(PreparedStep::Single(make_cmd(None, "npm install"))),
            make_sourced_step(PreparedStep::Single(make_cmd(
                Some("build"),
                "npm run build",
            ))),
        ];
        let (cmd, env) = build_pipeline_command(&steps);
        assert_eq!(cmd, "{ npm install; } && { npm run build; }");
        assert!(env.is_empty());
    }

    #[test]
    fn test_build_pipeline_command_serial_then_concurrent() {
        let steps = vec![
            make_sourced_step(PreparedStep::Single(make_cmd(None, "npm install"))),
            make_sourced_step(PreparedStep::Concurrent(vec![
                make_cmd(Some("build"), "npm run build"),
                make_cmd(Some("lint"), "npm run lint"),
            ])),
        ];
        let (cmd, env) = build_pipeline_command(&steps);
        assert_eq!(
            cmd,
            "{ npm install; } && { { npm run build; } & { npm run lint; } & wait; }"
        );
        assert!(env.is_empty());
    }

    #[test]
    fn test_build_pipeline_command_single_concurrent() {
        let steps = vec![make_sourced_step(PreparedStep::Concurrent(vec![make_cmd(
            Some("only"),
            "echo hi",
        )]))];
        let (cmd, env) = build_pipeline_command(&steps);
        assert_eq!(cmd, "{ { echo hi; } & wait; }");
        assert!(env.is_empty());
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
        assert!(summary.contains("cmd-0"));
        assert!(summary.contains("cmd-1"));
    }

    #[test]
    fn test_is_pipeline() {
        use worktrunk::config::CommandConfig;

        let single = CommandConfig::single("npm install");
        assert!(!single.is_pipeline());
    }

    fn make_lazy_cmd(name: Option<&str>, template: &str) -> PreparedCommand {
        PreparedCommand {
            name: name.map(String::from),
            expanded: template.to_string(),
            context_json: "{}".to_string(),
            lazy_template: Some(template.to_string()),
        }
    }

    #[test]
    fn test_build_pipeline_command_lazy_step() {
        let steps = vec![
            make_sourced_step(PreparedStep::Single(make_cmd(None, "echo setup"))),
            make_sourced_step(PreparedStep::Single(make_lazy_cmd(
                Some("db"),
                "docker run --name {{ vars.container }}",
            ))),
        ];
        let (cmd, env) = build_pipeline_command(&steps);

        // Lazy step uses eval wrapping with properly quoted env var reference.
        // The closing ")" must be outside $() — a missing " produces broken shell.
        assert!(
            cmd.contains("eval \"$("),
            "should contain eval wrapping: {cmd}"
        );
        assert!(
            cmd.contains("\"$__WT_TPL_0\")\""),
            "should close with proper quoting (var quote, paren, outer quote): {cmd}"
        );

        // Env var contains the raw template
        assert_eq!(env.len(), 1);
        assert_eq!(env[0].0, "__WT_TPL_0");
        assert_eq!(env[0].1, "docker run --name {{ vars.container }}");
    }

    #[test]
    fn test_build_pipeline_command_mixed_lazy_and_eager() {
        let steps = vec![
            make_sourced_step(PreparedStep::Single(make_cmd(None, "echo setup"))),
            make_sourced_step(PreparedStep::Concurrent(vec![
                make_lazy_cmd(Some("db"), "docker run {{ vars.name }}"),
                make_cmd(Some("lint"), "npm run lint"),
            ])),
        ];
        let (cmd, env) = build_pipeline_command(&steps);

        // First step is eager (no eval wrapping)
        assert!(cmd.starts_with("{ echo setup; }"), "eager step: {cmd}");

        // Concurrent group has one lazy and one eager
        assert!(cmd.contains("npm run lint"), "eager concurrent cmd: {cmd}");
        assert!(
            cmd.contains("\"$__WT_TPL_0\")\""),
            "lazy concurrent cmd should have proper quoting: {cmd}"
        );

        // Only one env var (for the lazy command)
        assert_eq!(env.len(), 1);
        assert_eq!(env[0].1, "docker run {{ vars.name }}");
    }

    #[test]
    fn test_build_pipeline_command_multiple_lazy_steps() {
        let steps = vec![
            make_sourced_step(PreparedStep::Single(make_cmd(None, "echo setup"))),
            make_sourced_step(PreparedStep::Single(make_lazy_cmd(
                None,
                "echo {{ vars.a }}",
            ))),
            make_sourced_step(PreparedStep::Single(make_lazy_cmd(
                None,
                "echo {{ vars.b }}",
            ))),
        ];
        let (cmd, env) = build_pipeline_command(&steps);

        // Each lazy step gets its own numbered env var with proper quoting
        assert_eq!(env.len(), 2);
        assert_eq!(env[0].0, "__WT_TPL_0");
        assert_eq!(env[1].0, "__WT_TPL_1");
        assert!(
            cmd.contains("\"$__WT_TPL_0\")\""),
            "first lazy step should have proper quoting: {cmd}"
        );
        assert!(
            cmd.contains("\"$__WT_TPL_1\")\""),
            "second lazy step should have proper quoting: {cmd}"
        );
    }
}
