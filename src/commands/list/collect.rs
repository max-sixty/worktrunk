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
use worktrunk::git::{GitError, LineDiff, Repository, Worktree};
use worktrunk::styling::INFO_EMOJI;

use super::ci_status::PrStatus;
use super::model::{
    AheadBehind, BranchDiffTotals, BranchState, CommitDetails, GitOperation, ItemKind, ListItem,
    MainDivergence, StatusSymbols, UpstreamDivergence, UpstreamStatus,
};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DivergenceKind {
    None,
    Ahead,
    Behind,
    Diverged,
}

fn classify_divergence(ahead: usize, behind: usize) -> DivergenceKind {
    match (ahead, behind) {
        (0, 0) => DivergenceKind::None,
        (a, 0) if a > 0 => DivergenceKind::Ahead,
        (0, b) if b > 0 => DivergenceKind::Behind,
        _ => DivergenceKind::Diverged,
    }
}

/// Compute main branch divergence state from ahead/behind counts.
fn compute_main_divergence(ahead: usize, behind: usize) -> MainDivergence {
    match classify_divergence(ahead, behind) {
        DivergenceKind::None => MainDivergence::None,
        DivergenceKind::Ahead => MainDivergence::Ahead,
        DivergenceKind::Behind => MainDivergence::Behind,
        DivergenceKind::Diverged => MainDivergence::Diverged,
    }
}

/// Compute upstream divergence state from ahead/behind counts.
fn compute_upstream_divergence(ahead: usize, behind: usize) -> UpstreamDivergence {
    match classify_divergence(ahead, behind) {
        DivergenceKind::None => UpstreamDivergence::None,
        DivergenceKind::Ahead => UpstreamDivergence::Ahead,
        DivergenceKind::Behind => UpstreamDivergence::Behind,
        DivergenceKind::Diverged => UpstreamDivergence::Diverged,
    }
}

fn compute_divergences(
    counts: &AheadBehind,
    upstream: &UpstreamStatus,
) -> (MainDivergence, UpstreamDivergence) {
    let main_divergence = compute_main_divergence(counts.ahead, counts.behind);
    let (upstream_ahead, upstream_behind) =
        upstream.active().map(|(_, a, b)| (a, b)).unwrap_or((0, 0));
    let upstream_divergence = compute_upstream_divergence(upstream_ahead, upstream_behind);

    (main_divergence, upstream_divergence)
}

/// Determine branch state for a worktree.
///
/// Returns:
/// - `BranchState::None` if main worktree or no base branch
/// - `BranchState::MatchesMain` if working tree matches main exactly (no commits, no diff)
/// - `BranchState::NoCommits` if no commits and working tree is clean
/// - `BranchState::None` otherwise
fn determine_worktree_branch_state(
    is_main: bool,
    default_branch: Option<&str>,
    ahead: usize,
    working_tree_diff: Option<&LineDiff>,
    working_tree_diff_with_main: &Option<Option<LineDiff>>,
) -> BranchState {
    if is_main || default_branch.is_none() {
        return BranchState::None;
    }

    let is_clean = working_tree_diff.map(|d| d.is_empty()).unwrap_or(true);

    // Check if working tree matches main exactly (requires diff with main to be computed)
    if let Some(Some(mdiff)) = working_tree_diff_with_main.as_ref()
        && mdiff.is_empty()
        && ahead == 0
    {
        return BranchState::MatchesMain;
    }

    // Check if no commits and clean working tree
    if ahead == 0 && is_clean {
        BranchState::NoCommits
    } else {
        BranchState::None
    }
}

/// Compute status symbols for a single item (worktrees and branches).
///
/// This is idempotent and can be called multiple times as new data arrives.
/// It will recompute with the latest available data.
///
/// Branches get a subset of status symbols (no working tree, git operation, or worktree attrs).
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

            // Item attributes - priority: prunable > locked (1 char max)
            let item_attrs = if data.prunable.is_some() {
                "⌫".to_string() // Prunable (directory missing)
            } else if data.locked.is_some() {
                "⊠".to_string() // Locked (protected)
            } else {
                String::new()
            };

            // Determine branch state (only for non-main worktrees with base branch)
            let branch_state = determine_worktree_branch_state(
                data.is_main,
                default_branch,
                counts.ahead,
                data.working_tree_diff.as_ref(),
                &data.working_tree_diff_with_main,
            );

            // Determine git operation
            let git_operation = match data.worktree_state.as_deref() {
                Some("rebase") => GitOperation::Rebase,
                Some("merge") => GitOperation::Merge,
                _ => GitOperation::None,
            };

            // Combine conflicts and branch state (mutually exclusive)
            let branch_state = if has_conflicts {
                BranchState::Conflicts
            } else if has_merge_tree_conflicts {
                BranchState::MergeTreeConflicts
            } else {
                branch_state
            };

            item.status_symbols = Some(StatusSymbols {
                branch_state,
                git_operation,
                item_attrs,
                main_divergence,
                upstream_divergence,
                working_tree: working_tree_symbols.unwrap_or("").to_string(),
                user_status,
            });
        }
        ItemKind::Branch => {
            // Simplified status computation for branches
            // Only compute symbols that apply to branches (no working tree, git operation, or worktree attrs)

            // Branch state - branches can only show Conflicts or NoCommits
            // (MatchesMain only applies to worktrees since branches don't have working trees)
            let branch_state = if has_merge_tree_conflicts {
                BranchState::MergeTreeConflicts
            } else if let Some(ref c) = item.counts {
                if c.ahead == 0 {
                    BranchState::NoCommits
                } else {
                    BranchState::None
                }
            } else {
                BranchState::None
            };

            item.status_symbols = Some(StatusSymbols {
                branch_state,
                git_operation: GitOperation::None,
                item_attrs: "⎇".to_string(), // Branch indicator
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
    mut on_update: impl FnMut(usize, &mut ListItem, bool, Option<String>, Option<&str>, bool),
) {
    // Temporary storage for data needed by status_symbols computation
    let mut merge_tree_conflicts_map: Vec<Option<bool>> = vec![None; items.len()];
    let mut user_status_map: Vec<Option<Option<String>>> = vec![None; items.len()];
    let mut working_tree_symbols_map: Vec<Option<String>> = vec![None; items.len()];
    let mut has_conflicts_map: Vec<Option<bool>> = vec![None; items.len()];

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
                // Store temporarily for status_symbols computation
                working_tree_symbols_map[item_idx] = Some(working_tree_symbols);
                has_conflicts_map[item_idx] = Some(has_conflicts);
            }
            CellUpdate::MergeTreeConflicts {
                item_idx,
                has_merge_tree_conflicts,
            } => {
                // Store temporarily for status_symbols computation
                merge_tree_conflicts_map[item_idx] = Some(has_merge_tree_conflicts);
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
                // Store temporarily for status_symbols computation
                user_status_map[item_idx] = Some(user_status);
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
        let has_merge_tree_conflicts = merge_tree_conflicts_map[item_idx].unwrap_or(false);
        let user_status = user_status_map[item_idx].clone().unwrap_or(None);
        let working_tree_symbols = working_tree_symbols_map[item_idx].as_deref();
        let has_conflicts = has_conflicts_map[item_idx].unwrap_or(false);
        on_update(
            item_idx,
            &mut items[item_idx],
            has_merge_tree_conflicts,
            user_status,
            working_tree_symbols,
            has_conflicts,
        );
    }
}

/// Get branches that don't have worktrees.
///
/// Returns (branch_name, commit_sha) pairs for all branches without associated worktrees.
fn get_branches_without_worktrees(
    repo: &Repository,
    worktrees: &[Worktree],
) -> Result<Vec<(String, String)>, GitError> {
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

/// Collect worktree data with optional progressive rendering.
///
/// When `show_progress` is true, renders a skeleton immediately and updates as data arrives.
/// When false, behavior depends on `render_table`:
/// - If `render_table` is true: renders final table (buffered mode)
/// - If `render_table` is false: returns data without rendering (JSON mode)
pub fn collect(
    repo: &Repository,
    show_branches: bool,
    show_full: bool,
    fetch_ci: bool,
    check_merge_tree_conflicts: bool,
    show_progress: bool,
    render_table: bool,
) -> Result<Option<super::model::ListData>, GitError> {
    use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
    use std::time::Duration;

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
        .unwrap_or_else(|| worktrees.worktrees[0].clone());
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

    // Initialize worktree items with identity fields and None for computed fields
    let mut all_items: Vec<super::model::ListItem> = sorted_worktrees
        .iter()
        .map(|wt| super::model::ListItem {
            // Common fields
            head: wt.head.clone(),
            branch: wt.branch.clone(),
            commit: None,
            counts: None,
            branch_diff: None,
            upstream: None,
            pr_status: None,
            status_symbols: None,
            display: super::model::DisplayFields::default(),
            // Type-specific data
            kind: super::model::ItemKind::Worktree(Box::new(
                super::model::WorktreeData::from_worktree(wt, wt.path == main_worktree.path),
            )),
        })
        .collect();

    // Initialize branch items with identity fields and None for computed fields
    let branch_start_idx = all_items.len();
    for (branch_name, commit_sha) in &branches_without_worktrees {
        all_items.push(super::model::ListItem {
            // Common fields
            head: commit_sha.clone(),
            branch: Some(branch_name.clone()),
            commit: None,
            counts: None,
            branch_diff: None,
            upstream: None,
            pr_status: None,
            status_symbols: None,
            display: super::model::DisplayFields::default(),
            // Type-specific data
            kind: super::model::ItemKind::Branch,
        });
    }

    // Calculate layout from items (both worktrees and branches)
    let layout = super::layout::calculate_layout_from_basics(&all_items, show_full, fetch_ci);

    // Single-line invariant: use safe width to prevent line wrapping
    // (which breaks indicatif's line-based cursor math).
    let max_width = super::layout::get_safe_list_width();

    // Clamp helper to keep progress output single-line in narrow terminals.
    // truncate_visible already returns early if no truncation needed, avoiding redundant work.
    let clamp = |s: &str| -> String { crate::display::truncate_visible(s, max_width, "…") };

    // Create MultiProgress with explicit draw target and cursor mode
    // Use stderr for progress bars so they don't interfere with stdout directives
    let multi = if show_progress {
        let mp = MultiProgress::with_draw_target(indicatif::ProgressDrawTarget::stderr_with_hz(10));
        mp.set_move_cursor(true); // Stable since bar count is fixed
        mp
    } else {
        MultiProgress::new()
    };

    let message_style = ProgressStyle::with_template("{msg}").unwrap();

    // Create header progress bar (part of transient UI, cleared with finish_and_clear)
    let header_pb = if show_progress {
        let pb = multi.add(ProgressBar::hidden());
        pb.set_style(message_style.clone());
        pb.set_message(clamp(&layout.format_header_line()));
        Some(pb)
    } else {
        None
    };

    // Create progress bars for all items (worktrees + branches)
    let progress_bars: Vec<_> = all_items
        .iter()
        .map(|item| {
            let pb = multi.add(ProgressBar::new_spinner());
            if show_progress {
                pb.set_style(message_style.clone());

                // Render skeleton immediately with clamping
                let skeleton = if item.worktree_data().is_some() {
                    // Worktree skeleton - show known data (branch, path, commit)
                    let is_current = item
                        .worktree_path()
                        .and_then(|p| current_worktree_path.as_ref().map(|cp| p == cp))
                        .unwrap_or(false);
                    layout.format_skeleton_row(item, is_current)
                } else {
                    // Branch skeleton - use full item rendering
                    layout.format_list_item_line(item, current_worktree_path.as_ref())
                };
                pb.set_message(clamp(&skeleton));
                pb.enable_steady_tick(Duration::from_millis(100));
            }
            pb
        })
        .collect();

    // Cache last rendered (unclamped) message per row to avoid redundant updates.
    // TODO(list-progressive): if we change clamping/detection strategy, keep a test case
    // for off-screen CI column updates to ensure we still refresh rows.
    let mut last_rendered_lines: Vec<String> = vec![String::new(); all_items.len()];

    // Footer progress bar with loading status
    // Uses determinate bar (no spinner) with {wide_msg} to prevent clearing artifacts
    let total_cells = all_items.len() * layout.columns.len();
    let num_worktrees = all_items
        .iter()
        .filter(|item| item.worktree_data().is_some())
        .count();
    let num_branches = all_items.len() - num_worktrees;
    let footer_base = if show_branches && num_branches > 0 {
        format!(
            "Showing {} worktrees, {} branches",
            num_worktrees, num_branches
        )
    } else {
        let plural = if num_worktrees == 1 { "" } else { "s" };
        format!("Showing {} worktree{}", num_worktrees, plural)
    };

    // Spacer: single-line blank between the table rows and the footer (no multiline messages)
    let spacer_style = ProgressStyle::with_template("{wide_msg}").unwrap();
    let spacer_pb = if show_progress {
        let gap = multi.add(ProgressBar::new(1));
        gap.set_style(spacer_style.clone());
        gap.set_message(" "); // padded blank line
        Some(gap)
    } else {
        None
    };

    // Footer is single-line; no '\n'. Will be replaced with final summary on finish.
    let footer_style = ProgressStyle::with_template("{wide_msg}").unwrap();

    let footer_pb = if show_progress {
        use anstyle::Style;
        let dim = Style::new().dimmed();

        // Footer with determinate bar (no spinner/tick)
        let footer = multi.add(ProgressBar::new(total_cells as u64));
        footer.set_style(footer_style);
        footer.set_message(format!(
            "{INFO_EMOJI} {dim}{footer_base} (0/{total_cells} cells loaded){dim:#}"
        ));
        Some(footer)
    } else {
        None
    };

    // Create channel for cell updates
    let (tx, rx) = chan::unbounded();

    // Create collection options
    let options = super::collect_progressive_impl::CollectOptions {
        fetch_ci,
        check_merge_tree_conflicts,
    };

    // Spawn worktree collection in background thread
    let sorted_worktrees_clone = sorted_worktrees.clone();
    let tx_worktrees = tx.clone();
    let default_branch_clone = default_branch.clone();
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
                );
            });
    });

    // Spawn branch collection in background thread (if requested)
    if show_branches {
        let branches_clone = branches_without_worktrees.clone();
        let main_path = main_worktree.path.clone();
        let tx_branches = tx.clone();
        let default_branch_clone = default_branch.clone();
        std::thread::spawn(move || {
            branches_clone
                .par_iter()
                .enumerate()
                .for_each(|(idx, (branch_name, commit_sha))| {
                    let item_idx = branch_start_idx + idx;
                    super::collect_progressive_impl::collect_branch_progressive(
                        branch_name,
                        commit_sha,
                        &main_path,
                        item_idx,
                        &default_branch_clone,
                        &options,
                        tx_branches.clone(),
                    );
                });
        });
    }

    // Drop the original sender so drain_cell_updates knows when all spawned threads are done
    drop(tx);

    // Track completed cells for footer progress
    let mut completed_cells = 0;

    // Drain cell updates with conditional progressive rendering
    drain_cell_updates(
        rx,
        &mut all_items,
        |item_idx,
         item,
         has_merge_tree_conflicts,
         user_status,
         working_tree_symbols,
         has_conflicts| {
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
                has_merge_tree_conflicts,
                user_status,
                working_tree_symbols,
                has_conflicts,
            );

            // Progressive mode only: update UI
            if show_progress {
                use anstyle::Style;
                let dim = Style::new().dimmed();

                completed_cells += 1;

                // Update footer progress
                if let Some(pb) = footer_pb.as_ref() {
                    pb.set_position(completed_cells as u64);
                    pb.set_message(format!(
                    "{INFO_EMOJI} {dim}{footer_base} ({completed_cells}/{total_cells} cells loaded){dim:#}"
                ));
                }

                // Re-render the row with caching and clamping (now includes status if computed)
                if let Some(pb) = progress_bars.get(item_idx) {
                    let rendered =
                        layout.format_list_item_line(item, current_worktree_path.as_ref());
                    let clamped = clamp(&rendered);

                    // Compare using full line so changes beyond the clamp (e.g., CI) still refresh.
                    if rendered != last_rendered_lines[item_idx] {
                        last_rendered_lines[item_idx] = rendered;
                        pb.set_message(clamped);
                    }
                }
            }
        },
    );

    // Finalize progress bars: no clearing race; footer morphs into summary on TTY
    if show_progress {
        use std::io::IsTerminal;
        let is_tty = std::io::stderr().is_terminal(); // Check stderr, not stdout ✅

        // Build final summary string once using helper function
        let final_msg =
            super::format_summary_message(&all_items, show_branches, layout.hidden_nonempty_count);

        if is_tty {
            // Interactive: morph footer → summary, keep rows in place
            // Do a final render pass to ensure all data is displayed
            // (progressive updates may have been clamped or timing-delayed)
            for (item_idx, pb) in progress_bars.iter().enumerate() {
                let rendered = layout
                    .format_list_item_line(&all_items[item_idx], current_worktree_path.as_ref());
                let clamped = clamp(&rendered);
                pb.set_message(clamped);
            }

            if let Some(pb) = spacer_pb.as_ref() {
                pb.finish(); // leave the blank line
            }
            if let Some(pb) = footer_pb.as_ref() {
                pb.finish_with_message(final_msg.clone());
            }
            if let Some(pb) = header_pb {
                pb.finish();
            }
            for pb in progress_bars {
                pb.finish();
            }
        } else {
            // Non-TTY: clear progress bars and print final table to stderr
            if let Some(pb) = spacer_pb {
                pb.finish_and_clear();
            }
            if let Some(pb) = footer_pb {
                pb.finish_and_clear();
            }
            if let Some(pb) = header_pb {
                pb.finish_and_clear();
            }
            for pb in progress_bars {
                pb.finish_and_clear();
            }
            // Ensure atomicity w.r.t. indicatif's draw thread
            multi.suspend(|| {
                // Redraw static table
                crate::output::raw_terminal(layout.format_header_line())?;
                for item in &all_items {
                    crate::output::raw_terminal(
                        layout.format_list_item_line(item, current_worktree_path.as_ref()),
                    )?;
                }
                // Blank line + summary (rendered here in non-tty mode)
                crate::output::raw_terminal("")?;
                crate::output::raw_terminal(final_msg.clone())
            })?;
        }
    } else {
        // Buffered mode (no progress bars shown)
        for pb in progress_bars {
            pb.finish();
        }

        // Render final table if requested (table format), otherwise just return data (JSON format)
        if render_table {
            // Build final summary string
            let final_msg = super::format_summary_message(
                &all_items,
                show_branches,
                layout.hidden_nonempty_count,
            );

            // Render complete table
            crate::output::raw_terminal(layout.format_header_line())?;
            for item in &all_items {
                crate::output::raw_terminal(
                    layout.format_list_item_line(item, current_worktree_path.as_ref()),
                )?;
            }
            // Blank line + summary
            crate::output::raw_terminal("")?;
            crate::output::raw_terminal(final_msg)?;
        }
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
