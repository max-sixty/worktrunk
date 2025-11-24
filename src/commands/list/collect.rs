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
    AheadBehind, BranchDiffTotals, BranchOpState, CommitDetails, DisplayFields, ItemKind, ListItem,
    MainDivergence, StatusSymbols, UpstreamDivergence, UpstreamStatus, WorktreeData,
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
#[derive(Debug, Clone)]
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
    WorktreeState {
        item_idx: usize,
        worktree_state: Option<String>,
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
            | CellUpdate::BranchDiff { item_idx, .. }
            | CellUpdate::WorkingTreeDiff { item_idx, .. }
            | CellUpdate::MergeTreeConflicts { item_idx, .. }
            | CellUpdate::WorktreeState { item_idx, .. }
            | CellUpdate::UserStatus { item_idx, .. }
            | CellUpdate::Upstream { item_idx, .. }
            | CellUpdate::CiStatus { item_idx, .. } => *item_idx,
        }
    }
}

/// Detect if a worktree is in the middle of a git operation (rebase/merge).
pub(super) fn detect_worktree_state(repo: &Repository) -> Option<String> {
    let git_dir = repo.git_dir().ok()?;

    if git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists() {
        Some("rebase".to_string())
    } else if git_dir.join("MERGE_HEAD").exists() {
        Some("merge".to_string())
    } else {
        None
    }
}

fn compute_divergences(
    counts: &AheadBehind,
    upstream: &UpstreamStatus,
) -> (MainDivergence, UpstreamDivergence) {
    let main_divergence = MainDivergence::from_counts(counts.ahead, counts.behind);
    let (upstream_ahead, upstream_behind) =
        upstream.active().map(|(_, a, b)| (a, b)).unwrap_or((0, 0));
    let upstream_divergence = UpstreamDivergence::from_counts(upstream_ahead, upstream_behind);

    (main_divergence, upstream_divergence)
}

/// Determine branch state for a worktree.
///
/// Returns:
/// - `BranchOpState::None` if main worktree or no base branch
/// - `BranchOpState::NoCommits` if no commits ahead and working tree is clean
/// - `BranchOpState::MatchesMain` if ahead == 0 but dirty tree happens to match main (rare)
/// - `BranchOpState::None` otherwise
fn determine_worktree_base_state(
    is_main: bool,
    default_branch: Option<&str>,
    ahead: usize,
    working_tree_diff: Option<&LineDiff>,
    working_tree_diff_with_main: &Option<Option<LineDiff>>,
) -> BranchOpState {
    if is_main || default_branch.is_none() {
        return BranchOpState::None;
    }

    let is_clean = working_tree_diff.map(|d| d.is_empty()).unwrap_or(true);

    // Check if no commits and clean working tree (most common removable state)
    if ahead == 0 && is_clean {
        return BranchOpState::NoCommits;
    }

    // Check if working tree matches main exactly (requires diff with main to be computed)
    // This is rare - only when ahead == 0, tree is dirty, but dirty changes happen to match main
    if let Some(Some(mdiff)) = working_tree_diff_with_main.as_ref()
        && mdiff.is_empty()
        && ahead == 0
    {
        BranchOpState::MatchesMain
    } else {
        BranchOpState::None
    }
}

/// Compute status symbols for a single item (worktrees and branches).
///
/// This is idempotent and can be called multiple times as new data arrives.
/// It will recompute with the latest available data.
///
/// Branches get a subset of status symbols (no working tree changes or worktree attrs).
// TODO(status-indicator): show a status glyph when a worktree's checked-out branch
// differs from the branch name we associate with it (e.g., worktree exists but on another branch).
fn compute_item_status_symbols(
    item: &mut ListItem,
    default_branch: Option<&str>,
    has_merge_tree_conflicts: bool,
    user_status: Option<String>,
    working_tree_symbols: Option<&str>,
    has_conflicts: bool,
) {
    // Common fields for both worktrees and branches
    let default_counts = AheadBehind::default();
    let default_upstream = UpstreamStatus::default();
    let counts = item.counts.as_ref().unwrap_or(&default_counts);
    let upstream = item.upstream.as_ref().unwrap_or(&default_upstream);
    let (main_divergence, upstream_divergence) = compute_divergences(counts, upstream);

    match &item.kind {
        ItemKind::Worktree(data) => {
            // Full status computation for worktrees
            // Use default_branch directly (None for main worktree)

            // Worktree state - priority: prunable > locked (1 char max)
            let worktree_state = if data.prunable.is_some() {
                "⌫".to_string() // Prunable (directory missing)
            } else if data.locked.is_some() {
                "⊠".to_string() // Locked (protected)
            } else {
                String::new()
            };

            // Determine base branch state (only for non-main worktrees with base branch)
            let base_state = determine_worktree_base_state(
                data.is_main,
                default_branch,
                counts.ahead,
                data.working_tree_diff.as_ref(),
                &data.working_tree_diff_with_main,
            );

            // Apply priority: Conflicts > Rebase > Merge > MergeTreeConflicts > base_state
            let branch_op_state = if has_conflicts {
                BranchOpState::Conflicts
            } else if data.worktree_state.as_deref() == Some("rebase") {
                BranchOpState::Rebase
            } else if data.worktree_state.as_deref() == Some("merge") {
                BranchOpState::Merge
            } else if has_merge_tree_conflicts {
                BranchOpState::MergeTreeConflicts
            } else {
                base_state
            };

            // Override main_divergence for the main worktree
            let main_divergence = if data.is_main {
                MainDivergence::IsMain
            } else {
                main_divergence
            };

            item.status_symbols = Some(StatusSymbols {
                branch_op_state,
                worktree_state,
                main_divergence,
                upstream_divergence,
                working_tree: working_tree_symbols.unwrap_or("").to_string(),
                user_status,
            });
        }
        ItemKind::Branch => {
            // Simplified status computation for branches
            // Only compute symbols that apply to branches (no working tree, git operation, or worktree attrs)

            // Branch op state - branches can only show MergeTreeConflicts or NoCommits
            // (MatchesMain only applies to worktrees since branches don't have working trees)
            let branch_op_state = if has_merge_tree_conflicts {
                BranchOpState::MergeTreeConflicts
            } else if let Some(ref c) = item.counts {
                if c.ahead == 0 {
                    BranchOpState::NoCommits
                } else {
                    BranchOpState::None
                }
            } else {
                BranchOpState::None
            };

            item.status_symbols = Some(StatusSymbols {
                branch_op_state,
                worktree_state: "⎇".to_string(), // Branch indicator
                main_divergence,
                upstream_divergence,
                working_tree: String::new(),
                user_status,
            });
        }
    }
}

/// Drain cell updates from the channel and apply them to items.
///
/// This is the shared logic between progressive and buffered collection modes.
/// The `on_update` callback is called after each update is processed with the
/// item index and a reference to the updated item, allowing progressive mode
/// to update progress bars while buffered mode does nothing.
fn drain_cell_updates(
    rx: chan::Receiver<CellUpdate>,
    items: &mut [ListItem],
    mut on_update: impl FnMut(usize, &mut ListItem, &StatusContext),
) {
    // Temporary storage for data needed by status_symbols computation
    let mut status_contexts: Vec<StatusContext> = (0..items.len())
        .map(|_| StatusContext {
            has_merge_tree_conflicts: false,
            user_status: None,
            working_tree_symbols: None,
            has_conflicts: false,
        })
        .collect();

    // Process cell updates as they arrive
    while let Ok(update) = rx.recv() {
        let item_idx = update.item_idx();

        match update {
            CellUpdate::CommitDetails { item_idx, commit } => {
                items[item_idx].commit = Some(commit);
            }
            CellUpdate::AheadBehind { item_idx, counts } => {
                items[item_idx].counts = Some(counts);
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
            CellUpdate::WorktreeState {
                item_idx,
                worktree_state,
            } => {
                if let ItemKind::Worktree(data) = &mut items[item_idx].kind {
                    data.worktree_state = worktree_state;
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

    // Sort worktrees for display order
    let sorted_worktrees = sort_worktrees(
        &worktrees.worktrees,
        &main_worktree,
        current_worktree_path.as_ref(),
    );

    // Get branches early for layout calculation and skeleton creation (when --branches is used)
    let branches_without_worktrees = if show_branches {
        get_branches_without_worktrees(repo, &worktrees.worktrees)?
    } else {
        Vec::new()
    };

    // Get remote branches (when --remotes is used)
    let remote_branches = if show_remotes {
        get_remote_branches(repo, &worktrees.worktrees)?
    } else {
        Vec::new()
    };

    // Initialize worktree items with identity fields and None for computed fields
    let mut all_items: Vec<ListItem> = sorted_worktrees
        .iter()
        .map(|wt| ListItem {
            head: wt.head.clone(),
            branch: wt.branch.clone(),
            commit: None,
            counts: None,
            branch_diff: None,
            upstream: None,
            pr_status: None,
            status_symbols: None,
            display: DisplayFields::default(),
            kind: ItemKind::Worktree(Box::new(WorktreeData::from_worktree(
                wt,
                wt.path == main_worktree.path,
            ))),
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
                    let is_current = item
                        .worktree_path()
                        .and_then(|p| current_worktree_path.as_ref().map(|cp| p == cp))
                        .unwrap_or(false);
                    layout.format_skeleton_row(item, is_current)
                } else {
                    layout.format_list_item_line(item, current_worktree_path.as_ref())
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
    drain_cell_updates(rx, &mut all_items, |item_idx, item, ctx| {
        // Compute/recompute status symbols as data arrives (both modes)
        // This is idempotent and updates status as new data (like upstream) arrives
        let item_default_branch = if item.is_main() {
            None
        } else {
            Some(default_branch.as_str())
        };
        compute_item_status_symbols(
            item,
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
            let rendered = layout.format_list_item_line(item, current_worktree_path.as_ref());

            // Compare using full line so changes beyond the clamp (e.g., CI) still refresh.
            if rendered != last_rendered_lines[item_idx] {
                last_rendered_lines[item_idx] = rendered.clone();
                if let Err(e) = table.update_row(item_idx, rendered) {
                    log::debug!("Progressive row update failed: {}", e);
                }
            }
        }
    });

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
                let rendered = layout.format_list_item_line(item, current_worktree_path.as_ref());
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
                final_lines
                    .push(layout.format_list_item_line(item, current_worktree_path.as_ref()));
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

        crate::output::raw_terminal(layout.format_header_line())?;
        for item in &all_items {
            crate::output::raw_terminal(
                layout.format_list_item_line(item, current_worktree_path.as_ref()),
            )?;
        }
        crate::output::raw_terminal("")?;
        crate::output::raw_terminal(final_msg)?;
    }

    // Status symbols are now computed during data collection (both modes), no fallback needed

    // Compute display fields for all items (used by JSON output)
    // Table rendering uses raw data directly; these fields provide pre-formatted strings for JSON
    for item in &mut all_items {
        item.display = super::model::DisplayFields::from_common_fields(
            &item.counts,
            &item.branch_diff,
            &item.upstream,
            &item.pr_status,
        );

        if let super::model::ItemKind::Worktree(ref mut wt_data) = item.kind
            && let Some(ref working_tree_diff) = wt_data.working_tree_diff
        {
            wt_data.working_diff_display = super::columns::ColumnKind::WorkingDiff
                .format_diff_plain(working_tree_diff.added, working_tree_diff.deleted);
        }
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

/// Sort worktrees for display (main first, then current, then by timestamp descending).
fn sort_worktrees(
    worktrees: &[Worktree],
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

    let mut indexed: Vec<_> = worktrees.iter().enumerate().collect();
    indexed.sort_by_key(|(idx, wt)| {
        let is_main = wt.path == main_worktree.path;
        let is_current = current_path.map(|cp| &wt.path == cp).unwrap_or(false);

        let priority = if is_main {
            0
        } else if is_current {
            1
        } else {
            2
        };

        (priority, std::cmp::Reverse(timestamps[*idx]))
    });

    indexed.into_iter().map(|(_, wt)| wt.clone()).collect()
}
