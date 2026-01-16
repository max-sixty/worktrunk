//! Task result types and context structures.
//!
//! Contains all the type definitions used by the collection system:
//! - `TaskResult` and `TaskKind` - result variants from task computations
//! - `TaskError` and `ErrorCause` - error handling for failed tasks
//! - `StatusContext` - context for status symbol computation
//! - `DrainOutcome` and `MissingResult` - timeout diagnostic info

use worktrunk::git::LineDiff;

use super::super::ci_status::PrStatus;
use super::super::model::{
    AheadBehind, BranchDiffTotals, CommitDetails, GitOperationState, ListItem, UpstreamStatus,
    WorkingTreeStatus,
};

/// Context for status symbol computation during result processing
#[derive(Clone, Default)]
pub(super) struct StatusContext {
    pub has_merge_tree_conflicts: bool,
    /// Working tree conflict check result (--full only, worktrees only).
    /// None = use commit check (task didn't run or working tree clean)
    /// Some(b) = dirty working tree, b is conflict result
    // TODO: If we need to distinguish "task didn't run" from "clean working tree",
    // expand to an enum. Currently both cases fall back to commit-based check.
    pub has_working_tree_conflicts: Option<bool>,
    pub user_marker: Option<String>,
    pub working_tree_status: Option<WorkingTreeStatus>,
    pub has_conflicts: bool,
}

impl StatusContext {
    pub fn apply_to(&self, item: &mut ListItem, target: &str) {
        // Main worktree case is handled inside check_integration_state()
        //
        // Prefer working tree conflicts (--full) when available.
        // None means task didn't run or working tree was clean - use commit check.
        let has_conflicts = self
            .has_working_tree_conflicts
            .unwrap_or(self.has_merge_tree_conflicts);

        item.compute_status_symbols(
            Some(target),
            has_conflicts,
            self.user_marker.clone(),
            self.working_tree_status,
            self.has_conflicts,
        );
    }
}

/// Task results sent as each git operation completes.
/// These enable progressive rendering - update UI as data arrives.
///
/// Each spawned task produces exactly one TaskResult. Multiple results
/// may feed into a single table column, and one result may feed multiple
/// columns. See `drain_results()` for how results map to ListItem fields.
///
/// The `EnumDiscriminants` derive generates a companion `TaskKind` enum
/// with the same variants but no payloads, used for type-safe tracking
/// of expected vs received results.
#[derive(Debug, Clone, strum::EnumDiscriminants)]
#[strum_discriminants(
    name(TaskKind),
    vis(pub),
    derive(Hash, Ord, PartialOrd, strum::IntoStaticStr),
    strum(serialize_all = "kebab-case")
)]
pub(crate) enum TaskResult {
    /// Commit timestamp and message
    CommitDetails {
        item_idx: usize,
        commit: CommitDetails,
    },
    /// Ahead/behind counts vs default branch
    AheadBehind {
        item_idx: usize,
        counts: AheadBehind,
        /// True if this is an orphan branch (no common ancestor with default branch)
        is_orphan: bool,
    },
    /// Whether HEAD's tree SHA matches integration target's tree SHA (committed content identical)
    CommittedTreesMatch {
        item_idx: usize,
        committed_trees_match: bool,
    },
    /// Whether branch has file changes beyond the merge-base with integration target (three-dot diff)
    HasFileChanges {
        item_idx: usize,
        has_file_changes: bool,
    },
    /// Whether merging branch into integration target would add changes (merge simulation)
    WouldMergeAdd {
        item_idx: usize,
        would_merge_add: bool,
    },
    /// Whether branch HEAD is ancestor of integration target (same commit or already merged)
    IsAncestor { item_idx: usize, is_ancestor: bool },
    /// Line diff vs default branch
    BranchDiff {
        item_idx: usize,
        branch_diff: BranchDiffTotals,
    },
    /// Working tree diff and status
    WorkingTreeDiff {
        item_idx: usize,
        working_tree_diff: LineDiff,
        /// Working tree change flags
        working_tree_status: WorkingTreeStatus,
        has_conflicts: bool,
    },
    /// Potential merge conflicts with default branch (merge-tree simulation on committed HEAD)
    MergeTreeConflicts {
        item_idx: usize,
        has_merge_tree_conflicts: bool,
    },
    /// Potential merge conflicts including working tree changes (--full only)
    ///
    /// For dirty worktrees, uses `git stash create` to get a tree object that
    /// includes uncommitted changes, then runs merge-tree against that.
    /// Returns None if working tree is clean (fall back to MergeTreeConflicts).
    WorkingTreeConflicts {
        item_idx: usize,
        /// None = working tree clean (use MergeTreeConflicts result)
        /// Some(true) = dirty working tree would conflict
        /// Some(false) = dirty working tree would not conflict
        has_working_tree_conflicts: Option<bool>,
    },
    /// Git operation in progress (rebase/merge)
    GitOperation {
        item_idx: usize,
        git_operation: GitOperationState,
    },
    /// User-defined status from git config
    UserMarker {
        item_idx: usize,
        user_marker: Option<String>,
    },
    /// Upstream tracking status
    Upstream {
        item_idx: usize,
        upstream: UpstreamStatus,
    },
    /// CI/PR status (slow operation)
    CiStatus {
        item_idx: usize,
        pr_status: Option<PrStatus>,
    },
    /// URL status (expanded URL and health check result)
    UrlStatus {
        item_idx: usize,
        /// Expanded URL from template (None if no template or no branch)
        url: Option<String>,
        /// Whether the port is listening (None if no URL or couldn't parse port)
        active: Option<bool>,
    },
}

impl TaskResult {
    /// Get the item index for this result
    pub fn item_idx(&self) -> usize {
        match self {
            TaskResult::CommitDetails { item_idx, .. }
            | TaskResult::AheadBehind { item_idx, .. }
            | TaskResult::CommittedTreesMatch { item_idx, .. }
            | TaskResult::HasFileChanges { item_idx, .. }
            | TaskResult::WouldMergeAdd { item_idx, .. }
            | TaskResult::IsAncestor { item_idx, .. }
            | TaskResult::BranchDiff { item_idx, .. }
            | TaskResult::WorkingTreeDiff { item_idx, .. }
            | TaskResult::MergeTreeConflicts { item_idx, .. }
            | TaskResult::WorkingTreeConflicts { item_idx, .. }
            | TaskResult::GitOperation { item_idx, .. }
            | TaskResult::UserMarker { item_idx, .. }
            | TaskResult::Upstream { item_idx, .. }
            | TaskResult::CiStatus { item_idx, .. }
            | TaskResult::UrlStatus { item_idx, .. } => *item_idx,
        }
    }
}

impl TaskKind {
    /// Whether this task requires network access.
    ///
    /// Network tasks are sorted to run last to avoid blocking local tasks.
    pub fn is_network(self) -> bool {
        matches!(self, TaskKind::CiStatus | TaskKind::UrlStatus)
    }
}

/// Result of draining task results - indicates whether all results were received
/// or if a timeout occurred.
#[derive(Debug)]
pub(super) enum DrainOutcome {
    /// All results received (channel closed normally)
    Complete,
    /// Timeout occurred - contains diagnostic info about what was received
    TimedOut {
        /// Number of task results received before timeout
        received_count: usize,
        /// Items with missing results
        items_with_missing: Vec<MissingResult>,
    },
}

/// Item with missing task results (for timeout diagnostics)
#[derive(Debug)]
pub(super) struct MissingResult {
    pub item_idx: usize,
    pub name: String,
    pub missing_kinds: Vec<TaskKind>,
}

/// Cause of a task error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCause {
    /// Command exceeded the configured timeout.
    Timeout,
    /// Any other error (permission denied, git error, etc.).
    Other,
}

/// Error during task execution.
///
/// Tasks return this instead of swallowing errors. The drain layer
/// applies defaults and collects errors for display after rendering.
#[derive(Debug, Clone)]
pub struct TaskError {
    pub item_idx: usize,
    pub kind: TaskKind,
    pub message: String,
    /// What caused this error. Use `is_timeout()` to check.
    cause: ErrorCause,
}

impl TaskError {
    pub fn new(
        item_idx: usize,
        kind: TaskKind,
        message: impl Into<String>,
        cause: ErrorCause,
    ) -> Self {
        Self {
            item_idx,
            kind,
            message: message.into(),
            cause,
        }
    }

    /// Whether this error was caused by a timeout.
    pub fn is_timeout(&self) -> bool {
        self.cause == ErrorCause::Timeout
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_error_other_is_not_timeout() {
        let error = TaskError::new(0, TaskKind::AheadBehind, "test error", ErrorCause::Other);
        assert!(!error.is_timeout());
    }

    #[test]
    fn test_task_error_timeout_is_timeout() {
        let error = TaskError::new(0, TaskKind::AheadBehind, "timed out", ErrorCause::Timeout);
        assert!(error.is_timeout());
    }
}
