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
use rayon::prelude::*;
use worktrunk::git::{LineDiff, Repository, Worktree};
use worktrunk::styling::INFO_EMOJI;

use super::ci_status::PrStatus;
use super::model::{
    AheadBehind, BranchDiffTotals, CommitDetails, DisplayFields, GitOperationState, ItemKind,
    ListItem, UpstreamStatus, WorktreeData,
};

/// Context for status symbol computation during cell updates
struct StatusContext {
    has_merge_tree_conflicts: bool,
    user_status: Option<String>,
    working_tree_symbols: Option<String>,
    has_conflicts: bool,
}

/// Cell update messages sent as each git operation completes.
/// These enable progressive rendering - update UI as data arrives.
#[derive(Debug, Clone, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub(super) enum CellUpdate {
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
    /// Line diff vs main branch
    BranchDiff {
        item_idx: usize,
        branch_diff: BranchDiffTotals,
    },
    /// Working tree diff and symbols (?, !, +, », ✘)
    WorkingTreeDiff {
        item_idx: usize,
        working_tree_diff: LineDiff,
        working_tree_diff_with_main: Option<LineDiff>,
        /// Symbols for uncommitted changes (?, !, +, », ✘)
        working_tree_symbols: String,
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
    UserStatus {
        item_idx: usize,
        user_status: Option<String>,
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

impl CellUpdate {
    /// Get the item index for this update
    fn item_idx(&self) -> usize {
        match self {
            CellUpdate::CommitDetails { item_idx, .. }
            | CellUpdate::AheadBehind { item_idx, .. }
            | CellUpdate::CommittedTreesMatch { item_idx, .. }
            | CellUpdate::BranchDiff { item_idx, .. }
            | CellUpdate::WorkingTreeDiff { item_idx, .. }
            | CellUpdate::MergeTreeConflicts { item_idx, .. }
            | CellUpdate::GitOperation { item_idx, .. }
            | CellUpdate::UserStatus { item_idx, .. }
            | CellUpdate::Upstream { item_idx, .. }
            | CellUpdate::CiStatus { item_idx, .. } => *item_idx,
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

/// Result of draining cell updates - indicates whether all updates were received
/// or if a timeout occurred.
#[derive(Debug)]
enum DrainResult {
    /// All updates received (channel closed normally)
    Complete,
    /// Timeout occurred - contains diagnostic info about what was received
    TimedOut {
        /// Number of cell updates received before timeout
        received_count: usize,
        /// Sample of items that had incomplete data (item_idx, branch_name, received_cells)
        incomplete_items: Vec<(usize, String, Vec<&'static str>)>,
    },
}

/// Drain cell updates from the channel and apply them to items.
///
/// This is the shared logic between progressive and buffered collection modes.
/// The `on_update` callback is called after each update is processed with the
/// item index and a reference to the updated item, allowing progressive mode
/// to update progress bars while buffered mode does nothing.
///
/// Uses a 30-second deadline to prevent infinite hangs if git commands stall.
/// When timeout occurs, returns `DrainResult::TimedOut` with diagnostic info.
///
/// Callers decide how to handle timeout:
/// - `collect()`: Shows user-facing diagnostic (interactive command)
/// - `populate_items()`: Logs silently (used by statusline)
fn drain_cell_updates(
    rx: chan::Receiver<CellUpdate>,
    items: &mut [ListItem],
    mut on_update: impl FnMut(usize, &mut ListItem, &StatusContext),
) -> DrainResult {
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    // Deadline for the entire drain operation (30 seconds should be more than enough)
    let deadline = Instant::now() + Duration::from_secs(30);

    // Track which cell types we've received per item (for timeout diagnostics)
    let mut received_by_item: HashMap<usize, Vec<&'static str>> = HashMap::new();

    // Temporary storage for data needed by status_symbols computation
    let mut status_contexts: Vec<StatusContext> = (0..items.len())
        .map(|_| StatusContext {
            has_merge_tree_conflicts: false,
            user_status: None,
            working_tree_symbols: None,
            has_conflicts: false,
        })
        .collect();

    // Process cell updates as they arrive (with deadline)
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            // Deadline exceeded - build diagnostic info
            let received_count: usize = received_by_item.values().map(|v| v.len()).sum();

            // Find items that likely have incomplete data (received some but not all cells)
            // We show items that received between 1 and 9 cells as "incomplete"
            // (full worktree = 10 cells, full branch = 7 cells)
            let mut incomplete_items: Vec<(usize, String, Vec<&'static str>)> = received_by_item
                .into_iter()
                .filter(|(_, cells)| cells.len() < 7) // Incomplete if fewer than branch minimum
                .take(5)
                .map(|(item_idx, cells)| {
                    let name = if item_idx < items.len() {
                        items[item_idx].branch.clone().unwrap_or_else(|| {
                            items[item_idx].head[..8.min(items[item_idx].head.len())].to_string()
                        })
                    } else {
                        format!("item_{item_idx}")
                    };
                    (item_idx, name, cells)
                })
                .collect();
            incomplete_items.sort_by_key(|(idx, _, _)| *idx);

            return DrainResult::TimedOut {
                received_count,
                incomplete_items,
            };
        }

        let update = match rx.recv_timeout(remaining) {
            Ok(update) => update,
            Err(chan::RecvTimeoutError::Timeout) => continue, // Check deadline in next iteration
            Err(chan::RecvTimeoutError::Disconnected) => break, // All senders dropped - done
        };

        // Track this update for diagnostics (strum::IntoStaticStr provides the conversion)
        let item_idx = update.item_idx();
        let cell_type: &'static str = (&update).into();
        received_by_item
            .entry(item_idx)
            .or_default()
            .push(cell_type);

        match update {
            CellUpdate::CommitDetails { item_idx, commit } => {
                items[item_idx].commit = Some(commit);
            }
            CellUpdate::AheadBehind { item_idx, counts } => {
                items[item_idx].counts = Some(counts);
            }
            CellUpdate::CommittedTreesMatch {
                item_idx,
                committed_trees_match,
            } => {
                items[item_idx].committed_trees_match = Some(committed_trees_match);
            }
            CellUpdate::BranchDiff {
                item_idx,
                branch_diff,
            } => {
                items[item_idx].branch_diff = Some(branch_diff);
            }
            CellUpdate::WorkingTreeDiff {
                item_idx,
                working_tree_diff,
                working_tree_diff_with_main,
                working_tree_symbols,
                has_conflicts,
            } => {
                if let ItemKind::Worktree(data) = &mut items[item_idx].kind {
                    data.working_tree_diff = Some(working_tree_diff);
                    data.working_tree_diff_with_main = Some(working_tree_diff_with_main);
                }
                // Store for status_symbols computation
                status_contexts[item_idx].working_tree_symbols = Some(working_tree_symbols);
                status_contexts[item_idx].has_conflicts = has_conflicts;
            }
            CellUpdate::MergeTreeConflicts {
                item_idx,
                has_merge_tree_conflicts,
            } => {
                // Store for status_symbols computation
                status_contexts[item_idx].has_merge_tree_conflicts = has_merge_tree_conflicts;
            }
            CellUpdate::GitOperation {
                item_idx,
                git_operation,
            } => {
                if let ItemKind::Worktree(data) = &mut items[item_idx].kind {
                    data.git_operation = git_operation;
                }
            }
            CellUpdate::UserStatus {
                item_idx,
                user_status,
            } => {
                // Store for status_symbols computation
                status_contexts[item_idx].user_status = user_status;
            }
            CellUpdate::Upstream { item_idx, upstream } => {
                items[item_idx].upstream = Some(upstream);
            }
            CellUpdate::CiStatus {
                item_idx,
                pr_status,
            } => {
                // Wrap in Some() to indicate "loaded" (Some(None) = no CI, Some(Some(status)) = has CI)
                items[item_idx].pr_status = Some(pr_status);
            }
        }

        // Invoke rendering callback (progressive mode re-renders rows, buffered mode does nothing)
        on_update(item_idx, &mut items[item_idx], &status_contexts[item_idx]);
    }

    DrainResult::Complete
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
    show_full: bool,
    fetch_ci: bool,
    check_merge_tree_conflicts: bool,
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
    // Main worktree is the worktree on the default branch (if exists), else first worktree
    let main_worktree = worktrees
        .worktrees
        .iter()
        .find(|wt| wt.branch.as_deref() == Some(default_branch.as_str()))
        .cloned()
        .unwrap_or_else(|| worktrees.main().clone());
    let current_worktree_path = repo.worktree_root().ok();
    let previous_branch = repo.get_switch_history().and_then(|h| h.previous);

    // Sort worktrees: current first, main second, then by timestamp descending
    let sorted_worktrees = sort_worktrees(
        worktrees.worktrees.clone(),
        &main_worktree,
        current_worktree_path.as_ref(),
    );

    // Get main worktree directory name for path template expansion
    let main_worktree_name = main_worktree
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

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

    // Initialize worktree items with identity fields and None for computed fields
    let mut all_items: Vec<ListItem> = sorted_worktrees
        .iter()
        .map(|wt| {
            let is_main = wt.path == main_worktree.path;
            let is_current = current_worktree_path
                .as_ref()
                .is_some_and(|cp| &wt.path == cp);
            let is_previous = previous_branch
                .as_deref()
                .is_some_and(|prev| wt.branch.as_deref() == Some(prev));

            // Compute path mismatch: does the actual path match what the template would generate?
            // Main worktree on default branch is the reference point (no mismatch possible)
            // Main worktree on OTHER branch should show mismatch (it's "not at home")
            let path_mismatch = if is_main && wt.branch.as_deref() == Some(default_branch.as_str())
            {
                false
            } else if let Some(branch) = &wt.branch {
                // Expand template and compare with actual path
                match config.format_path(main_worktree_name, branch) {
                    Ok(expected_relative) => {
                        // Template path is relative to main worktree, resolve it
                        let expected_path = main_worktree.path.join(&expected_relative);
                        // Canonicalize both paths for comparison (handles symlinks, .., etc.)
                        let actual_canonical = wt.path.canonicalize().ok();
                        let expected_canonical = expected_path.canonicalize().ok();
                        match (actual_canonical, expected_canonical) {
                            (Some(actual), Some(expected)) => actual != expected,
                            // Can't canonicalize one or both paths (e.g., worktree deleted,
                            // expected parent doesn't exist). Fall back to direct comparison.
                            _ => wt.path != expected_path,
                        }
                    }
                    Err(e) => {
                        log::debug!("Template expansion failed for branch {}: {}", branch, e);
                        false // Template expansion failed, don't mark as mismatch
                    }
                }
            } else {
                // Detached HEAD - not on any branch, so "not at home"
                true
            };

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
    let layout = super::layout::calculate_layout_from_basics(&all_items, show_full, fetch_ci);

    // Single-line invariant: use safe width to prevent line wrapping
    let max_width = super::layout::get_safe_list_width();

    // Create collection options
    let options = super::collect_progressive_impl::CollectOptions {
        fetch_ci,
        check_merge_tree_conflicts,
    };

    // Counter for total cells - incremented as spawns are queued
    let cell_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
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

        // Build skeleton rows
        let skeletons: Vec<String> = all_items
            .iter()
            .map(|item| {
                if item.worktree_data().is_some() {
                    // Worktrees get skeleton rows (format_skeleton_row extracts is_current/is_previous internally)
                    layout.format_skeleton_row(item)
                } else {
                    // Branches render immediately (no skeleton needed)
                    layout.format_list_item_line(item, previous_branch.as_deref())
                }
            })
            .collect();

        let initial_footer = format!("{INFO_EMOJI} {dim}{footer_base} (loading...){dim:#}");

        Some(ProgressiveTable::new(
            layout.format_header_line(),
            skeletons,
            initial_footer,
            max_width,
        )?)
    } else {
        None
    };

    // Early exit for benchmarking skeleton render time
    if std::env::var("WT_SKELETON_ONLY").is_ok() {
        return Ok(None);
    }

    // Cache last rendered (unclamped) message per row to avoid redundant updates.
    let mut last_rendered_lines: Vec<String> = vec![String::new(); all_items.len()];

    // Create channel for cell updates
    let (tx, rx) = chan::unbounded();

    // Spawn worktree collection in background thread
    let sorted_worktrees_clone = sorted_worktrees.clone();
    let tx_worktrees = tx.clone();
    let default_branch_clone = default_branch.clone();
    let cell_count_wt = cell_count.clone();
    std::thread::spawn(move || {
        sorted_worktrees_clone
            .par_iter()
            .enumerate()
            .for_each(|(idx, wt)| {
                // Always pass default_branch for ahead/behind/diff computation
                // Status symbols will filter based on is_main flag
                super::collect_progressive_impl::collect_worktree_progressive(
                    wt,
                    idx,
                    &default_branch_clone,
                    &options,
                    tx_worktrees.clone(),
                    &cell_count_wt,
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
        let cell_count_br = cell_count.clone();
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
                        &options,
                        tx_branches.clone(),
                        &cell_count_br,
                    );
                });
        });
    }

    // Drop the original sender so drain_cell_updates knows when all spawned threads are done
    drop(tx);

    // Track completed cells for footer progress
    let mut completed_cells = 0;

    // Drain cell updates with conditional progressive rendering
    let drain_result = drain_cell_updates(rx, &mut all_items, |item_idx, item, ctx| {
        // Compute/recompute status symbols as data arrives (both modes)
        // This is idempotent and updates status as new data (like upstream) arrives
        let item_default_branch = if item.is_main() {
            None
        } else {
            Some(default_branch.as_str())
        };
        item.compute_status_symbols(
            item_default_branch,
            ctx.has_merge_tree_conflicts,
            ctx.user_status.clone(),
            ctx.working_tree_symbols.as_deref(),
            ctx.has_conflicts,
        );

        // Progressive mode only: update UI
        if let Some(ref mut table) = progressive_table {
            use anstyle::Style;
            let dim = Style::new().dimmed();

            completed_cells += 1;
            let total_cells = cell_count.load(std::sync::atomic::Ordering::Relaxed);

            // Update footer progress
            let footer_msg = format!(
                "{INFO_EMOJI} {dim}{footer_base} ({completed_cells}/{total_cells} cells loaded){dim:#}"
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
    });

    // Handle timeout if it occurred
    if let DrainResult::TimedOut {
        received_count,
        incomplete_items,
    } = drain_result
    {
        // Build diagnostic message showing what we received (not what's missing)
        let mut diag = format!("wt list timed out after 30s ({received_count} cells received)");

        if !incomplete_items.is_empty() {
            diag.push_str("\nIncomplete items:");
            for (_, name, cells) in &incomplete_items {
                diag.push_str(&format!("\n  - {name}: received {}", cells.join(", ")));
            }
        }

        diag.push_str(
            "\n\nThis likely indicates a git command hung. Run with RUST_LOG=debug for details.",
        );

        crate::output::warning(diag)?;
    }

    // Finalize progressive table or render buffered output
    if let Some(mut table) = progressive_table {
        // Build final summary string
        let final_msg = super::format_summary_message(
            &all_items,
            show_branches || show_remotes,
            layout.hidden_nonempty_count,
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
            layout.hidden_nonempty_count,
        );

        crate::output::table(layout.format_header_line())?;
        for item in &all_items {
            crate::output::table(layout.format_list_item_line(item, previous_branch.as_deref()))?;
        }
        crate::output::table("")?;
        crate::output::table(final_msg)?;
    }

    // Status symbols are now computed during data collection (both modes), no fallback needed

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
/// This is the blocking version used by statusline. For progressive rendering
/// with callbacks, see the `collect()` function.
pub fn populate_items(
    items: &mut [ListItem],
    default_branch: &str,
    options: CollectOptions,
) -> anyhow::Result<()> {
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    if items.is_empty() {
        return Ok(());
    }

    // Create channel for cell updates
    let (tx, rx) = chan::unbounded();

    // Counter for cells (not used for progress, but required by spawn functions)
    let cell_count = Arc::new(AtomicUsize::new(0));

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
                &options,
                tx.clone(),
                &cell_count,
            );
        }
    });

    // Drain cell updates (blocking until all complete)
    let drain_result = drain_cell_updates(rx, items, |_item_idx, item, ctx| {
        // Compute status symbols as data arrives (same logic as in collect())
        let item_default_branch = if item.is_main() {
            None
        } else {
            Some(default_branch)
        };
        item.compute_status_symbols(
            item_default_branch,
            ctx.has_merge_tree_conflicts,
            ctx.user_status.clone(),
            ctx.working_tree_symbols.as_deref(),
            ctx.has_conflicts,
        );
    });

    // Handle timeout (silent for statusline - just log it)
    if let DrainResult::TimedOut { received_count, .. } = drain_result {
        log::warn!("populate_items timed out after 30s ({received_count} cells received)");
    }

    // Populate display fields (including status_line for statusline command)
    for item in items.iter_mut() {
        item.finalize_display();
    }

    Ok(())
}
