use crate::display::{find_common_prefix, get_terminal_width};
use std::path::{Path, PathBuf};
use unicode_width::UnicodeWidthStr;

use super::ListItem;

/// Helper: Try to allocate space for a column. Returns the allocated width if successful.
/// Updates `remaining` by subtracting the allocated width + spacing.
fn try_allocate(remaining: &mut usize, ideal_width: usize, spacing: usize) -> usize {
    if ideal_width == 0 || *remaining < ideal_width + spacing {
        return 0;
    }
    *remaining = remaining.saturating_sub(ideal_width + spacing);
    ideal_width
}

/// Width information for diff columns (e.g., "+128 -147")
#[derive(Clone, Copy)]
pub struct DiffWidths {
    pub total: usize,
    pub added_digits: usize,
    pub deleted_digits: usize,
}

impl DiffWidths {
    pub fn zero() -> Self {
        Self {
            total: 0,
            added_digits: 0,
            deleted_digits: 0,
        }
    }
}

pub struct ColumnWidths {
    pub branch: usize,
    pub time: usize,
    pub message: usize,
    pub ahead_behind: usize,
    pub working_diff: DiffWidths,
    pub branch_diff: DiffWidths,
    pub upstream: usize,
    pub states: usize,
}

pub struct LayoutConfig {
    pub widths: ColumnWidths,
    pub common_prefix: PathBuf,
    pub max_message_len: usize,
}

pub fn calculate_column_widths(items: &[ListItem]) -> ColumnWidths {
    // Initialize with header label widths to ensure headers always fit
    let mut max_branch = "Branch".width();
    let mut max_time = "Age".width();
    let mut max_message = "Message".width();
    let mut max_ahead_behind = "Cmts".width();
    let mut max_upstream = "Remote".width();
    let mut max_states = "State".width();

    // Track diff component widths separately
    let mut max_wt_added_digits = 0;
    let mut max_wt_deleted_digits = 0;
    let mut max_br_added_digits = 0;
    let mut max_br_deleted_digits = 0;

    for item in items {
        let (commit, counts, branch_diff, upstream, worktree_info) = match item {
            ListItem::Worktree(info) => (
                &info.commit,
                &info.counts,
                info.branch_diff.diff,
                &info.upstream,
                Some(info),
            ),
            ListItem::Branch(info) => (
                &info.commit,
                &info.counts,
                info.branch_diff.diff,
                &info.upstream,
                None,
            ),
        };

        // Branch name
        max_branch = max_branch.max(item.branch_name().width());

        // Time
        let time_str = crate::display::format_relative_time(commit.timestamp);
        max_time = max_time.max(time_str.width());

        // Message (truncate to 50 chars max)
        let msg_len = commit.commit_message.chars().take(50).count();
        max_message = max_message.max(msg_len);

        // Ahead/behind (only for non-primary items)
        if !item.is_primary() && (counts.ahead > 0 || counts.behind > 0) {
            let ahead_behind_len = format!("↑{} ↓{}", counts.ahead, counts.behind).width();
            max_ahead_behind = max_ahead_behind.max(ahead_behind_len);
        }

        // Working tree diff (worktrees only) - track digits separately
        if let Some(info) = worktree_info
            && (info.working_tree_diff.0 > 0 || info.working_tree_diff.1 > 0)
        {
            max_wt_added_digits =
                max_wt_added_digits.max(info.working_tree_diff.0.to_string().len());
            max_wt_deleted_digits =
                max_wt_deleted_digits.max(info.working_tree_diff.1.to_string().len());
        }

        // Branch diff (only for non-primary items) - track digits separately
        if !item.is_primary() && (branch_diff.0 > 0 || branch_diff.1 > 0) {
            max_br_added_digits = max_br_added_digits.max(branch_diff.0.to_string().len());
            max_br_deleted_digits = max_br_deleted_digits.max(branch_diff.1.to_string().len());
        }

        // Upstream tracking
        if let Some((remote_name, upstream_ahead, upstream_behind)) = upstream.active() {
            let upstream_len =
                format!("{} ↑{} ↓{}", remote_name, upstream_ahead, upstream_behind).width();
            max_upstream = max_upstream.max(upstream_len);
        }

        // States (worktrees only)
        if let Some(info) = worktree_info {
            let states = super::render::format_all_states(info);
            if !states.is_empty() {
                max_states = max_states.max(states.width());
            }
        }
    }

    // Calculate diff widths: "+{added} -{deleted}"
    // Format: "+" + digits + " " + "-" + digits
    let working_diff_total = if max_wt_added_digits > 0 || max_wt_deleted_digits > 0 {
        let data_width = 1 + max_wt_added_digits + 1 + 1 + max_wt_deleted_digits;
        data_width.max("WT +/-".width()) // Ensure header fits if we have data
    } else {
        0 // No data, no column
    };
    let branch_diff_total = if max_br_added_digits > 0 || max_br_deleted_digits > 0 {
        let data_width = 1 + max_br_added_digits + 1 + 1 + max_br_deleted_digits;
        data_width.max("Cmt +/-".width()) // Ensure header fits if we have data
    } else {
        0 // No data, no column
    };

    // Reset sparse column widths to 0 if they're still at header width (no data found)
    let header_ahead_behind = "Cmts".width();
    let header_upstream = "Remote".width();
    let header_states = "State".width();

    let final_ahead_behind = if max_ahead_behind == header_ahead_behind {
        0 // No data found
    } else {
        max_ahead_behind
    };

    let final_upstream = if max_upstream == header_upstream {
        0 // No data found
    } else {
        max_upstream
    };

    let final_states = if max_states == header_states {
        0 // No data found
    } else {
        max_states
    };

    ColumnWidths {
        branch: max_branch,
        time: max_time,
        message: max_message,
        ahead_behind: final_ahead_behind,
        working_diff: DiffWidths {
            total: working_diff_total,
            added_digits: max_wt_added_digits,
            deleted_digits: max_wt_deleted_digits,
        },
        branch_diff: DiffWidths {
            total: branch_diff_total,
            added_digits: max_br_added_digits,
            deleted_digits: max_br_deleted_digits,
        },
        upstream: final_upstream,
        states: final_states,
    }
}

/// Calculate responsive layout based on terminal width
pub fn calculate_responsive_layout(items: &[ListItem]) -> LayoutConfig {
    let terminal_width = get_terminal_width();
    let paths: Vec<&Path> = items
        .iter()
        .filter_map(|item| match item {
            ListItem::Worktree(info) => Some(info.worktree.path.as_path()),
            ListItem::Branch(_) => None,
        })
        .collect();
    let common_prefix = find_common_prefix(&paths);

    // Calculate ideal column widths
    let ideal_widths = calculate_column_widths(items);

    // Calculate actual maximum path width (after common prefix removal)
    let max_path_width = items
        .iter()
        .filter_map(|item| match item {
            ListItem::Worktree(info) => Some(info),
            ListItem::Branch(_) => None,
        })
        .map(|info| {
            use crate::display::shorten_path;
            use unicode_width::UnicodeWidthStr;
            shorten_path(&info.worktree.path, &common_prefix).width()
        })
        .max()
        .unwrap_or(20); // fallback to 20 if no paths

    // Essential columns (always shown):
    // - branch: variable
    // - short HEAD: 8 chars
    // - path: variable (calculated above)
    // - spacing: 2 chars between columns

    let spacing = 2;
    let short_head = 8;

    // Calculate base width needed
    let base_width = ideal_widths.branch + spacing + short_head + spacing + max_path_width;

    // Available width for optional columns
    let available = terminal_width.saturating_sub(base_width);

    // Priority order for columns (from high to low):
    // 1. time (15-20 chars)
    // 2. message (20-50 chars, flexible)
    // 3. ahead_behind - commits difference
    // 4. working_diff - line diff in working tree
    // 5. branch_diff - line diff in commits
    // 6. upstream
    // 7. states
    //
    // Each column is shown if it has any data (ideal_width > 0) and fits in remaining space.

    let mut remaining = available;
    let mut widths = ColumnWidths {
        branch: ideal_widths.branch,
        time: 0,
        message: 0,
        ahead_behind: 0,
        working_diff: DiffWidths::zero(),
        branch_diff: DiffWidths::zero(),
        upstream: 0,
        states: 0,
    };

    // Time column (high priority, ~15 chars)
    widths.time = try_allocate(&mut remaining, ideal_widths.time, spacing);

    // Message column (flexible, 20-50 chars)
    let max_message_len = if remaining >= 50 + spacing {
        remaining = remaining.saturating_sub(50 + spacing);
        50
    } else if remaining >= 30 + spacing {
        let msg_len = remaining.saturating_sub(spacing).min(ideal_widths.message);
        remaining = remaining.saturating_sub(msg_len + spacing);
        msg_len
    } else if remaining >= 20 + spacing {
        let msg_len = 20;
        remaining = remaining.saturating_sub(msg_len + spacing);
        msg_len
    } else {
        0
    };

    if max_message_len > 0 {
        widths.message = max_message_len.min(ideal_widths.message);
    }

    // Ahead/behind column (if it has data and fits)
    widths.ahead_behind = try_allocate(&mut remaining, ideal_widths.ahead_behind, spacing);

    // Working diff column (if it has data and fits)
    let allocated_width = try_allocate(&mut remaining, ideal_widths.working_diff.total, spacing);
    if allocated_width > 0 {
        widths.working_diff = ideal_widths.working_diff;
    }

    // Branch diff column (if it has data and fits)
    let allocated_width = try_allocate(&mut remaining, ideal_widths.branch_diff.total, spacing);
    if allocated_width > 0 {
        widths.branch_diff = ideal_widths.branch_diff;
    }

    // Upstream column (if it has data and fits)
    widths.upstream = try_allocate(&mut remaining, ideal_widths.upstream, spacing);

    // States column (if it has data and fits)
    widths.states = try_allocate(&mut remaining, ideal_widths.states, spacing);

    // Expand message column with any leftover space (up to 100 chars total)
    let final_max_message_len = if widths.message > 0 && remaining > 0 {
        let max_expansion = 100_usize.saturating_sub(max_message_len);
        let expansion = remaining.saturating_sub(spacing).min(max_expansion);
        let new_len = max_message_len + expansion;
        let allocated_len = new_len.min(ideal_widths.message);
        widths.message = allocated_len;
        allocated_len // Return the actual allocated width, not new_len
    } else {
        max_message_len
    };

    LayoutConfig {
        widths,
        common_prefix,
        max_message_len: final_max_message_len,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_column_width_calculation_with_unicode() {
        use crate::commands::list::{
            AheadBehind, BranchDiffTotals, CommitDetails, UpstreamStatus, WorktreeInfo,
        };

        let info1 = WorktreeInfo {
            worktree: worktrunk::git::Worktree {
                path: PathBuf::from("/test"),
                head: "abc123".to_string(),
                branch: Some("main".to_string()),
                bare: false,
                detached: false,
                locked: None,
                prunable: None,
            },
            commit: CommitDetails {
                timestamp: 0,
                commit_message: "Test".to_string(),
            },
            counts: AheadBehind {
                ahead: 3,
                behind: 2,
            },
            working_tree_diff: (100, 50),
            branch_diff: BranchDiffTotals { diff: (200, 30) },
            is_primary: false,
            upstream: UpstreamStatus {
                remote: Some("origin".to_string()),
                ahead: 4,
                behind: 0,
            },
            worktree_state: None,
        };

        let widths = calculate_column_widths(&[super::ListItem::Worktree(info1)]);

        // "↑3 ↓2" has visual width 5 (not 9 bytes)
        assert_eq!(widths.ahead_behind, 5, "↑3 ↓2 should have width 5");

        // "+100 -50" has width 8
        assert_eq!(widths.working_diff.total, 8, "+100 -50 should have width 8");
        assert_eq!(widths.working_diff.added_digits, 3, "100 has 3 digits");
        assert_eq!(widths.working_diff.deleted_digits, 2, "50 has 2 digits");

        // "+200 -30" has width 8
        assert_eq!(widths.branch_diff.total, 8, "+200 -30 should have width 8");
        assert_eq!(widths.branch_diff.added_digits, 3, "200 has 3 digits");
        assert_eq!(widths.branch_diff.deleted_digits, 2, "30 has 2 digits");

        // "origin ↑4 ↓0" has visual width 12 (not more due to Unicode arrows)
        assert_eq!(widths.upstream, 12, "origin ↑4 ↓0 should have width 12");
    }
}
