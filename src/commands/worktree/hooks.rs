//! Hook execution for worktree operations.
//!
//! CommandContext implementations for post-create, post-start, and post-switch hooks.

use worktrunk::HookType;

use crate::commands::command_executor::CommandContext;
use crate::commands::hooks::{
    HookFailureStrategy, prepare_hook_commands, spawn_hook_commands_background,
};

impl<'a> CommandContext<'a> {
    /// Execute post-create commands sequentially (blocking)
    ///
    /// Runs user hooks first, then project hooks.
    /// Shows path in hook announcements when shell integration isn't active (user's shell
    /// won't cd to the new worktree, so they need to know where hooks ran).
    ///
    /// `extra_vars`: Additional template variables (e.g., `base`, `base_worktree_path`).
    pub fn execute_post_create_commands(&self, extra_vars: &[(&str, &str)]) -> anyhow::Result<()> {
        let project_config = self.repo.load_project_config()?;
        crate::commands::hooks::run_hook_with_filter(
            self,
            self.config.hooks.post_create.as_ref(),
            project_config
                .as_ref()
                .and_then(|c| c.hooks.post_create.as_ref()),
            HookType::PostCreate,
            extra_vars,
            HookFailureStrategy::Warn,
            None,
            crate::output::post_hook_display_path(self.worktree_path),
        )
    }

    /// Spawn post-start commands in parallel as background processes (non-blocking)
    ///
    /// `extra_vars`: Additional template variables (e.g., `base`, `base_worktree_path`).
    /// `display_path`: When `Some`, shows the path in hook announcements. Pass this when
    /// the user's shell won't be in the worktree (shell integration not active).
    pub fn spawn_post_start_commands(
        &self,
        extra_vars: &[(&str, &str)],
        display_path: Option<&std::path::Path>,
    ) -> anyhow::Result<()> {
        let project_config = self.repo.load_project_config()?;

        let commands = prepare_hook_commands(
            self,
            self.config.hooks.post_start.as_ref(),
            project_config
                .as_ref()
                .and_then(|c| c.hooks.post_start.as_ref()),
            HookType::PostStart,
            extra_vars,
            None,
            display_path,
        )?;

        spawn_hook_commands_background(self, commands, HookType::PostStart)
    }

    /// Spawn post-switch commands in parallel as background processes (non-blocking)
    ///
    /// Runs on every switch, including to existing worktrees and newly created ones.
    ///
    /// `extra_vars`: Additional template variables (e.g., `base`, `base_worktree_path`).
    /// `display_path`: When `Some`, shows the path in hook announcements. Pass this when
    /// the user's shell won't be in the worktree (shell integration not active).
    pub fn spawn_post_switch_commands(
        &self,
        extra_vars: &[(&str, &str)],
        display_path: Option<&std::path::Path>,
    ) -> anyhow::Result<()> {
        let project_config = self.repo.load_project_config()?;

        let commands = prepare_hook_commands(
            self,
            self.config.hooks.post_switch.as_ref(),
            project_config
                .as_ref()
                .and_then(|c| c.hooks.post_switch.as_ref()),
            HookType::PostSwitch,
            extra_vars,
            None,
            display_path,
        )?;

        spawn_hook_commands_background(self, commands, HookType::PostSwitch)
    }
}
