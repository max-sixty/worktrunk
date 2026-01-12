//! Progressive worktree collection with parallel git operations.
//!
//! This module provides a typed task framework for cell-by-cell progressive rendering.
//! Git operations run in parallel and send results as they complete.
//!
//! ## Architecture
//!
//! All tasks are executed in a single Rayon thread pool (flat parallelism):
//!
//! 1. **Work item generation**: `work_items_for_worktree()` and `work_items_for_branch()`
//!    generate `WorkItem` instances for each task, registering expected results upfront.
//!
//! 2. **Parallel execution**: All work items are collected into a `Vec` and processed
//!    via `into_par_iter()`. Rayon schedules across its thread pool (size set at startup,
//!    defaulting to 2x CPU cores unless `RAYON_NUM_THREADS` is set).
//!
//! 3. **Result delivery**: Each `WorkItem::execute()` returns a result; the caller sends it.
//!
//! This avoids nested parallelism (Rayon → thread::scope) which could create 100+ threads.

use crossbeam_channel::Sender;
use std::fmt::Display;
use std::sync::Arc;
use worktrunk::git::{BranchRef, LineDiff, Repository, WorktreeInfo};

use super::ci_status::PrStatus;
use super::collect::{ExpectedResults, TaskError, TaskKind, TaskResult, detect_git_operation};
use super::model::{
    AheadBehind, BranchDiffTotals, CommitDetails, UpstreamStatus, WorkingTreeStatus,
};

// ============================================================================
// Options and Context
// ============================================================================

/// Options for controlling what data to collect.
///
/// This is operation parameters for a single `wt list` invocation, not a cache.
/// For cached repo data, see Repository's global cache.
#[derive(Clone, Default)]
pub struct CollectOptions {
    /// Tasks to skip (not compute). Empty set means compute everything.
    ///
    /// This controls both:
    /// - Work item generation (in `work_items_for_worktree`/`work_items_for_branch`)
    /// - Column visibility (layout filters columns via `ColumnSpec::requires_task`)
    pub skip_tasks: std::collections::HashSet<super::collect::TaskKind>,

    /// URL template from project config (e.g., "http://localhost:{{ branch | hash_port }}").
    /// Expanded per-item in task spawning (post-skeleton) to minimize time-to-skeleton.
    pub url_template: Option<String>,

    /// Branches to skip expensive tasks for (behind > threshold).
    ///
    /// Presence in set = skip expensive tasks for this branch (HasFileChanges,
    /// IsAncestor, WouldMergeAdd, BranchDiff, MergeTreeConflicts).
    ///
    /// Built by filtering `batch_ahead_behind()` results on local branches only.
    /// Remote-only branches are never in this set (they use individual git commands).
    /// The threshold (default 50) is applied at construction time. Ahead/behind
    /// counts are cached in Repository and looked up by AheadBehindTask.
    ///
    /// **Display implications:** When tasks are skipped:
    /// - BranchDiff column shows `…` instead of diff stats
    /// - Status symbols (conflict `✗`, integrated `⊂`) may be missing or incorrect
    ///   since they depend on skipped tasks
    ///
    /// Note: `wt select` doesn't show the BranchDiff column, so `…` isn't visible there.
    /// This is similar to how `✗` conflict only shows with `--full` even in `wt list`.
    ///
    /// TODO: Consider adding a visible indicator in Status column when integration
    /// checks are skipped, so users know the `⊂` symbol may be incomplete.
    pub stale_branches: std::collections::HashSet<String>,
}

/// Context for task computation. Cloned and moved into spawned threads.
///
/// Contains all data needed by any task. The `repo` field shares its cache
/// across all clones via `Arc<RepoCache>`, so parallel tasks benefit from
/// cached merge-base results, ahead/behind counts, default branch, and
/// integration target.
#[derive(Clone)]
pub struct TaskContext {
    /// Shared repository handle. All clones share the same cache via Arc.
    pub repo: Repository,
    /// The branch this task operates on. Contains branch name, commit SHA,
    /// and optional worktree path.
    ///
    /// For worktree-specific operations, use `self.worktree()` which returns
    /// `Some(WorkingTree)` only when this ref has a worktree path.
    pub branch_ref: BranchRef,
    pub item_idx: usize,
    /// Expanded URL for this item (from project config template).
    /// UrlStatusTask uses this to check if the port is listening.
    pub item_url: Option<String>,
}

impl TaskContext {
    fn error(&self, kind: TaskKind, message: impl Display) -> TaskError {
        TaskError::new(self.item_idx, kind, message.to_string())
    }

    /// Get the default branch (cached in Repository).
    ///
    /// Used for informational stats (ahead/behind, branch diff).
    /// Returns None if default branch cannot be determined.
    fn default_branch(&self) -> Option<String> {
        self.repo.default_branch()
    }

    /// Get the integration target (cached in Repository).
    ///
    /// Used for integration checks (status symbols, safe deletion).
    /// Returns None if default branch cannot be determined.
    fn integration_target(&self) -> Option<String> {
        self.repo.integration_target()
    }
}

// ============================================================================
// Task Trait and Spawner
// ============================================================================

/// A task that computes a single `TaskResult`.
///
/// Each task type has a compile-time `KIND` that determines which `TaskResult`
/// variant it produces. The `compute()` function receives a cloned context and
/// returns a Result - either the successful result or an error.
///
/// Tasks should propagate errors via `?` rather than swallowing them.
/// The drain layer handles defaults and collects errors for display.
pub trait Task: Send + Sync + 'static {
    /// The kind of result this task produces (compile-time constant).
    const KIND: TaskKind;

    /// Compute the task result. Called in a spawned thread.
    /// Returns Ok(result) on success, Err(TaskError) on failure.
    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError>;
}

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
    tx: &Sender<Result<TaskResult, TaskError>>,
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
            worktrunk::config::expand_template(template, &vars, false).ok()
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

// ============================================================================
// Task Implementations
// ============================================================================

/// Task 1: Commit details (timestamp, message)
pub struct CommitDetailsTask;

impl Task for CommitDetailsTask {
    const KIND: TaskKind = TaskKind::CommitDetails;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        let repo = &ctx.repo;
        let (timestamp, commit_message) = repo
            .commit_details(&ctx.branch_ref.commit_sha)
            .map_err(|e| ctx.error(Self::KIND, e))?;
        Ok(TaskResult::CommitDetails {
            item_idx: ctx.item_idx,
            commit: CommitDetails {
                timestamp,
                commit_message,
            },
        })
    }
}

/// Task 2: Ahead/behind counts vs local default branch (informational stats)
pub struct AheadBehindTask;

impl Task for AheadBehindTask {
    const KIND: TaskKind = TaskKind::AheadBehind;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        // When default_branch is None, return zero counts (cells show empty)
        let Some(base) = ctx.default_branch() else {
            return Ok(TaskResult::AheadBehind {
                item_idx: ctx.item_idx,
                counts: AheadBehind::default(),
            });
        };
        let repo = &ctx.repo;

        // Check cache first (populated by batch_ahead_behind if it ran).
        // Cache lookup has minor overhead (rev-parse for cache key + allocations),
        // but saves the expensive ahead_behind computation on cache hit.
        let (ahead, behind) = if let Some(branch) = ctx.branch_ref.branch.as_deref() {
            if let Some(counts) = repo.get_cached_ahead_behind(&base, branch) {
                counts
            } else {
                repo.ahead_behind(&base, &ctx.branch_ref.commit_sha)
                    .map_err(|e| ctx.error(Self::KIND, e))?
            }
        } else {
            repo.ahead_behind(&base, &ctx.branch_ref.commit_sha)
                .map_err(|e| ctx.error(Self::KIND, e))?
        };

        Ok(TaskResult::AheadBehind {
            item_idx: ctx.item_idx,
            counts: AheadBehind { ahead, behind },
        })
    }
}

/// Task 3: Tree identity check (does the item's commit tree match integration target's tree?)
///
/// Uses target for integration detection (squash merge, rebase).
pub struct CommittedTreesMatchTask;

impl Task for CommittedTreesMatchTask {
    const KIND: TaskKind = TaskKind::CommittedTreesMatch;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        // When integration_target is None, return false (conservative: don't mark as integrated)
        let Some(base) = ctx.integration_target() else {
            return Ok(TaskResult::CommittedTreesMatch {
                item_idx: ctx.item_idx,
                committed_trees_match: false,
            });
        };
        let repo = &ctx.repo;
        // Use the item's commit instead of HEAD, since for branches without
        // worktrees, HEAD is the main worktree's HEAD.
        let committed_trees_match = repo
            .trees_match(&ctx.branch_ref.commit_sha, &base)
            .map_err(|e| ctx.error(Self::KIND, e))?;
        Ok(TaskResult::CommittedTreesMatch {
            item_idx: ctx.item_idx,
            committed_trees_match,
        })
    }
}

/// Task 3b: File changes check (does branch have file changes beyond merge-base?)
///
/// Uses three-dot diff (`target...branch`) to detect if the branch has any file
/// changes relative to the merge-base with target. Returns false when the diff
/// is empty, indicating the branch content is already integrated.
///
/// This catches branches where commits exist (ahead > 0) but those commits
/// don't add any file changes - e.g., squash-merged branches, merge commits
/// that pulled in main, or commits whose changes were reverted.
///
/// Uses target for integration detection.
pub struct HasFileChangesTask;

impl Task for HasFileChangesTask {
    const KIND: TaskKind = TaskKind::HasFileChanges;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        // No branch name (detached HEAD) - return conservative default (assume has changes)
        let Some(branch) = ctx.branch_ref.branch.as_deref() else {
            return Ok(TaskResult::HasFileChanges {
                item_idx: ctx.item_idx,
                has_file_changes: true,
            });
        };
        // When integration_target is None, return true (conservative: assume has changes)
        let Some(target) = ctx.integration_target() else {
            return Ok(TaskResult::HasFileChanges {
                item_idx: ctx.item_idx,
                has_file_changes: true,
            });
        };
        let repo = &ctx.repo;
        let has_file_changes = repo
            .has_added_changes(branch, &target)
            .map_err(|e| ctx.error(Self::KIND, e))?;

        Ok(TaskResult::HasFileChanges {
            item_idx: ctx.item_idx,
            has_file_changes,
        })
    }
}

/// Task 3b: Merge simulation
///
/// Checks if merging the branch into target would add any changes by simulating
/// the merge with `git merge-tree --write-tree`. Returns false when the merge
/// result equals target's tree, indicating the branch is already integrated.
///
/// This catches branches where target has advanced past the squash-merge point -
/// the three-dot diff might show changes, but those changes are already in target
/// via the squash merge.
///
/// Uses target for integration detection.
pub struct WouldMergeAddTask;

impl Task for WouldMergeAddTask {
    const KIND: TaskKind = TaskKind::WouldMergeAdd;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        // No branch name (detached HEAD) - return conservative default (assume would add)
        let Some(branch) = ctx.branch_ref.branch.as_deref() else {
            return Ok(TaskResult::WouldMergeAdd {
                item_idx: ctx.item_idx,
                would_merge_add: true,
            });
        };
        // When integration_target is None, return true (conservative: assume would add)
        let Some(base) = ctx.integration_target() else {
            return Ok(TaskResult::WouldMergeAdd {
                item_idx: ctx.item_idx,
                would_merge_add: true,
            });
        };
        let repo = &ctx.repo;
        let would_merge_add = repo
            .would_merge_add_to_target(branch, &base)
            .map_err(|e| ctx.error(Self::KIND, e))?;
        Ok(TaskResult::WouldMergeAdd {
            item_idx: ctx.item_idx,
            would_merge_add,
        })
    }
}

/// Task 3c: Ancestor check (is branch HEAD an ancestor of integration target?)
///
/// Checks if branch is an ancestor of target - runs `git merge-base --is-ancestor`.
/// Returns true when the branch HEAD is in target's history (merged via fast-forward
/// or rebase).
///
/// Uses target (target) for the Ancestor integration reason in `⊂`.
/// The `_` symbol uses ahead/behind counts (vs default_branch) instead.
pub struct IsAncestorTask;

impl Task for IsAncestorTask {
    const KIND: TaskKind = TaskKind::IsAncestor;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        // When integration_target is None, return false (conservative: don't mark as ancestor)
        let Some(base) = ctx.integration_target() else {
            return Ok(TaskResult::IsAncestor {
                item_idx: ctx.item_idx,
                is_ancestor: false,
            });
        };
        let repo = &ctx.repo;
        let is_ancestor = repo
            .is_ancestor(&ctx.branch_ref.commit_sha, &base)
            .map_err(|e| ctx.error(Self::KIND, e))?;

        Ok(TaskResult::IsAncestor {
            item_idx: ctx.item_idx,
            is_ancestor,
        })
    }
}

/// Task 4: Branch diff stats vs local default branch (informational stats)
pub struct BranchDiffTask;

impl Task for BranchDiffTask {
    const KIND: TaskKind = TaskKind::BranchDiff;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        // When default_branch is None, return empty diff (cells show empty)
        let Some(base) = ctx.default_branch() else {
            return Ok(TaskResult::BranchDiff {
                item_idx: ctx.item_idx,
                branch_diff: BranchDiffTotals::default(),
            });
        };
        let repo = &ctx.repo;
        let diff = repo
            .branch_diff_stats(&base, &ctx.branch_ref.commit_sha)
            .map_err(|e| ctx.error(Self::KIND, e))?;

        Ok(TaskResult::BranchDiff {
            item_idx: ctx.item_idx,
            branch_diff: BranchDiffTotals { diff },
        })
    }
}

/// Task 5 (worktree only): Working tree diff + status flags
///
/// Parallelizes `git status` with `rev-parse --verify` + `diff-tree --quiet` since they're
/// independent. The `diff-tree` compares committed trees (HEAD vs main), not the working tree,
/// so it can run while status is scanning the working tree.
pub struct WorkingTreeDiffTask;

impl Task for WorkingTreeDiffTask {
    const KIND: TaskKind = TaskKind::WorkingTreeDiff;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        use worktrunk::git::TreesMatchResult;

        // This task is only spawned for worktree items, so worktree path is always present.
        let wt = ctx
            .branch_ref
            .working_tree(&ctx.repo)
            .expect("WorkingTreeDiffTask requires a worktree");

        // Use default_branch (local default branch) for informational display
        let default_branch = ctx.default_branch();

        // Parallelize: status (reads working tree) || trees_match (compares committed trees)
        // These are independent - diff-tree compares HEAD vs main trees, not the working tree.
        let (status_result, trees_match_result) = rayon::join(
            || {
                // Use --no-optional-locks to avoid index lock contention with WorkingTreeConflictsTask's
                // `git stash create` which needs the index lock.
                wt.run_command(&["--no-optional-locks", "status", "--porcelain"])
            },
            || wt.trees_match_base(default_branch.as_deref()),
        );

        let status_output = status_result.map_err(|e| ctx.error(Self::KIND, e))?;
        let trees_match = trees_match_result.map_err(|e| ctx.error(Self::KIND, e))?;

        let (working_tree_status, is_dirty, has_conflicts) =
            parse_working_tree_status(&status_output);

        let working_tree_diff = if is_dirty {
            wt.working_tree_diff_stats()
                .map_err(|e| ctx.error(Self::KIND, e))?
        } else {
            LineDiff::default()
        };

        // Compute working tree diff vs main based on trees_match result
        let working_tree_diff_with_main = match trees_match {
            TreesMatchResult::NoBaseBranch => Some(LineDiff::default()),
            TreesMatchResult::BranchNotFound | TreesMatchResult::TreesDiffer => None,
            TreesMatchResult::TreesMatch { branch } => {
                if is_dirty {
                    Some(
                        wt.working_tree_diff_vs_ref(branch)
                            .map_err(|e| ctx.error(Self::KIND, e))?,
                    )
                } else {
                    Some(LineDiff::default())
                }
            }
        };

        Ok(TaskResult::WorkingTreeDiff {
            item_idx: ctx.item_idx,
            working_tree_diff,
            working_tree_diff_with_main,
            working_tree_status,
            has_conflicts,
        })
    }
}

/// Task 6: Potential merge conflicts check (merge-tree vs local main)
///
/// Uses default_branch (local main) for consistency with other Main subcolumn symbols.
/// Shows whether merging to your local main would conflict.
pub struct MergeTreeConflictsTask;

impl Task for MergeTreeConflictsTask {
    const KIND: TaskKind = TaskKind::MergeTreeConflicts;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        // When default_branch is None, return false (no conflicts can be detected)
        let Some(base) = ctx.default_branch() else {
            return Ok(TaskResult::MergeTreeConflicts {
                item_idx: ctx.item_idx,
                has_merge_tree_conflicts: false,
            });
        };
        let repo = &ctx.repo;
        let has_merge_tree_conflicts = repo
            .has_merge_conflicts(&base, &ctx.branch_ref.commit_sha)
            .map_err(|e| ctx.error(Self::KIND, e))?;
        Ok(TaskResult::MergeTreeConflicts {
            item_idx: ctx.item_idx,
            has_merge_tree_conflicts,
        })
    }
}

/// Task 6b (worktree only, --full only): Working tree conflict check
///
/// For dirty worktrees, uses `git stash create` to get a tree object that
/// includes uncommitted changes, then runs merge-tree against that.
/// Returns None if working tree is clean (caller should fall back to MergeTreeConflicts).
pub struct WorkingTreeConflictsTask;

impl Task for WorkingTreeConflictsTask {
    const KIND: TaskKind = TaskKind::WorkingTreeConflicts;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        // When default_branch is None, return None (skip conflict check)
        let Some(base) = ctx.default_branch() else {
            return Ok(TaskResult::WorkingTreeConflicts {
                item_idx: ctx.item_idx,
                has_working_tree_conflicts: None,
            });
        };
        // This task is only spawned for worktree items, so worktree path is always present.
        let wt = ctx
            .branch_ref
            .working_tree(&ctx.repo)
            .expect("WorkingTreeConflictsTask requires a worktree");

        // Use --no-optional-locks to avoid index lock contention with WorkingTreeDiffTask.
        // Both tasks run in parallel, and `git stash create` below needs the index lock.
        let status_output = wt
            .run_command(&["--no-optional-locks", "status", "--porcelain"])
            .map_err(|e| ctx.error(Self::KIND, e))?;

        let is_dirty = !status_output.trim().is_empty();

        if !is_dirty {
            // Clean working tree - return None to signal "use commit-based check"
            return Ok(TaskResult::WorkingTreeConflicts {
                item_idx: ctx.item_idx,
                has_working_tree_conflicts: None,
            });
        }

        // Dirty working tree - create a temporary tree object via stash create
        // `git stash create` returns a commit SHA without modifying refs
        //
        // Note: stash create fails when there are unmerged files (merge conflict in progress).
        // In that case, fall back to the commit-based check.
        let stash_result = wt.run_command(&["stash", "create"]);

        let stash_sha = match stash_result {
            Ok(sha) => sha,
            Err(_) => {
                // Stash create failed (likely unmerged files during rebase/merge)
                // Fall back to commit-based check
                return Ok(TaskResult::WorkingTreeConflicts {
                    item_idx: ctx.item_idx,
                    has_working_tree_conflicts: None,
                });
            }
        };

        let stash_sha = stash_sha.trim();

        // If stash create returns empty, working tree is clean (shouldn't happen but handle it)
        if stash_sha.is_empty() {
            return Ok(TaskResult::WorkingTreeConflicts {
                item_idx: ctx.item_idx,
                has_working_tree_conflicts: None,
            });
        }

        // Run merge-tree with the stash commit (repo-wide operation, doesn't need worktree)
        let has_conflicts = ctx
            .repo
            .has_merge_conflicts(&base, stash_sha)
            .map_err(|e| ctx.error(Self::KIND, e))?;

        Ok(TaskResult::WorkingTreeConflicts {
            item_idx: ctx.item_idx,
            has_working_tree_conflicts: Some(has_conflicts),
        })
    }
}

/// Task 7 (worktree only): Git operation state detection (rebase/merge)
pub struct GitOperationTask;

impl Task for GitOperationTask {
    const KIND: TaskKind = TaskKind::GitOperation;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        // This task is only spawned for worktree items, so worktree path is always present.
        let wt = ctx
            .branch_ref
            .working_tree(&ctx.repo)
            .expect("GitOperationTask requires a worktree");
        let git_operation = detect_git_operation(&wt);
        Ok(TaskResult::GitOperation {
            item_idx: ctx.item_idx,
            git_operation,
        })
    }
}

/// Task 8 (worktree only): User-defined status from git config
pub struct UserMarkerTask;

impl Task for UserMarkerTask {
    const KIND: TaskKind = TaskKind::UserMarker;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        let repo = &ctx.repo;
        let user_marker = repo.user_marker(ctx.branch_ref.branch.as_deref());
        Ok(TaskResult::UserMarker {
            item_idx: ctx.item_idx,
            user_marker,
        })
    }
}

/// Task 9: Upstream tracking status
pub struct UpstreamTask;

impl Task for UpstreamTask {
    const KIND: TaskKind = TaskKind::Upstream;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        let repo = &ctx.repo;

        // No branch means no upstream
        let Some(branch) = ctx.branch_ref.branch.as_deref() else {
            return Ok(TaskResult::Upstream {
                item_idx: ctx.item_idx,
                upstream: UpstreamStatus::default(),
            });
        };

        // Get upstream branch (None is valid - just means no upstream configured)
        let upstream_branch = repo
            .upstream_branch(branch)
            .map_err(|e| ctx.error(Self::KIND, e))?;
        let Some(upstream_branch) = upstream_branch else {
            return Ok(TaskResult::Upstream {
                item_idx: ctx.item_idx,
                upstream: UpstreamStatus::default(),
            });
        };

        let remote = upstream_branch.split_once('/').map(|(r, _)| r.to_string());
        let (ahead, behind) = repo
            .ahead_behind(&upstream_branch, &ctx.branch_ref.commit_sha)
            .map_err(|e| ctx.error(Self::KIND, e))?;

        Ok(TaskResult::Upstream {
            item_idx: ctx.item_idx,
            upstream: UpstreamStatus {
                remote,
                ahead,
                behind,
            },
        })
    }
}

/// Task 10: CI/PR status
///
/// Always checks for open PRs/MRs regardless of upstream tracking.
/// For branch workflow/pipeline fallback (no PR), requires upstream tracking
/// to prevent false matches from similarly-named branches on the remote.
pub struct CiStatusTask;

impl Task for CiStatusTask {
    const KIND: TaskKind = TaskKind::CiStatus;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        let repo = &ctx.repo;
        let pr_status = ctx.branch_ref.branch.as_deref().and_then(|branch| {
            let has_upstream = repo.upstream_branch(branch).ok().flatten().is_some();
            PrStatus::detect(repo, branch, &ctx.branch_ref.commit_sha, has_upstream)
        });

        Ok(TaskResult::CiStatus {
            item_idx: ctx.item_idx,
            pr_status,
        })
    }
}

/// Task 13: URL health check (port availability).
///
/// The URL itself is sent immediately after template expansion (in spawning code)
/// so it appears in normal styling right away. This task only checks if the port
/// is listening, and if not, the URL dims.
pub struct UrlStatusTask;

impl Task for UrlStatusTask {
    const KIND: TaskKind = TaskKind::UrlStatus;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        use std::net::{SocketAddr, TcpStream};
        use std::time::Duration;

        // URL already sent in spawning code; this task only checks port availability
        let Some(ref url) = ctx.item_url else {
            return Ok(TaskResult::UrlStatus {
                item_idx: ctx.item_idx,
                url: None,
                active: None,
            });
        };

        // Parse port from URL and check if it's listening
        // Skip health check in tests to avoid flaky results from random local processes
        let active = if std::env::var("WORKTRUNK_TEST_SKIP_URL_HEALTH_CHECK").is_ok() {
            Some(false)
        } else {
            parse_port_from_url(url).map(|port| {
                // Quick TCP connect check with 50ms timeout
                let addr = SocketAddr::from(([127, 0, 0, 1], port));
                TcpStream::connect_timeout(&addr, Duration::from_millis(50)).is_ok()
            })
        };

        // Return only active status (url=None to avoid overwriting the already-sent URL)
        Ok(TaskResult::UrlStatus {
            item_idx: ctx.item_idx,
            url: None,
            active,
        })
    }
}

/// Parse port number from a URL string (e.g., "http://localhost:12345" -> 12345)
pub(crate) fn parse_port_from_url(url: &str) -> Option<u16> {
    // Strip scheme
    let url = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    // Extract host:port (before path, query, or fragment)
    let host_port = url.split(&['/', '?', '#'][..]).next()?;
    let (_host, port_str) = host_port.rsplit_once(':')?;
    port_str.parse().ok()
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Parse git status output to extract working tree status and conflict state.
/// Returns (WorkingTreeStatus, is_dirty, has_conflicts).
fn parse_working_tree_status(status_output: &str) -> (WorkingTreeStatus, bool, bool) {
    let mut has_untracked = false;
    let mut has_modified = false;
    let mut has_staged = false;
    let mut has_renamed = false;
    let mut has_deleted = false;
    let mut has_conflicts = false;

    for line in status_output.lines() {
        if line.len() < 2 {
            continue;
        }

        let bytes = line.as_bytes();
        let index_status = bytes[0] as char;
        let worktree_status = bytes[1] as char;

        if index_status == '?' && worktree_status == '?' {
            has_untracked = true;
        }

        // Worktree changes: M = modified, A = intent-to-add (git add -N), T = type change (file↔symlink)
        if matches!(worktree_status, 'M' | 'A' | 'T') {
            has_modified = true;
        }

        // Index changes: A = added, M = modified, C = copied, T = type change (file↔symlink)
        if matches!(index_status, 'A' | 'M' | 'C' | 'T') {
            has_staged = true;
        }

        if index_status == 'R' {
            has_renamed = true;
        }

        if index_status == 'D' || worktree_status == 'D' {
            has_deleted = true;
        }

        // Detect unmerged/conflicting paths (porcelain v1 two-letter codes)
        // Only U codes and AA/DD indicate actual merge conflicts.
        // AD/DA are normal staging states (staged then deleted, or deleted then restored).
        let is_unmerged_pair = matches!(
            (index_status, worktree_status),
            ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D')
        );
        if is_unmerged_pair {
            has_conflicts = true;
        }
    }

    let working_tree_status = WorkingTreeStatus::new(
        has_staged,
        has_modified,
        has_untracked,
        has_renamed,
        has_deleted,
    );

    let is_dirty = working_tree_status.is_dirty();

    (working_tree_status, is_dirty, has_conflicts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_status_ad_not_conflict() {
        // AD = added to index, deleted from worktree (not a conflict)
        let (status, is_dirty, has_conflicts) = parse_working_tree_status("AD file.txt\n");
        assert!(!has_conflicts, "AD should not be treated as conflict");
        assert!(is_dirty);
        assert!(status.staged, "AD should show staged symbol");
    }

    #[test]
    fn test_parse_status_da_not_conflict() {
        // DA = deleted from index, then restored to worktree (not a conflict)
        let (_, is_dirty, has_conflicts) = parse_working_tree_status("DA file.txt\n");
        assert!(!has_conflicts, "DA should not be treated as conflict");
        assert!(is_dirty);
    }

    #[test]
    fn test_parse_status_uu_is_conflict() {
        // UU = both modified (actual conflict)
        let (_, _, has_conflicts) = parse_working_tree_status("UU file.txt\n");
        assert!(has_conflicts, "UU should be treated as conflict");
    }

    #[test]
    fn test_parse_status_aa_is_conflict() {
        // AA = both added (actual conflict)
        let (_, _, has_conflicts) = parse_working_tree_status("AA file.txt\n");
        assert!(has_conflicts, "AA should be treated as conflict");
    }

    #[test]
    fn test_parse_status_dd_is_conflict() {
        // DD = both deleted (actual conflict)
        let (_, _, has_conflicts) = parse_working_tree_status("DD file.txt\n");
        assert!(has_conflicts, "DD should be treated as conflict");
    }

    #[test]
    fn test_parse_status_u_variants_are_conflicts() {
        // All U codes indicate conflicts
        for code in ["AU", "UA", "DU", "UD"] {
            let input = format!("{} file.txt\n", code);
            let (_, _, has_conflicts) = parse_working_tree_status(&input);
            assert!(has_conflicts, "{} should be treated as conflict", code);
        }
    }

    #[test]
    fn test_parse_status_md_not_conflict() {
        // MD = modified in index, deleted from worktree (not a conflict)
        let (_, is_dirty, has_conflicts) = parse_working_tree_status("MD file.txt\n");
        assert!(!has_conflicts, "MD should not be treated as conflict");
        assert!(is_dirty);
    }

    #[test]
    fn test_parse_status_intent_to_add() {
        // " A" = intent-to-add (git add -N): file recorded but content not staged
        let (status, is_dirty, has_conflicts) = parse_working_tree_status(" A file.txt\n");
        assert!(
            !has_conflicts,
            "intent-to-add should not be treated as conflict"
        );
        assert!(is_dirty, "intent-to-add should be dirty");
        assert!(status.modified, "intent-to-add should show modified symbol");
    }

    #[test]
    fn test_parse_status_type_change() {
        // " T" = type change in worktree (e.g., file changed to symlink)
        let (status, is_dirty, has_conflicts) = parse_working_tree_status(" T file.txt\n");
        assert!(
            !has_conflicts,
            "type change should not be treated as conflict"
        );
        assert!(is_dirty, "type change should be dirty");
        assert!(status.modified, "type change should show modified symbol");
    }

    #[test]
    fn test_parse_status_staged_type_change() {
        // "T " = type change in index (staged), no worktree changes
        // Example: file changed to symlink and the change is staged
        let (status, is_dirty, has_conflicts) = parse_working_tree_status("T  file.txt\n");
        assert!(
            !has_conflicts,
            "staged type change should not be treated as conflict"
        );
        assert!(is_dirty, "staged type change should be dirty");
        assert!(
            status.staged,
            "staged type change should show staged symbol (+)"
        );
    }

    #[test]
    fn test_parse_status_staged_type_change_with_worktree_mod() {
        // "TM" = type change in index, then modified in worktree
        let (status, is_dirty, has_conflicts) = parse_working_tree_status("TM file.txt\n");
        assert!(!has_conflicts);
        assert!(is_dirty, "should be dirty");
        assert!(
            status.staged,
            "should show staged symbol for index type change"
        );
        assert!(
            status.modified,
            "should show modified symbol for worktree modification"
        );
    }

    #[test]
    fn test_parse_port_from_url_basic() {
        assert_eq!(parse_port_from_url("http://localhost:12345"), Some(12345));
        assert_eq!(parse_port_from_url("https://localhost:8080"), Some(8080));
    }

    #[test]
    fn test_parse_port_from_url_with_path() {
        assert_eq!(
            parse_port_from_url("http://localhost:12345/path/to/page"),
            Some(12345)
        );
        assert_eq!(parse_port_from_url("http://localhost:3000/"), Some(3000));
    }

    #[test]
    fn test_parse_port_from_url_no_port() {
        assert_eq!(parse_port_from_url("http://localhost"), None);
        assert_eq!(parse_port_from_url("http://localhost/path"), None);
    }

    #[test]
    fn test_parse_port_from_url_no_scheme() {
        // Without http:// or https:// prefix, returns None
        assert_eq!(parse_port_from_url("localhost:8080"), None);
    }

    #[test]
    fn test_parse_port_from_url_edge_cases() {
        assert_eq!(parse_port_from_url("http://127.0.0.1:9000"), Some(9000));
        assert_eq!(parse_port_from_url("http://0.0.0.0:5000"), Some(5000));
    }
}
