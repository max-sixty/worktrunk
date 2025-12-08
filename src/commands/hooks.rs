use color_print::cformat;
use worktrunk::HookType;
use worktrunk::config::{CommandConfig, CommandPhase, ProjectConfig};
use worktrunk::git::{GitError, WorktrunkError};
use worktrunk::styling::{format_bash_with_gutter, progress_message, warning_message};

use super::command_executor::{CommandContext, PreparedCommand, prepare_project_commands};
use crate::commands::process::spawn_detached;
use crate::output::execute_command_in_worktree;

/// Controls how hook execution should respond to failures.
pub enum HookFailureStrategy {
    /// Stop on first failure and surface a `HookCommandFailed` error.
    FailFast,
    /// Log warnings and continue executing remaining commands.
    /// For PostMerge hooks, propagates exit code after all commands complete.
    Warn,
}

/// Helper for preparing and executing project hook commands.
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
        phase: CommandPhase,
        auto_trust: bool,
        extra_vars: &[(&str, &str)],
        name_filter: Option<&str>,
    ) -> anyhow::Result<Vec<PreparedCommand>> {
        let commands =
            prepare_project_commands(command_config, &self.ctx, auto_trust, extra_vars, phase)?;

        // Filter by name if specified
        match name_filter {
            Some(name) => {
                // Collect available names before consuming the iterator
                let available: Vec<String> =
                    commands.iter().filter_map(|cmd| cmd.name.clone()).collect();

                let filtered: Vec<_> = commands
                    .into_iter()
                    .filter(|cmd| cmd.name.as_deref() == Some(name))
                    .collect();

                if filtered.is_empty() {
                    return Err(GitError::HookCommandNotFound {
                        name: name.to_string(),
                        available,
                    }
                    .into());
                }

                Ok(filtered)
            }
            None => Ok(commands),
        }
    }

    /// Run hook commands sequentially, using the provided failure strategy.
    #[allow(clippy::too_many_arguments)]
    pub fn run_sequential(
        &self,
        command_config: &CommandConfig,
        phase: CommandPhase,
        auto_trust: bool,
        extra_vars: &[(&str, &str)],
        label_prefix: &str,
        hook_type: HookType,
        failure_strategy: HookFailureStrategy,
        name_filter: Option<&str>,
    ) -> anyhow::Result<()> {
        let commands =
            self.prepare_commands(command_config, phase, auto_trust, extra_vars, name_filter)?;
        if commands.is_empty() {
            return Ok(());
        }

        // Track first failure for Warn strategy (to propagate exit code after all commands run)
        let mut first_failure: Option<(String, Option<String>, i32)> = None;

        for prepared in commands {
            let label =
                crate::commands::format_command_label(label_prefix, prepared.name.as_deref());
            crate::output::print(progress_message(format!("{label}:")))?;
            crate::output::gutter(format_bash_with_gutter(&prepared.expanded, ""))?;

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

        Ok(())
    }

    /// Spawn hook commands in the background (used for post-start hooks).
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_detached(
        &self,
        command_config: &CommandConfig,
        phase: CommandPhase,
        auto_trust: bool,
        extra_vars: &[(&str, &str)],
        label_prefix: &str,
        name_filter: Option<&str>,
    ) -> anyhow::Result<()> {
        let commands =
            self.prepare_commands(command_config, phase, auto_trust, extra_vars, name_filter)?;
        if commands.is_empty() {
            return Ok(());
        }

        for prepared in commands {
            let label =
                crate::commands::format_command_label(label_prefix, prepared.name.as_deref());
            crate::output::print(progress_message(format!("{label}:")))?;
            crate::output::gutter(format_bash_with_gutter(&prepared.expanded, ""))?;

            let name = prepared.name.as_deref().unwrap_or("cmd");
            let operation = format!("post-start-{}", name);
            if let Err(err) = spawn_detached(
                self.ctx.repo,
                self.ctx.worktree_path,
                &prepared.expanded,
                self.ctx.branch,
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
        auto_trust: bool,
        name_filter: Option<&str>,
    ) -> anyhow::Result<()> {
        let Some(pre_commit_config) = &project_config.pre_commit else {
            return Ok(());
        };

        let extra_vars: Vec<(&str, &str)> = target_branch
            .into_iter()
            .map(|target| ("target", target))
            .collect();

        self.run_sequential(
            pre_commit_config,
            CommandPhase::PreCommit,
            auto_trust,
            &extra_vars,
            "pre-commit",
            HookType::PreCommit,
            HookFailureStrategy::FailFast,
            name_filter,
        )
    }
}
