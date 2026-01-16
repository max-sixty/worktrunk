//! Work item generation and execution.
//!
//! Contains the flat parallelism infrastructure:
//! - `WorkItem` - unit of work for the thread pool
//! - `dispatch_task()` - route TaskKind to the correct Task implementation
//! - `work_items_for_worktree()` / `work_items_for_branch()` - generate work items
//! - `ExpectedResults` - track expected results for timeout diagnostics

use std::sync::Arc;

use crossbeam_channel as chan;
use worktrunk::git::{BranchRef, Repository, WorktreeInfo};

use super::CollectOptions;
use super::tasks::{
    AheadBehindTask, BranchDiffTask, CiStatusTask, CommitDetailsTask, CommittedTreesMatchTask,
    GitOperationTask, HasFileChangesTask, IsAncestorTask, MergeTreeConflictsTask, Task,
    TaskContext, UpstreamTask, UrlStatusTask, UserMarkerTask, WorkingTreeConflictsTask,
    WorkingTreeDiffTask, WouldMergeAddTask,
};
use super::types::{TaskError, TaskKind, TaskResult};

// Tasks that are expensive because they require merge-base computation or merge simulation.
// These are skipped for branches that are far behind the default branch (in wt select).
// AheadBehind is NOT here - we use batch data for it instead of skipping.
// CommittedTreesMatch is NOT here - it's a cheap tree comparison that aids integration detection.
const EXPENSIVE_TASKS: &[TaskKind] = &[
    TaskKind::HasFileChanges,     // git diff with three-dot range
    TaskKind::IsAncestor,         // git merge-base --is-ancestor
    TaskKind::WouldMergeAdd,      // git merge-tree simulation
    TaskKind::BranchDiff,         // git diff with three-dot range
    TaskKind::MergeTreeConflicts, // git merge-tree simulation
];

// ============================================================================
// Work Item Dispatch (for flat parallelism)
// ============================================================================

/// A unit of work for the thread pool.
///
/// Each work item represents a single task to be executed. Work items are
/// collected upfront and then processed in parallel via Rayon's thread pool,
/// avoiding nested parallelism (Rayon par_iter → thread::scope).
#[derive(Clone)]
pub struct WorkItem {
    pub ctx: TaskContext,
    pub kind: TaskKind,
}

impl WorkItem {
    /// Execute this work item, returning the task result.
    pub fn execute(self) -> Result<TaskResult, TaskError> {
        let result = dispatch_task(self.kind, self.ctx);
        if let Ok(ref task_result) = result {
            debug_assert_eq!(TaskKind::from(task_result), self.kind);
        }
        result
    }
}

/// Dispatch a task by kind, calling the appropriate Task::compute().
fn dispatch_task(kind: TaskKind, ctx: TaskContext) -> Result<TaskResult, TaskError> {
    match kind {
        TaskKind::CommitDetails => CommitDetailsTask::compute(ctx),
        TaskKind::AheadBehind => AheadBehindTask::compute(ctx),
        TaskKind::CommittedTreesMatch => CommittedTreesMatchTask::compute(ctx),
        TaskKind::HasFileChanges => HasFileChangesTask::compute(ctx),
        TaskKind::WouldMergeAdd => WouldMergeAddTask::compute(ctx),
        TaskKind::IsAncestor => IsAncestorTask::compute(ctx),
        TaskKind::BranchDiff => BranchDiffTask::compute(ctx),
        TaskKind::WorkingTreeDiff => WorkingTreeDiffTask::compute(ctx),
        TaskKind::MergeTreeConflicts => MergeTreeConflictsTask::compute(ctx),
        TaskKind::WorkingTreeConflicts => WorkingTreeConflictsTask::compute(ctx),
        TaskKind::GitOperation => GitOperationTask::compute(ctx),
        TaskKind::UserMarker => UserMarkerTask::compute(ctx),
        TaskKind::Upstream => UpstreamTask::compute(ctx),
        TaskKind::CiStatus => CiStatusTask::compute(ctx),
        TaskKind::UrlStatus => UrlStatusTask::compute(ctx),
    }
}

// ============================================================================
// Expected Results Tracking
// ============================================================================

/// Tracks expected result types per item for timeout diagnostics.
///
/// Populated at spawn time so we know exactly which results to expect,
/// without hardcoding result lists that could drift from the spawn functions.
#[derive(Default)]
pub(crate) struct ExpectedResults {
    inner: std::sync::Mutex<Vec<Vec<TaskKind>>>,
}

impl ExpectedResults {
    /// Record that we expect a result of the given kind for the given item.
    /// Called internally by `TaskSpawner::spawn()`.
    pub fn expect(&self, item_idx: usize, kind: TaskKind) {
        let mut inner = self.inner.lock().unwrap();
        if inner.len() <= item_idx {
            inner.resize_with(item_idx + 1, Vec::new);
        }
        inner[item_idx].push(kind);
    }

    /// Total number of expected results (for progress display).
    pub fn count(&self) -> usize {
        self.inner.lock().unwrap().iter().map(|v| v.len()).sum()
    }

    /// Expected results for a specific item.
    pub fn results_for(&self, item_idx: usize) -> Vec<TaskKind> {
        self.inner
            .lock()
            .unwrap()
            .get(item_idx)
            .cloned()
            .unwrap_or_default()
    }
}

// ============================================================================
// Work Item Generation
// ============================================================================

/// Generate work items for a worktree.
///
/// Returns a list of work items representing all tasks that should run for this
/// worktree. Expected results are registered internally as each work item is added.
/// The caller is responsible for executing the work items.
///
/// The `repo` parameter is cloned into each TaskContext, sharing its cache via Arc.
pub fn work_items_for_worktree(
    repo: &Repository,
    wt: &WorktreeInfo,
    item_idx: usize,
    options: &CollectOptions,
    expected_results: &Arc<ExpectedResults>,
    tx: &chan::Sender<Result<TaskResult, TaskError>>,
) -> Vec<WorkItem> {
    // Skip git operations for prunable worktrees (directory missing).
    if wt.is_prunable() {
        return vec![];
    }

    let skip = &options.skip_tasks;

    // Expand URL template for this item
    let item_url = options.url_template.as_ref().and_then(|template| {
        wt.branch.as_ref().and_then(|branch| {
            let mut vars = std::collections::HashMap::new();
            vars.insert("branch", branch.as_str());
            worktrunk::config::expand_template(template, &vars, false, repo).ok()
        })
    });

    // Send URL immediately (before health check) so it appears right away.
    // The UrlStatusTask will later update with active status.
    if let Some(ref url) = item_url {
        expected_results.expect(item_idx, TaskKind::UrlStatus);
        let _ = tx.send(Ok(TaskResult::UrlStatus {
            item_idx,
            url: Some(url.clone()),
            active: None,
        }));
    }

    let ctx = TaskContext {
        repo: repo.clone(),
        branch_ref: BranchRef::from(wt),
        item_idx,
        item_url,
    };

    // Check if this branch is stale and should skip expensive tasks.
    let is_stale = wt
        .branch
        .as_deref()
        .is_some_and(|b| options.stale_branches.contains(b));

    let mut items = Vec::with_capacity(15);

    // Helper to add a work item and register the expected result
    let mut add_item = |kind: TaskKind| {
        expected_results.expect(item_idx, kind);
        items.push(WorkItem {
            ctx: ctx.clone(),
            kind,
        });
    };

    for kind in [
        TaskKind::CommitDetails,
        TaskKind::AheadBehind,
        TaskKind::CommittedTreesMatch,
        TaskKind::HasFileChanges,
        TaskKind::IsAncestor,
        TaskKind::Upstream,
        TaskKind::WorkingTreeDiff,
        TaskKind::GitOperation,
        TaskKind::UserMarker,
        TaskKind::WorkingTreeConflicts,
        TaskKind::BranchDiff,
        TaskKind::MergeTreeConflicts,
        TaskKind::CiStatus,
        TaskKind::WouldMergeAdd,
    ] {
        if skip.contains(&kind) {
            continue;
        }
        // Skip expensive tasks for stale branches (far behind default branch)
        if is_stale && EXPENSIVE_TASKS.contains(&kind) {
            continue;
        }
        add_item(kind);
    }
    // URL status health check task (if we have a URL).
    // Note: We already registered and sent an immediate UrlStatus above with url + active=None.
    // This work item will send a second UrlStatus with active=Some(bool) after health check.
    // Both results must be registered and expected.
    if !skip.contains(&TaskKind::UrlStatus) && ctx.item_url.is_some() {
        expected_results.expect(item_idx, TaskKind::UrlStatus);
        items.push(WorkItem {
            ctx: ctx.clone(),
            kind: TaskKind::UrlStatus,
        });
    }

    items
}

/// Generate work items for a branch (no worktree).
///
/// Returns a list of work items representing all tasks that should run for this
/// branch. Branches have fewer tasks than worktrees (no working tree operations).
///
/// The `repo` parameter is cloned into each TaskContext, sharing its cache via Arc.
pub fn work_items_for_branch(
    repo: &Repository,
    branch_name: &str,
    commit_sha: &str,
    item_idx: usize,
    options: &CollectOptions,
    expected_results: &Arc<ExpectedResults>,
) -> Vec<WorkItem> {
    let skip = &options.skip_tasks;

    let ctx = TaskContext {
        repo: repo.clone(),
        branch_ref: BranchRef::branch_only(branch_name, commit_sha),
        item_idx,
        item_url: None, // Branches without worktrees don't have URLs
    };

    // Check if this branch is stale and should skip expensive tasks.
    let is_stale = options.stale_branches.contains(branch_name);

    let mut items = Vec::with_capacity(11);

    // Helper to add a work item and register the expected result
    let mut add_item = |kind: TaskKind| {
        expected_results.expect(item_idx, kind);
        items.push(WorkItem {
            ctx: ctx.clone(),
            kind,
        });
    };

    for kind in [
        TaskKind::CommitDetails,
        TaskKind::AheadBehind,
        TaskKind::CommittedTreesMatch,
        TaskKind::HasFileChanges,
        TaskKind::IsAncestor,
        TaskKind::Upstream,
        TaskKind::BranchDiff,
        TaskKind::MergeTreeConflicts,
        TaskKind::CiStatus,
        TaskKind::WouldMergeAdd,
    ] {
        if skip.contains(&kind) {
            continue;
        }
        // Skip expensive tasks for stale branches (far behind default branch)
        if is_stale && EXPENSIVE_TASKS.contains(&kind) {
            continue;
        }
        add_item(kind);
    }

    items
}
