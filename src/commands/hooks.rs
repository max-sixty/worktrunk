use worktrunk::HookType;
use worktrunk::config::{CommandConfig, CommandPhase, ProjectConfig};
use worktrunk::git::GitError;
use worktrunk::styling::{CYAN, WARNING, WARNING_BOLD, format_bash_with_gutter};

use super::command_executor::{CommandContext, PreparedCommand, prepare_project_commands};
use crate::commands::process::spawn_detached;
use crate::output::execute_command_in_worktree;

/// Controls how hook execution should respond to failures.
pub enum HookFailureStrategy {
    /// Stop on first failure and surface a `HookCommandFailed` error with the provided hook type.
    FailFast { hook_type: HookType },
    /// Log warnings and continue executing remaining commands.
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
    ) -> Result<Vec<PreparedCommand>, GitError> {
        prepare_project_commands(command_config, &self.ctx, auto_trust, extra_vars, phase)
    }

    /// Run hook commands sequentially, using the provided failure strategy.
    pub fn run_sequential(
        &self,
        command_config: &CommandConfig,
        phase: CommandPhase,
        auto_trust: bool,
        extra_vars: &[(&str, &str)],
        label_prefix: &str,
        failure_strategy: HookFailureStrategy,
    ) -> Result<(), GitError> {
        let commands = self.prepare_commands(command_config, phase, auto_trust, extra_vars)?;
        if commands.is_empty() {
            return Ok(());
        }

        for prepared in commands {
            let label =
                crate::commands::format_command_label(label_prefix, prepared.name.as_deref());
            crate::output::progress(format!("{CYAN}{label}:{CYAN:#}"))?;
            crate::output::gutter(format_bash_with_gutter(&prepared.expanded, ""))?;

            if let Err(err) =
                execute_command_in_worktree(self.ctx.worktree_path, &prepared.expanded)
            {
                let err_msg = err.to_string();
                match &failure_strategy {
                    HookFailureStrategy::FailFast { hook_type } => {
                        let exit_code = match &err {
                            GitError::ChildProcessExited { code, .. } => Some(*code),
                            _ => None,
                        };
                        return Err(GitError::HookCommandFailed {
                            hook_type: *hook_type,
                            command_name: prepared.name.clone(),
                            error: err_msg,
                            exit_code,
                        });
                    }
                    HookFailureStrategy::Warn => {
                        let message = match &prepared.name {
                            Some(name) => format!(
                                "{WARNING}Command {WARNING_BOLD}{name}{WARNING_BOLD:#} failed: {err_msg}{WARNING:#}"
                            ),
                            None => format!("{WARNING}Command failed: {err_msg}{WARNING:#}"),
                        };
                        crate::output::warning(message)?;
                    }
                }
            }
        }

        crate::output::flush()?;
        Ok(())
    }

    /// Spawn hook commands in the background (used for post-start hooks).
    pub fn spawn_detached(
        &self,
        command_config: &CommandConfig,
        phase: CommandPhase,
        auto_trust: bool,
        extra_vars: &[(&str, &str)],
        label_prefix: &str,
    ) -> Result<(), GitError> {
        let commands = self.prepare_commands(command_config, phase, auto_trust, extra_vars)?;
        if commands.is_empty() {
            return Ok(());
        }

        for prepared in commands {
            let label =
                crate::commands::format_command_label(label_prefix, prepared.name.as_deref());
            crate::output::progress(format!("{CYAN}{label}:{CYAN:#}"))?;
            crate::output::gutter(format_bash_with_gutter(&prepared.expanded, ""))?;

            let name = prepared.name.as_deref().unwrap_or("cmd");
            if let Err(err) = spawn_detached(self.ctx.worktree_path, &prepared.expanded, name) {
                let err_msg = err.to_string();
                let message = match &prepared.name {
                    Some(name) => {
                        format!("{WARNING}Failed to spawn '{name}': {err_msg}{WARNING:#}")
                    }
                    None => format!("{WARNING}Failed to spawn command: {err_msg}{WARNING:#}"),
                };
                crate::output::warning(message)?;
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
    ) -> Result<(), GitError> {
        let Some(pre_commit_config) = &project_config.pre_commit_command else {
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
            HookFailureStrategy::FailFast {
                hook_type: HookType::PreCommit,
            },
        )
    }
}
