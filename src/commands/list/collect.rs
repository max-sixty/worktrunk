//! Worktree data collection with parallelized git operations.
//!
//! This module provides an efficient approach to collecting worktree data:
//! - All tasks flattened into a single Rayon work queue
//! - Network tasks (CI, URL) sorted to run last
//! - Progressive updates via channels (update UI as each task completes)
//!
//! ## Skeleton Performance
//!
//! The skeleton (placeholder table with loading indicators) must render as fast as possible
//! to give users immediate feedback. Every git command before skeleton adds latency.
//!
//! ### Fixed Command Count (O(1), not O(N))
//!
//! Pre-skeleton runs a **fixed number of git commands** regardless of worktree count.
//! This is achieved through:
//! - **Batching** — timestamp fetch passes all SHAs to one `git show` command
//! - **Parallelization** — independent commands run concurrently via `join!` macro
//!
//! **Steady-state (4-6 commands):**
//!
//! | Command | Purpose |
//! |---------|---------|
//! | `git worktree list --porcelain` | Enumerate worktrees |
//! | `git config worktrunk.default-branch` | Cached default branch |
//! | `git show -s --format='%H %ct' SHA1 SHA2 ...` | **Batched** timestamps for sorting |
//! | `git config --bool core.bare` | Bare repo check for expected-path logic |
//! | `git for-each-ref refs/heads` | Only with `--branches` flag |
//! | `git for-each-ref refs/remotes` | Only with `--remotes` flag |
//!
//! **Non-git operations (negligible latency):**
//! - Path canonicalization — detect current worktree
//! - Project config file read — check if URL column needed (no template expansion)
//!
//! ### First-Run Behavior
//!
//! When `worktrunk.default-branch` is not cached, `default_branch()` runs additional
//! commands to detect it:
//! 1. Query primary remote (origin/HEAD or `git ls-remote`)
//! 2. Fall back to local inference (check init.defaultBranch, common names)
//! 3. Cache result to `git config worktrunk.default-branch`
//!
//! Subsequent runs use the cached value — only one `git config` call.
//!
//! ### Post-Skeleton Operations
//!
//! Everything else runs **after** the skeleton appears:
//! - `get_switch_previous()` — previous branch detection (updates gutter symbol)
//! - `effective_integration_target()` — upstream vs local target check
//! - URL template expansion — parallelized in task spawning
//! - All computed fields (ahead/behind, diffs, CI status, etc.)
//!
//! When adding new features, ask: "Can this be computed after skeleton?" If yes, defer it.
//! The skeleton shows `·` placeholder for gutter symbols, filled in when data loads.
//!
//! ## Unified Collection Architecture
//!
//! Progressive and buffered modes use the same collection and rendering code.
//! The only difference is whether intermediate updates are shown during collection:
//! - Progressive: renders a skeleton table and updates rows/footer as data arrives (TTY),
//!   or renders once at the end (non-TTY)
//! - Buffered: collects silently, then renders the final table
//!
//! Both modes render the final table in `collect()`, ensuring a single canonical rendering path.
//!
//! **Flat parallelism**: All tasks (for all worktrees and branches) are collected into a single
//! work queue and processed via Rayon's thread pool. This avoids nested parallelism and keeps
//! utilization high regardless of worktree count (pool size is set at startup; default is 2x CPU
//! cores unless `RAYON_NUM_THREADS` is set).
//!
//! **Task ordering**: Work items are sorted so local git operations run first, network tasks
//! (CI status, URL health checks) run last. This ensures the table fills in quickly with local
//! data while slower network requests complete in the background.
use anyhow::Context;
use color_print::cformat;
use crossbeam_channel as chan;
use dunce::canonicalize;
use rayon::prelude::*;
use rayon_join_macro::join;
use worktrunk::git::{LineDiff, Repository, WorktreeInfo};
use worktrunk::styling::{INFO_SYMBOL, format_with_gutter, warning_message};

use crate::commands::is_worktree_at_expected_path;

use super::ci_status::PrStatus;
use super::model::{
    AheadBehind, BranchDiffTotals, CommitDetails, DisplayFields, GitOperationState, ItemKind,
    ListItem, UpstreamStatus, WorktreeData,
};

use super::model::WorkingTreeStatus;

/// Context for status symbol computation during result processing
#[derive(Clone, Default)]
struct StatusContext {
    has_merge_tree_conflicts: bool,
    /// Working tree conflict check result (--full only, worktrees only).
    /// None = use commit check (task didn't run or working tree clean)
    /// Some(b) = dirty working tree, b is conflict result
    // TODO: If we need to distinguish "task didn't run" from "clean working tree",
    // expand to an enum. Currently both cases fall back to commit-based check.
    has_working_tree_conflicts: Option<bool>,
    user_marker: Option<String>,
    working_tree_status: Option<WorkingTreeStatus>,
    has_conflicts: bool,
}

impl StatusContext {
    fn apply_to(&self, item: &mut ListItem, target: &str) {
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
pub(super) enum TaskResult {
    /// Commit timestamp and message
    CommitDetails {
        item_idx: usize,
        commit: CommitDetails,
    },
    /// Ahead/behind counts vs default branch
    AheadBehind {
        item_idx: usize,
        counts: AheadBehind,
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
        working_tree_diff_with_main: Option<LineDiff>,
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
    fn item_idx(&self) -> usize {
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

/// Detect if a worktree is in the middle of a git operation (rebase/merge).
pub(super) fn detect_git_operation(wt: &worktrunk::git::WorkingTree<'_>) -> GitOperationState {
    if wt.is_rebasing().unwrap_or(false) {
        GitOperationState::Rebase
    } else if wt.is_merging().unwrap_or(false) {
        GitOperationState::Merge
    } else {
        GitOperationState::None
    }
}

/// Result of draining task results - indicates whether all results were received
/// or if a timeout occurred.
#[derive(Debug)]
enum DrainOutcome {
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
struct MissingResult {
    item_idx: usize,
    name: String,
    missing_kinds: Vec<TaskKind>,
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
}

impl TaskError {
    pub fn new(item_idx: usize, kind: TaskKind, message: impl Into<String>) -> Self {
        Self {
            item_idx,
            kind,
            message: message.into(),
        }
    }
}

/// Tracks expected result types per item for timeout diagnostics.
///
/// Populated at spawn time so we know exactly which results to expect,
/// without hardcoding result lists that could drift from the spawn functions.
#[derive(Default)]
pub(super) struct ExpectedResults {
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
    fn results_for(&self, item_idx: usize) -> Vec<TaskKind> {
        self.inner
            .lock()
            .unwrap()
            .get(item_idx)
            .cloned()
            .unwrap_or_default()
    }
}

/// Apply default values for a failed task.
///
/// When a task fails, we still need to populate the item fields with sensible
/// defaults so the UI can render. This centralizes all default logic in one place.
fn apply_default(items: &mut [ListItem], status_contexts: &mut [StatusContext], error: &TaskError) {
    let idx = error.item_idx;
    match error.kind {
        TaskKind::CommitDetails => {
            items[idx].commit = Some(CommitDetails::default());
        }
        TaskKind::AheadBehind => {
            items[idx].counts = Some(AheadBehind::default());
        }
        TaskKind::CommittedTreesMatch => {
            // Conservative: don't claim integrated if we couldn't check
            items[idx].committed_trees_match = Some(false);
        }
        TaskKind::HasFileChanges => {
            // Conservative: assume has changes if we couldn't check
            items[idx].has_file_changes = Some(true);
        }
        TaskKind::WouldMergeAdd => {
            // Conservative: assume would add changes if we couldn't check
            items[idx].would_merge_add = Some(true);
        }
        TaskKind::IsAncestor => {
            // Conservative: don't claim merged if we couldn't check
            items[idx].is_ancestor = Some(false);
        }
        TaskKind::BranchDiff => {
            // Leave as None — UI shows `…` for skipped/failed tasks
        }
        TaskKind::WorkingTreeDiff => {
            if let ItemKind::Worktree(data) = &mut items[idx].kind {
                data.working_tree_diff = Some(LineDiff::default());
                data.working_tree_diff_with_main = Some(None);
            } else {
                debug_assert!(false, "WorkingTreeDiff task spawned for non-worktree item");
            }
            status_contexts[idx].working_tree_status = Some(WorkingTreeStatus::default());
            status_contexts[idx].has_conflicts = false;
        }
        TaskKind::MergeTreeConflicts => {
            // Don't show conflict symbol if we couldn't check
            status_contexts[idx].has_merge_tree_conflicts = false;
        }
        TaskKind::WorkingTreeConflicts => {
            // Fall back to commit-based check on failure
            status_contexts[idx].has_working_tree_conflicts = None;
        }
        TaskKind::GitOperation => {
            // Already defaults to GitOperationState::None in WorktreeData
        }
        TaskKind::UserMarker => {
            // Already defaults to None
            status_contexts[idx].user_marker = None;
        }
        TaskKind::Upstream => {
            items[idx].upstream = Some(UpstreamStatus::default());
        }
        TaskKind::CiStatus => {
            // Leave as None (not fetched) on error. This allows the hint path
            // in mod.rs to run and show "install gh/glab" when CI tools fail.
            // Some(None) means "CI tool ran successfully but found no PR".
        }
        TaskKind::UrlStatus => {
            // URL is set at item creation, only default url_active
            items[idx].url_active = None;
        }
    }
}

/// Drain task results from the channel and apply them to items.
///
/// This is the shared logic between progressive and buffered collection modes.
/// The `on_result` callback is called after each result is processed with the
/// item index and a reference to the updated item, allowing progressive mode
/// to update the live table while buffered mode does nothing.
///
/// Uses a 30-second deadline to prevent infinite hangs if git commands stall.
/// When timeout occurs, returns `DrainOutcome::TimedOut` with diagnostic info.
///
/// Errors are collected in the `errors` vec for display after rendering.
/// Default values are applied for failed tasks so the UI can still render.
///
/// Callers decide how to handle timeout:
/// - `collect()`: Shows user-facing diagnostic (interactive command)
/// - `populate_item()`: Logs silently (used by statusline)
fn drain_results(
    rx: chan::Receiver<Result<TaskResult, TaskError>>,
    items: &mut [ListItem],
    errors: &mut Vec<TaskError>,
    expected_results: &ExpectedResults,
    mut on_result: impl FnMut(usize, &mut ListItem, &StatusContext),
) -> DrainOutcome {
    use std::time::{Duration, Instant};

    // Deadline for the entire drain operation (30 seconds should be more than enough)
    let deadline = Instant::now() + Duration::from_secs(30);

    // Track which result kinds we've received per item (for timeout diagnostics)
    let mut received_by_item: Vec<Vec<TaskKind>> = vec![Vec::new(); items.len()];

    // Temporary storage for data needed by status_symbols computation
    let mut status_contexts = vec![StatusContext::default(); items.len()];

    // Process task results as they arrive (with deadline)
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            // Deadline exceeded - build diagnostic info showing MISSING results
            let received_count: usize = received_by_item.iter().map(|v| v.len()).sum();

            // Find items with missing results by comparing received vs expected
            let mut items_with_missing: Vec<MissingResult> = Vec::new();

            for (item_idx, item) in items.iter().enumerate() {
                // Get expected results for this item (populated at spawn time)
                let expected = expected_results.results_for(item_idx);

                // Get received results for this item (empty vec if none received)
                let received = received_by_item[item_idx].as_slice();

                // Find missing results
                let missing_kinds: Vec<TaskKind> = expected
                    .iter()
                    .filter(|kind| !received.contains(kind))
                    .copied()
                    .collect();

                if !missing_kinds.is_empty() {
                    let name = item
                        .branch
                        .clone()
                        .unwrap_or_else(|| item.head[..8.min(item.head.len())].to_string());
                    items_with_missing.push(MissingResult {
                        item_idx,
                        name,
                        missing_kinds,
                    });
                }
            }

            // Sort by item index and limit to first 5
            items_with_missing.sort_by_key(|result| result.item_idx);
            items_with_missing.truncate(5);

            return DrainOutcome::TimedOut {
                received_count,
                items_with_missing,
            };
        }

        let outcome = match rx.recv_timeout(remaining) {
            Ok(outcome) => outcome,
            Err(chan::RecvTimeoutError::Timeout) => continue, // Check deadline in next iteration
            Err(chan::RecvTimeoutError::Disconnected) => break, // All senders dropped - done
        };

        // Handle success or error
        let (item_idx, kind) = match outcome {
            Ok(ref result) => (result.item_idx(), TaskKind::from(result)),
            Err(ref error) => (error.item_idx, error.kind),
        };

        // Track this result for diagnostics (both success and error count as "received")
        received_by_item[item_idx].push(kind);

        // Handle error case: apply defaults and collect error
        if let Err(error) = outcome {
            apply_default(items, &mut status_contexts, &error);
            errors.push(error);
            let item = &mut items[item_idx];
            let status_ctx = &status_contexts[item_idx];
            on_result(item_idx, item, status_ctx);
            continue;
        }

        // Handle success case
        let result = outcome.unwrap();
        let item = &mut items[item_idx];
        let status_ctx = &mut status_contexts[item_idx];

        match result {
            TaskResult::CommitDetails { commit, .. } => {
                item.commit = Some(commit);
            }
            TaskResult::AheadBehind { counts, .. } => {
                item.counts = Some(counts);
            }
            TaskResult::CommittedTreesMatch {
                committed_trees_match,
                ..
            } => {
                item.committed_trees_match = Some(committed_trees_match);
            }
            TaskResult::HasFileChanges {
                has_file_changes, ..
            } => {
                item.has_file_changes = Some(has_file_changes);
            }
            TaskResult::WouldMergeAdd {
                would_merge_add, ..
            } => {
                item.would_merge_add = Some(would_merge_add);
            }
            TaskResult::IsAncestor { is_ancestor, .. } => {
                item.is_ancestor = Some(is_ancestor);
            }
            TaskResult::BranchDiff { branch_diff, .. } => {
                item.branch_diff = Some(branch_diff);
            }
            TaskResult::WorkingTreeDiff {
                working_tree_diff,
                working_tree_diff_with_main,
                working_tree_status,
                has_conflicts,
                ..
            } => {
                if let ItemKind::Worktree(data) = &mut item.kind {
                    data.working_tree_diff = Some(working_tree_diff);
                    data.working_tree_diff_with_main = Some(working_tree_diff_with_main);
                } else {
                    debug_assert!(false, "WorkingTreeDiff result for non-worktree item");
                }
                // Store for status_symbols computation
                status_ctx.working_tree_status = Some(working_tree_status);
                status_ctx.has_conflicts = has_conflicts;
            }
            TaskResult::MergeTreeConflicts {
                has_merge_tree_conflicts,
                ..
            } => {
                // Store for status_symbols computation
                status_ctx.has_merge_tree_conflicts = has_merge_tree_conflicts;
            }
            TaskResult::WorkingTreeConflicts {
                has_working_tree_conflicts,
                ..
            } => {
                // Store for status_symbols computation (takes precedence over commit check)
                status_ctx.has_working_tree_conflicts = has_working_tree_conflicts;
            }
            TaskResult::GitOperation { git_operation, .. } => {
                if let ItemKind::Worktree(data) = &mut item.kind {
                    data.git_operation = git_operation;
                } else {
                    debug_assert!(false, "GitOperation result for non-worktree item");
                }
            }
            TaskResult::UserMarker { user_marker, .. } => {
                // Store for status_symbols computation
                status_ctx.user_marker = user_marker;
            }
            TaskResult::Upstream { upstream, .. } => {
                item.upstream = Some(upstream);
            }
            TaskResult::CiStatus { pr_status, .. } => {
                // Wrap in Some() to indicate "loaded" (Some(None) = no CI, Some(Some(status)) = has CI)
                item.pr_status = Some(pr_status);
            }
            TaskResult::UrlStatus { url, active, .. } => {
                // Two-phase URL rendering:
                // 1. First result (from spawning code): url=Some, active=None → URL appears in normal styling
                // 2. Second result (from health check): url=None, active=Some → dims if inactive
                // Only update non-None fields to preserve values from earlier results.
                if url.is_some() {
                    item.url = url;
                }
                if active.is_some() {
                    item.url_active = active;
                }
            }
        }

        // Invoke callback (progressive mode re-renders rows, buffered mode does nothing)
        on_result(item_idx, item, status_ctx);
    }

    DrainOutcome::Complete
}

/// Get branches that don't have worktrees.
///
/// Returns (branch_name, commit_sha) pairs for all branches without associated worktrees.
fn get_branches_without_worktrees(
    repo: &Repository,
    worktrees: &[WorktreeInfo],
) -> anyhow::Result<Vec<(String, String)>> {
    // Get all local branches
    let all_branches = repo.list_local_branches()?;

    // Build a set of branch names that have worktrees
    let worktree_branches = worktree_branch_set(worktrees);

    // Filter to branches without worktrees
    let branches_without_worktrees: Vec<_> = all_branches
        .into_iter()
        .filter(|(branch_name, _)| !worktree_branches.contains(branch_name.as_str()))
        .collect();

    Ok(branches_without_worktrees)
}

fn worktree_branch_set(worktrees: &[WorktreeInfo]) -> std::collections::HashSet<&str> {
    worktrees
        .iter()
        .filter_map(|wt| wt.branch.as_deref())
        .collect()
}

/// Collect worktree data with optional progressive rendering.
///
/// When `show_progress` is true, renders a skeleton immediately and updates as data arrives.
/// When false, behavior depends on `render_table`:
/// - If `render_table` is true: renders final table (buffered mode)
/// - If `render_table` is false: returns data without rendering (JSON mode)
///
/// The `command_timeout` parameter, if set, limits how long individual git commands can run.
/// This is useful for `wt select` to show the TUI faster by skipping slow operations.
///
/// TODO: Now that we skip expensive tasks for stale branches (see `skip_expensive_for_stale`),
/// the timeout may be unnecessary. Consider removing it if it doesn't provide value.
///
/// The `skip_expensive_for_stale` parameter enables batch-fetching ahead/behind counts and
/// skipping expensive merge-base operations for branches far behind the default branch.
/// This dramatically improves performance for repos with many stale branches.
#[allow(clippy::too_many_arguments)]
pub fn collect(
    repo: &Repository,
    show_branches: bool,
    show_remotes: bool,
    skip_tasks: &std::collections::HashSet<TaskKind>,
    show_progress: bool,
    render_table: bool,
    config: &worktrunk::config::WorktrunkConfig,
    command_timeout: Option<std::time::Duration>,
    skip_expensive_for_stale: bool,
) -> anyhow::Result<Option<super::model::ListData>> {
    use super::progressive_table::ProgressiveTable;
    worktrunk::shell_exec::trace_instant("List collect started");

    // Phase 1: Get worktree list (required for everything else)
    let worktrees = repo.list_worktrees().context("Failed to list worktrees")?;
    if worktrees.is_empty() {
        return Ok(None);
    }

    // Detect current worktree by checking if repo path is inside any worktree.
    // This avoids a git command - we just compare canonicalized paths.
    let repo_path_canonical = canonicalize(repo.discovery_path()).ok();
    let current_worktree_path = repo_path_canonical.as_ref().and_then(|repo_path| {
        worktrees.iter().find_map(|wt| {
            canonicalize(&wt.path)
                .ok()
                .filter(|wt_path| repo_path.starts_with(wt_path))
        })
    });

    // Phase 2: Parallel fetch of independent git data
    // These operations don't depend on each other, only on worktree list.
    // Running them in parallel reduces pre-skeleton time (e.g., ~46ms to ~28ms).
    let (default_branch, branches_without_worktrees, remote_branches) = join!(
        || repo.default_branch(),
        || {
            if show_branches {
                get_branches_without_worktrees(repo, &worktrees)
            } else {
                Ok(Vec::new())
            }
        },
        || {
            if show_remotes {
                repo.list_untracked_remote_branches()
            } else {
                Ok(Vec::new())
            }
        }
    );
    // Show warning if user configured a default branch that doesn't exist locally
    if let Some(configured) = repo.invalid_default_branch_config() {
        let msg =
            cformat!("Configured default branch <bold>{configured}</> does not exist locally");
        crate::output::print(warning_message(msg))?;
        let hint = cformat!("Run <bright-black>wt config state default-branch clear</> to reset");
        crate::output::print(worktrunk::styling::hint_message(hint))?;
    }

    let branches_without_worktrees = branches_without_worktrees?;
    let remote_branches = remote_branches?;

    // Main worktree is the worktree on the default branch (if exists), else first non-prunable worktree.
    // find_home returns None if all worktrees are prunable or the list is empty.
    // TODO: show ellipsis or indicator when default_branch is None and columns are empty
    let main_worktree =
        WorktreeInfo::find_home(&worktrees, default_branch.as_deref().unwrap_or(""))
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("No worktrees found"))?;

    // Defer previous_branch lookup until after skeleton - set is_previous later
    // (skeleton shows placeholder gutter, actual symbols appear when data loads)

    // Phase 3: Batch fetch timestamps (needs all SHAs from worktrees + branches)
    let all_shas: Vec<&str> = worktrees
        .iter()
        .map(|wt| wt.head.as_str())
        .chain(
            branches_without_worktrees
                .iter()
                .map(|(_, sha)| sha.as_str()),
        )
        .chain(remote_branches.iter().map(|(_, sha)| sha.as_str()))
        .collect();
    let timestamps = repo.commit_timestamps(&all_shas).unwrap_or_default();

    // Sort worktrees: current first, main second, then by timestamp descending
    let sorted_worktrees = sort_worktrees_with_cache(
        worktrees.clone(),
        &main_worktree,
        current_worktree_path.as_ref(),
        &timestamps,
    );

    // Sort branches by timestamp (most recent first)
    let branches_without_worktrees =
        sort_by_timestamp_desc_with_cache(branches_without_worktrees, &timestamps, |(_, sha)| {
            sha.as_str()
        });
    let remote_branches =
        sort_by_timestamp_desc_with_cache(remote_branches, &timestamps, |(_, sha)| sha.as_str());

    // Pre-canonicalize main_worktree.path for is_main comparison
    // (paths from git worktree list may differ based on symlinks or working directory)
    let main_worktree_canonical = canonicalize(&main_worktree.path).ok();

    // Check if URL template is configured (for layout column allocation).
    // Template expansion is deferred to post-skeleton to minimize time-to-skeleton.
    let url_template = repo.url_template();
    // Initialize worktree items with identity fields and None for computed fields
    let mut all_items: Vec<ListItem> = sorted_worktrees
        .iter()
        .map(|wt| {
            // Canonicalize paths for comparison - git worktree list may return different
            // path representations depending on symlinks or which directory you run from
            let wt_canonical = canonicalize(&wt.path).ok();
            let is_main = match (&wt_canonical, &main_worktree_canonical) {
                (Some(wt_c), Some(main_c)) => wt_c == main_c,
                // Fallback to direct comparison if canonicalization fails
                _ => wt.path == main_worktree.path,
            };
            let is_current = current_worktree_path
                .as_ref()
                .is_some_and(|cp| wt_canonical.as_ref() == Some(cp));
            // is_previous set to false initially - computed after skeleton
            let is_previous = false;

            // Check if worktree is at its expected path based on config template
            let branch_worktree_mismatch = !is_worktree_at_expected_path(wt, repo, config);

            let mut worktree_data =
                WorktreeData::from_worktree(wt, is_main, is_current, is_previous);
            worktree_data.branch_worktree_mismatch = branch_worktree_mismatch;

            // URL expanded post-skeleton to minimize time-to-skeleton
            ListItem {
                head: wt.head.clone(),
                branch: wt.branch.clone(),
                commit: None,
                counts: None,
                branch_diff: None,
                committed_trees_match: None,
                has_file_changes: None,
                would_merge_add: None,
                is_ancestor: None,
                upstream: None,
                pr_status: None,
                url: None,
                url_active: None,
                status_symbols: None,
                display: DisplayFields::default(),
                kind: ItemKind::Worktree(Box::new(worktree_data)),
            }
        })
        .collect();

    // Initialize branch items (local and remote) - URLs expanded post-skeleton
    let branch_start_idx = all_items.len();
    all_items.extend(
        branches_without_worktrees
            .iter()
            .map(|(name, sha)| ListItem::new_branch(sha.clone(), name.clone())),
    );

    let remote_start_idx = all_items.len();
    all_items.extend(
        remote_branches
            .iter()
            .map(|(name, sha)| ListItem::new_branch(sha.clone(), name.clone())),
    );

    // If no URL template configured, add UrlStatus to skip_tasks
    let mut effective_skip_tasks = skip_tasks.clone();
    if url_template.is_none() {
        effective_skip_tasks.insert(TaskKind::UrlStatus);
    }

    // Calculate layout from items (worktrees, local branches, and remote branches)
    let layout = super::layout::calculate_layout_from_basics(
        &all_items,
        &effective_skip_tasks,
        &main_worktree.path,
        url_template.as_deref(),
    );

    // Single-line invariant: use safe width to prevent line wrapping
    let max_width = crate::display::get_terminal_width();

    // Create collection options from skip set
    let mut options = super::collect_progressive_impl::CollectOptions {
        skip_tasks: effective_skip_tasks,
        url_template: url_template.clone(),
        ..Default::default()
    };

    // Track expected results per item - populated as spawns are queued
    let expected_results = std::sync::Arc::new(ExpectedResults::default());
    let num_worktrees = all_items
        .iter()
        .filter(|item| item.worktree_data().is_some())
        .count();
    let num_local_branches = branches_without_worktrees.len();
    let num_remote_branches = remote_branches.len();

    let footer_base =
        if (show_branches && num_local_branches > 0) || (show_remotes && num_remote_branches > 0) {
            let mut parts = vec![format!("{} worktrees", num_worktrees)];
            if show_branches && num_local_branches > 0 {
                parts.push(format!("{} branches", num_local_branches));
            }
            if show_remotes && num_remote_branches > 0 {
                parts.push(format!("{} remote branches", num_remote_branches));
            }
            format!("Showing {}", parts.join(", "))
        } else {
            let plural = if num_worktrees == 1 { "" } else { "s" };
            format!("Showing {} worktree{}", num_worktrees, plural)
        };

    // Create progressive table if showing progress
    let mut progressive_table = if show_progress {
        use anstyle::Style;
        let dim = Style::new().dimmed();

        // Build skeleton rows for both worktrees and branches
        // All items need skeleton rendering since computed data (timestamp, ahead/behind, etc.)
        // hasn't been loaded yet. Using format_list_item_line would show default values like "55y".
        let skeletons: Vec<String> = all_items
            .iter()
            .map(|item| layout.format_skeleton_row(item))
            .collect();

        let initial_footer = format!("{INFO_SYMBOL} {dim}{footer_base} (loading...){dim:#}");

        let mut table = ProgressiveTable::new(
            layout.format_header_line(),
            skeletons,
            initial_footer,
            max_width,
        );
        table.render_skeleton()?;
        worktrunk::shell_exec::trace_instant("Skeleton rendered");
        Some(table)
    } else {
        None
    };

    // Early exit for benchmarking skeleton render time
    if std::env::var("WORKTRUNK_SKELETON_ONLY").is_ok() {
        return Ok(None);
    }

    // === Post-skeleton computations (deferred to minimize time-to-skeleton) ===
    worktrunk::shell_exec::trace_instant("Post-skeleton started");

    // Compute previous_branch and update is_previous on items
    let previous_branch = repo.get_switch_previous();
    if let Some(prev) = previous_branch.as_deref() {
        for item in &mut all_items {
            if item.branch.as_deref() == Some(prev)
                && let Some(wt_data) = item.worktree_data_mut()
            {
                wt_data.is_previous = true;
            }
        }
    }

    // Effective target for integration checks: upstream if ahead of local, else local.
    // This handles the case where a branch was merged remotely but user hasn't pulled yet.
    // Deferred until after skeleton to avoid blocking initial render.
    // Uses cached integration_target() which returns None if default_branch unknown.
    let integration_target = repo.integration_target();

    // Batch-fetch ahead/behind counts to identify branches that are far behind.
    // This allows skipping expensive merge-base operations for diverged branches, dramatically
    // improving performance on repos with many stale branches (e.g., wt select).
    //
    // Uses `git for-each-ref --format='%(ahead-behind:...)'` (git 2.36+) which gets all
    // counts in a single command. On older git versions, returns empty and all tasks run.
    // Skip if default_branch is unknown.
    if skip_expensive_for_stale && let Some(ref db) = default_branch {
        // Branches more than 50 commits behind skip expensive merge-base operations.
        // 50 is low enough to catch truly stale branches while keeping info for
        // recently-diverged ones. The "behind" count is the primary expense driver -
        // git must traverse all those commits to find the merge-base.
        let threshold: usize = std::env::var("WORKTRUNK_TEST_SKIP_EXPENSIVE_THRESHOLD")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(50);
        // batch_ahead_behind populates the Repository cache with all counts
        let ahead_behind = repo.batch_ahead_behind(db);
        // Filter to stale branches (behind > threshold). The set indicates which
        // branches should skip expensive tasks; counts come from the cache.
        options.stale_branches = ahead_behind
            .into_iter()
            .filter_map(|(branch, (_, behind))| (behind > threshold).then_some(branch))
            .collect();
    }

    // Note: URL template expansion is deferred to task spawning (in collect_worktree_progressive
    // and collect_branch_progressive). This parallelizes the work and minimizes time-to-skeleton.

    // Pre-start fsmonitor daemons on macOS to avoid auto-start races.
    //
    // Git's builtin fsmonitor on macOS has race conditions under parallel load that can
    // cause git commands to hang. When multiple git status commands try to auto-start
    // the daemon simultaneously, they can wedge. Pre-starting the daemons before parallel
    // operations avoids this race.
    //
    // See: https://gitlab.com/gitlab-org/git/-/merge_requests/148 (scalar's workaround)
    // See: https://github.com/jj-vcs/jj/issues/6440 (jj hit same issue)
    #[cfg(target_os = "macos")]
    {
        if repo.is_builtin_fsmonitor_enabled() {
            // Parallelize daemon starts - explicit `start` is safe to call concurrently
            // (each returns quickly if daemon already running for that worktree).
            sorted_worktrees.par_iter().for_each(|wt| {
                if !wt.is_prunable() {
                    repo.start_fsmonitor_daemon_at(&wt.path);
                }
            });
        }
    }

    // Cache last rendered (unclamped) message per row to avoid redundant updates.
    let mut last_rendered_lines: Vec<String> = vec![String::new(); all_items.len()];

    // Create channel for task results
    let (tx, rx) = chan::unbounded::<Result<TaskResult, TaskError>>();

    // Collect errors for display after rendering
    let mut errors: Vec<TaskError> = Vec::new();

    // Collect all work items upfront, then execute in a single Rayon pool.
    // This avoids nested parallelism (Rayon par_iter → thread::scope per worktree)
    // which could create 100+ threads. Instead, we have one pool with the configured
    // thread count (default 2x CPU cores unless overridden by RAYON_NUM_THREADS).
    let sorted_worktrees_clone = sorted_worktrees.clone();
    let tx_worker = tx.clone();
    let expected_results_clone = expected_results.clone();

    // Clone repo for the worker thread (shares cache via Arc)
    let repo_clone = repo.clone();

    // Prepare branch data if needed (before moving into closure)
    let branch_data: Vec<(usize, String, String)> = if show_branches || show_remotes {
        let mut all_branches = Vec::new();
        if show_branches {
            all_branches.extend(
                branches_without_worktrees
                    .iter()
                    .enumerate()
                    .map(|(idx, (name, sha))| (branch_start_idx + idx, name.clone(), sha.clone())),
            );
        }
        if show_remotes {
            all_branches.extend(
                remote_branches
                    .iter()
                    .enumerate()
                    .map(|(idx, (name, sha))| (remote_start_idx + idx, name.clone(), sha.clone())),
            );
        }
        all_branches
    } else {
        Vec::new()
    };

    worktrunk::shell_exec::trace_instant("Spawning worker thread");
    std::thread::spawn(move || {
        use super::collect_progressive_impl::{work_items_for_branch, work_items_for_worktree};

        // Phase 1: Generate all work items (sequential, fast)
        // Work items are collected upfront so we can process them all in a single par_iter.
        let mut all_work_items = Vec::new();

        // Worktree work items
        for (idx, wt) in sorted_worktrees_clone.iter().enumerate() {
            all_work_items.extend(work_items_for_worktree(
                &repo_clone,
                wt,
                idx,
                &options,
                &expected_results_clone,
                &tx_worker,
            ));
        }

        // Branch work items (local + remote)
        for (item_idx, branch_name, commit_sha) in &branch_data {
            all_work_items.extend(work_items_for_branch(
                &repo_clone,
                branch_name,
                commit_sha,
                *item_idx,
                &options,
                &expected_results_clone,
            ));
        }

        // Sort work items: network tasks last to avoid blocking local operations
        all_work_items.sort_by_key(|item| item.kind.is_network());

        // Phase 2: Execute all work items in parallel
        worktrunk::shell_exec::trace_instant("Parallel execution started");
        all_work_items.into_par_iter().for_each(|item| {
            worktrunk::shell_exec::set_command_timeout(command_timeout);
            let result = item.execute();
            let _ = tx_worker.send(result);
        });
    });

    // Drop the original sender so drain_results knows when all spawned threads are done
    drop(tx);

    // Track completed results for footer progress
    let mut completed_results = 0;
    let mut progress_overflow = false;
    let mut first_result_traced = false;

    // Drain task results with conditional progressive rendering
    let drain_outcome = drain_results(
        rx,
        &mut all_items,
        &mut errors,
        &expected_results,
        |item_idx, item, ctx| {
            // Trace first result arrival
            if !first_result_traced {
                first_result_traced = true;
                worktrunk::shell_exec::trace_instant("First result received");
            }

            // Compute/recompute status symbols as data arrives (both modes).
            // This is idempotent and updates status as new data (like upstream) arrives.
            if let Some(ref target) = integration_target {
                ctx.apply_to(item, target.as_str());
            }

            // Progressive mode only: update UI
            if let Some(ref mut table) = progressive_table {
                use anstyle::Style;
                let dim = Style::new().dimmed();

                completed_results += 1;
                let total_results = expected_results.count();

                // Catch counting bugs: completed should never exceed expected
                debug_assert!(
                    completed_results <= total_results,
                    "completed ({completed_results}) > expected ({total_results}): \
                     task result sent without registering expectation"
                );
                if completed_results > total_results {
                    progress_overflow = true;
                }

                // Update footer progress
                let footer_msg = format!(
                    "{INFO_SYMBOL} {dim}{footer_base} ({completed_results}/{total_results} loaded){dim:#}"
                );
                table.update_footer(footer_msg);

                // Re-render the row with caching (now includes status if computed)
                let rendered = layout.format_list_item_line(item, previous_branch.as_deref());

                // Compare using full line so changes beyond the clamp (e.g., CI) still refresh.
                if rendered != last_rendered_lines[item_idx] {
                    last_rendered_lines[item_idx] = rendered.clone();
                    table.update_row(item_idx, rendered);
                }

                // Flush updates to terminal
                if let Err(e) = table.flush() {
                    log::debug!("Progressive table flush failed: {}", e);
                }
            }
        },
    );
    worktrunk::shell_exec::trace_instant("All results drained");

    // Handle timeout if it occurred
    if let DrainOutcome::TimedOut {
        received_count,
        items_with_missing,
    } = drain_outcome
    {
        // Build diagnostic message showing what's MISSING (more useful for debugging)
        let mut diag = format!("wt list timed out after 30s ({received_count} results received)");

        if !items_with_missing.is_empty() {
            diag.push_str("\nMissing results:");
            let missing_lines: Vec<String> = items_with_missing
                .iter()
                .map(|result| {
                    let missing_names: Vec<&str> =
                        result.missing_kinds.iter().map(|k| k.into()).collect();
                    cformat!("<bold>{}</>: {}", result.name, missing_names.join(", "))
                })
                .collect();
            diag.push_str(&format!(
                "\n{}",
                format_with_gutter(&missing_lines.join("\n"), None)
            ));
        }

        diag.push_str(
            "\n\nThis likely indicates a git command hung. Run with -v for details, -vv to create a diagnostic file.",
        );

        crate::output::print(warning_message(&diag))?;

        // Show issue reporting hint (free function - doesn't collect diagnostic data)
        crate::output::print(worktrunk::styling::hint_message(
            crate::diagnostic::issue_hint(),
        ))?;
    }

    // Compute status symbols for prunable worktrees (skipped during task spawning).
    // They didn't receive any task results, so status_symbols is still None.
    for item in &mut all_items {
        if item.status_symbols.is_none()
            && let Some(data) = item.worktree_data()
            && data.is_prunable()
        {
            // Use default context - no tasks ran, so no conflict/status info
            let ctx = StatusContext::default();
            if let Some(ref target) = integration_target {
                ctx.apply_to(item, target.as_str());
            }
        }
    }

    // Finalize progressive table or render buffered output
    if let Some(mut table) = progressive_table {
        // Build final summary string
        let final_msg = super::format_summary_message(
            &all_items,
            show_branches || show_remotes,
            layout.hidden_column_count,
        );

        if table.is_tty() {
            // Interactive: do final render pass and update footer to summary
            for (item_idx, item) in all_items.iter().enumerate() {
                let rendered = layout.format_list_item_line(item, previous_branch.as_deref());
                table.update_row(item_idx, rendered);
            }
            table.finalize(final_msg)?;
        } else {
            // Non-TTY: output to stdout (same as buffered mode)
            // Progressive skeleton was suppressed; now output the final table
            crate::output::stdout(layout.format_header_line())?;
            for item in &all_items {
                crate::output::stdout(
                    layout.format_list_item_line(item, previous_branch.as_deref()),
                )?;
            }
            crate::output::stdout("")?;
            crate::output::stdout(final_msg)?;
        }
    } else if render_table {
        // Buffered mode: render final table
        let final_msg = super::format_summary_message(
            &all_items,
            show_branches || show_remotes,
            layout.hidden_column_count,
        );

        crate::output::stdout(layout.format_header_line())?;
        for item in &all_items {
            crate::output::stdout(layout.format_list_item_line(item, previous_branch.as_deref()))?;
        }
        crate::output::stdout("")?;
        crate::output::stdout(final_msg)?;
    }

    // Status symbols are now computed during data collection (both modes), no fallback needed

    // Display collection errors/warnings (after table rendering)
    if !errors.is_empty() || progress_overflow {
        let mut warning_parts = Vec::new();

        if !errors.is_empty() {
            // Sort for deterministic output (tasks complete in arbitrary order)
            errors.sort_by_key(|e| (e.item_idx, e.kind));
            let error_lines: Vec<String> = errors
                .iter()
                .map(|error| {
                    let name = all_items[error.item_idx].branch_name();
                    let kind_str: &'static str = error.kind.into();
                    // Take first line only - git errors can be multi-line with usage hints
                    let msg = error.message.lines().next().unwrap_or(&error.message);
                    cformat!("<bold>{}</>: {} ({})", name, kind_str, msg)
                })
                .collect();
            warning_parts.push(format!(
                "Some git operations failed:\n{}",
                format_with_gutter(&error_lines.join("\n"), None)
            ));
        }

        if progress_overflow {
            // Defensive: should never trigger now that immediate URL sends register expectations,
            // but kept to detect future counting bugs
            warning_parts.push("Progress counter overflow (completed > expected)".to_string());
        }

        let warning = warning_parts.join("\n");
        crate::output::print(warning_message(&warning))?;

        // Show issue reporting hint (free function - doesn't collect diagnostic data)
        crate::output::print(worktrunk::styling::hint_message(
            crate::diagnostic::issue_hint(),
        ))?;
    }

    // Populate display fields for all items (used by JSON output and statusline)
    for item in &mut all_items {
        item.finalize_display();
    }

    // all_items now contains both worktrees and branches (if requested)
    let items = all_items;

    // Table rendering complete (when render_table=true):
    // - Progressive + TTY: rows morphed in place, footer became summary
    // - Progressive + Non-TTY: rendered final table (no intermediate output)
    // - Buffered: rendered final table
    // JSON mode (render_table=false): no rendering, data returned for serialization
    worktrunk::shell_exec::trace_instant("List collect complete");

    Ok(Some(super::model::ListData {
        items,
        main_worktree_path: main_worktree.path.clone(),
    }))
}

/// Sort items by timestamp descending using pre-fetched timestamps.
fn sort_by_timestamp_desc_with_cache<T, F>(
    items: Vec<T>,
    timestamps: &std::collections::HashMap<String, i64>,
    get_sha: F,
) -> Vec<T>
where
    F: Fn(&T) -> &str,
{
    // Embed timestamp in tuple to avoid parallel Vec and index lookups
    let mut with_ts: Vec<_> = items
        .into_iter()
        .map(|item| {
            let ts = *timestamps.get(get_sha(&item)).unwrap_or(&0);
            (item, ts)
        })
        .collect();
    with_ts.sort_by_key(|(_, ts)| std::cmp::Reverse(*ts));
    with_ts.into_iter().map(|(item, _)| item).collect()
}

/// Sort worktrees: current first, main second, then by timestamp descending.
/// Uses pre-fetched timestamps for efficiency.
fn sort_worktrees_with_cache(
    worktrees: Vec<WorktreeInfo>,
    main_worktree: &WorktreeInfo,
    current_path: Option<&std::path::PathBuf>,
    timestamps: &std::collections::HashMap<String, i64>,
) -> Vec<WorktreeInfo> {
    // Embed timestamp and priority in tuple to avoid parallel Vec and index lookups
    let mut with_sort_key: Vec<_> = worktrees
        .into_iter()
        .map(|wt| {
            let priority = if current_path.is_some_and(|cp| &wt.path == cp) {
                0 // Current first
            } else if wt.path == main_worktree.path {
                1 // Main second
            } else {
                2 // Rest by timestamp
            };
            let ts = *timestamps.get(&wt.head).unwrap_or(&0);
            (wt, priority, ts)
        })
        .collect();

    with_sort_key.sort_by_key(|(_, priority, ts)| (*priority, std::cmp::Reverse(*ts)));
    with_sort_key.into_iter().map(|(wt, _, _)| wt).collect()
}

// ============================================================================
// Public API for single-worktree collection (used by statusline)
// ============================================================================

pub use super::collect_progressive_impl::CollectOptions;

/// Build a ListItem for a single worktree with identity fields only.
///
/// Computed fields (counts, diffs, CI) are left as None. Use `populate_item()`
/// to fill them in.
pub fn build_worktree_item(
    wt: &WorktreeInfo,
    is_main: bool,
    is_current: bool,
    is_previous: bool,
) -> ListItem {
    ListItem {
        head: wt.head.clone(),
        branch: wt.branch.clone(),
        commit: None,
        counts: None,
        branch_diff: None,
        committed_trees_match: None,
        has_file_changes: None,
        would_merge_add: None,
        is_ancestor: None,
        upstream: None,
        pr_status: None,
        url: None,
        url_active: None,
        status_symbols: None,
        display: DisplayFields::default(),
        kind: ItemKind::Worktree(Box::new(WorktreeData::from_worktree(
            wt,
            is_main,
            is_current,
            is_previous,
        ))),
    }
}

/// Populate computed fields for items in parallel (blocking).
///
/// Spawns parallel git operations and collects results. Modifies items in place
/// with: commit details, ahead/behind, diffs, upstream, CI, etc.
///
/// # Parameters
/// - `repo`: Repository handle (cloned into background thread, shares cache via Arc)
///
/// This is the blocking version used by statusline. For progressive rendering
/// with callbacks, see the `collect()` function.
pub fn populate_item(
    repo: &Repository,
    item: &mut ListItem,
    options: CollectOptions,
) -> anyhow::Result<()> {
    use std::sync::Arc;

    // Extract worktree data (skip if not a worktree item)
    let Some(data) = item.worktree_data() else {
        return Ok(());
    };

    // Get integration target for status symbol computation (cached in repo)
    // None if default branch cannot be determined - status symbols will be skipped
    let target = repo.integration_target();

    // Create channel for task results
    let (tx, rx) = chan::unbounded::<Result<TaskResult, TaskError>>();

    // Track expected results (populated at spawn time)
    let expected_results = Arc::new(ExpectedResults::default());

    // Collect errors (logged silently for statusline)
    let mut errors: Vec<TaskError> = Vec::new();

    // Extract data for background thread (can't send borrows across threads)
    let wt = WorktreeInfo {
        path: data.path.clone(),
        head: item.head.clone(),
        branch: item.branch.clone(),
        bare: false,
        detached: false,
        locked: None,
        prunable: None,
    };
    let repo_clone = repo.clone();
    let expected_results_clone = expected_results.clone();

    // Spawn collection in background thread
    std::thread::spawn(move || {
        use super::collect_progressive_impl::work_items_for_worktree;

        // Generate work items for this single worktree
        let mut work_items = work_items_for_worktree(
            &repo_clone,
            &wt,
            0, // Single item, always index 0
            &options,
            &expected_results_clone,
            &tx,
        );

        // Sort: network tasks last
        work_items.sort_by_key(|item| item.kind.is_network());

        // Execute all tasks in parallel
        work_items.into_par_iter().for_each(|item| {
            let result = item.execute();
            let _ = tx.send(result);
        });
    });

    // Drain task results (blocking until complete)
    let drain_outcome = drain_results(
        rx,
        std::slice::from_mut(item),
        &mut errors,
        &expected_results,
        |_item_idx, item, ctx| {
            if let Some(ref t) = target {
                ctx.apply_to(item, t);
            }
        },
    );

    // Handle timeout (silent for statusline - just log it)
    if let DrainOutcome::TimedOut { received_count, .. } = drain_outcome {
        log::warn!("populate_item timed out after 30s ({received_count} results received)");
    }

    // Log errors silently (statusline shouldn't spam warnings)
    if !errors.is_empty() {
        log::warn!("populate_item had {} task errors", errors.len());
        for error in &errors {
            let kind_str: &'static str = error.kind.into();
            log::debug!(
                "  - item {}: {} ({})",
                error.item_idx,
                kind_str,
                error.message
            );
        }
    }

    // Populate display fields (including status_line for statusline command)
    item.finalize_display();

    Ok(())
}
