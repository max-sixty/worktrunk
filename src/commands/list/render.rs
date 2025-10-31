use crate::display::{format_relative_time, shorten_path, truncate_at_word_boundary};
use anstyle::{AnsiColor, Color, Style};
use worktrunk::styling::{ADDITION, CURRENT, DELETION, StyledLine};

use super::layout::{DiffWidths, LayoutConfig};
use super::model::{ListItem, WorktreeInfo};

/// Format diff values as styled segments (right-aligned with attached signs)
fn format_diff_column(
    added: usize,
    deleted: usize,
    widths: &DiffWidths,
    green: Style,
    red: Style,
) -> StyledLine {
    let mut diff_segment = StyledLine::new();

    if added > 0 || deleted > 0 {
        let added_part = format!(
            "{:>width$}",
            format!("+{}", added),
            width = 1 + widths.added_digits
        );
        let deleted_part = format!(
            "{:>width$}",
            format!("-{}", deleted),
            width = 1 + widths.deleted_digits
        );

        // Calculate the content width
        let content_width = (1 + widths.added_digits) + 1 + (1 + widths.deleted_digits);
        // Add left padding to align to total width
        let left_padding = widths.total.saturating_sub(content_width);

        if left_padding > 0 {
            diff_segment.push_raw(" ".repeat(left_padding));
        }
        diff_segment.push_styled(added_part, green);
        diff_segment.push_raw(" ");
        diff_segment.push_styled(deleted_part, red);
    } else {
        diff_segment.push_raw(" ".repeat(widths.total));
    }

    diff_segment
}

fn append_line(target: &mut StyledLine, source: StyledLine) {
    for segment in source.segments {
        target.push(segment);
    }
}

fn push_gap(line: &mut StyledLine) {
    line.push_raw("  ");
}

fn push_blank(line: &mut StyledLine, width: usize) {
    if width > 0 {
        line.push_raw(" ".repeat(width));
    }
}

fn push_diff(line: &mut StyledLine, added: usize, deleted: usize, widths: &DiffWidths) {
    append_line(
        line,
        format_diff_column(added, deleted, widths, ADDITION, DELETION),
    );
}

pub fn format_all_states(info: &WorktreeInfo) -> String {
    let mut states = Vec::new();

    if let Some(state) = info.worktree_state.as_ref() {
        states.push(format!("[{}]", state));
    }

    if info.worktree.bare {
        states.push("(bare)".to_string());
    }

    if let Some(state) = optional_reason_state("locked", info.worktree.locked.as_deref()) {
        states.push(state);
    }
    if let Some(state) = optional_reason_state("prunable", info.worktree.prunable.as_deref()) {
        states.push(state);
    }

    states.join(" ")
}

pub fn format_header_line(layout: &LayoutConfig) {
    let widths = &layout.widths;
    let dim = Style::new().dimmed();
    let mut line = StyledLine::new();

    push_header(&mut line, "Branch", widths.branch, dim);
    push_optional_header(&mut line, "Age", widths.time, dim);
    push_optional_header(&mut line, "Cmts", widths.ahead_behind, dim);
    push_optional_header(&mut line, "Cmt +/-", widths.branch_diff.total, dim);
    push_optional_header(&mut line, "WT +/-", widths.working_diff.total, dim);
    push_optional_header(&mut line, "Remote", widths.upstream, dim);
    push_header(&mut line, "Commit", 8, dim);
    push_optional_header(&mut line, "Message", widths.message, dim);
    push_optional_header(&mut line, "State", widths.states, dim);
    line.push_styled("Path", dim);

    println!("{}", line.render());
}

fn optional_reason_state(label: &str, reason: Option<&str>) -> Option<String> {
    reason.map(|value| {
        if value.is_empty() {
            format!("({label})")
        } else {
            format!("({label}: {value})")
        }
    })
}

fn push_header(line: &mut StyledLine, label: &str, width: usize, dim: Style) {
    let header = format!("{:width$}", label, width = width);
    line.push_styled(header, dim);
    line.push_raw("  ");
}

fn push_optional_header(line: &mut StyledLine, label: &str, width: usize, dim: Style) {
    if width > 0 {
        push_header(line, label, width, dim);
    }
}

/// Render a list item (worktree or branch) as a formatted line
pub fn format_list_item_line(
    item: &ListItem,
    layout: &LayoutConfig,
    current_worktree_path: Option<&std::path::PathBuf>,
) {
    let widths = &layout.widths;

    let head = item.head();
    let commit = item.commit_details();
    let counts = item.counts();
    let branch_diff = item.branch_diff().diff;
    let upstream = item.upstream();
    let worktree_info = item.worktree_info();
    let short_head = &head[..8.min(head.len())];

    // Determine styling (worktree-specific)
    let text_style = worktree_info.and_then(|info| {
        let is_current = current_worktree_path
            .map(|p| p == &info.worktree.path)
            .unwrap_or(false);
        match (is_current, info.is_primary) {
            (true, _) => Some(CURRENT),
            (_, true) => Some(Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan)))),
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
    push_gap(&mut line);

    // Age (Time)
    if widths.time > 0 {
        let time_str = format!(
            "{:width$}",
            format_relative_time(commit.timestamp),
            width = widths.time
        );
        line.push_styled(time_str, Style::new().dimmed());
        push_gap(&mut line);
    }

    // Ahead/behind (commits difference)
    if widths.ahead_behind > 0 {
        if !item.is_primary() {
            if counts.ahead > 0 || counts.behind > 0 {
                let ahead_behind_text = format!(
                    "{:width$}",
                    format!("↑{} ↓{}", counts.ahead, counts.behind),
                    width = widths.ahead_behind
                );
                line.push_styled(
                    ahead_behind_text,
                    Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow))),
                );
            } else {
                push_blank(&mut line, widths.ahead_behind);
            }
        } else {
            push_blank(&mut line, widths.ahead_behind);
        }
        push_gap(&mut line);
    }

    // Branch diff (line diff in commits)
    if widths.branch_diff.total > 0 {
        if !item.is_primary() {
            push_diff(&mut line, branch_diff.0, branch_diff.1, &widths.branch_diff);
        } else {
            push_blank(&mut line, widths.branch_diff.total);
        }
        push_gap(&mut line);
    }

    // Working tree diff (worktrees only)
    if widths.working_diff.total > 0 {
        if let Some(info) = worktree_info {
            let (wt_added, wt_deleted) = info.working_tree_diff;
            push_diff(&mut line, wt_added, wt_deleted, &widths.working_diff);
        } else {
            push_blank(&mut line, widths.working_diff.total);
        }
        push_gap(&mut line);
    }

    // Upstream tracking
    if widths.upstream > 0 {
        if let Some((remote_name, upstream_ahead, upstream_behind)) = upstream.active() {
            let mut upstream_segment = StyledLine::new();
            upstream_segment.push_styled(remote_name, Style::new().dimmed());
            upstream_segment.push_raw(" ");
            upstream_segment.push_styled(format!("↑{}", upstream_ahead), ADDITION);
            upstream_segment.push_raw(" ");
            upstream_segment.push_styled(format!("↓{}", upstream_behind), DELETION);
            upstream_segment.pad_to(widths.upstream);
            append_line(&mut line, upstream_segment);
        } else {
            push_blank(&mut line, widths.upstream);
        }
        push_gap(&mut line);
    }

    // Commit (short HEAD) - always dimmed (reference info)
    line.push_styled(short_head, Style::new().dimmed());
    push_gap(&mut line);

    // Message
    if widths.message > 0 {
        let msg = truncate_at_word_boundary(&commit.commit_message, layout.max_message_len);
        let msg_start = line.width();
        line.push_styled(msg, Style::new().dimmed());
        // Pad to correct visual width (not character count - important for unicode!)
        line.pad_to(msg_start + widths.message);
        push_gap(&mut line);
    }

    // States (worktrees only)
    if widths.states > 0 {
        if let Some(info) = worktree_info {
            let states = format_all_states(info);
            if !states.is_empty() {
                let states_text = format!("{:width$}", states, width = widths.states);
                line.push_raw(states_text);
            } else {
                push_blank(&mut line, widths.states);
            }
        } else {
            push_blank(&mut line, widths.states);
        }
        push_gap(&mut line);
    }

    // Path (worktrees only)
    if let Some(info) = worktree_info {
        let path_str = shorten_path(&info.worktree.path, &layout.common_prefix);
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
    use crate::commands::list::layout::{ColumnWidths, LayoutConfig};
    use crate::commands::list::model::{
        AheadBehind, BranchDiffTotals, CommitDetails, UpstreamStatus, WorktreeInfo,
    };
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
            commit: CommitDetails {
                timestamp: 0,
                commit_message: "Test message".to_string(),
            },
            counts: AheadBehind {
                ahead: 3,
                behind: 2,
            },
            working_tree_diff: (100, 50),
            branch_diff: BranchDiffTotals { diff: (200, 30) },
            is_primary: false,
            upstream: UpstreamStatus::from_parts(Some("origin".to_string()), 4, 0),
            worktree_state: None,
        };

        let layout = LayoutConfig {
            widths: ColumnWidths {
                branch: 11,
                time: 13,
                message: 12,
                ahead_behind: 5,
                working_diff: crate::commands::list::layout::DiffWidths {
                    total: 8,
                    added_digits: 3,
                    deleted_digits: 2,
                },
                branch_diff: crate::commands::list::layout::DiffWidths {
                    total: 8,
                    added_digits: 3,
                    deleted_digits: 2,
                },
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
            width = layout.widths.branch_diff.total
        ));
        header.push_raw("  ");
        header.push_raw(format!(
            "{:width$}",
            "WT +/-",
            width = layout.widths.working_diff.total
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
        branch_diff_segment.pad_to(layout.widths.branch_diff.total);
        for seg in branch_diff_segment.segments {
            data.push(seg);
        }
        data.push_raw("  ");
        // Working diff
        let mut working_diff_segment = StyledLine::new();
        working_diff_segment.push_raw("+100 -50");
        working_diff_segment.pad_to(layout.widths.working_diff.total);
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

    #[test]
    fn test_format_diff_column_pads_to_total_width() {
        // Test that diff column is padded to total width when content is smaller

        // Case 1: Single-digit diffs with total=6 (to fit "WT +/-" header)
        let widths = DiffWidths {
            total: 6,
            added_digits: 1,
            deleted_digits: 1,
        };
        let result = format_diff_column(1, 1, &widths, ADDITION, DELETION);
        assert_eq!(
            result.width(),
            6,
            "Diff '+1 -1' should be padded to 6 chars"
        );

        // Case 2: Two-digit diffs with total=8
        let widths = DiffWidths {
            total: 8,
            added_digits: 2,
            deleted_digits: 2,
        };
        let result = format_diff_column(10, 50, &widths, ADDITION, DELETION);
        assert_eq!(
            result.width(),
            8,
            "Diff '+10 -50' should be padded to 8 chars"
        );

        // Case 3: Asymmetric digit counts with total=9
        let widths = DiffWidths {
            total: 9,
            added_digits: 3,
            deleted_digits: 2,
        };
        let result = format_diff_column(100, 50, &widths, ADDITION, DELETION);
        assert_eq!(
            result.width(),
            9,
            "Diff '+100 -50' should be padded to 9 chars"
        );

        // Case 4: Zero diff should also pad to total width
        let widths = DiffWidths {
            total: 6,
            added_digits: 1,
            deleted_digits: 1,
        };
        let result = format_diff_column(0, 0, &widths, ADDITION, DELETION);
        assert_eq!(result.width(), 6, "Empty diff should be 6 spaces");
    }

    #[test]
    fn test_format_diff_column_right_alignment() {
        // Test that diff values are right-aligned within the total width
        let widths = DiffWidths {
            total: 6,
            added_digits: 1,
            deleted_digits: 1,
        };

        let result = format_diff_column(1, 1, &widths, ADDITION, DELETION);
        let rendered = result.render();

        // Strip ANSI codes to check alignment
        let ansi_escape = regex::Regex::new(r"\x1b\[[0-9;]*m").unwrap();
        let clean = ansi_escape.replace_all(&rendered, "");

        // Should be " +1 -1" (with leading space for right-alignment)
        assert_eq!(clean.as_ref(), " +1 -1", "Diff should be right-aligned");
    }

    #[test]
    fn test_message_padding_with_unicode() {
        use unicode_width::UnicodeWidthStr;

        // Test that messages with wide unicode characters (emojis, CJK) are padded correctly

        // Case 1: Message with emoji (☕ takes 2 visual columns but 1 character)
        let msg_with_emoji = "Fix bug with café ☕...";
        assert_eq!(
            msg_with_emoji.chars().count(),
            22,
            "Emoji message should be 22 characters"
        );
        assert_eq!(
            msg_with_emoji.width(),
            23,
            "Emoji message should have visual width 23"
        );

        let mut line = StyledLine::new();
        let msg_start = line.width(); // 0
        line.push_styled(msg_with_emoji.to_string(), Style::new().dimmed());
        line.pad_to(msg_start + 24); // Pad to width 24

        // After padding, line should have visual width 24
        assert_eq!(
            line.width(),
            24,
            "Line with emoji should be padded to visual width 24"
        );

        // The rendered output should have correct spacing
        let rendered = line.render();
        let ansi_escape = regex::Regex::new(r"\x1b\[[0-9;]*m").unwrap();
        let clean = ansi_escape.replace_all(&rendered, "");
        assert_eq!(
            clean.width(),
            24,
            "Rendered line should have visual width 24"
        );

        // Case 2: Message with only ASCII should also pad to 24
        let msg_ascii = "Add support for...";
        assert_eq!(
            msg_ascii.width(),
            18,
            "ASCII message should have visual width 18"
        );

        let mut line2 = StyledLine::new();
        let msg_start2 = line2.width();
        line2.push_styled(msg_ascii.to_string(), Style::new().dimmed());
        line2.pad_to(msg_start2 + 24);

        assert_eq!(
            line2.width(),
            24,
            "Line with ASCII should be padded to visual width 24"
        );

        // Both should have the same visual width
        assert_eq!(
            line.width(),
            line2.width(),
            "Unicode and ASCII messages should pad to same visual width"
        );
    }

    #[test]
    fn test_branch_name_padding_with_unicode() {
        use unicode_width::UnicodeWidthStr;

        // Test that branch names with unicode are padded correctly

        // Case 1: Branch with Japanese characters (each takes 2 visual columns)
        let branch_ja = "feature-日本語-test";
        // "feature-" (8) + "日本語" (6 visual, 3 chars) + "-test" (5) = 19 visual width
        assert_eq!(branch_ja.width(), 19);

        let mut line1 = StyledLine::new();
        line1.push_styled(
            branch_ja.to_string(),
            Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan))),
        );
        line1.pad_to(20); // Pad to width 20

        assert_eq!(line1.width(), 20, "Japanese branch should pad to 20");

        // Case 2: Regular ASCII branch
        let branch_ascii = "feature-test";
        assert_eq!(branch_ascii.width(), 12);

        let mut line2 = StyledLine::new();
        line2.push_styled(
            branch_ascii.to_string(),
            Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan))),
        );
        line2.pad_to(20);

        assert_eq!(line2.width(), 20, "ASCII branch should pad to 20");

        // Both should have the same visual width after padding
        assert_eq!(
            line1.width(),
            line2.width(),
            "Unicode and ASCII branches should pad to same visual width"
        );
    }
}
