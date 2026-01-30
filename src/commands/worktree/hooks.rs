//! Hook execution for worktree operations.
//!
//! CommandContext implementations for post-create, post-start, post-switch, and post-remove hooks.

use std::path::Path;

use worktrunk::HookType;
use worktrunk::path::to_posix_path;

use crate::commands::command_executor::CommandContext;
use crate::commands::hooks::{
    HookFailureStrategy, execute_hook, prepare_hook_commands, spawn_hook_background,
    spawn_hook_commands_background,
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
        execute_hook(
            self,
            HookType::PostCreate,
            extra_vars,
            HookFailureStrategy::Warn,
            None,
            crate::output::post_hook_display_path(self.worktree_path),
        )
    }

    /// Spawn post-start commands in parallel as background processes (non-blocking)
    pub fn spawn_post_start_commands(
        &self,
        extra_vars: &[(&str, &str)],
        display_path: Option<&Path>,
    ) -> anyhow::Result<()> {
        spawn_hook_background(self, HookType::PostStart, extra_vars, None, display_path)
    }

    /// Spawn post-switch commands in parallel as background processes (non-blocking)
    pub fn spawn_post_switch_commands(
        &self,
        extra_vars: &[(&str, &str)],
        display_path: Option<&Path>,
    ) -> anyhow::Result<()> {
        spawn_hook_background(self, HookType::PostSwitch, extra_vars, None, display_path)
    }

    /// Spawn post-remove commands in parallel as background processes (non-blocking)
    ///
    /// Runs after worktree removal. Commands execute from the invoking worktree (where
    /// the user ends up after removal), but template variables reflect the removed
    /// worktree so hooks can reference paths and names correctly.
    ///
    /// `removed_branch`: The branch that was removed (for `{{ branch }}`).
    /// `removed_worktree_path`: The removed worktree's path (for `{{ worktree_path }}`, etc.).
    /// `removed_commit`: The commit SHA of the removed worktree's HEAD (for `{{ commit }}`).
    /// `display_path`: When `Some`, shows the path in hook announcements.
    pub fn spawn_post_remove_commands(
        &self,
        removed_branch: &str,
        removed_worktree_path: &Path,
        removed_commit: Option<&str>,
        display_path: Option<&Path>,
    ) -> anyhow::Result<()> {
        let project_config = self.repo.load_project_config()?;

        // Template variables should reflect the removed worktree, not where we run from.
        // The removed worktree path no longer exists, but hooks may need to reference it
        // (e.g., for cleanup scripts that use the path in container names).
        let worktree_path_str = to_posix_path(&removed_worktree_path.to_string_lossy());
        let worktree_name = removed_worktree_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        // Build extra_vars with all removed worktree context.
        // Commit is captured before removal to ensure it reflects the removed worktree's state.
        let commit = removed_commit.unwrap_or("");
        let short_commit = if commit.len() >= 7 {
            &commit[..7]
        } else {
            commit
        };
        let extra_vars: Vec<(&str, &str)> = vec![
            ("branch", removed_branch),
            ("worktree_path", &worktree_path_str),
            ("worktree", &worktree_path_str), // deprecated alias
            ("worktree_name", worktree_name),
            ("commit", commit),
            ("short_commit", short_commit),
        ];

        let user_hooks = self.config.hooks(self.project_id().as_deref());
        let commands = prepare_hook_commands(
            self,
            user_hooks.post_remove.as_ref(),
            project_config
                .as_ref()
                .and_then(|c| c.hooks.post_remove.as_ref()),
            HookType::PostRemove,
            &extra_vars,
            None,
            display_path,
        )?;

        spawn_hook_commands_background(self, commands, HookType::PostRemove)
    }
}
