//! Progressive worktree collection with parallel git operations.
//!
//! This module provides a typed task framework for cell-by-cell progressive rendering.
//! Git operations run in parallel and send results as they complete.
//!
//! ## Architecture
//!
//! The framework guarantees that every spawned task is registered in `ExpectedResults`
//! and sends exactly one `TaskResult`:
//!
//! - `Task` trait: Each task type implements `compute()` returning a `TaskResult`
//! - `TaskSpawner`: Ties together registration + spawn + send in a single operation
//! - `define_task_suite!`: Generates spawn lists and expected sets from the same source
//!
//! This eliminates the "spawn but forget to register" failure mode from the old design.
//!
//! TODO(error-handling): Current implementation silently swallows git errors
//! and logs warnings to stderr. Consider whether failures should:
//! - Propagate to user (fail-fast)
//! - Show error placeholder in UI
//! - Continue silently (current behavior)

use crate::output;
use crossbeam_channel::Sender;
use std::path::PathBuf;
use std::sync::Arc;
use worktrunk::git::{LineDiff, Repository, Worktree};
use worktrunk::path::format_path_for_display;
use worktrunk::styling::warning_message;

use super::ci_status::PrStatus;
use super::collect::{ExpectedResults, TaskKind, TaskResult, detect_git_operation};
use super::model::{
    AheadBehind, BranchDiffTotals, CommitDetails, UpstreamStatus, WorkingTreeStatus,
};

// ============================================================================
// Options and Context
// ============================================================================

/// Options for controlling what data to collect.
///
/// Uses a skip set to control which tasks are spawned. Tasks not in the set
/// will be computed; tasks in the set will be skipped.
#[derive(Clone, Default)]
pub struct CollectOptions {
    /// Tasks to skip (not compute). Empty set means compute everything.
    ///
    /// This controls both:
    /// - Task spawning (in `collect_worktree_progressive`/`collect_branch_progressive`)
    /// - Column visibility (layout filters columns via `ColumnSpec::requires_task`)
    pub skip_tasks: std::collections::HashSet<super::collect::TaskKind>,
}

/// Context for task computation. Cloned and moved into spawned threads.
///
/// Contains all data needed by any task.
#[derive(Clone)]
pub struct TaskContext {
    pub repo_path: PathBuf,
    pub commit_sha: String,
    pub branch: Option<String>,
    /// Local default branch for informational stats (ahead/behind, branch diff).
    /// Always the local ref (e.g., "main"), providing stable comparisons.
    pub default_branch: Option<String>,
    /// Effective target for integration checks (status symbols, safe deletion).
    /// May be upstream (e.g., "origin/main") if it's ahead of local, catching remotely-merged branches.
    pub target: Option<String>,
    pub item_idx: usize,
    pub verbose_errors: bool,
}

// ============================================================================
// Task Trait and Spawner
// ============================================================================

/// A task that computes a single `TaskResult`.
///
/// Each task type has a compile-time `KIND` that determines which `TaskResult`
/// variant it produces. The `compute()` function receives a cloned context and
/// returns the result.
pub trait Task: Send + Sync + 'static {
    /// The kind of result this task produces (compile-time constant).
    const KIND: TaskKind;

    /// Compute the task result. Called in a spawned thread.
    fn compute(ctx: TaskContext) -> TaskResult;
}

/// Spawner that ties together registration + spawn + send.
///
/// Using `TaskSpawner::spawn<T>()` is the only way to run a task, and it
/// automatically registers the expected result kind before spawning.
pub struct TaskSpawner {
    tx: Sender<TaskResult>,
    expected: Arc<ExpectedResults>,
}

impl TaskSpawner {
    pub fn new(tx: Sender<TaskResult>, expected: Arc<ExpectedResults>) -> Self {
        Self { tx, expected }
    }

    /// Spawn a task, registering its expected result and sending on completion.
    ///
    /// This is the only way to run a `Task`. It guarantees:
    /// 1. The expected result is registered before the task runs
    /// 2. Exactly one result is sent when the task completes
    pub fn spawn<'scope, T: Task>(
        &self,
        scope: &'scope std::thread::Scope<'scope, '_>,
        ctx: &TaskContext,
    ) {
        // 1. Register expectation
        self.expected.expect(ctx.item_idx, T::KIND);

        // 2. Clone for the spawned thread
        let tx = self.tx.clone();
        let ctx = ctx.clone();

        // 3. Spawn the work
        scope.spawn(move || {
            let result = T::compute(ctx);
            debug_assert_eq!(TaskKind::from(&result), T::KIND);
            let _ = tx.send(result);
        });
    }
}

// ============================================================================
// Task Implementations
// ============================================================================

/// Task 1: Commit details (timestamp, message)
pub struct CommitDetailsTask;

impl Task for CommitDetailsTask {
    const KIND: TaskKind = TaskKind::CommitDetails;

    fn compute(ctx: TaskContext) -> TaskResult {
        let repo = Repository::at(&ctx.repo_path);
        let timestamp = match repo.commit_timestamp(&ctx.commit_sha) {
            Ok(ts) => ts,
            Err(e) => {
                log::warn!("commit_timestamp failed for {}: {}", ctx.commit_sha, e);
                0
            }
        };
        let commit_message = match repo.commit_message(&ctx.commit_sha) {
            Ok(msg) => msg,
            Err(e) => {
                log::warn!("commit_message failed for {}: {}", ctx.commit_sha, e);
                String::new()
            }
        };
        TaskResult::CommitDetails {
            item_idx: ctx.item_idx,
            commit: CommitDetails {
                timestamp,
                commit_message,
            },
        }
    }
}

/// Task 2: Ahead/behind counts vs local default branch (informational stats)
pub struct AheadBehindTask;

impl Task for AheadBehindTask {
    const KIND: TaskKind = TaskKind::AheadBehind;

    fn compute(ctx: TaskContext) -> TaskResult {
        // Use default_branch (local default branch) for stable informational display
        let (ahead, behind) = if let Some(base) = ctx.default_branch.as_deref() {
            let repo = Repository::at(&ctx.repo_path);
            match repo.ahead_behind(base, &ctx.commit_sha) {
                Ok((a, b)) => (a, b),
                Err(e) => {
                    log::warn!(
                        "ahead_behind failed for {} vs {}: {}",
                        ctx.commit_sha,
                        base,
                        e
                    );
                    (0, 0)
                }
            }
        } else {
            (0, 0)
        };

        TaskResult::AheadBehind {
            item_idx: ctx.item_idx,
            counts: AheadBehind { ahead, behind },
        }
    }
}

/// Task 3: Tree identity check (does the item's commit tree match integration target's tree?)
///
/// Uses target for integration detection (squash merge, rebase).
pub struct CommittedTreesMatchTask;

impl Task for CommittedTreesMatchTask {
    const KIND: TaskKind = TaskKind::CommittedTreesMatch;

    fn compute(ctx: TaskContext) -> TaskResult {
        // Use target for integration detection
        let committed_trees_match = if let Some(base) = ctx.target.as_deref() {
            let repo = Repository::at(&ctx.repo_path);
            // Use ctx.commit_sha (the item's commit) instead of HEAD,
            // since for branches without worktrees, HEAD is the main worktree's HEAD
            repo.trees_match(&ctx.commit_sha, base).unwrap_or(false)
        } else {
            false
        };

        TaskResult::CommittedTreesMatch {
            item_idx: ctx.item_idx,
            committed_trees_match,
        }
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

    fn compute(ctx: TaskContext) -> TaskResult {
        // Use target for integration detection
        let has_file_changes = if let Some(base) = ctx.target.as_deref() {
            let repo = Repository::at(&ctx.repo_path);
            if let Some(branch) = ctx.branch.as_deref() {
                // Check if three-dot diff is empty
                repo.has_added_changes(branch, base).unwrap_or(true)
            } else {
                // No branch name (detached HEAD) - assume has changes
                true
            }
        } else {
            // No integration base - assume has changes
            true
        };

        TaskResult::HasFileChanges {
            item_idx: ctx.item_idx,
            has_file_changes,
        }
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

    fn compute(ctx: TaskContext) -> TaskResult {
        // Use target for integration detection
        let would_merge_add = if let Some(base) = ctx.target.as_deref() {
            let repo = Repository::at(&ctx.repo_path);
            if let Some(branch) = ctx.branch.as_deref() {
                // Simulate merging branch into target
                repo.would_merge_add_to_target(branch, base).unwrap_or(true)
            } else {
                // No branch name (detached HEAD) - assume would add changes
                true
            }
        } else {
            // No integration base - assume would add changes
            true
        };

        TaskResult::WouldMergeAdd {
            item_idx: ctx.item_idx,
            would_merge_add,
        }
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

    fn compute(ctx: TaskContext) -> TaskResult {
        // Use target (target) for Ancestor integration reason
        let is_ancestor = if let Some(base) = ctx.target.as_deref() {
            let repo = Repository::at(&ctx.repo_path);
            // Check if branch HEAD is ancestor of target (or same commit)
            repo.is_ancestor(&ctx.commit_sha, base).unwrap_or(false)
        } else {
            // No integration base - assume not ancestor
            false
        };

        TaskResult::IsAncestor {
            item_idx: ctx.item_idx,
            is_ancestor,
        }
    }
}

/// Task 4: Branch diff stats vs local default branch (informational stats)
pub struct BranchDiffTask;

impl Task for BranchDiffTask {
    const KIND: TaskKind = TaskKind::BranchDiff;

    fn compute(ctx: TaskContext) -> TaskResult {
        // Use default_branch (local default branch) for stable informational display
        let diff = if let Some(base) = ctx.default_branch.as_deref() {
            let repo = Repository::at(&ctx.repo_path);
            match repo.branch_diff_stats(base, &ctx.commit_sha) {
                Ok(d) => d,
                Err(e) => {
                    log::warn!(
                        "branch_diff_stats failed for {} vs {}: {}",
                        ctx.commit_sha,
                        base,
                        e
                    );
                    LineDiff::default()
                }
            }
        } else {
            LineDiff::default()
        };

        TaskResult::BranchDiff {
            item_idx: ctx.item_idx,
            branch_diff: BranchDiffTotals { diff },
        }
    }
}

/// Task 5 (worktree only): Working tree diff + status flags
pub struct WorkingTreeDiffTask;

impl Task for WorkingTreeDiffTask {
    const KIND: TaskKind = TaskKind::WorkingTreeDiff;

    fn compute(ctx: TaskContext) -> TaskResult {
        let repo = Repository::at(&ctx.repo_path);
        let status_output = match repo.run_command(&["status", "--porcelain"]) {
            Ok(output) => output,
            Err(e) => {
                log::warn!("git status failed for {}: {}", ctx.repo_path.display(), e);
                return TaskResult::WorkingTreeDiff {
                    item_idx: ctx.item_idx,
                    working_tree_diff: LineDiff::default(),
                    working_tree_diff_with_main: None,
                    working_tree_status: WorkingTreeStatus::default(),
                    has_conflicts: false,
                };
            }
        };

        let (working_tree_status, is_dirty, has_conflicts) =
            parse_working_tree_status(&status_output);

        let working_tree_diff = if is_dirty {
            repo.working_tree_diff_stats().unwrap_or_default()
        } else {
            LineDiff::default()
        };

        // Use default_branch (local default branch) for informational display
        let working_tree_diff_with_main = repo
            .working_tree_diff_with_base(ctx.default_branch.as_deref(), is_dirty)
            .ok()
            .flatten();

        TaskResult::WorkingTreeDiff {
            item_idx: ctx.item_idx,
            working_tree_diff,
            working_tree_diff_with_main,
            working_tree_status,
            has_conflicts,
        }
    }
}

/// Task 6: Potential merge conflicts check (merge-tree vs local main)
///
/// Uses default_branch (local main) for consistency with other Main subcolumn symbols.
/// Shows whether merging to your local main would conflict.
pub struct MergeTreeConflictsTask;

impl Task for MergeTreeConflictsTask {
    const KIND: TaskKind = TaskKind::MergeTreeConflicts;

    fn compute(ctx: TaskContext) -> TaskResult {
        // Use default_branch (local main) - consistent with other Main subcolumn symbols
        let has_merge_tree_conflicts = if let Some(base) = ctx.default_branch.as_deref() {
            let repo = Repository::at(&ctx.repo_path);
            repo.has_merge_conflicts(base, &ctx.commit_sha)
                .unwrap_or(false)
        } else {
            false
        };

        TaskResult::MergeTreeConflicts {
            item_idx: ctx.item_idx,
            has_merge_tree_conflicts,
        }
    }
}

/// Task 7 (worktree only): Git operation state detection (rebase/merge)
pub struct GitOperationTask;

impl Task for GitOperationTask {
    const KIND: TaskKind = TaskKind::GitOperation;

    fn compute(ctx: TaskContext) -> TaskResult {
        let repo = Repository::at(&ctx.repo_path);
        let git_operation = detect_git_operation(&repo);
        TaskResult::GitOperation {
            item_idx: ctx.item_idx,
            git_operation,
        }
    }
}

/// Task 8 (worktree only): User-defined status from git config
pub struct UserMarkerTask;

impl Task for UserMarkerTask {
    const KIND: TaskKind = TaskKind::UserMarker;

    fn compute(ctx: TaskContext) -> TaskResult {
        let repo = Repository::at(&ctx.repo_path);
        let user_marker = repo.user_marker(ctx.branch.as_deref());
        TaskResult::UserMarker {
            item_idx: ctx.item_idx,
            user_marker,
        }
    }
}

/// Task 9: Upstream tracking status
pub struct UpstreamTask;

impl Task for UpstreamTask {
    const KIND: TaskKind = TaskKind::Upstream;

    fn compute(ctx: TaskContext) -> TaskResult {
        let repo = Repository::at(&ctx.repo_path);
        let upstream = ctx
            .branch
            .as_deref()
            .and_then(|branch| match repo.upstream_branch(branch) {
                Ok(Some(upstream_branch)) => {
                    let remote = upstream_branch.split_once('/').map(|(r, _)| r.to_string());
                    match repo.ahead_behind(&upstream_branch, &ctx.commit_sha) {
                        Ok((ahead, behind)) => Some(UpstreamStatus {
                            remote,
                            ahead,
                            behind,
                        }),
                        Err(e) => {
                            if ctx.verbose_errors {
                                let _ = output::print(warning_message(format!(
                                    "ahead_behind failed for {}: {}",
                                    format_path_for_display(&ctx.repo_path),
                                    e
                                )));
                            }
                            None
                        }
                    }
                }
                Ok(None) => None,
                Err(e) => {
                    if ctx.verbose_errors {
                        let _ = output::print(warning_message(format!(
                            "upstream_branch failed for {}: {}",
                            format_path_for_display(&ctx.repo_path),
                            e
                        )));
                    }
                    None
                }
            })
            .unwrap_or_default();

        TaskResult::Upstream {
            item_idx: ctx.item_idx,
            upstream,
        }
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

    fn compute(ctx: TaskContext) -> TaskResult {
        let repo = Repository::at(&ctx.repo_path);
        let repo_path = repo
            .worktree_root()
            .ok()
            .unwrap_or_else(|| ctx.repo_path.clone());

        let pr_status = ctx.branch.as_deref().and_then(|branch| {
            let has_upstream = repo.upstream_branch(branch).ok().flatten().is_some();
            PrStatus::detect(branch, &ctx.commit_sha, &repo_path, has_upstream)
        });

        TaskResult::CiStatus {
            item_idx: ctx.item_idx,
            pr_status,
        }
    }
}

// ============================================================================
// Collection Entry Points
// ============================================================================

/// Collect worktree data progressively, sending results as each task completes.
///
/// Spawns parallel git operations (up to 10). Each task sends a TaskResult when it
/// completes, enabling progressive UI updates. Tasks in `options.skip_tasks` are not spawned.
///
/// # Parameters
/// - `default_branch`: Local default branch for informational stats (ahead/behind, branch diff)
/// - `target`: Effective target for integration checks (may be upstream if ahead)
pub fn collect_worktree_progressive(
    wt: &Worktree,
    item_idx: usize,
    default_branch: &str,
    target: &str,
    options: &CollectOptions,
    tx: Sender<TaskResult>,
    expected_results: &Arc<ExpectedResults>,
) {
    use super::collect::TaskKind;

    let ctx = TaskContext {
        repo_path: wt.path.clone(),
        commit_sha: wt.head.clone(),
        branch: wt.branch.clone(),
        default_branch: Some(default_branch.to_string()),
        target: Some(target.to_string()),
        item_idx,
        verbose_errors: true, // Worktrees show verbose errors
    };

    let spawner = TaskSpawner::new(tx, expected_results.clone());
    let skip = &options.skip_tasks;

    std::thread::scope(|s| {
        // Core tasks (always run)
        spawner.spawn::<CommitDetailsTask>(s, &ctx);
        spawner.spawn::<AheadBehindTask>(s, &ctx);
        spawner.spawn::<CommittedTreesMatchTask>(s, &ctx);
        spawner.spawn::<HasFileChangesTask>(s, &ctx);
        spawner.spawn::<IsAncestorTask>(s, &ctx);
        spawner.spawn::<WorkingTreeDiffTask>(s, &ctx);
        spawner.spawn::<GitOperationTask>(s, &ctx);
        spawner.spawn::<UserMarkerTask>(s, &ctx);
        spawner.spawn::<UpstreamTask>(s, &ctx);

        // Optional tasks (check skip set)
        if !skip.contains(&TaskKind::BranchDiff) {
            spawner.spawn::<BranchDiffTask>(s, &ctx);
        }
        if !skip.contains(&TaskKind::MergeTreeConflicts) {
            spawner.spawn::<MergeTreeConflictsTask>(s, &ctx);
        }
        if !skip.contains(&TaskKind::CiStatus) {
            spawner.spawn::<CiStatusTask>(s, &ctx);
        }
        if !skip.contains(&TaskKind::WouldMergeAdd) {
            spawner.spawn::<WouldMergeAddTask>(s, &ctx);
        }
    });
}

/// Collect branch data progressively, sending results as each task completes.
///
/// Spawns parallel git operations (up to 7, similar to worktrees but without working
/// tree operations). Tasks in `options.skip_tasks` are not spawned.
///
/// # Parameters
/// - `default_branch`: Local default branch for informational stats (ahead/behind, branch diff)
/// - `target`: Effective target for integration checks (may be upstream if ahead)
#[allow(clippy::too_many_arguments)]
pub fn collect_branch_progressive(
    branch_name: &str,
    commit_sha: &str,
    repo_path: &std::path::Path,
    item_idx: usize,
    default_branch: &str,
    target: &str,
    options: &CollectOptions,
    tx: Sender<TaskResult>,
    expected_results: &Arc<ExpectedResults>,
) {
    use super::collect::TaskKind;

    let ctx = TaskContext {
        repo_path: repo_path.to_path_buf(),
        commit_sha: commit_sha.to_string(),
        branch: Some(branch_name.to_string()),
        default_branch: Some(default_branch.to_string()),
        target: Some(target.to_string()),
        item_idx,
        verbose_errors: false, // Branches don't show verbose errors
    };

    let spawner = TaskSpawner::new(tx, expected_results.clone());
    let skip = &options.skip_tasks;

    std::thread::scope(|s| {
        // Core tasks (always run)
        spawner.spawn::<CommitDetailsTask>(s, &ctx);
        spawner.spawn::<AheadBehindTask>(s, &ctx);
        spawner.spawn::<CommittedTreesMatchTask>(s, &ctx);
        spawner.spawn::<HasFileChangesTask>(s, &ctx);
        spawner.spawn::<IsAncestorTask>(s, &ctx);
        spawner.spawn::<UpstreamTask>(s, &ctx);

        // Optional tasks (check skip set)
        if !skip.contains(&TaskKind::BranchDiff) {
            spawner.spawn::<BranchDiffTask>(s, &ctx);
        }
        if !skip.contains(&TaskKind::MergeTreeConflicts) {
            spawner.spawn::<MergeTreeConflictsTask>(s, &ctx);
        }
        if !skip.contains(&TaskKind::CiStatus) {
            spawner.spawn::<CiStatusTask>(s, &ctx);
        }
        if !skip.contains(&TaskKind::WouldMergeAdd) {
            spawner.spawn::<WouldMergeAddTask>(s, &ctx);
        }
    });
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
}
