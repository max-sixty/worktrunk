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
        is_dirty: bool,
    },
    /// Merge conflicts with main
    Conflicts {
        item_idx: usize,
        has_conflicts: bool,
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
            CellUpdate::CommitDetails { item_idx, .. } => *item_idx,
            CellUpdate::AheadBehind { item_idx, .. } => *item_idx,
            CellUpdate::BranchDiff { item_idx, .. } => *item_idx,
            CellUpdate::WorkingTreeDiff { item_idx, .. } => *item_idx,
            CellUpdate::Conflicts { item_idx, .. } => *item_idx,
            CellUpdate::WorktreeState { item_idx, .. } => *item_idx,
            CellUpdate::UserStatus { item_idx, .. } => *item_idx,
            CellUpdate::Upstream { item_idx, .. } => *item_idx,
            CellUpdate::CiStatus { item_idx, .. } => *item_idx,
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

/// Compute status symbols for a single item (worktrees and branches).
///
/// This is idempotent and can be called multiple times as new data arrives.
/// It will recompute with the latest available data.
///
/// Branches get a subset of status symbols (no working tree, git operation, or worktree attrs).
fn compute_item_status_symbols(item: &mut ListItem, base_branch: Option<&str>) {
    // Common fields for both worktrees and branches
    let default_counts = AheadBehind::default();
    let default_upstream = UpstreamStatus::default();
    let counts = item.counts.as_ref().unwrap_or(&default_counts);
    let upstream = item.upstream.as_ref().unwrap_or(&default_upstream);
    let has_conflicts = item.has_conflicts.unwrap_or(false);
    let user_status = item.user_status.clone();

    match &item.kind {
        ItemKind::Worktree(data) => {
            // Full status computation for worktrees
            // Use base_branch directly (None for primary worktree)

            // Worktree attributes (used directly from data instead of separate Worktree)
            let mut worktree_attrs = String::new();
            if data.locked.is_some() {
                worktree_attrs.push('⊠');
            }
            if data.prunable.is_some() {
                worktree_attrs.push('⚠');
            }

            // Determine branch state (only for non-primary worktrees with base branch)
            let branch_state = if !data.is_primary && base_branch.is_some() {
                // Check for MatchesMain (requires mdiff to confirm working tree matches main's tree)
                if let Some(mdiff) = data
                    .working_tree_diff_with_main
                    .as_ref()
                    .and_then(|opt| opt.as_ref())
                {
                    if mdiff.added == 0 && mdiff.deleted == 0 && counts.ahead == 0 {
                        BranchState::MatchesMain
                    } else if counts.ahead == 0
                        && data
                            .working_tree_diff
                            .as_ref()
                            .map(|d| d.is_empty())
                            .unwrap_or(true)
                    {
                        BranchState::NoCommits
                    } else {
                        BranchState::None
                    }
                } else {
                    // mdiff is None (optimization when trees differ)
                    // Can still determine NoCommits without computing diff
                    if counts.ahead == 0
                        && data
                            .working_tree_diff
                            .as_ref()
                            .map(|d| d.is_empty())
                            .unwrap_or(true)
                    {
                        BranchState::NoCommits
                    } else {
                        BranchState::None
                    }
                }
            } else {
                BranchState::None
            };

            // Determine git operation
            let git_operation = match data.worktree_state.as_deref() {
                Some("rebase") => GitOperation::Rebase,
                Some("merge") => GitOperation::Merge,
                _ => GitOperation::None,
            };

            // Main divergence
            let main_divergence = match (counts.ahead, counts.behind) {
                (0, 0) => MainDivergence::None,
                (a, 0) if a > 0 => MainDivergence::Ahead,
                (0, b) if b > 0 => MainDivergence::Behind,
                _ => MainDivergence::Diverged,
            };

            // Upstream divergence
            let (upstream_ahead, upstream_behind) =
                upstream.active().map(|(_, a, b)| (a, b)).unwrap_or((0, 0));
            let upstream_divergence = match (upstream_ahead, upstream_behind) {
                (0, 0) => UpstreamDivergence::None,
                (a, 0) if a > 0 => UpstreamDivergence::Ahead,
                (0, b) if b > 0 => UpstreamDivergence::Behind,
                _ => UpstreamDivergence::Diverged,
            };

            item.status_symbols = Some(StatusSymbols {
                has_conflicts,
                has_potential_conflicts: false,
                branch_state,
                git_operation,
                worktree_attrs,
                main_divergence,
                upstream_divergence,
                working_tree: data
                    .working_tree_symbols
                    .as_deref()
                    .unwrap_or("")
                    .to_string(),
                user_status,
            });
        }
        ItemKind::Branch => {
            // Simplified status computation for branches
            // Only compute symbols that apply to branches (no working tree, git operation, or worktree attrs)

            // Main divergence
            let main_divergence = match (counts.ahead, counts.behind) {
                (0, 0) => MainDivergence::None,
                (a, 0) if a > 0 => MainDivergence::Ahead,
                (0, b) if b > 0 => MainDivergence::Behind,
                _ => MainDivergence::Diverged,
            };

            // Upstream divergence
            let (upstream_ahead, upstream_behind) =
                upstream.active().map(|(_, a, b)| (a, b)).unwrap_or((0, 0));
            let upstream_divergence = match (upstream_ahead, upstream_behind) {
                (0, 0) => UpstreamDivergence::None,
                (a, 0) if a > 0 => UpstreamDivergence::Ahead,
                (0, b) if b > 0 => UpstreamDivergence::Behind,
                _ => UpstreamDivergence::Diverged,
            };

            // Branch state - only compute if we have actual counts data
            // Branches without worktrees can only show NoCommits (no unique commits) or None
            let branch_state = match item.counts {
                Some(ref c) if c.ahead == 0 => BranchState::NoCommits,
                _ => BranchState::None,
            };

            item.status_symbols = Some(StatusSymbols {
                has_conflicts,
                has_potential_conflicts: false,
                branch_state,
                git_operation: GitOperation::None,
                worktree_attrs: "⎇".to_string(), // Branch indicator
                main_divergence,
                upstream_divergence,
                working_tree: String::new(),
                user_status,
            });
        }
    }
}

/// Drain cell updates from the channel and apply them to worktree_items.
///
/// This is the shared logic between progressive and buffered collection modes.
/// The `on_update` callback is called after each update is processed with the
/// item index and a reference to the updated info, allowing progressive mode
/// to update progress bars while buffered mode does nothing.
fn drain_cell_updates(
    rx: chan::Receiver<CellUpdate>,
    worktree_items: &mut [ListItem],
    mut on_update: impl FnMut(usize, &mut ListItem),
) {
    // Process cell updates as they arrive
    while let Ok(update) = rx.recv() {
        let item_idx = update.item_idx();

        match update {
            CellUpdate::CommitDetails { item_idx, commit } => {
                worktree_items[item_idx].commit = Some(commit);
            }
            CellUpdate::AheadBehind { item_idx, counts } => {
                worktree_items[item_idx].counts = Some(counts);
            }
            CellUpdate::BranchDiff {
                item_idx,
                branch_diff,
            } => {
                worktree_items[item_idx].branch_diff = Some(branch_diff);
            }
            CellUpdate::WorkingTreeDiff {
                item_idx,
                working_tree_diff,
                working_tree_diff_with_main,
                working_tree_symbols,
                is_dirty,
            } => {
                if let ItemKind::Worktree(data) = &mut worktree_items[item_idx].kind {
                    data.working_tree_diff = Some(working_tree_diff);
                    data.working_tree_diff_with_main = Some(working_tree_diff_with_main);
                    data.working_tree_symbols = Some(working_tree_symbols);
                    data.is_dirty = Some(is_dirty);
                }
            }
            CellUpdate::Conflicts {
                item_idx,
                has_conflicts,
            } => {
                worktree_items[item_idx].has_conflicts = Some(has_conflicts);
            }
            CellUpdate::WorktreeState {
                item_idx,
                worktree_state,
            } => {
                if let ItemKind::Worktree(data) = &mut worktree_items[item_idx].kind {
                    data.worktree_state = worktree_state;
                }
            }
            CellUpdate::UserStatus {
                item_idx,
                user_status,
            } => {
                worktree_items[item_idx].user_status = user_status;
            }
            CellUpdate::Upstream { item_idx, upstream } => {
                worktree_items[item_idx].upstream = Some(upstream);
            }
            CellUpdate::CiStatus {
                item_idx,
                pr_status,
            } => {
                // Wrap in Some() to indicate "loaded" (Some(None) = no CI, Some(Some(status)) = has CI)
                worktree_items[item_idx].pr_status = Some(pr_status);
            }
        }

        // Invoke rendering callback (progressive mode re-renders rows, buffered mode does nothing)
        on_update(item_idx, &mut worktree_items[item_idx]);
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
    check_conflicts: bool,
    show_progress: bool,
    render_table: bool,
) -> Result<Option<super::model::ListData>, GitError> {
    use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
    use std::time::Duration;

    let worktrees = repo.list_worktrees()?;
    if worktrees.worktrees.is_empty() {
        return Ok(None);
    }

    let primary = worktrees.worktrees[0].clone();
    let current_worktree_path = repo.worktree_root().ok();

    // Sort worktrees for display order
    let sorted_worktrees = sort_worktrees(
        &worktrees.worktrees,
        &primary,
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
            has_conflicts: None,
            user_status: None,
            status_symbols: None,
            display: super::model::DisplayFields::default(),
            // Type-specific data
            kind: super::model::ItemKind::Worktree(Box::new(
                super::model::WorktreeData::from_worktree(wt, wt.path == primary.path),
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
            has_conflicts: None,
            user_status: None,
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

    let clamp = |s: &str| -> String {
        if console::measure_text_width(s) > max_width {
            console::truncate_str(s, max_width, "…").into_owned()
        } else {
            s.to_owned()
        }
    };

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

    // Cache last rendered message per row to avoid redundant updates
    let mut last_lines: Vec<String> = vec![String::new(); all_items.len()];

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

    // Spawn worktree collection in background thread
    let sorted_worktrees_clone = sorted_worktrees.clone();
    let primary_clone = primary.clone();
    let tx_worktrees = tx.clone();
    std::thread::spawn(move || {
        sorted_worktrees_clone
            .par_iter()
            .enumerate()
            .for_each(|(idx, wt)| {
                super::collect_progressive_impl::collect_worktree_progressive(
                    wt,
                    &primary_clone,
                    idx,
                    fetch_ci,
                    check_conflicts,
                    tx_worktrees.clone(),
                );
            });
    });

    // Spawn branch collection in background thread (if requested)
    if show_branches {
        let branches_clone = branches_without_worktrees.clone();
        let primary_clone = primary.clone();
        let tx_branches = tx.clone();
        std::thread::spawn(move || {
            branches_clone
                .par_iter()
                .enumerate()
                .for_each(|(idx, (branch_name, commit_sha))| {
                    let item_idx = branch_start_idx + idx;
                    super::collect_progressive_impl::collect_branch_progressive(
                        branch_name,
                        commit_sha,
                        &primary_clone,
                        item_idx,
                        fetch_ci,
                        check_conflicts,
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
    let base_branch = primary.branch.as_deref();
    drain_cell_updates(rx, &mut all_items, |item_idx, info| {
        // Compute/recompute status symbols as data arrives (both modes)
        // This is idempotent and updates status as new data (like upstream) arrives
        // Base branch is None for primary worktree, Some(primary.branch) for others
        let item_base_branch = if info.is_primary() { None } else { base_branch };
        compute_item_status_symbols(info, item_base_branch);

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
                let rendered = layout.format_list_item_line(info, current_worktree_path.as_ref());
                let clamped = clamp(&rendered);

                // Only update if content changed
                if clamped != last_lines[item_idx] {
                    last_lines[item_idx] = clamped.clone();
                    pb.set_message(clamped);
                }
            }
        }
    });

    // Finalize progress bars: no clearing race; footer morphs into summary on TTY
    if show_progress {
        use std::io::IsTerminal;
        let is_tty = std::io::stderr().is_terminal(); // Check stderr, not stdout ✅

        // Build final summary string once using helper function
        let final_msg =
            super::format_summary_message(&all_items, show_branches, layout.hidden_nonempty_count);

        if is_tty {
            // Interactive: morph footer → summary, keep rows in place
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
    for info in &mut all_items {
        info.display = super::model::DisplayFields::from_common_fields(
            &info.counts,
            &info.branch_diff,
            &info.upstream,
            &info.pr_status,
        );

        if let super::model::ItemKind::Worktree(ref mut wt_data) = info.kind
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

/// Sort worktrees for display (primary first, then current, then by timestamp descending).
fn sort_worktrees(
    worktrees: &[Worktree],
    primary: &Worktree,
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
        let is_primary = wt.path == primary.path;
        let is_current = current_path.map(|cp| &wt.path == cp).unwrap_or(false);

        let priority = if is_primary {
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
