//! Types for worktree operations.
//!
//! Core data structures used by switch, remove, and push operations.

use std::path::{Path, PathBuf};

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
    Existing(PathBuf),
    /// Created new worktree at the given path
    Created {
        path: PathBuf,
        /// True if we used `-b` to create a new branch (--create flag)
        created_branch: bool,
        /// Base branch when creating new branch (e.g., "main")
        base_branch: Option<String>,
        /// Absolute path to base branch's worktree (POSIX format for shell compatibility)
        base_worktree_path: Option<String>,
        /// Remote tracking branch if created from remote (e.g., "origin/feature")
        /// This is set when git's DWIM created a local branch from a remote
        from_remote: Option<String>,
    },
}

impl SwitchResult {
    /// Get the worktree path
    pub fn path(&self) -> &PathBuf {
        match self {
            SwitchResult::AlreadyAt(path) => path,
            SwitchResult::Existing(path) => path,
            SwitchResult::Created { path, .. } => path,
        }
    }
}

/// Branch state for a switch operation.
#[derive(Debug, Clone)]
pub struct SwitchBranchInfo {
    /// The branch being switched to
    pub branch: String,
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
    /// Fork PR: fetch from refs/pull/N/head, create branch, configure pushRemote
    ForkPr {
        pr_number: u32,
        fork_push_url: String,
        pr_url: String,
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
        branch: String,
        /// Expected path for mismatch detection
        expected_path: PathBuf,
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

    /// Get the branch name for this plan.
    pub fn branch(&self) -> &str {
        match self {
            SwitchPlan::Existing { branch, .. } | SwitchPlan::Create { branch, .. } => branch,
        }
    }

    /// Returns true if this plan will create a new worktree.
    pub fn is_create(&self) -> bool {
        matches!(self, SwitchPlan::Create { .. })
    }
}

/// How the branch should be handled after worktree removal.
///
/// This enum replaces the previous `no_delete_branch: bool, force_delete: bool` pattern,
/// making the three valid states explicit and preventing invalid combinations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchDeletionMode {
    /// Keep the branch regardless of merge status (--no-delete-branch flag).
    Keep,
    /// Delete the branch only if it's fully merged into the target branch (default).
    SafeDelete,
    /// Delete the branch even if it's not merged (-D flag).
    ForceDelete,
}

impl BranchDeletionMode {
    /// Create from CLI flags.
    ///
    /// `--no-delete-branch` takes precedence over `-D` (force delete).
    pub fn from_flags(no_delete_branch: bool, force_delete: bool) -> Self {
        if no_delete_branch {
            Self::Keep
        } else if force_delete {
            Self::ForceDelete
        } else {
            Self::SafeDelete
        }
    }

    /// Whether the branch should be kept (not deleted).
    pub fn should_keep(&self) -> bool {
        matches!(self, Self::Keep)
    }

    /// Whether to force delete even if not merged.
    pub fn is_force(&self) -> bool {
        matches!(self, Self::ForceDelete)
    }
}

/// Result of a worktree remove operation
pub enum RemoveResult {
    /// Removed worktree and returned to main (if needed)
    RemovedWorktree {
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
    },
    /// Branch exists but has no worktree - attempt branch deletion only
    BranchOnly {
        branch_name: String,
        deletion_mode: BranchDeletionMode,
    },
}

/// Context for worktree resolution - determines which checks are performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionContext {
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
        let result = SwitchResult::Existing(path.clone());
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
    fn test_merge_operations_debug() {
        let ops = MergeOperations {
            committed: true,
            squashed: false,
            rebased: true,
        };
        let debug = format!("{:?}", ops);
        assert!(debug.contains("committed: true"));
        assert!(debug.contains("squashed: false"));
        assert!(debug.contains("rebased: true"));
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
            }
            _ => panic!("Expected RemovedWorktree variant"),
        }
    }

    #[test]
    fn test_remove_result_branch_only() {
        let result = RemoveResult::BranchOnly {
            branch_name: "stale-branch".to_string(),
            deletion_mode: BranchDeletionMode::Keep,
        };
        match result {
            RemoveResult::BranchOnly {
                branch_name,
                deletion_mode,
            } => {
                assert_eq!(branch_name, "stale-branch");
                assert!(deletion_mode.should_keep());
                assert!(!deletion_mode.is_force());
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
