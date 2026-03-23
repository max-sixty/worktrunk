//! Worktree remove operations.

use worktrunk::config::UserConfig;
use worktrunk::git::Repository;

use super::types::{BranchDeletionMode, RemoveResult};
use crate::commands::branch_deletion::delete_branch_if_safe;
use crate::commands::repository_ext::{RemoveTarget, RepositoryCliExt};

/// Remove a worktree by branch name.
pub fn handle_remove(
    worktree_name: &str,
    keep_branch: bool,
    force_delete: bool,
    force_worktree: bool,
    config: &UserConfig,
) -> anyhow::Result<RemoveResult> {
    let repo = Repository::current()?;

    // Progress message is shown in handle_removed_worktree_output() after pre-remove hooks run
    repo.prepare_worktree_removal(
        RemoveTarget::Branch(worktree_name),
        BranchDeletionMode::from_flags(keep_branch, force_delete),
        force_worktree,
        config,
    )
}

/// Execute worktree removal: stop fsmonitor, remove worktree, delete branch.
///
/// Core removal logic without output, hooks, or cd directives. Used by the
/// picker (which runs inside skim's TUI). Performs the same core steps as
/// `handle_removed_worktree_output`, which adds progress messages and hooks.
#[cfg_attr(windows, allow(dead_code))] // Used only by picker module (unix-only)
pub fn execute_removal(result: &RemoveResult) -> anyhow::Result<()> {
    let RemoveResult::RemovedWorktree {
        main_path,
        worktree_path,
        branch_name,
        deletion_mode,
        target_branch,
        force_worktree,
        ..
    } = result
    else {
        // BranchOnly: no worktree to remove
        return Ok(());
    };

    let repo = Repository::at(main_path)?;
    let _ = repo
        .worktree_at(worktree_path)
        .run_command(&["fsmonitor--daemon", "stop"]);
    repo.remove_worktree(worktree_path, *force_worktree)?;

    if let Some(branch) = branch_name
        && !deletion_mode.should_keep()
    {
        let target = target_branch.as_deref().unwrap_or("HEAD");
        let _ = delete_branch_if_safe(&repo, branch, target, deletion_mode.is_force());
    }

    Ok(())
}

/// Remove a worktree by path (supports detached HEAD worktrees).
pub fn handle_remove_path(
    path: &std::path::Path,
    keep_branch: bool,
    force_delete: bool,
    force_worktree: bool,
    config: &UserConfig,
) -> anyhow::Result<RemoveResult> {
    let repo = Repository::current()?;

    repo.prepare_worktree_removal(
        RemoveTarget::Path(path),
        BranchDeletionMode::from_flags(keep_branch, force_delete),
        force_worktree,
        config,
    )
}

/// Handle removing the current worktree (supports detached HEAD state).
///
/// This is the path-based removal that handles the "@" shorthand, including
/// when HEAD is detached.
pub fn handle_remove_current(
    keep_branch: bool,
    force_delete: bool,
    force_worktree: bool,
    config: &UserConfig,
) -> anyhow::Result<RemoveResult> {
    let repo = Repository::current()?;

    // Progress message is shown in handle_removed_worktree_output() after pre-remove hooks run
    repo.prepare_worktree_removal(
        RemoveTarget::Current,
        BranchDeletionMode::from_flags(keep_branch, force_delete),
        force_worktree,
        config,
    )
}
