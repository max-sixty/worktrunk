// TODO(hook-naming): Refine hook display and filtering when user and project have same name
//
// Current behavior with `wt hook pre-merge foo`:
// - Both user's "foo" and project's "foo" run (name filter applied to each source separately)
// - Output: "Running user pre-merge foo:" then "Running project pre-merge foo:"
//
// Alternative approaches to consider:
// 1. Show source in name: "Running pre-merge user:foo" / "Running pre-merge project:foo"
// 2. Allow filtering by source: `wt hook pre-merge user:foo` runs only user's foo
// 3. Current approach: always show "user"/"project" prefix, filter runs both
//
// The source prefix in filtering (option 2) would need to be used elsewhere too to justify
// the syntax. Current behavior is reasonable but worth revisiting if users find it confusing.

use color_print::cformat;
use worktrunk::HookType;
use worktrunk::config::{CommandConfig, ProjectConfig};
use worktrunk::git::WorktrunkError;
use worktrunk::styling::{format_bash_with_gutter, progress_message, warning_message};

use super::command_executor::{CommandContext, PreparedCommand, prepare_commands};
use crate::commands::process::spawn_detached;
use crate::output::execute_command_in_worktree;

/// Controls how hook execution should respond to failures.
#[derive(Clone, Copy)]
pub enum HookFailureStrategy {
    /// Stop on first failure and surface a `HookCommandFailed` error.
    FailFast,
    /// Log warnings and continue executing remaining commands.
    /// For PostMerge hooks, propagates exit code after all commands complete.
    Warn,
}

/// Distinguishes between user hooks and project hooks for command preparation.
///
/// Approval for project hooks is handled at the gate (command entry point),
/// not during hook execution.
#[derive(strum::Display)]
#[strum(serialize_all = "kebab-case")]
pub enum HookSource {
    /// User hooks from ~/.config/worktrunk/config.toml (no approval required)
    User,
    /// Project hooks from .worktrunk.toml (approval handled at gate)
    Project,
}

impl HookSource {
    /// Format a label for display: "user pre-merge" or "project pre-merge"
    fn format_label(&self, hook_type: HookType) -> String {
        format!("{} {}", self, hook_type)
    }
}

/// Helper for preparing and executing hook commands.
pub struct HookPipeline<'a> {
    ctx: CommandContext<'a>,
}

impl<'a> HookPipeline<'a> {
    pub fn new(ctx: CommandContext<'a>) -> Self {
        Self { ctx }
    }

    fn prepare_commands(
        &self,
        command_config: &CommandConfig,
        hook_type: HookType,
        _source: &HookSource,
        extra_vars: &[(&str, &str)],
        name_filter: Option<&str>,
    ) -> anyhow::Result<Vec<PreparedCommand>> {
        let commands = prepare_commands(command_config, &self.ctx, extra_vars, hook_type)?;
        Ok(Self::filter_by_name(commands, name_filter))
    }

    fn announce_command(
        &self,
        label_prefix: &str,
        prepared: &PreparedCommand,
    ) -> anyhow::Result<()> {
        let label = crate::commands::format_command_label(label_prefix, prepared.name.as_deref());
        crate::output::print(progress_message(format!("{label}:")))?;
        crate::output::gutter(format_bash_with_gutter(&prepared.expanded))?;
        Ok(())
    }

    /// Filter commands by name (returns empty vec if name not found - caller decides if that's an error)
    fn filter_by_name(
        commands: Vec<PreparedCommand>,
        name_filter: Option<&str>,
    ) -> Vec<PreparedCommand> {
        match name_filter {
            Some(name) => commands
                .into_iter()
                .filter(|cmd| cmd.name.as_deref() == Some(name))
                .collect(),
            None => commands,
        }
    }

    /// Run hook commands sequentially, using the provided failure strategy.
    /// Returns the number of commands that were run.
    pub fn run_sequential(
        &self,
        command_config: &CommandConfig,
        hook_type: HookType,
        source: HookSource,
        extra_vars: &[(&str, &str)],
        failure_strategy: HookFailureStrategy,
        name_filter: Option<&str>,
    ) -> anyhow::Result<usize> {
        let commands =
            self.prepare_commands(command_config, hook_type, &source, extra_vars, name_filter)?;
        if commands.is_empty() {
            return Ok(0);
        }
        let command_count = commands.len();

        // Track first failure for Warn strategy (to propagate exit code after all commands run)
        let mut first_failure: Option<(String, Option<String>, i32)> = None;

        let label_prefix = source.format_label(hook_type);

        for prepared in commands {
            self.announce_command(&label_prefix, &prepared)?;

            if let Err(err) = execute_command_in_worktree(
                self.ctx.worktree_path,
                &prepared.expanded,
                Some(&prepared.context_json),
            ) {
                // Extract raw message and exit code from error
                let (err_msg, exit_code) =
                    if let Some(wt_err) = err.downcast_ref::<WorktrunkError>() {
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
                            command_name: prepared.name.clone(),
                            error: err_msg,
                            exit_code,
                        }
                        .into());
                    }
                    HookFailureStrategy::Warn => {
                        let message = match &prepared.name {
                            Some(name) => {
                                cformat!("Command <bold>{name}</> failed: {err_msg}")
                            }
                            None => format!("Command failed: {err_msg}"),
                        };
                        crate::output::print(warning_message(message))?;

                        // Track first failure to propagate exit code later (only for PostMerge)
                        if first_failure.is_none() && hook_type == HookType::PostMerge {
                            first_failure =
                                Some((err_msg, prepared.name.clone(), exit_code.unwrap_or(1)));
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

        Ok(command_count)
    }

    /// Spawn hook commands in the background (used for post-start hooks).
    pub fn spawn_background(
        &self,
        command_config: &CommandConfig,
        hook_type: HookType,
        source: HookSource,
        extra_vars: &[(&str, &str)],
        name_filter: Option<&str>,
    ) -> anyhow::Result<()> {
        let commands =
            self.prepare_commands(command_config, hook_type, &source, extra_vars, name_filter)?;
        if commands.is_empty() {
            return Ok(());
        }

        // Derive operation name from hook type (e.g., "post-start")
        let operation_prefix = hook_type.to_string();
        let label_prefix = source.format_label(hook_type);

        for prepared in commands {
            self.announce_command(&label_prefix, &prepared)?;

            let name = prepared.name.as_deref().unwrap_or("cmd");
            // Include source in operation name to prevent log file collisions between
            // user and project hooks with the same name
            let operation = format!("{}-{}-{}", source, operation_prefix, name);
            if let Err(err) = spawn_detached(
                self.ctx.repo,
                self.ctx.worktree_path,
                &prepared.expanded,
                self.ctx.branch_or_head(),
                &operation,
                Some(&prepared.context_json),
            ) {
                let err_msg = err.to_string();
                let message = match &prepared.name {
                    Some(name) => format!("Failed to spawn \"{name}\": {err_msg}"),
                    None => format!("Failed to spawn command: {err_msg}"),
                };
                crate::output::print(warning_message(message))?;
            }
        }

        crate::output::flush()?;
        Ok(())
    }

    pub fn run_pre_commit(
        &self,
        project_config: &ProjectConfig,
        target_branch: Option<&str>,
        name_filter: Option<&str>,
    ) -> anyhow::Result<()> {
        let Some(pre_commit_config) = &project_config.hooks.pre_commit else {
            return Ok(());
        };

        let extra_vars: Vec<(&str, &str)> = target_branch
            .into_iter()
            .map(|target| ("target", target))
            .collect();

        self.run_sequential(
            pre_commit_config,
            HookType::PreCommit,
            HookSource::Project,
            &extra_vars,
            HookFailureStrategy::FailFast,
            name_filter,
        )?;
        Ok(())
    }
}

/// A single hook command failure (for concurrent execution).
#[derive(Debug, Clone)]
struct HookFailure {
    name: Option<String>,
    error: String,
    exit_code: Option<i32>,
}

/// Check if a name filter was provided but no commands matched.
/// Returns an error listing available command names if so.
fn check_name_filter_matched(
    name_filter: Option<&str>,
    total_commands_run: usize,
    user_config: Option<&CommandConfig>,
    project_config: Option<&CommandConfig>,
) -> anyhow::Result<()> {
    if let Some(name) = name_filter
        && total_commands_run == 0
    {
        let mut available = Vec::new();
        if let Some(config) = user_config {
            available.extend(
                config
                    .commands()
                    .iter()
                    .filter_map(|c| c.name.as_ref().map(|n| format!("user:{n}"))),
            );
        }
        if let Some(config) = project_config {
            available.extend(
                config
                    .commands()
                    .iter()
                    .filter_map(|c| c.name.as_ref().map(|n| format!("project:{n}"))),
            );
        }
        return Err(worktrunk::git::GitError::HookCommandNotFound {
            name: name.to_string(),
            available,
        }
        .into());
    }
    Ok(())
}

/// Run user and project hooks for a given hook type.
///
/// This is the canonical implementation for running hooks from both sources.
/// Runs user hooks first, then project hooks. Handles name filtering and
/// returns an error if a name filter was provided but no matching command found.
pub fn run_hook_with_filter(
    ctx: &CommandContext,
    user_config: Option<&CommandConfig>,
    project_config: Option<&CommandConfig>,
    hook_type: HookType,
    extra_vars: &[(&str, &str)],
    failure_strategy: HookFailureStrategy,
    name_filter: Option<&str>,
) -> anyhow::Result<()> {
    let pipeline = HookPipeline::new(*ctx);
    let mut total_commands_run = 0;

    // Run user hooks first (no approval required)
    if let Some(config) = user_config {
        total_commands_run += pipeline.run_sequential(
            config,
            hook_type,
            HookSource::User,
            extra_vars,
            failure_strategy,
            name_filter,
        )?;
    }

    // Then run project hooks (approval handled at gate)
    if let Some(config) = project_config {
        total_commands_run += pipeline.run_sequential(
            config,
            hook_type,
            HookSource::Project,
            extra_vars,
            failure_strategy,
            name_filter,
        )?;
    }

    check_name_filter_matched(name_filter, total_commands_run, user_config, project_config)
}

/// Run user and project hooks concurrently (for hook types that normally run in background).
///
/// All commands from both sources run in parallel together. Collects all failures and returns
/// a combined error at the end. Handles name filtering and returns an error if a name
/// filter was provided but no matching command found.
pub fn run_hook_concurrent_with_filter(
    ctx: &CommandContext,
    user_config: Option<&CommandConfig>,
    project_config: Option<&CommandConfig>,
    hook_type: HookType,
    extra_vars: &[(&str, &str)],
    name_filter: Option<&str>,
) -> anyhow::Result<()> {
    use std::process::Stdio;
    use std::thread;
    use worktrunk::shell_exec::ShellConfig;

    let pipeline = HookPipeline::new(*ctx);

    // Prepare all commands from both sources upfront
    let mut all_commands: Vec<(PreparedCommand, String)> = Vec::new();

    if let Some(config) = user_config {
        let label = HookSource::User.format_label(hook_type);
        let commands = prepare_commands(config, &pipeline.ctx, extra_vars, hook_type)?;
        let filtered = HookPipeline::filter_by_name(commands, name_filter);
        all_commands.extend(filtered.into_iter().map(|c| (c, label.clone())));
    }

    if let Some(config) = project_config {
        let label = HookSource::Project.format_label(hook_type);
        let commands = prepare_commands(config, &pipeline.ctx, extra_vars, hook_type)?;
        let filtered = HookPipeline::filter_by_name(commands, name_filter);
        all_commands.extend(filtered.into_iter().map(|c| (c, label.clone())));
    }

    let total_commands_run = all_commands.len();
    check_name_filter_matched(name_filter, total_commands_run, user_config, project_config)?;

    if all_commands.is_empty() {
        return Ok(());
    }

    // Announce all commands upfront
    for (prepared, label) in &all_commands {
        pipeline.announce_command(label, prepared)?;
    }
    crate::output::flush()?;

    // Reset ANSI codes to prevent color bleeding from our output into command output
    use std::io::Write;
    use worktrunk::styling::{eprint, stderr};
    eprint!("{}", anstyle::Reset);
    stderr().flush().ok();

    // Spawn all commands in parallel together.
    //
    // Note: Unlike sequential execution (execute_streaming), we don't use process_group(0)
    // or sophisticated signal forwarding here. Children inherit the foreground process group,
    // so Ctrl+C sends SIGINT to all of them together. This simpler behavior is acceptable for
    // "best effort" concurrent hooks - if the user interrupts, everything stops.
    //
    // Adding proper signal forwarding would require either spawning all children from the main
    // thread (not worker threads) or sharing child PIDs across threads with a coordinating
    // signal handler. The complexity isn't warranted for this use case.
    let shell = ShellConfig::get();
    let worktree_path = pipeline.ctx.worktree_path.to_path_buf();

    let handles: Vec<_> = all_commands
        .into_iter()
        .map(|(prepared, _label)| {
            let shell = shell.clone();
            let worktree_path = worktree_path.clone();

            thread::spawn(move || {
                use std::io::Write;

                let mut cmd = shell.command(&prepared.expanded);
                cmd.current_dir(&worktree_path)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::from(std::io::stderr()))
                    .stderr(Stdio::inherit())
                    .env_remove(worktrunk::shell_exec::DIRECTIVE_FILE_ENV_VAR);

                let mut child = match cmd.spawn() {
                    Ok(child) => child,
                    Err(e) => {
                        return Some(HookFailure {
                            name: prepared.name.clone(),
                            error: e.to_string(),
                            exit_code: None,
                        });
                    }
                };

                // Pipe context JSON to stdin (same as sequential execution)
                if let Some(mut stdin) = child.stdin.take() {
                    // Ignore write errors - command may not read stdin
                    let _ = stdin.write_all(prepared.context_json.as_bytes());
                }

                match child.wait() {
                    Ok(status) if status.success() => None,
                    Ok(status) => Some(HookFailure {
                        name: prepared.name.clone(),
                        error: format!("exit status: {}", status.code().unwrap_or(-1)),
                        exit_code: status.code(),
                    }),
                    Err(e) => Some(HookFailure {
                        name: prepared.name.clone(),
                        error: e.to_string(),
                        exit_code: None,
                    }),
                }
            })
        })
        .collect();

    // Wait for all and collect failures
    let all_failures: Vec<HookFailure> = handles
        .into_iter()
        .filter_map(|h| match h.join() {
            Ok(result) => result,
            Err(_) => {
                // Thread panicked - treat as failure (command name context is lost)
                Some(HookFailure {
                    name: None,
                    error: "thread panicked".to_string(),
                    exit_code: None,
                })
            }
        })
        .collect();

    // Report all failures at the end
    if !all_failures.is_empty() {
        let first = &all_failures[0];
        let error_msg = if all_failures.len() == 1 {
            match &first.name {
                Some(name) => format!("{}: {}", name, first.error),
                None => first.error.clone(),
            }
        } else {
            let names: Vec<_> = all_failures
                .iter()
                .map(|f| f.name.as_deref().unwrap_or("(unnamed)"))
                .collect();
            format!(
                "{} commands failed: {}",
                all_failures.len(),
                names.join(", ")
            )
        };

        return Err(WorktrunkError::HookCommandFailed {
            hook_type,
            command_name: first.name.clone(),
            error: error_msg,
            exit_code: first.exit_code,
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
    fn test_hook_source_format_label() {
        assert_eq!(
            HookSource::User.format_label(HookType::PreMerge),
            "user pre-merge"
        );
        assert_eq!(
            HookSource::Project.format_label(HookType::PostCreate),
            "project post-create"
        );
        assert_eq!(
            HookSource::User.format_label(HookType::PreCommit),
            "user pre-commit"
        );
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
}
