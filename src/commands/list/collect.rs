//! Worktree data collection with parallelized git operations.
//!
//! This module provides an efficient approach to collecting worktree data:
//! - Parallel collection across worktrees (using Rayon)
//! - Parallel operations within each worktree (using scoped threads)
//! - Progressive updates via channels (update UI as each worktree completes)
//!
//! ## Unified Collection Architecture
//!
//! Progressive and buffered modes use the same collection and rendering code.
//! The only difference is whether intermediate updates are shown during collection:
//! - Progressive: shows progress bars with updates, then finalizes in place (TTY) or redraws (non-TTY)
//! - Buffered: collects silently, then renders the final table
//!
//! Both modes render the final table in `collect()`, ensuring a single canonical rendering path.
//!
//! **Parallelism at two levels**:
//! - Across worktrees: Multiple worktrees collected concurrently via Rayon
//! - Within worktrees: Git operations (ahead/behind, diffs, CI) run concurrently via scoped threads
//!
//! This ensures fast operations don't wait for slow ones (e.g., CI doesn't block ahead/behind counts)
use crossbeam_channel as chan;
use dunce::canonicalize;
use rayon::prelude::*;
use worktrunk::git::{LineDiff, Repository, Worktree};
use worktrunk::styling::{INFO_EMOJI, warning_message};

use crate::commands::is_worktree_at_expected_path;

use super::ci_status::PrStatus;
use super::model::{
    AheadBehind, BranchDiffTotals, CommitDetails, DisplayFields, GitOperationState, ItemKind,
    ListItem, UpstreamStatus, WorktreeData,
};

use super::model::WorkingTreeStatus;

/// Context for status symbol computation during result processing
struct StatusContext {
    has_merge_tree_conflicts: bool,
    user_marker: Option<String>,
    working_tree_status: Option<WorkingTreeStatus>,
    has_conflicts: bool,
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
    derive(Hash, strum::IntoStaticStr),
    strum(serialize_all = "kebab-case")
)]
pub(super) enum TaskResult {
    /// Commit timestamp and message
    CommitDetails {
        item_idx: usize,
        commit: CommitDetails,
    },
    /// Ahead/behind counts vs main
    AheadBehind {
        item_idx: usize,
        counts: AheadBehind,
    },
    /// Whether HEAD's tree SHA matches main's tree SHA (committed content identical)
    CommittedTreesMatch {
        item_idx: usize,
        committed_trees_match: bool,
    },
    /// Whether branch has file changes beyond the merge-base (three-dot diff)
    HasFileChanges {
        item_idx: usize,
        has_file_changes: bool,
    },
    /// Whether merging branch into main would add changes (merge simulation)
    WouldMergeAdd {
        item_idx: usize,
        would_merge_add: bool,
    },
    /// Whether branch HEAD is ancestor of main (same commit or already merged)
    IsAncestor { item_idx: usize, is_ancestor: bool },
    /// Line diff vs main branch
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
    /// Potential merge conflicts with main (merge-tree simulation)
    MergeTreeConflicts {
        item_idx: usize,
        has_merge_tree_conflicts: bool,
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
            | TaskResult::GitOperation { item_idx, .. }
            | TaskResult::UserMarker { item_idx, .. }
            | TaskResult::Upstream { item_idx, .. }
            | TaskResult::CiStatus { item_idx, .. } => *item_idx,
        }
    }
}

/// Detect if a worktree is in the middle of a git operation (rebase/merge).
pub(super) fn detect_git_operation(repo: &Repository) -> GitOperationState {
    if repo.is_rebasing().unwrap_or(false) {
        GitOperationState::Rebase
    } else if repo.is_merging().unwrap_or(false) {
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
    inner: std::sync::Mutex<std::collections::HashMap<usize, Vec<TaskKind>>>,
}

impl ExpectedResults {
    /// Record that we expect a result of the given kind for the given item.
    /// Called internally by `TaskSpawner::spawn()`.
    pub fn expect(&self, item_idx: usize, kind: TaskKind) {
        self.inner
            .lock()
            .unwrap()
            .entry(item_idx)
            .or_default()
            .push(kind);
    }

    /// Total number of expected results (for progress display).
    pub fn count(&self) -> usize {
        self.inner.lock().unwrap().values().map(|v| v.len()).sum()
    }

    /// Expected results for a specific item.
    fn results_for(&self, item_idx: usize) -> Vec<TaskKind> {
        self.inner
            .lock()
            .unwrap()
            .get(&item_idx)
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
            items[idx].branch_diff = Some(BranchDiffTotals::default());
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
            // Some(None) means "loaded but no CI"
            items[idx].pr_status = Some(None);
        }
    }
}

/// Drain task results from the channel and apply them to items.
///
/// This is the shared logic between progressive and buffered collection modes.
/// The `on_result` callback is called after each result is processed with the
/// item index and a reference to the updated item, allowing progressive mode
/// to update progress bars while buffered mode does nothing.
///
/// Uses a 30-second deadline to prevent infinite hangs if git commands stall.
/// When timeout occurs, returns `DrainOutcome::TimedOut` with diagnostic info.
///
/// Errors are collected in the `errors` vec for display after rendering.
/// Default values are applied for failed tasks so the UI can still render.
///
/// Callers decide how to handle timeout:
/// - `collect()`: Shows user-facing diagnostic (interactive command)
/// - `populate_items()`: Logs silently (used by statusline)
fn drain_results(
    rx: chan::Receiver<Result<TaskResult, TaskError>>,
    items: &mut [ListItem],
    errors: &mut Vec<TaskError>,
    expected_results: &ExpectedResults,
    mut on_result: impl FnMut(usize, &mut ListItem, &StatusContext),
) -> DrainOutcome {
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    // Deadline for the entire drain operation (30 seconds should be more than enough)
    let deadline = Instant::now() + Duration::from_secs(30);

    // Track which result kinds we've received per item (for timeout diagnostics)
    let mut received_by_item: HashMap<usize, Vec<TaskKind>> = HashMap::new();

    // Temporary storage for data needed by status_symbols computation
    let mut status_contexts: Vec<StatusContext> = (0..items.len())
        .map(|_| StatusContext {
            has_merge_tree_conflicts: false,
            user_marker: None,
            working_tree_status: None,
            has_conflicts: false,
        })
        .collect();

    // Process task results as they arrive (with deadline)
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            // Deadline exceeded - build diagnostic info showing MISSING results
            let received_count: usize = received_by_item.values().map(|v| v.len()).sum();

            // Find items with missing results by comparing received vs expected
            let mut items_with_missing: Vec<MissingResult> = Vec::new();

            for (item_idx, item) in items.iter().enumerate() {
                // Get expected results for this item (populated at spawn time)
                let expected = expected_results.results_for(item_idx);

                // Get received results for this item (empty vec if none received)
                let received = received_by_item
                    .get(&item_idx)
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);

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
        received_by_item.entry(item_idx).or_default().push(kind);

        // Handle error case: apply defaults and collect error
        if let Err(error) = outcome {
            apply_default(items, &mut status_contexts, &error);
            errors.push(error);
            on_result(item_idx, &mut items[item_idx], &status_contexts[item_idx]);
            continue;
        }

        // Handle success case
        let result = outcome.unwrap();
        match result {
            TaskResult::CommitDetails { item_idx, commit } => {
                items[item_idx].commit = Some(commit);
            }
            TaskResult::AheadBehind { item_idx, counts } => {
                items[item_idx].counts = Some(counts);
            }
            TaskResult::CommittedTreesMatch {
                item_idx,
                committed_trees_match,
            } => {
                items[item_idx].committed_trees_match = Some(committed_trees_match);
            }
            TaskResult::HasFileChanges {
                item_idx,
                has_file_changes,
            } => {
                items[item_idx].has_file_changes = Some(has_file_changes);
            }
            TaskResult::WouldMergeAdd {
                item_idx,
                would_merge_add,
            } => {
                items[item_idx].would_merge_add = Some(would_merge_add);
            }
            TaskResult::IsAncestor {
                item_idx,
                is_ancestor,
            } => {
                items[item_idx].is_ancestor = Some(is_ancestor);
            }
            TaskResult::BranchDiff {
                item_idx,
                branch_diff,
            } => {
                items[item_idx].branch_diff = Some(branch_diff);
            }
            TaskResult::WorkingTreeDiff {
                item_idx,
                working_tree_diff,
                working_tree_diff_with_main,
                working_tree_status,
                has_conflicts,
            } => {
                if let ItemKind::Worktree(data) = &mut items[item_idx].kind {
                    data.working_tree_diff = Some(working_tree_diff);
                    data.working_tree_diff_with_main = Some(working_tree_diff_with_main);
                } else {
                    debug_assert!(false, "WorkingTreeDiff result for non-worktree item");
                }
                // Store for status_symbols computation
                status_contexts[item_idx].working_tree_status = Some(working_tree_status);
                status_contexts[item_idx].has_conflicts = has_conflicts;
            }
            TaskResult::MergeTreeConflicts {
                item_idx,
                has_merge_tree_conflicts,
            } => {
                // Store for status_symbols computation
                status_contexts[item_idx].has_merge_tree_conflicts = has_merge_tree_conflicts;
            }
            TaskResult::GitOperation {
                item_idx,
                git_operation,
            } => {
                if let ItemKind::Worktree(data) = &mut items[item_idx].kind {
                    data.git_operation = git_operation;
                } else {
                    debug_assert!(false, "GitOperation result for non-worktree item");
                }
            }
            TaskResult::UserMarker {
                item_idx,
                user_marker,
            } => {
                // Store for status_symbols computation
                status_contexts[item_idx].user_marker = user_marker;
            }
            TaskResult::Upstream { item_idx, upstream } => {
                items[item_idx].upstream = Some(upstream);
            }
            TaskResult::CiStatus {
                item_idx,
                pr_status,
            } => {
                // Wrap in Some() to indicate "loaded" (Some(None) = no CI, Some(Some(status)) = has CI)
                items[item_idx].pr_status = Some(pr_status);
            }
        }

        // Invoke callback (progressive mode re-renders rows, buffered mode does nothing)
        on_result(item_idx, &mut items[item_idx], &status_contexts[item_idx]);
    }

    DrainOutcome::Complete
}

/// Get branches that don't have worktrees.
///
/// Returns (branch_name, commit_sha) pairs for all branches without associated worktrees.
fn get_branches_without_worktrees(
    repo: &Repository,
    worktrees: &[Worktree],
) -> anyhow::Result<Vec<(String, String)>> {
    // Get all local branches
    let all_branches = repo.list_local_branches()?;

    // Build a set of branch names that have worktrees
    let worktree_branches: std::collections::HashSet<String> = worktrees
        .iter()
        .filter_map(|wt| wt.branch.clone())
        .collect();

    // Filter to branches without worktrees
    let branches_without_worktrees: Vec<_> = all_branches
        .into_iter()
        .filter(|(branch_name, _)| !worktree_branches.contains(branch_name))
        .collect();

    Ok(branches_without_worktrees)
}

/// Get remote branches from all remotes that don't have local worktrees.
///
/// Returns (branch_name, commit_sha) pairs for remote branches.
/// Filters out branches that already have worktrees (whether the worktree is on the
/// local tracking branch or not).
fn get_remote_branches(
    repo: &Repository,
    worktrees: &[Worktree],
) -> anyhow::Result<Vec<(String, String)>> {
    // Get all remote branches from all remotes
    let all_remote_branches = repo.list_remote_branches()?;

    // Build a set of branch names that have worktrees
    let worktree_branches: std::collections::HashSet<String> = worktrees
        .iter()
        .filter_map(|wt| wt.branch.clone())
        .collect();

    // Filter to remote branches whose local equivalent doesn't have a worktree
    let remote_branches: Vec<_> = all_remote_branches
        .into_iter()
        .filter(|(remote_branch_name, _)| {
            // First '/' separates remote from branch: "origin/feature/foo" → "feature/foo"
            if let Some((_, local_name)) = remote_branch_name.split_once('/') {
                // Include remote branch if local branch doesn't have a worktree
                !worktree_branches.contains(local_name)
            } else {
                // Skip branches without a remote prefix
                false
            }
        })
        .collect();

    Ok(remote_branches)
}

/// Collect worktree data with optional progressive rendering.
///
/// When `show_progress` is true, renders a skeleton immediately and updates as data arrives.
/// When false, behavior depends on `render_table`:
/// - If `render_table` is true: renders final table (buffered mode)
/// - If `render_table` is false: returns data without rendering (JSON mode)
#[allow(clippy::too_many_arguments)]
pub fn collect(
    repo: &Repository,
    show_branches: bool,
    show_remotes: bool,
    skip_tasks: &std::collections::HashSet<TaskKind>,
    show_progress: bool,
    render_table: bool,
    config: &worktrunk::config::WorktrunkConfig,
) -> anyhow::Result<Option<super::model::ListData>> {
    use super::progressive_table::ProgressiveTable;

    let worktrees = repo.list_worktrees()?;
    if worktrees.worktrees.is_empty() {
        return Ok(None);
    }

    let default_branch = repo.default_branch()?;
    // Effective target for integration checks: upstream if ahead of local, else local.
    // This handles the case where a branch was merged remotely but user hasn't pulled yet.
    let integration_target = repo.effective_integration_target(&default_branch);
    // Main worktree is the worktree on the default branch (if exists), else first worktree
    let main_worktree = worktrees
        .worktrees
        .iter()
        .find(|wt| wt.branch.as_deref() == Some(default_branch.as_str()))
        .cloned()
        .unwrap_or_else(|| worktrees.main().clone());
    let current_worktree_path = repo.worktree_root().ok();
    let previous_branch = repo.get_switch_previous();

    // Sort worktrees: current first, main second, then by timestamp descending
    let sorted_worktrees = sort_worktrees(
        worktrees.worktrees.clone(),
        &main_worktree,
        current_worktree_path.as_ref(),
    );

    // Get branches early for layout calculation and skeleton creation (when --branches is used)
    // Sort by timestamp (most recent first)
    let branches_without_worktrees = if show_branches {
        let branches = get_branches_without_worktrees(repo, &worktrees.worktrees)?;
        sort_by_timestamp_desc(branches, |(_, sha)| repo.commit_timestamp(sha).unwrap_or(0))
    } else {
        Vec::new()
    };

    // Get remote branches (when --remotes is used)
    // Sort by timestamp (most recent first)
    let remote_branches = if show_remotes {
        let branches = get_remote_branches(repo, &worktrees.worktrees)?;
        sort_by_timestamp_desc(branches, |(_, sha)| repo.commit_timestamp(sha).unwrap_or(0))
    } else {
        Vec::new()
    };

    // Pre-canonicalize main_worktree.path for is_main comparison
    // (paths from git worktree list may differ based on symlinks or working directory)
    let main_worktree_canonical = canonicalize(&main_worktree.path).ok();

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
            let is_previous = previous_branch
                .as_deref()
                .is_some_and(|prev| wt.branch.as_deref() == Some(prev));

            // Check if worktree is at its expected path based on config template
            let path_mismatch = !is_worktree_at_expected_path(wt, repo, config);

            let mut worktree_data =
                WorktreeData::from_worktree(wt, is_main, is_current, is_previous);
            worktree_data.path_mismatch = path_mismatch;

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
                status_symbols: None,
                display: DisplayFields::default(),
                kind: ItemKind::Worktree(Box::new(worktree_data)),
            }
        })
        .collect();

    // Initialize branch items (local and remote)
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

    // Calculate layout from items (worktrees, local branches, and remote branches)
    let layout = super::layout::calculate_layout_from_basics(&all_items, skip_tasks);

    // Single-line invariant: use safe width to prevent line wrapping
    let max_width = super::layout::get_safe_list_width();

    // Create collection options from skip set
    let options = super::collect_progressive_impl::CollectOptions {
        skip_tasks: skip_tasks.clone(), // Clone for thread spawns
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

        let initial_footer = format!("{INFO_EMOJI} {dim}{footer_base} (loading...){dim:#}");

        let table = ProgressiveTable::new(
            layout.format_header_line(),
            skeletons,
            initial_footer,
            max_width,
        );
        table.render_initial()?;
        Some(table)
    } else {
        None
    };

    // Early exit for benchmarking skeleton render time
    if std::env::var("WT_SKELETON_ONLY").is_ok() {
        return Ok(None);
    }

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
        let main_repo = Repository::at(&main_worktree.path);
        if main_repo.is_builtin_fsmonitor_enabled() {
            for wt in &sorted_worktrees {
                Repository::at(&wt.path).start_fsmonitor_daemon();
            }
        }
    }

    // Cache last rendered (unclamped) message per row to avoid redundant updates.
    let mut last_rendered_lines: Vec<String> = vec![String::new(); all_items.len()];

    // Create channel for task results
    let (tx, rx) = chan::unbounded::<Result<TaskResult, TaskError>>();

    // Collect errors for display after rendering
    let mut errors: Vec<TaskError> = Vec::new();

    // Spawn worktree collection in background thread
    let sorted_worktrees_clone = sorted_worktrees.clone();
    let tx_worktrees = tx.clone();
    let default_branch_clone = default_branch.clone();
    let target_clone = integration_target.clone();
    let expected_results_wt = expected_results.clone();
    let options_wt = options.clone();
    std::thread::spawn(move || {
        sorted_worktrees_clone
            .par_iter()
            .enumerate()
            .for_each(|(idx, wt)| {
                // Pass default_branch (local default) for stable informational stats,
                // and target (effective target) for integration checks.
                super::collect_progressive_impl::collect_worktree_progressive(
                    wt,
                    idx,
                    &default_branch_clone,
                    &target_clone,
                    &options_wt,
                    tx_worktrees.clone(),
                    &expected_results_wt,
                );
            });
    });

    // Spawn branch collection in background thread (local + remote)
    if show_branches || show_remotes {
        // Combine local and remote branches with their item indices
        let mut all_branches: Vec<(usize, String, String)> = Vec::new();
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

        let main_path = main_worktree.path.clone();
        let tx_branches = tx.clone();
        let default_branch_clone = default_branch.clone();
        let target_clone = integration_target.clone();
        let expected_results_br = expected_results.clone();
        let options_br = options.clone();
        std::thread::spawn(move || {
            all_branches
                .par_iter()
                .for_each(|(item_idx, branch_name, commit_sha)| {
                    super::collect_progressive_impl::collect_branch_progressive(
                        branch_name,
                        commit_sha,
                        &main_path,
                        *item_idx,
                        &default_branch_clone,
                        &target_clone,
                        &options_br,
                        tx_branches.clone(),
                        &expected_results_br,
                    );
                });
        });
    }

    // Drop the original sender so drain_results knows when all spawned threads are done
    drop(tx);

    // Track completed results for footer progress
    let mut completed_results = 0;

    // Drain task results with conditional progressive rendering
    let drain_outcome = drain_results(
        rx,
        &mut all_items,
        &mut errors,
        &expected_results,
        |item_idx, item, ctx| {
            // Compute/recompute status symbols as data arrives (both modes)
            // This is idempotent and updates status as new data (like upstream) arrives
            // Main worktree case is handled inside check_integration_state()
            item.compute_status_symbols(
                Some(integration_target.as_str()),
                ctx.has_merge_tree_conflicts,
                ctx.user_marker.clone(),
                ctx.working_tree_status,
                ctx.has_conflicts,
            );

            // Progressive mode only: update UI
            if let Some(ref mut table) = progressive_table {
                use anstyle::Style;
                let dim = Style::new().dimmed();

                completed_results += 1;
                let total_results = expected_results.count();

                // Update footer progress
                let footer_msg = format!(
                    "{INFO_EMOJI} {dim}{footer_base} ({completed_results}/{total_results} loaded){dim:#}"
                );
                if let Err(e) = table.update_footer(footer_msg) {
                    log::debug!("Progressive footer update failed: {}", e);
                }

                // Re-render the row with caching (now includes status if computed)
                let rendered = layout.format_list_item_line(item, previous_branch.as_deref());

                // Compare using full line so changes beyond the clamp (e.g., CI) still refresh.
                if rendered != last_rendered_lines[item_idx] {
                    last_rendered_lines[item_idx] = rendered.clone();
                    if let Err(e) = table.update_row(item_idx, rendered) {
                        log::debug!("Progressive row update failed: {}", e);
                    }
                }
            }
        },
    );

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
            for result in &items_with_missing {
                let missing_names: Vec<&str> =
                    result.missing_kinds.iter().map(|k| k.into()).collect();
                diag.push_str(&format!(
                    "\n  - {}: {}",
                    result.name,
                    missing_names.join(", ")
                ));
            }
        }

        diag.push_str(
            "\n\nThis likely indicates a git command hung. Run with RUST_LOG=debug for details.",
        );

        crate::output::print(warning_message(diag))?;
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
                if let Err(e) = table.update_row(item_idx, rendered) {
                    log::debug!("Final row update failed: {}", e);
                }
            }
            table.finalize_tty(final_msg)?;
        } else {
            // Non-TTY: print final static table
            let mut final_lines = Vec::new();
            final_lines.push(layout.format_header_line());
            for item in &all_items {
                final_lines.push(layout.format_list_item_line(item, previous_branch.as_deref()));
            }
            final_lines.push(String::new()); // Spacer
            final_lines.push(final_msg);
            table.finalize_non_tty(final_lines)?;
        }
    } else if render_table {
        // Buffered mode: render final table
        let final_msg = super::format_summary_message(
            &all_items,
            show_branches || show_remotes,
            layout.hidden_column_count,
        );

        crate::output::table(layout.format_header_line())?;
        for item in &all_items {
            crate::output::table(layout.format_list_item_line(item, previous_branch.as_deref()))?;
        }
        crate::output::table("")?;
        crate::output::table(final_msg)?;
    }

    // Status symbols are now computed during data collection (both modes), no fallback needed

    // Display collection errors as warnings (after table rendering)
    if !errors.is_empty() {
        let mut warning = String::from("Some git operations failed:");
        for error in &errors {
            let name = all_items[error.item_idx].branch_name();
            let kind_str: &'static str = error.kind.into();
            warning.push_str(&format!("\n  - {}: {} ({})", name, kind_str, error.message));
        }
        crate::output::print(warning_message(warning))?;
    }

    // Populate display fields for all items (used by JSON output and statusline)
    for item in &mut all_items {
        item.finalize_display();
    }

    // all_items now contains both worktrees and branches (if requested)
    let items = all_items;

    // Table rendering complete (when render_table=true):
    // - Progressive + TTY: rows morphed in place, footer became summary
    // - Progressive + Non-TTY: cleared progress bars, rendered final table
    // - Buffered: rendered final table (no progress bars)
    // JSON mode (render_table=false): no rendering, data returned for serialization

    Ok(Some(super::model::ListData { items }))
}

/// Sort items by timestamp descending (most recent first).
/// Uses parallel timestamp collection for performance.
fn sort_by_timestamp_desc<T, F>(items: Vec<T>, get_timestamp: F) -> Vec<T>
where
    T: Send + Sync,
    F: Fn(&T) -> i64 + Send + Sync,
{
    let timestamps: Vec<i64> = items.par_iter().map(&get_timestamp).collect();
    let mut indexed: Vec<_> = items.into_iter().enumerate().collect();
    indexed.sort_by_key(|(idx, _)| std::cmp::Reverse(timestamps[*idx]));
    indexed.into_iter().map(|(_, item)| item).collect()
}

/// Sort worktrees: current first, main second, then by timestamp descending.
fn sort_worktrees(
    worktrees: Vec<Worktree>,
    main_worktree: &Worktree,
    current_path: Option<&std::path::PathBuf>,
) -> Vec<Worktree> {
    let timestamps: Vec<i64> = worktrees
        .par_iter()
        .map(|wt| {
            Repository::at(&wt.path)
                .commit_timestamp(&wt.head)
                .unwrap_or(0)
        })
        .collect();

    let mut indexed: Vec<_> = worktrees.into_iter().enumerate().collect();
    indexed.sort_by_key(|(idx, wt)| {
        let priority = if current_path.is_some_and(|cp| &wt.path == cp) {
            0 // Current first
        } else if wt.path == main_worktree.path {
            1 // Main second
        } else {
            2 // Rest by timestamp
        };
        (priority, std::cmp::Reverse(timestamps[*idx]))
    });

    indexed.into_iter().map(|(_, wt)| wt).collect()
}

// ============================================================================
// Public API for single-worktree collection (used by statusline)
// ============================================================================

pub use super::collect_progressive_impl::CollectOptions;

/// Build a ListItem for a single worktree with identity fields only.
///
/// Computed fields (counts, diffs, CI) are left as None. Use `populate_items()`
/// to fill them in.
pub fn build_worktree_item(
    wt: &Worktree,
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
/// - `default_branch`: Local default branch for informational stats (ahead/behind, branch diff)
/// - `target`: Effective target for integration checks (may be upstream if ahead)
///
/// This is the blocking version used by statusline. For progressive rendering
/// with callbacks, see the `collect()` function.
pub fn populate_items(
    items: &mut [ListItem],
    default_branch: &str,
    target: &str,
    options: CollectOptions,
) -> anyhow::Result<()> {
    use std::sync::Arc;

    if items.is_empty() {
        return Ok(());
    }

    // Create channel for task results
    let (tx, rx) = chan::unbounded::<Result<TaskResult, TaskError>>();

    // Track expected results per item (populated at spawn time)
    let expected_results = Arc::new(ExpectedResults::default());

    // Collect errors (logged silently for statusline)
    let mut errors: Vec<TaskError> = Vec::new();

    // Collect worktree info: (index, path, head, branch)
    let worktree_info: Vec<_> = items
        .iter()
        .enumerate()
        .filter_map(|(idx, item)| {
            item.worktree_data().map(|data| {
                (
                    idx,
                    data.path.clone(),
                    item.head.clone(),
                    item.branch.clone(),
                )
            })
        })
        .collect();

    // Spawn collection in background thread
    let default_branch_clone = default_branch.to_string();
    let target_clone = target.to_string();
    let expected_results_clone = expected_results.clone();
    std::thread::spawn(move || {
        for (idx, path, head, branch) in worktree_info {
            // Create a minimal Worktree struct for the collection function
            let wt = Worktree {
                path,
                head,
                branch,
                bare: false,
                detached: false,
                locked: None,
                prunable: None,
            };
            super::collect_progressive_impl::collect_worktree_progressive(
                &wt,
                idx,
                &default_branch_clone,
                &target_clone,
                &options,
                tx.clone(),
                &expected_results_clone,
            );
        }
    });

    // Drain task results (blocking until all complete)
    // TODO: This callback duplicates logic from collect()'s drain_results callback.
    // Consider unifying by making collect() take options to skip progressive rendering.
    let drain_outcome = drain_results(
        rx,
        items,
        &mut errors,
        &expected_results,
        |_item_idx, item, ctx| {
            // Main worktree case is handled inside check_integration_state()
            item.compute_status_symbols(
                Some(target),
                ctx.has_merge_tree_conflicts,
                ctx.user_marker.clone(),
                ctx.working_tree_status,
                ctx.has_conflicts,
            );
        },
    );

    // Handle timeout (silent for statusline - just log it)
    if let DrainOutcome::TimedOut { received_count, .. } = drain_outcome {
        log::warn!("populate_items timed out after 30s ({received_count} results received)");
    }

    // Log errors silently (statusline shouldn't spam warnings)
    if !errors.is_empty() {
        log::warn!("populate_items had {} task errors", errors.len());
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
    for item in items.iter_mut() {
        item.finalize_display();
    }

    Ok(())
}
