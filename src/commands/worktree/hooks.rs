//! Hook execution for worktree operations.
//!
//! CommandContext implementations for pre-start hooks, and PostRemoveContext
//! for building template variables for post-remove hooks.

use std::path::Path;

use worktrunk::HookType;
use worktrunk::git::Repository;
use worktrunk::path::to_posix_path;

use crate::commands::command_executor::CommandContext;
use crate::commands::hooks::{HookFailureStrategy, execute_hook};

impl<'a> CommandContext<'a> {
    /// Execute pre-start commands sequentially (blocking)
    ///
    /// Runs user hooks first, then project hooks.
    /// Shows path in hook announcements when shell integration isn't active (user's shell
    /// won't cd to the new worktree, so they need to know where hooks ran).
    ///
    /// `extra_vars`: Additional template variables (e.g., `base`, `base_worktree_path`).
    pub fn execute_pre_start_commands(&self, extra_vars: &[(&str, &str)]) -> anyhow::Result<()> {
        execute_hook(
            self,
            HookType::PreStart,
            extra_vars,
            HookFailureStrategy::FailFast,
            &[],
            crate::output::post_hook_display_path(self.worktree_path),
        )
    }
}

/// Context for post-remove hooks, holding owned strings for template variables.
///
/// Post-remove hooks need template variables that reflect the *removed* worktree
/// (not the destination), since hooks may reference the removed path and branch
/// (e.g., for cleanup scripts that use the path in container names). This struct
/// owns the computed strings so callers can borrow them as extra_vars.
pub(crate) struct PostRemoveContext {
    worktree_path_str: String,
    worktree_name: String,
    commit: String,
    short_commit: String,
    target_path_str: String,
    target_branch: String,
}

impl PostRemoveContext {
    pub fn new(
        removed_worktree_path: &Path,
        removed_commit: Option<&str>,
        main_path: &Path,
        repo: &Repository,
    ) -> Self {
        let worktree_path_str = to_posix_path(&removed_worktree_path.to_string_lossy());
        let worktree_name = removed_worktree_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let commit = removed_commit.unwrap_or("").to_string();
        let short_commit = if commit.len() >= 7 {
            commit[..7].to_string()
        } else {
            commit.clone()
        };

        // Target vars: where the user ends up after removal (primary worktree).
        let target_path_str = to_posix_path(&main_path.to_string_lossy());
        let target_branch = repo
            .worktree_at(main_path)
            .branch()
            .ok()
            .flatten()
            .unwrap_or_default();

        Self {
            worktree_path_str,
            worktree_name,
            commit,
            short_commit,
            target_path_str,
            target_branch,
        }
    }

    /// Build extra_vars that override the base context with removed-worktree identity.
    ///
    /// `removed_branch` is borrowed from the caller (it outlives the returned Vec).
    pub fn extra_vars<'a>(&'a self, removed_branch: &'a str) -> Vec<(&'a str, &'a str)> {
        vec![
            ("branch", removed_branch),
            ("worktree_path", &self.worktree_path_str),
            ("worktree", &self.worktree_path_str), // deprecated alias
            ("worktree_name", &self.worktree_name),
            ("commit", &self.commit),
            ("short_commit", &self.short_commit),
            ("target", &self.target_branch),
            ("target_worktree_path", &self.target_path_str),
        ]
    }
}
