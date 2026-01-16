//! Worktree remove operations.

use worktrunk::config::WorktrunkConfig;
use worktrunk::git::Repository;

use super::types::{BranchDeletionMode, RemoveResult};
use crate::commands::repository_ext::{RemoveTarget, RepositoryCliExt};

/// Remove a worktree by branch name.
pub fn handle_remove(
    worktree_name: &str,
    no_delete_branch: bool,
    force_delete: bool,
    force_worktree: bool,
    config: &WorktrunkConfig,
) -> anyhow::Result<RemoveResult> {
    let repo = Repository::current()?;

    // Progress message is shown in handle_removed_worktree_output() after pre-remove hooks run
    repo.prepare_worktree_removal(
        RemoveTarget::Branch(worktree_name),
        BranchDeletionMode::from_flags(no_delete_branch, force_delete),
        force_worktree,
        config,
    )
}

/// Handle removing the current worktree (supports detached HEAD state).
///
/// This is the path-based removal that handles the "@" shorthand, including
/// when HEAD is detached.
pub fn handle_remove_current(
    no_delete_branch: bool,
    force_delete: bool,
    force_worktree: bool,
    config: &WorktrunkConfig,
) -> anyhow::Result<RemoveResult> {
    let repo = Repository::current()?;

    // Progress message is shown in handle_removed_worktree_output() after pre-remove hooks run
    repo.prepare_worktree_removal(
        RemoveTarget::Current,
        BranchDeletionMode::from_flags(no_delete_branch, force_delete),
        force_worktree,
        config,
    )
}
