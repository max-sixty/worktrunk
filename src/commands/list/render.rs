use crate::display::{format_relative_time, shorten_path, truncate_at_word_boundary};
use worktrunk::styling::{
    StyledLine, addition_style, current_style, deletion_style, dim_style, neutral_style,
    primary_style,
};

use super::layout::LayoutConfig;
use super::{ListItem, WorktreeInfo};

pub fn format_all_states(info: &WorktreeInfo) -> String {
    let mut states = Vec::new();

    // Worktree state (merge/rebase/etc)
    if let Some(ref state) = info.worktree_state {
        states.push(format!("[{}]", state));
    }

    // Don't show detached state if branch is None (already shown in branch column)
    if info.worktree.detached && info.worktree.branch.is_some() {
        states.push("(detached)".to_string());
    }
    if info.worktree.bare {
        states.push("(bare)".to_string());
    }
    if let Some(ref reason) = info.worktree.locked {
        if reason.is_empty() {
            states.push("(locked)".to_string());
        } else {
            states.push(format!("(locked: {})", reason));
        }
    }
    if let Some(ref reason) = info.worktree.prunable {
        if reason.is_empty() {
            states.push("(prunable)".to_string());
        } else {
            states.push(format!("(prunable: {})", reason));
        }
    }

    states.join(" ")
}

pub fn format_header_line(layout: &LayoutConfig) {
    let widths = &layout.widths;
    let mut line = StyledLine::new();
    let dim = dim_style();

    // Branch
    let header = format!("{:width$}", "Branch", width = widths.branch);
    line.push_styled(header, dim);
    line.push_raw("  ");

    // Age (Time)
    if widths.time > 0 {
        let header = format!("{:width$}", "Age", width = widths.time);
        line.push_styled(header, dim);
        line.push_raw("  ");
    }

    // Ahead/behind (commits)
    if widths.ahead_behind > 0 {
        let header = format!("{:width$}", "Cmts", width = widths.ahead_behind);
        line.push_styled(header, dim);
        line.push_raw("  ");
    }

    // Branch diff (line diff in commits)
    if widths.branch_diff > 0 {
        let header = format!("{:width$}", "Cmt +/-", width = widths.branch_diff);
        line.push_styled(header, dim);
        line.push_raw("  ");
    }

    // Working tree diff
    if widths.working_diff > 0 {
        let header = format!("{:width$}", "WT +/-", width = widths.working_diff);
        line.push_styled(header, dim);
        line.push_raw("  ");
    }

    // Upstream
    if widths.upstream > 0 {
        let header = format!("{:width$}", "Remote", width = widths.upstream);
        line.push_styled(header, dim);
        line.push_raw("  ");
    }

    // Commit (fixed width: 8 chars)
    line.push_styled("Commit  ", dim);
    line.push_raw("  ");

    // Message
    if widths.message > 0 {
        let header = format!("{:width$}", "Message", width = widths.message);
        line.push_styled(header, dim);
        line.push_raw("  ");
    }

    // States
    if widths.states > 0 {
        let header = format!("{:width$}", "State", width = widths.states);
        line.push_styled(header, dim);
        line.push_raw("  ");
    }

    // Path
    line.push_styled("Path", dim);

    println!("{}", line.render());
}

/// Render a list item (worktree or branch) as a formatted line
pub fn format_list_item_line(
    item: &ListItem,
    layout: &LayoutConfig,
    current_worktree_path: Option<&std::path::PathBuf>,
) {
    format_item_line(item, layout, current_worktree_path)
}

fn format_item_line(
    item: &ListItem,
    layout: &LayoutConfig,
    current_worktree_path: Option<&std::path::PathBuf>,
) {
    let widths = &layout.widths;
    let primary = primary_style();
    let current = current_style();
    let green = addition_style();
    let red = deletion_style();
    let yellow = neutral_style();
    let dim = dim_style();

    let short_head = &item.commit_head()[..8.min(item.commit_head().len())];

    // Determine styling (worktree-specific)
    let text_style = item.worktree_info().and_then(|info| {
        let is_current = current_worktree_path
            .map(|p| p == &info.worktree.path)
            .unwrap_or(false);
        match (is_current, item.is_primary()) {
            (true, _) => Some(current),
            (_, true) => Some(primary),
            _ => None,
        }
    });

    // Start building the line
    let mut line = StyledLine::new();

    // Branch name
    let branch_text = format!("{:width$}", item.branch_name(), width = widths.branch);
    if let Some(style) = text_style {
        line.push_styled(branch_text, style);
    } else {
        line.push_raw(branch_text);
    }
    line.push_raw("  ");

    // Age (Time)
    if widths.time > 0 {
        let time_str = format!(
            "{:width$}",
            format_relative_time(item.timestamp()),
            width = widths.time
        );
        line.push_styled(time_str, dim);
        line.push_raw("  ");
    }

    // Ahead/behind (commits difference)
    if widths.ahead_behind > 0 {
        if !item.is_primary() && (item.ahead() > 0 || item.behind() > 0) {
            let ahead_behind_text = format!(
                "{:width$}",
                format!("↑{} ↓{}", item.ahead(), item.behind()),
                width = widths.ahead_behind
            );
            line.push_styled(ahead_behind_text, yellow);
        } else {
            line.push_raw(" ".repeat(widths.ahead_behind));
        }
        line.push_raw("  ");
    }

    // Branch diff (line diff in commits)
    if widths.branch_diff > 0 {
        if !item.is_primary() {
            let (br_added, br_deleted) = item.branch_diff();
            if br_added > 0 || br_deleted > 0 {
                let mut diff_segment = StyledLine::new();
                diff_segment.push_styled(format!("+{}", br_added), green);
                diff_segment.push_raw(" ");
                diff_segment.push_styled(format!("-{}", br_deleted), red);
                diff_segment.pad_to(widths.branch_diff);
                for segment in diff_segment.segments {
                    line.push(segment);
                }
            } else {
                line.push_raw(" ".repeat(widths.branch_diff));
            }
        } else {
            line.push_raw(" ".repeat(widths.branch_diff));
        }
        line.push_raw("  ");
    }

    // Working tree diff (worktrees only)
    if widths.working_diff > 0 {
        if let Some((wt_added, wt_deleted)) = item.working_tree_diff() {
            if wt_added > 0 || wt_deleted > 0 {
                let mut diff_segment = StyledLine::new();
                diff_segment.push_styled(format!("+{}", wt_added), green);
                diff_segment.push_raw(" ");
                diff_segment.push_styled(format!("-{}", wt_deleted), red);
                diff_segment.pad_to(widths.working_diff);
                for segment in diff_segment.segments {
                    line.push(segment);
                }
            } else {
                line.push_raw(" ".repeat(widths.working_diff));
            }
        } else {
            line.push_raw(" ".repeat(widths.working_diff));
        }
        line.push_raw("  ");
    }

    // Upstream tracking
    if widths.upstream > 0 {
        if let Some((remote_name, upstream_ahead, upstream_behind)) = item.upstream_info() {
            let mut upstream_segment = StyledLine::new();
            upstream_segment.push_styled(remote_name, dim);
            upstream_segment.push_raw(" ");
            upstream_segment.push_styled(format!("↑{}", upstream_ahead), green);
            upstream_segment.push_raw(" ");
            upstream_segment.push_styled(format!("↓{}", upstream_behind), red);
            upstream_segment.pad_to(widths.upstream);
            for segment in upstream_segment.segments {
                line.push(segment);
            }
        } else {
            line.push_raw(" ".repeat(widths.upstream));
        }
        line.push_raw("  ");
    }

    // Commit (short HEAD)
    if let Some(style) = text_style {
        line.push_styled(short_head, style);
    } else {
        line.push_styled(short_head, dim);
    }
    line.push_raw("  ");

    // Message
    if widths.message > 0 {
        let msg = format!(
            "{:width$}",
            truncate_at_word_boundary(item.commit_message(), layout.max_message_len),
            width = widths.message
        );
        line.push_styled(msg, dim);
        line.push_raw("  ");
    }

    // States (worktrees only)
    if widths.states > 0 {
        if let Some(worktree_info) = item.worktree_info() {
            let states = format_all_states(worktree_info);
            if !states.is_empty() {
                let states_text = format!("{:width$}", states, width = widths.states);
                line.push_raw(states_text);
            } else {
                line.push_raw(" ".repeat(widths.states));
            }
        } else {
            line.push_raw(" ".repeat(widths.states));
        }
        line.push_raw("  ");
    }

    // Path (worktrees only)
    if let Some(worktree_info) = item.worktree_info() {
        let path_str = shorten_path(&worktree_info.worktree.path, &layout.common_prefix);
        if let Some(style) = text_style {
            line.push_styled(path_str, style);
        } else {
            line.push_raw(path_str);
        }
    }

    println!("{}", line.render());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::list::WorktreeInfo;
    use crate::commands::list::layout::{ColumnWidths, LayoutConfig};
    use crate::display::shorten_path;
    use std::path::PathBuf;
    use worktrunk::styling::StyledLine;

    #[test]
    fn test_column_alignment_with_all_columns() {
        // Create test data with all columns populated
        let info = WorktreeInfo {
            worktree: worktrunk::git::Worktree {
                path: PathBuf::from("/test/path"),
                head: "abc12345".to_string(),
                branch: Some("test-branch".to_string()),
                bare: false,
                detached: false,
                locked: Some("test lck".to_string()), // "(locked: test lck)" = 18 chars
                prunable: None,
            },
            timestamp: 0,
            commit_message: "Test message".to_string(),
            ahead: 3,
            behind: 2,
            working_tree_diff: (100, 50),
            branch_diff: (200, 30),
            is_primary: false,
            upstream_remote: Some("origin".to_string()),
            upstream_ahead: 4,
            upstream_behind: 0,
            worktree_state: None,
        };

        let layout = LayoutConfig {
            widths: ColumnWidths {
                branch: 11,
                time: 13,
                message: 12,
                ahead_behind: 5,
                working_diff: 8,
                branch_diff: 8,
                upstream: 12,
                states: 18,
            },
            common_prefix: PathBuf::from("/test"),
            max_message_len: 12,
        };

        // Build header line manually (mimicking format_header_line logic)
        let mut header = StyledLine::new();
        header.push_raw(format!("{:width$}", "Branch", width = layout.widths.branch));
        header.push_raw("  ");
        header.push_raw(format!("{:width$}", "Age", width = layout.widths.time));
        header.push_raw("  ");
        header.push_raw(format!(
            "{:width$}",
            "Cmts",
            width = layout.widths.ahead_behind
        ));
        header.push_raw("  ");
        header.push_raw(format!(
            "{:width$}",
            "Cmt +/-",
            width = layout.widths.branch_diff
        ));
        header.push_raw("  ");
        header.push_raw(format!(
            "{:width$}",
            "WT +/-",
            width = layout.widths.working_diff
        ));
        header.push_raw("  ");
        header.push_raw(format!(
            "{:width$}",
            "Remote",
            width = layout.widths.upstream
        ));
        header.push_raw("  ");
        header.push_raw("Commit  ");
        header.push_raw("  ");
        header.push_raw(format!(
            "{:width$}",
            "Message",
            width = layout.widths.message
        ));
        header.push_raw("  ");
        header.push_raw(format!("{:width$}", "State", width = layout.widths.states));
        header.push_raw("  ");
        header.push_raw("Path");

        // Build data line manually (mimicking format_worktree_line logic)
        let mut data = StyledLine::new();
        data.push_raw(format!(
            "{:width$}",
            "test-branch",
            width = layout.widths.branch
        ));
        data.push_raw("  ");
        data.push_raw(format!(
            "{:width$}",
            "9 months ago",
            width = layout.widths.time
        ));
        data.push_raw("  ");
        // Ahead/behind
        let ahead_behind_text = format!("{:width$}", "↑3 ↓2", width = layout.widths.ahead_behind);
        data.push_raw(ahead_behind_text);
        data.push_raw("  ");
        // Branch diff
        let mut branch_diff_segment = StyledLine::new();
        branch_diff_segment.push_raw("+200 -30");
        branch_diff_segment.pad_to(layout.widths.branch_diff);
        for seg in branch_diff_segment.segments {
            data.push(seg);
        }
        data.push_raw("  ");
        // Working diff
        let mut working_diff_segment = StyledLine::new();
        working_diff_segment.push_raw("+100 -50");
        working_diff_segment.pad_to(layout.widths.working_diff);
        for seg in working_diff_segment.segments {
            data.push(seg);
        }
        data.push_raw("  ");
        // Upstream
        let mut upstream_segment = StyledLine::new();
        upstream_segment.push_raw("origin ↑4 ↓0");
        upstream_segment.pad_to(layout.widths.upstream);
        for seg in upstream_segment.segments {
            data.push(seg);
        }
        data.push_raw("  ");
        // Commit (fixed 8 chars)
        data.push_raw("abc12345");
        data.push_raw("  ");
        // Message
        data.push_raw(format!(
            "{:width$}",
            "Test message",
            width = layout.widths.message
        ));
        data.push_raw("  ");
        // State
        let states = format_all_states(&info);
        data.push_raw(format!("{:width$}", states, width = layout.widths.states));
        data.push_raw("  ");
        // Path
        data.push_raw(shorten_path(&info.worktree.path, &layout.common_prefix));

        // Verify both lines have columns at the same positions
        // We'll check this by verifying specific column start positions
        let header_str = header.render();
        let data_str = data.render();

        // Remove ANSI codes for position checking (our test data doesn't have styles anyway)
        assert!(header_str.contains("Branch"));
        assert!(data_str.contains("test-branch"));

        // The key test: both lines should have the same visual width up to "Path" column
        // (Path is variable width, so we only check up to there)
        let header_width_without_path = header.width() - "Path".len();
        let data_width_without_path =
            data.width() - shorten_path(&info.worktree.path, &layout.common_prefix).len();

        assert_eq!(
            header_width_without_path, data_width_without_path,
            "Header and data rows should have same width before Path column"
        );
    }
}
