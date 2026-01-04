use std::path::{Path, PathBuf};

use color_print::cformat;
use worktrunk::HookType;
use worktrunk::config::CommandConfig;
use worktrunk::git::WorktrunkError;
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{format_bash_with_gutter, progress_message, warning_message};

use super::command_executor::{CommandContext, PreparedCommand, prepare_commands};
use crate::commands::process::spawn_detached;
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
    /// Announce this command before execution.
    ///
    /// Format: "Running pre-merge user:foo:" for named, "Running post-create user hook:" for unnamed
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
                cformat!("{full_label} @ <bold>{path_display}</>:")
            }
            None => format!("{full_label}:"),
        };
        crate::output::print(progress_message(message))?;
        crate::output::print(format_bash_with_gutter(&self.prepared.expanded))?;
        Ok(())
    }
}

/// Controls how hook execution should respond to failures.
#[derive(Clone, Copy)]
pub enum HookFailureStrategy {
    /// Stop on first failure and surface a `HookCommandFailed` error.
    FailFast,
    /// Log warnings and continue executing remaining commands.
    /// For PostMerge hooks, propagates exit code after all commands complete.
    Warn,
}

// Re-export for backward compatibility with existing imports
pub use super::hook_filter::{HookSource, ParsedFilter};

/// Prepare hook commands from both user and project configs.
///
/// Collects commands from user config first, then project config, applying the name filter.
/// The filter supports source prefixes: `user:foo` or `project:foo` to run only from one source.
/// Returns a flat list of commands with source information for execution.
///
/// `display_path`: When `Some`, the path is shown in hook announcements (e.g., "@ ~/repo").
/// Use this when commands run in a different directory than where the user invoked the command.
#[allow(clippy::too_many_arguments)]
pub fn prepare_hook_commands(
    ctx: &CommandContext,
    user_config: Option<&CommandConfig>,
    project_config: Option<&CommandConfig>,
    hook_type: HookType,
    extra_vars: &[(&str, &str)],
    name_filter: Option<&str>,
    display_path: Option<&Path>,
) -> anyhow::Result<Vec<SourcedCommand>> {
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

        let prepared = prepare_commands(config, ctx, extra_vars, hook_type)?;
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
pub fn spawn_hook_commands_background(
    ctx: &CommandContext,
    commands: Vec<SourcedCommand>,
    hook_type: HookType,
) -> anyhow::Result<()> {
    if commands.is_empty() {
        return Ok(());
    }

    let operation_prefix = hook_type.to_string();

    // Track index for unnamed commands to prevent log collisions
    let mut unnamed_index = 0usize;

    for cmd in commands {
        cmd.announce()?;

        let name = match &cmd.prepared.name {
            Some(n) => n.clone(),
            None => {
                let idx = unnamed_index;
                unnamed_index += 1;
                format!("cmd-{idx}")
            }
        };
        // Include source in operation name to prevent log file collisions between
        // user and project hooks with the same name
        let operation = format!("{}-{}-{}", cmd.source, operation_prefix, name);

        if let Err(err) = spawn_detached(
            ctx.repo,
            ctx.worktree_path,
            &cmd.prepared.expanded,
            ctx.branch_or_head(),
            &operation,
            Some(&cmd.prepared.context_json),
        ) {
            let err_msg = err.to_string();
            let message = match &cmd.prepared.name {
                Some(name) => format!("Failed to spawn \"{name}\": {err_msg}"),
                None => format!("Failed to spawn command: {err_msg}"),
            };
            crate::output::print(warning_message(message))?;
        }
    }

    crate::output::flush()?;
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
                    .iter()
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
/// `display_path`: When `Some`, shows the path in hook announcements. Use when hooks
/// run in a different directory than where the user invoked the command.
#[allow(clippy::too_many_arguments)]
pub fn run_hook_with_filter(
    ctx: &CommandContext,
    user_config: Option<&CommandConfig>,
    project_config: Option<&CommandConfig>,
    hook_type: HookType,
    extra_vars: &[(&str, &str)],
    failure_strategy: HookFailureStrategy,
    name_filter: Option<&str>,
    display_path: Option<&Path>,
) -> anyhow::Result<()> {
    let commands = prepare_hook_commands(
        ctx,
        user_config,
        project_config,
        hook_type,
        extra_vars,
        name_filter,
        display_path,
    )?;

    check_name_filter_matched(name_filter, commands.len(), user_config, project_config)?;

    if commands.is_empty() {
        return Ok(());
    }

    // Track first failure for Warn strategy (to propagate exit code after all commands run)
    let mut first_failure: Option<(String, Option<String>, i32)> = None;

    for cmd in commands {
        cmd.announce()?;

        if let Err(err) = execute_command_in_worktree(
            ctx.worktree_path,
            &cmd.prepared.expanded,
            Some(&cmd.prepared.context_json),
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
                    crate::output::flush()?;
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
                    crate::output::print(warning_message(message))?;

                    // Track first failure to propagate exit code later (only for PostMerge)
                    if first_failure.is_none() && hook_type == HookType::PostMerge {
                        first_failure =
                            Some((err_msg, cmd.prepared.name.clone(), exit_code.unwrap_or(1)));
                    }
                }
            }
        }
    }

    crate::output::flush()?;

    // For Warn strategy with PostMerge: if any command failed, propagate the exit code
    // This matches git's behavior: post-hooks can't stop the operation but affect exit status
    if let Some((error, command_name, exit_code)) = first_failure {
        return Err(WorktrunkError::HookCommandFailed {
            hook_type,
            command_name,
            error,
            exit_code: Some(exit_code),
        }
        .into());
    }

    Ok(())
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
    fn test_parsed_filter_no_prefix() {
        let filter = ParsedFilter::parse("foo");
        assert!(filter.source.is_none());
        assert_eq!(filter.name, "foo");
        assert!(filter.matches_source(HookSource::User));
        assert!(filter.matches_source(HookSource::Project));
    }

    #[test]
    fn test_parsed_filter_user_prefix() {
        let filter = ParsedFilter::parse("user:foo");
        assert_eq!(filter.source, Some(HookSource::User));
        assert_eq!(filter.name, "foo");
        assert!(filter.matches_source(HookSource::User));
        assert!(!filter.matches_source(HookSource::Project));
    }

    #[test]
    fn test_parsed_filter_project_prefix() {
        let filter = ParsedFilter::parse("project:bar");
        assert_eq!(filter.source, Some(HookSource::Project));
        assert_eq!(filter.name, "bar");
        assert!(!filter.matches_source(HookSource::User));
        assert!(filter.matches_source(HookSource::Project));
    }

    #[test]
    fn test_parsed_filter_colon_in_name() {
        // A name like "my:hook" without valid prefix should be parsed as-is
        let filter = ParsedFilter::parse("my:hook");
        assert!(filter.source.is_none());
        assert_eq!(filter.name, "my:hook");
    }

    #[test]
    fn test_parsed_filter_source_only() {
        // "user:" means all user hooks (empty name)
        let filter = ParsedFilter::parse("user:");
        assert_eq!(filter.source, Some(HookSource::User));
        assert_eq!(filter.name, "");
        assert!(filter.matches_source(HookSource::User));
        assert!(!filter.matches_source(HookSource::Project));

        // "project:" means all project hooks
        let filter = ParsedFilter::parse("project:");
        assert_eq!(filter.source, Some(HookSource::Project));
        assert_eq!(filter.name, "");
    }
}
