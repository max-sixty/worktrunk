//! Work item generation and execution.
//!
//! Contains the flat parallelism infrastructure:
//! - `WorkItem` - unit of work for the thread pool
//! - `dispatch_task()` - route TaskKind to the correct Task implementation
//! - `work_items_for_worktree()` / `work_items_for_branch()` - generate work items
//! - `ExpectedResults` - track expected results for timeout diagnostics
//! - `seed_skipped_task_defaults()` - conservative sentinels for skipped tasks
//!
//! ## Status field bookkeeping at spawn time
//!
//! `compute_status_symbols` refuses to compute until every required field on
//! `ListItem` / `WorktreeData` is `Some`. Tasks that *will* run are written
//! by the drain as their results arrive. Tasks that will *not* run (stale
//! branches skip expensive tasks, unborn branches skip commit-dependent
//! tasks, `--skip-tasks` filters, branches with no worktree have no
//! worktree-only tasks) have their fields seeded here via
//! [`seed_skipped_task_defaults`] with conservative defaults — otherwise
//! the fields would stay `None` forever and status would never compute.

use std::sync::Arc;

use crossbeam_channel as chan;
use worktrunk::git::{BranchRef, Repository, WorktreeInfo};

use super::super::model::{
    ActiveGitOperation, ItemKind, ListItem, UpstreamStatus, WorkingTreeStatus,
};
use super::CollectOptions;
use super::tasks::{
    AheadBehindTask, BranchDiffTask, CiStatusTask, CommittedTreesMatchTask, GitOperationTask,
    HasFileChangesTask, IsAncestorTask, MergeTreeConflictsTask, SummaryGenerateTask, Task,
    TaskContext, UpstreamTask, UrlStatusTask, UserMarkerTask, WorkingTreeConflictsTask,
    WorkingTreeDiffTask, WouldMergeAddTask,
};
use super::types::{TaskError, TaskKind, TaskResult};

/// Tasks that require a valid commit SHA. Skipped for unborn branches (no commits yet).
/// Without this, these tasks would fail on the null OID and show as errors in the table.
const COMMIT_TASKS: &[TaskKind] = &[
    TaskKind::AheadBehind,
    TaskKind::CommittedTreesMatch,
    TaskKind::HasFileChanges,
    TaskKind::IsAncestor,
    TaskKind::BranchDiff,
    TaskKind::MergeTreeConflicts,
    TaskKind::WouldMergeAdd,
    TaskKind::CiStatus,
    TaskKind::Upstream,
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
        TaskKind::SummaryGenerate => SummaryGenerateTask::compute(ctx),
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
// Skipped Task Sentinels
// ============================================================================

/// Seed conservative defaults for a task that will not run for an item.
///
/// `refresh_status_symbols` keeps gates at `None` while their inputs are
/// unloaded, which renders as the `·` placeholder. For tasks that will
/// *never* run (stale branch, user `--skip-tasks`, unborn item missing
/// commit-dependent tasks, prunable worktree), seeding a conservative
/// default up front lets the gate resolve normally instead of showing `·`
/// forever.
///
/// **Not seeded by this function:** `item.counts`. The ahead/behind counts
/// are the only field that leaks directly into JSON output (`JsonMain` at
/// `json_output.rs:~305`), so seeding them with `(0, 0)` would falsely
/// claim "in sync" in `wt list --format=json` output. Callers that need to
/// resolve gate 3 (main_state) for items without counts should instead
/// pre-seed `status_symbols.main_state` directly — see
/// [`seed_unborn_main_state`].
///
/// Non-status-feeding tasks (`BranchDiff`, `CiStatus`, `UrlStatus`,
/// `SummaryGenerate`) are rendered by their own columns with their own
/// placeholders; `refresh_status_symbols` doesn't read them, so there is
/// nothing to seed.
pub(super) fn seed_skipped_task_defaults(item: &mut ListItem, kind: TaskKind) {
    match kind {
        // Not consumed by refresh_status_symbols — columns handle their own
        // loading state.
        TaskKind::BranchDiff
        | TaskKind::CiStatus
        | TaskKind::UrlStatus
        | TaskKind::SummaryGenerate => {}

        TaskKind::AheadBehind => {
            // Seed `is_orphan` (safe — not in JSON) but NOT `counts`
            // (leaks to `JsonMain`). Gate 3 callers that need counts-less
            // resolution use `seed_unborn_main_state` to pre-seed
            // `status_symbols.main_state` directly.
            item.is_orphan = Some(false);
        }
        TaskKind::Upstream => {
            // Safe to seed: `UpstreamStatus::default()` has
            // `remote: None`, so `active()` returns `None` and
            // `upstream_to_json` elides the `remote` JSON key. No leak.
            item.upstream = Some(UpstreamStatus::default());
        }
        TaskKind::CommittedTreesMatch => {
            // Conservative: don't claim integrated if we haven't checked.
            item.committed_trees_match = Some(false);
        }
        TaskKind::HasFileChanges => {
            // Conservative: assume unique changes exist.
            item.has_file_changes = Some(true);
        }
        TaskKind::WouldMergeAdd => {
            // Conservative: assume the merge would add changes.
            item.would_merge_add = Some(true);
            item.is_patch_id_match = Some(false);
        }
        TaskKind::IsAncestor => {
            // Conservative: don't claim merged if we haven't checked.
            item.is_ancestor = Some(false);
        }
        TaskKind::MergeTreeConflicts => {
            item.has_merge_tree_conflicts = Some(false);
        }
        TaskKind::UserMarker => {
            item.user_marker = Some(None);
        }
        TaskKind::WorkingTreeDiff => {
            // Do not seed. `working_tree_diff` feeds gate 3's `is_clean`
            // check and flows into JSON output. Seeding `Some(default())`
            // would misreport dirty worktrees as clean+removable and
            // fabricate an empty diff in `--format=json`. Same rule as
            // `item.counts`: leaks-to-JSON inputs stay `None`; the gate
            // waits and renders `·`.
        }
        TaskKind::WorkingTreeConflicts => {
            if let ItemKind::Worktree(data) = &mut item.kind {
                // `Some(None)` = "task did not run, fall back to the
                // committed-HEAD merge-tree check" — matches the semantics
                // of a clean working tree under `--full`.
                data.has_working_tree_conflicts = Some(None);
            }
        }
        TaskKind::GitOperation => {
            if let ItemKind::Worktree(data) = &mut item.kind {
                data.git_operation = Some(ActiveGitOperation::None);
            }
        }
    }
}

/// Pre-seed gate 3 (`main_state`) for unborn items whose `AheadBehind`
/// task will not run.
///
/// Gate 3's tier 4 requires `item.counts` to be loaded, but `item.counts`
/// is a JSON-leaking field we can't safely seed. For unborn items there is
/// no ahead/behind relationship to compute — the main worktree always
/// resolves to `MainState::IsMain`, and a linked unborn worktree (rare:
/// `git worktree add -b new main` before the first commit) resolves to
/// `MainState::None` (no symbol). Both are known at spawn time without
/// any task output.
pub(super) fn seed_unborn_main_state(item: &mut ListItem) {
    let is_main = matches!(&item.kind, ItemKind::Worktree(data) if data.is_main);
    item.status_symbols.main_state = Some(if is_main {
        super::super::model::MainState::IsMain
    } else {
        super::super::model::MainState::None
    });
}

/// Pre-seed every gate on a prunable worktree so the only visible symbol
/// is `⊟` (from the metadata `worktree_state`).
///
/// Prunable worktrees have their directory missing from disk, so no task
/// can run for them. Without this seeding, every gate would stay `None`
/// forever and the cell would render as seven `·` placeholders. This
/// helper replaces the runtime fallback in `refresh_status_symbols`
/// (introduced as a shim in step 4) with spawn-time seeding.
pub(super) fn seed_prunable_item(item: &mut ListItem) {
    use super::super::model::{
        Divergence, MainState, OperationState, StatusSymbols, WorktreeState,
    };
    item.status_symbols = StatusSymbols {
        working_tree: Some(WorkingTreeStatus::default()),
        operation_state: Some(OperationState::None),
        worktree_state: Some(WorktreeState::Prunable),
        main_state: Some(MainState::None),
        upstream_divergence: Some(Divergence::None),
        user_marker: Some(None),
    };
}

// ============================================================================
// Work Item Generation
// ============================================================================

/// Generate work items for a worktree.
///
/// Returns a list of work items representing all tasks that should run for
/// this worktree. Expected results are registered internally as each work
/// item is added. The caller is responsible for executing the work items.
///
/// **Side effects on `item`:**
/// - Seeds conservative sentinels on fields corresponding to tasks that will
///   *not* run (stale/unborn/skipped). See module docstring.
///
/// **Side effects on `tx`:**
/// - Sends an immediate `TaskResult::UrlStatus { url: Some, active: None }`
///   for items with a URL template, so the row redraws as soon as the
///   first drain tick fires (the slower follow-up `UrlStatus` health
///   check then updates `url_active`). The drain pipeline is the only
///   path that triggers progressive row redraws, so writing `item.url`
///   directly here would leave it stuck behind whatever task happens to
///   complete first.
///
/// Task preconditions (stale branch, unborn branch, missing llm_command) are
/// enforced here — not in callers. This function is called from both
/// `collect()` and `populate_item()`, so guards must live here to cover all
/// entry points.
///
/// The `repo` parameter is cloned into each TaskContext, sharing its cache
/// via Arc.
pub fn work_items_for_worktree(
    repo: &Repository,
    wt: &WorktreeInfo,
    item_idx: usize,
    options: &CollectOptions,
    expected_results: &Arc<ExpectedResults>,
    tx: &chan::Sender<Result<TaskResult, TaskError>>,
    item: &mut ListItem,
) -> Vec<WorkItem> {
    // Prunable worktrees have their directory missing — no task can run.
    // Seed every gate directly so the cell shows just the `⊟` metadata
    // symbol rather than seven `·` placeholders.
    if wt.is_prunable() {
        seed_prunable_item(item);
        return vec![];
    }

    let skip = &options.skip_tasks;

    let include_url = !skip.contains(&TaskKind::UrlStatus);

    // Expand URL template for this item (only if URL status is enabled).
    let item_url = if include_url {
        options.url_template.as_ref().and_then(|template| {
            wt.branch.as_ref().and_then(|branch| {
                let mut vars = std::collections::HashMap::new();
                vars.insert("branch", branch.as_str());
                worktrunk::config::expand_template(template, &vars, false, repo, "url-template")
                    .ok()
            })
        })
    } else {
        None
    };

    // Send the URL through the drain channel so the row redraws as soon as
    // the result is processed. Without this round trip, the URL would only
    // appear when *some other* task happens to complete and trigger a
    // refresh — often the slow `UrlStatusTask` itself.
    if include_url && let Some(ref url) = item_url {
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
        llm_command: options.llm_command.clone(),
        default_branch: options.default_branch.clone(),
        integration_target: options.integration_target.clone(),
    };

    let has_commits = wt.has_commits();

    let mut items = Vec::with_capacity(15);

    for kind in [
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
        // TODO: For dirty worktrees, WorkingTreeConflicts already runs merge-tree
        // (via stash-create + merge-tree). MergeTreeConflicts duplicates that call
        // against HEAD. Could skip MergeTreeConflicts when WorkingTreeConflicts
        // produces a non-None answer, but needs result-ordering changes since both
        // tasks run in parallel today.
        TaskKind::MergeTreeConflicts,
        TaskKind::CiStatus,
        TaskKind::WouldMergeAdd,
        TaskKind::SummaryGenerate,
    ] {
        let will_skip = skip.contains(&kind)
            || (!has_commits && COMMIT_TASKS.contains(&kind))
            || (kind == TaskKind::SummaryGenerate && options.llm_command.is_none());
        if will_skip {
            seed_skipped_task_defaults(item, kind);
            continue;
        }
        expected_results.expect(item_idx, kind);
        items.push(WorkItem {
            ctx: ctx.clone(),
            kind,
        });
    }

    // Unborn items: their `AheadBehind` task was skipped in the loop
    // above, so `item.counts` stays `None` (by design — seeding it would
    // leak `{ahead:0, behind:0}` into JSON). Pre-seed `main_state`
    // directly so gate 3 resolves without needing counts.
    if !has_commits {
        seed_unborn_main_state(item);
    }

    // URL status health check task (if we have a URL). Only this single
    // work item is queued per item — `item.url` was set directly above, so
    // no placeholder send is needed.
    if include_url && ctx.item_url.is_some() {
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
/// Task preconditions are enforced here, same as [`work_items_for_worktree`].
///
/// The `repo` parameter is cloned into each TaskContext, sharing its cache via Arc.
/// The `is_remote` flag indicates whether this is a remote-tracking branch (e.g., "origin/feature")
/// vs a local branch. This is known definitively at collection time and avoids guessing later.
/// Identity of a branch item being spawned (grouped to keep
/// `work_items_for_branch` under the clippy arg-count limit).
pub struct BranchSpawn<'a> {
    pub name: &'a str,
    pub commit_sha: &'a str,
    pub item_idx: usize,
    pub is_remote: bool,
}

pub fn work_items_for_branch(
    repo: &Repository,
    branch: BranchSpawn<'_>,
    options: &CollectOptions,
    expected_results: &Arc<ExpectedResults>,
    item: &mut ListItem,
) -> Vec<WorkItem> {
    let BranchSpawn {
        name: branch_name,
        commit_sha,
        item_idx,
        is_remote,
    } = branch;

    let skip = &options.skip_tasks;

    let branch_ref = if is_remote {
        BranchRef::remote_branch(branch_name, commit_sha)
    } else {
        BranchRef::local_branch(branch_name, commit_sha)
    };

    let ctx = TaskContext {
        repo: repo.clone(),
        branch_ref,
        item_idx,
        item_url: None, // Branches without worktrees don't have URLs
        llm_command: options.llm_command.clone(),
        default_branch: options.default_branch.clone(),
        integration_target: options.integration_target.clone(),
    };

    let mut items = Vec::with_capacity(11);

    for kind in [
        TaskKind::AheadBehind,
        TaskKind::CommittedTreesMatch,
        TaskKind::HasFileChanges,
        TaskKind::IsAncestor,
        TaskKind::Upstream,
        TaskKind::BranchDiff,
        TaskKind::MergeTreeConflicts,
        TaskKind::CiStatus,
        TaskKind::WouldMergeAdd,
        TaskKind::SummaryGenerate,
    ] {
        let will_skip = skip.contains(&kind)
            || (kind == TaskKind::SummaryGenerate && options.llm_command.is_none());
        if will_skip {
            seed_skipped_task_defaults(item, kind);
            continue;
        }
        expected_results.expect(item_idx, kind);
        items.push(WorkItem {
            ctx: ctx.clone(),
            kind,
        });
    }
    // `UserMarker` is never in the branch task list above (it only runs
    // for worktrees), but the branch arm of `compute_status_symbols`
    // *does* read `item.user_marker` and will bail while it is `None`.
    // Seed it here so branches can compute status.
    seed_skipped_task_defaults(item, TaskKind::UserMarker);
    // `WorkingTreeDiff` / `WorkingTreeConflicts` / `GitOperation` only
    // affect the worktree arm of `compute_status_symbols`, which is never
    // entered for `ItemKind::Branch`. Seeding them is a no-op today (the
    // seed helper branches on `ItemKind::Worktree` internally) but makes
    // the per-item task set explicit and keeps this loop forward-compatible
    // if the branch arm ever starts reading them.
    for kind in [
        TaskKind::WorkingTreeDiff,
        TaskKind::WorkingTreeConflicts,
        TaskKind::GitOperation,
    ] {
        seed_skipped_task_defaults(item, kind);
    }

    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::list::collect::build_worktree_item;
    use std::collections::HashSet;

    #[test]
    fn test_skip_url_status_suppresses_placeholder_and_task() {
        let test = worktrunk::testing::TestRepo::new();
        let repo = Repository::at(test.path()).expect("repo");
        let wt = WorktreeInfo {
            path: test.path().to_path_buf(),
            head: "deadbeef".to_string(),
            branch: Some("main".to_string()),
            bare: false,
            detached: false,
            locked: None,
            prunable: None,
        };

        let skip_tasks: HashSet<TaskKind> = [TaskKind::UrlStatus].into_iter().collect();
        let options = CollectOptions {
            skip_tasks,
            url_template: Some("http://localhost/{{ branch }}".to_string()),
            llm_command: None,
            default_branch: None,
            integration_target: None,
        };

        let expected_results = Arc::new(ExpectedResults::default());
        let (tx, rx) = chan::unbounded::<Result<TaskResult, TaskError>>();
        let mut item = build_worktree_item(&wt, true, false, false);

        let items =
            work_items_for_worktree(&repo, &wt, 0, &options, &expected_results, &tx, &mut item);

        // No placeholder UrlStatus result was sent to the channel.
        assert!(rx.try_recv().is_err());
        // No UrlStatus work item created
        assert!(!items.iter().any(|w| w.kind == TaskKind::UrlStatus));
        // No UrlStatus in expected results
        assert!(
            !expected_results
                .results_for(0)
                .contains(&TaskKind::UrlStatus)
        );
        // item_url is None for all items
        assert!(items.iter().all(|w| w.ctx.item_url.is_none()));
    }

    #[test]
    fn test_no_llm_command_skips_summary_generate() {
        let test = worktrunk::testing::TestRepo::new();
        let repo = Repository::at(test.path()).expect("repo");
        let wt = WorktreeInfo {
            path: test.path().to_path_buf(),
            head: "deadbeef".to_string(),
            branch: Some("main".to_string()),
            bare: false,
            detached: false,
            locked: None,
            prunable: None,
        };

        // No llm_command, no skip_tasks — SummaryGenerate should still be skipped
        let options = CollectOptions {
            skip_tasks: HashSet::new(),
            llm_command: None,
            url_template: None,
            default_branch: None,
            integration_target: None,
        };

        let expected_results = Arc::new(ExpectedResults::default());
        let (tx, _rx) = chan::unbounded::<Result<TaskResult, TaskError>>();
        let mut item = build_worktree_item(&wt, true, false, false);

        let items =
            work_items_for_worktree(&repo, &wt, 0, &options, &expected_results, &tx, &mut item);

        assert!(
            !items.iter().any(|w| w.kind == TaskKind::SummaryGenerate),
            "SummaryGenerate should be skipped when llm_command is None"
        );
    }
}
