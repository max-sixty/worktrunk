//! Types for worktree operations.
//!
//! Core data structures used by switch, remove, and push operations.

use std::path::{Path, PathBuf};

use worktrunk::git::{BranchDeletionMode, RefType};

/// Flags indicating which merge operations occurred
#[derive(Debug, Clone, Copy)]
pub struct MergeOperations {
    pub committed: bool,
    pub squashed: bool,
    pub rebased: bool,
}

/// Result of a worktree switch operation
pub enum SwitchResult {
    /// Already at the target worktree (no action taken)
    AlreadyAt(PathBuf),
    /// Switched to existing worktree at the given path
    Existing { path: PathBuf },
    /// Created new worktree at the given path
    Created {
        path: PathBuf,
        /// True if the user requested branch creation (--create flag)
        created_branch: bool,
        /// Base branch when creating new branch (e.g., "main")
        base_branch: Option<String>,
        /// Absolute path to base branch's worktree (POSIX format for shell compatibility)
        base_worktree_path: Option<String>,
        /// Remote tracking branch if auto-created from remote (e.g., "origin/feature")
        from_remote: Option<String>,
    },
}

impl SwitchResult {
    /// Get the worktree path
    pub fn path(&self) -> &PathBuf {
        match self {
            SwitchResult::AlreadyAt(path) => path,
            SwitchResult::Existing { path, .. } => path,
            SwitchResult::Created { path, .. } => path,
        }
    }
}

/// Branch state for a switch operation.
#[derive(Debug, Clone)]
pub struct SwitchBranchInfo {
    /// The branch being switched to. `None` for detached HEAD worktrees.
    pub branch: Option<String>,
    /// Expected path when there's a branch-worktree mismatch (None = path matches template)
    pub expected_path: Option<PathBuf>,
}

/// How the worktree will be created.
#[derive(Debug)]
pub enum CreationMethod {
    /// Use `git worktree add` - handles existing branch, DWIM from remote, or -b for new
    Regular {
        /// True if using `-b` to create a new branch (--create flag)
        create_branch: bool,
        /// Base branch for creation (resolved, validated to exist)
        base_branch: Option<String>,
    },
    /// Fork PR/MR: fetch from refs/pull/N/head or refs/merge-requests/N/head,
    /// create branch, configure pushRemote.
    ///
    /// The remote is resolved during planning (before approval prompts) to ensure
    /// early failure if no matching remote exists.
    ForkRef {
        /// The reference type (PR or MR).
        ref_type: RefType,
        /// The PR/MR number.
        number: u32,
        /// The ref path (e.g., "pull/123/head" or "merge-requests/42/head").
        ref_path: String,
        /// URL to push to (the fork's URL). `None` when using a prefixed branch
        /// name (e.g., `contributor/main`) because push won't work.
        fork_push_url: Option<String>,
        /// Web URL for the PR/MR.
        ref_url: String,
        /// Resolved remote name where PR/MR refs live (e.g., "origin", "upstream").
        remote: String,
    },
}

/// Validated plan for a switch operation.
///
/// Created by `plan_switch()`, consumed by `execute_switch()`.
/// This separation allows validation to happen before approval prompts,
/// ensuring users aren't asked to approve hooks for operations that will fail.
#[derive(Debug)]
pub enum SwitchPlan {
    /// Branch already has a worktree - just switch to it (no git commands needed)
    Existing {
        path: PathBuf,
        /// The branch at this worktree. `None` for detached HEAD.
        branch: Option<String>,
        /// Branch to record as "previous" for `wt switch -`
        new_previous: Option<String>,
    },
    /// Need to create a new worktree
    Create {
        branch: String,
        worktree_path: PathBuf,
        /// How to create the worktree
        method: CreationMethod,
        /// If path exists and --clobber, this is the backup path to move it to
        clobber_backup: Option<PathBuf>,
        /// Branch to record as "previous" for `wt switch -`
        new_previous: Option<String>,
    },
}

impl SwitchPlan {
    /// Get the worktree path for this plan.
    pub fn worktree_path(&self) -> &Path {
        match self {
            SwitchPlan::Existing { path, .. } => path,
            SwitchPlan::Create { worktree_path, .. } => worktree_path,
        }
    }

    /// Get the branch name for this plan. `None` for detached HEAD worktrees.
    pub fn branch(&self) -> Option<&str> {
        match self {
            SwitchPlan::Existing { branch, .. } => branch.as_deref(),
            SwitchPlan::Create { branch, .. } => Some(branch),
        }
    }

    /// Returns true if this plan will create a new worktree.
    pub fn is_create(&self) -> bool {
        matches!(self, SwitchPlan::Create { .. })
    }
}

/// Result of a worktree remove operation
pub enum RemoveResult {
    /// Removed worktree and changed directory (if needed)
    RemovedWorktree {
        /// Stable working directory for post-removal execution: hooks run here,
        /// background removal spawns from here, and `cd` directs the shell here.
        /// Usually the primary worktree; falls back to cwd when removing the
        /// primary worktree itself (bare repo edge case), or the target branch's
        /// worktree in `wt merge`.
        main_path: PathBuf,
        worktree_path: PathBuf,
        changed_directory: bool,
        /// Branch name, if known. None for detached HEAD state.
        branch_name: Option<String>,
        deletion_mode: BranchDeletionMode,
        target_branch: Option<String>,
        /// Pre-computed integration reason (if branch is integrated with target).
        /// Computed upfront to avoid race conditions when removing multiple worktrees
        /// in background mode (background git operations can hold locks that cause
        /// subsequent integration checks to fail).
        integration_reason: Option<worktrunk::git::IntegrationReason>,
        /// Force git worktree removal even with untracked files.
        force_worktree: bool,
        /// Expected path based on config template. `Some` when actual path differs
        /// from expected (path mismatch), `None` when path matches template.
        expected_path: Option<PathBuf>,
        /// Commit SHA of the removed worktree's HEAD, captured before removal.
        /// Used for post-remove hook template variables so they reference the
        /// removed worktree's state, not the execution context.
        removed_commit: Option<String>,
    },
    /// Branch exists but has no worktree - attempt branch deletion only.
    ///
    /// `pruned` indicates whether the worktree was pruned (directory was missing).
    /// When true, shows an info message instead of a warning.
    BranchOnly {
        branch_name: String,
        deletion_mode: BranchDeletionMode,
        /// True if the worktree was pruned before returning this result.
        pruned: bool,
        /// Integration target for display. May be the effective target (e.g.,
        /// `origin/main` when upstream is ahead) or the local default branch.
        /// `None` when no default branch is configured.
        target_branch: Option<String>,
        /// Pre-computed integration reason, same as `RemovedWorktree`.
        /// Computed in `prepare_worktree_removal` so the output handler
        /// doesn't need to re-derive a `Repository` for the check.
        integration_reason: Option<worktrunk::git::IntegrationReason>,
    },
}

impl RemoveResult {
    /// Convert to a JSON value for structured output.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            RemoveResult::RemovedWorktree {
                worktree_path,
                branch_name,
                deletion_mode,
                ..
            } => serde_json::json!({
                "kind": "worktree",
                "branch": branch_name,
                "path": worktree_path,
                "branch_deleted": !deletion_mode.should_keep(),
            }),
            RemoveResult::BranchOnly {
                branch_name,
                deletion_mode,
                pruned,
                ..
            } => serde_json::json!({
                "kind": "branch_only",
                "branch": branch_name,
                "pruned": pruned,
                "branch_deleted": !deletion_mode.should_keep(),
            }),
        }
    }
}

/// Operation mode for worktree resolution - determines which checks are performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationMode {
    /// Creating or switching to a worktree - path occupation is an error
    /// because we need to create a worktree at the expected path.
    CreateOrSwitch,
    /// Removing a worktree - we only care if the branch has a worktree,
    /// path occupation is irrelevant since we're not creating anything.
    Remove,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_switch_result_path_already_at() {
        let path = PathBuf::from("/test/path");
        let result = SwitchResult::AlreadyAt(path.clone());
        assert_eq!(result.path(), &path);
    }

    #[test]
    fn test_switch_result_path_existing() {
        let path = PathBuf::from("/test/existing");
        let result = SwitchResult::Existing { path: path.clone() };
        assert_eq!(result.path(), &path);
    }

    #[test]
    fn test_switch_result_path_created() {
        let path = PathBuf::from("/test/created");
        let result = SwitchResult::Created {
            path: path.clone(),
            created_branch: true,
            base_branch: Some("main".to_string()),
            base_worktree_path: Some("/test/main".to_string()),
            from_remote: None,
        };
        assert_eq!(result.path(), &path);
    }

    #[test]
    fn test_switch_result_created_with_remote() {
        let path = PathBuf::from("/test/remote");
        let result = SwitchResult::Created {
            path: path.clone(),
            created_branch: false,
            base_branch: None,
            base_worktree_path: None,
            from_remote: Some("origin/feature".to_string()),
        };
        assert_eq!(result.path(), &path);
    }

    #[test]
    fn test_merge_operations_struct() {
        let ops = MergeOperations {
            committed: true,
            squashed: false,
            rebased: true,
        };
        assert!(ops.committed);
        assert!(!ops.squashed);
        assert!(ops.rebased);
    }

    #[test]
    fn test_merge_operations_clone() {
        let ops = MergeOperations {
            committed: true,
            squashed: true,
            rebased: false,
        };
        // MergeOperations implements both Clone and Copy
        // Use Clone explicitly to test the Clone impl
        let cloned = Clone::clone(&ops);
        assert_eq!(ops.committed, cloned.committed);
        assert_eq!(ops.squashed, cloned.squashed);
        assert_eq!(ops.rebased, cloned.rebased);
    }

    #[test]
    fn test_merge_operations_copy() {
        let ops = MergeOperations {
            committed: false,
            squashed: false,
            rebased: true,
        };
        let copied = ops; // Copy trait
        assert_eq!(ops.committed, copied.committed);
        assert_eq!(ops.squashed, copied.squashed);
        assert_eq!(ops.rebased, copied.rebased);
    }

    #[test]
    fn test_remove_result_removed_worktree() {
        let result = RemoveResult::RemovedWorktree {
            main_path: PathBuf::from("/main"),
            worktree_path: PathBuf::from("/worktree"),
            changed_directory: true,
            branch_name: Some("feature".to_string()),
            deletion_mode: BranchDeletionMode::SafeDelete,
            target_branch: Some("main".to_string()),
            integration_reason: Some(worktrunk::git::IntegrationReason::SameCommit),
            force_worktree: false,
            expected_path: None,
            removed_commit: Some("abc1234567890".to_string()),
        };
        match result {
            RemoveResult::RemovedWorktree {
                main_path,
                worktree_path,
                changed_directory,
                branch_name,
                deletion_mode,
                target_branch,
                integration_reason,
                force_worktree,
                expected_path,
                removed_commit,
            } => {
                assert_eq!(main_path.to_str().unwrap(), "/main");
                assert_eq!(worktree_path.to_str().unwrap(), "/worktree");
                assert!(changed_directory);
                assert_eq!(branch_name.as_deref(), Some("feature"));
                assert!(!deletion_mode.should_keep());
                assert!(!deletion_mode.is_force());
                assert_eq!(target_branch.as_deref(), Some("main"));
                assert!(integration_reason.is_some());
                assert!(!force_worktree);
                assert!(expected_path.is_none());
                assert_eq!(removed_commit.as_deref(), Some("abc1234567890"));
            }
            _ => panic!("Expected RemovedWorktree variant"),
        }
    }

    #[test]
    fn test_remove_result_branch_only() {
        let result = RemoveResult::BranchOnly {
            branch_name: "stale-branch".to_string(),
            deletion_mode: BranchDeletionMode::Keep,
            pruned: false,
            target_branch: None,
            integration_reason: None,
        };
        match result {
            RemoveResult::BranchOnly {
                branch_name,
                deletion_mode,
                pruned,
                target_branch,
                integration_reason,
            } => {
                assert_eq!(branch_name, "stale-branch");
                assert!(deletion_mode.should_keep());
                assert!(!deletion_mode.is_force());
                assert!(!pruned);
                assert!(target_branch.is_none());
                assert!(integration_reason.is_none());
            }
            _ => panic!("Expected BranchOnly variant"),
        }
    }

    #[test]
    fn test_remove_result_branch_only_pruned() {
        let result = RemoveResult::BranchOnly {
            branch_name: "pruned-branch".to_string(),
            deletion_mode: BranchDeletionMode::SafeDelete,
            pruned: true,
            target_branch: Some("main".to_string()),
            integration_reason: None,
        };
        match result {
            RemoveResult::BranchOnly {
                branch_name,
                deletion_mode,
                pruned,
                target_branch,
                integration_reason,
            } => {
                assert_eq!(branch_name, "pruned-branch");
                assert!(!deletion_mode.should_keep());
                assert!(pruned);
                assert_eq!(target_branch.as_deref(), Some("main"));
                assert!(integration_reason.is_none());
            }
            _ => panic!("Expected BranchOnly variant"),
        }
    }

    #[test]
    fn test_remove_result_with_force_delete() {
        let result = RemoveResult::RemovedWorktree {
            main_path: PathBuf::from("/main"),
            worktree_path: PathBuf::from("/worktree"),
            changed_directory: false,
            branch_name: None, // Detached HEAD
            deletion_mode: BranchDeletionMode::ForceDelete,
            target_branch: None,
            integration_reason: None, // Force delete skips integration check
            force_worktree: true,
            expected_path: None,
            removed_commit: None, // Detached HEAD may not have meaningful commit
        };
        match result {
            RemoveResult::RemovedWorktree {
                branch_name,
                deletion_mode,
                force_worktree,
                ..
            } => {
                assert!(branch_name.is_none());
                assert!(deletion_mode.is_force());
                assert!(force_worktree);
            }
            _ => panic!("Expected RemovedWorktree variant"),
        }
    }
}
