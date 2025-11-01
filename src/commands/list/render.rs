use crate::display::{format_relative_time, shorten_path, truncate_at_word_boundary};
use anstyle::{AnsiColor, Color, Style};
use worktrunk::styling::{ADDITION, CURRENT, DELETION, StyledLine, println};

use super::ci_status::{CiStatus, PrStatus};
use super::layout::{DiffWidths, LayoutConfig};
use super::model::{ListItem, WorktreeInfo};

/// Format ahead/behind counts as plain text with ANSI colors (for json-pretty)
pub fn format_ahead_behind_plain(ahead: usize, behind: usize) -> Option<String> {
    match (ahead, behind) {
        (0, 0) => None,
        (a, 0) => Some(format!("{}↑{}{}", ADDITION, a, ADDITION.render_reset())),
        (0, b) => {
            let dim_deletion = DELETION.dimmed();
            Some(format!(
                "{}↓{}{}",
                dim_deletion,
                b,
                dim_deletion.render_reset()
            ))
        }
        (a, b) => {
            let dim_deletion = DELETION.dimmed();
            Some(format!(
                "{}↑{}{} {}↓{}{}",
                ADDITION,
                a,
                ADDITION.render_reset(),
                dim_deletion,
                b,
                dim_deletion.render_reset()
            ))
        }
    }
}

/// Format diff values as plain text with ANSI colors (for json-pretty)
pub fn format_diff_plain(added: usize, deleted: usize) -> Option<String> {
    match (added, deleted) {
        (0, 0) => None,
        (a, 0) => Some(format!("{}+{}{}", ADDITION, a, ADDITION.render_reset())),
        (0, d) => Some(format!("{}-{}{}", DELETION, d, DELETION.render_reset())),
        (a, d) => Some(format!(
            "{}+{}{} {}-{}{}",
            ADDITION,
            a,
            ADDITION.render_reset(),
            DELETION,
            d,
            DELETION.render_reset()
        )),
    }
}

/// Determine the style for a CI status (color + optional dimming)
fn ci_status_style(pr_status: &PrStatus) -> Style {
    let color = match pr_status.ci_status {
        CiStatus::Passed => AnsiColor::Green,
        CiStatus::Running => AnsiColor::Blue,
        CiStatus::Failed => AnsiColor::Red,
        CiStatus::Conflicts => AnsiColor::Yellow,
        CiStatus::NoCI => AnsiColor::BrightBlack,
    };

    if pr_status.is_stale {
        Style::new().fg_color(Some(Color::Ansi(color))).dimmed()
    } else {
        Style::new().fg_color(Some(Color::Ansi(color)))
    }
}

/// Format CI status as plain text with ANSI colors (for json-pretty)
pub fn format_ci_status_plain(pr_status: &PrStatus) -> String {
    let style = ci_status_style(pr_status);

    let status_str = match pr_status.ci_status {
        CiStatus::Passed => "passed",
        CiStatus::Running => "running",
        CiStatus::Failed => "failed",
        CiStatus::Conflicts => "conflicts",
        CiStatus::NoCI => "no-ci",
    };

    format!("{}● {}{}", style, status_str, style.render_reset())
}

/// Format arrow-based counts (e.g., "↑6 ↓1") with alignment
/// Down arrows always appear at the same column position by reserving space for ahead part
fn format_arrow_column(
    ahead: usize,
    behind: usize,
    widths: &DiffWidths,
    green: Style,
    red: Style,
) -> StyledLine {
    let mut segment = StyledLine::new();

    if ahead > 0 || behind > 0 {
        // Always reserve full width for ahead part (↑ + max_digits)
        if ahead > 0 {
            let ahead_str = format!("↑{}", ahead);
            segment.push_styled(&ahead_str, green);
            // Pad ahead part to fixed width
            let ahead_padding = widths.added_digits.saturating_sub(ahead.to_string().len());
            if ahead_padding > 0 {
                segment.push_raw(" ".repeat(ahead_padding));
            }
        } else {
            // Reserve full space when ahead is zero
            segment.push_raw(" ".repeat(1 + widths.added_digits));
        }

        // Always add separator space
        segment.push_raw(" ");

        // Always reserve full width for behind part (↓ + max_digits)
        if behind > 0 {
            let behind_str = format!("↓{}", behind);
            segment.push_styled(&behind_str, red);
            // Pad behind part to fixed width
            let behind_padding = widths
                .deleted_digits
                .saturating_sub(behind.to_string().len());
            if behind_padding > 0 {
                segment.push_raw(" ".repeat(behind_padding));
            }
        } else {
            // Reserve full space when behind is zero
            segment.push_raw(" ".repeat(1 + widths.deleted_digits));
        }

        // Pad to total width if header is wider than data
        // (e.g., "Commits" header = 7, but data "↓50" = 5)
        segment.pad_to(widths.total);
    } else {
        segment.push_raw(" ".repeat(widths.total));
    }

    segment
}

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
        // Always maintain full column width for alignment
        // Format: [padding] [+nnn] [ ] [-nnn]
        let content_width = (1 + widths.added_digits) + 1 + (1 + widths.deleted_digits);
        let left_padding = widths.total.saturating_sub(content_width);

        if left_padding > 0 {
            diff_segment.push_raw(" ".repeat(left_padding));
        }

        // Added part: show value or spaces
        if added > 0 {
            let added_part = format!(
                "{:>width$}",
                format!("+{}", added),
                width = 1 + widths.added_digits
            );
            diff_segment.push_styled(added_part, green);
        } else {
            // Blank space to maintain alignment
            diff_segment.push_raw(" ".repeat(1 + widths.added_digits));
        }

        // Space between added and deleted
        diff_segment.push_raw(" ");

        // Deleted part: show value or spaces
        if deleted > 0 {
            let deleted_part = format!(
                "{:>width$}",
                format!("-{}", deleted),
                width = 1 + widths.deleted_digits
            );
            diff_segment.push_styled(deleted_part, red);
        } else {
            // Blank space to maintain alignment
            diff_segment.push_raw(" ".repeat(1 + widths.deleted_digits));
        }
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

/// Format CI status indicator using the statusline.sh color scheme
fn format_ci_status(pr_status: &PrStatus) -> StyledLine {
    let mut segment = StyledLine::new();
    let style = ci_status_style(pr_status);
    segment.push_styled("●".to_string(), style);
    segment
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
    let positions = &layout.positions;
    let dim = Style::new().dimmed();
    let mut line = StyledLine::new();

    // Use absolute positions for guaranteed alignment
    push_header_at(&mut line, "Branch", widths.branch, positions.branch, dim);
    push_header_at(
        &mut line,
        "Working ±",
        widths.working_diff.total,
        positions.working_diff,
        dim,
    );
    push_header_at(
        &mut line,
        "Main ↕",
        widths.ahead_behind.total,
        positions.ahead_behind,
        dim,
    );
    push_header_at(
        &mut line,
        "Main ±",
        widths.branch_diff.total,
        positions.branch_diff,
        dim,
    );
    push_header_at(&mut line, "⚠️", widths.conflicts, positions.conflicts, dim);
    push_header_at(&mut line, "State", widths.states, positions.states, dim);
    push_header_at(&mut line, "Path", widths.path, positions.path, dim);
    push_header_at(
        &mut line,
        "Remote ↕",
        widths.upstream.total,
        positions.upstream,
        dim,
    );
    push_header_at(&mut line, "Age", widths.time, positions.time, dim);
    push_header_at(&mut line, "CI", widths.ci_status, positions.ci_status, dim);
    push_header_at(&mut line, "Commit", widths.commit, positions.commit, dim);
    push_header_at(&mut line, "Message", widths.message, positions.message, dim);

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

/// Push a header at an absolute column position
fn push_header_at(line: &mut StyledLine, label: &str, width: usize, position: usize, dim: Style) {
    if width > 0 {
        // Pad to absolute position
        line.pad_to(position);
        // Add header content padded to width
        let header = format!("{:width$}", label, width = width);
        line.push_styled(header, dim);
    }
}

/// Check if a branch is potentially removable (nothing ahead, no uncommitted changes)
fn is_potentially_removable(item: &ListItem) -> bool {
    let counts = item.counts();
    let wt_diff = item
        .worktree_info()
        .map(|info| info.working_tree_diff)
        .unwrap_or((0, 0));

    !item.is_primary() && counts.ahead == 0 && wt_diff == (0, 0)
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

    // Check if branch is potentially removable
    let removable = is_potentially_removable(item);

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

    // Override styling if removable (dim the row)
    let text_style = if removable {
        Some(Style::new().dimmed())
    } else {
        text_style
    };

    // Start building the line using absolute column positions
    let mut line = StyledLine::new();
    let positions = &layout.positions;

    // Branch name (dimmed if removable)
    if widths.branch > 0 {
        line.pad_to(positions.branch);
        let branch_text = format!("{:width$}", item.branch_name(), width = widths.branch);
        if let Some(style) = text_style {
            line.push_styled(branch_text, style);
        } else {
            line.push_raw(branch_text);
        }
    }

    // Working tree diff (worktrees only)
    if widths.working_diff.total > 0 {
        line.pad_to(positions.working_diff);
        if let Some(info) = worktree_info {
            let (wt_added, wt_deleted) = info.working_tree_diff;
            push_diff(&mut line, wt_added, wt_deleted, &widths.working_diff);
        } else {
            push_blank(&mut line, widths.working_diff.total);
        }
    }

    // Ahead/behind (commits difference) - green ahead, dim red behind
    if widths.ahead_behind.total > 0 {
        line.pad_to(positions.ahead_behind);
        if !item.is_primary() && (counts.ahead > 0 || counts.behind > 0) {
            let dim_deletion = DELETION.dimmed();
            append_line(
                &mut line,
                format_arrow_column(
                    counts.ahead,
                    counts.behind,
                    &widths.ahead_behind,
                    ADDITION,
                    dim_deletion,
                ),
            );
        } else {
            push_blank(&mut line, widths.ahead_behind.total);
        }
    }

    // Branch diff (line diff in commits)
    if widths.branch_diff.total > 0 {
        line.pad_to(positions.branch_diff);
        if !item.is_primary() {
            push_diff(&mut line, branch_diff.0, branch_diff.1, &widths.branch_diff);
        } else {
            push_blank(&mut line, widths.branch_diff.total);
        }
    }

    // Conflicts indicator
    if widths.conflicts > 0 {
        line.pad_to(positions.conflicts);
        if item.has_conflicts() {
            line.push_styled(
                "⚠️".to_string(),
                Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow))),
            );
        } else {
            push_blank(&mut line, widths.conflicts);
        }
    }

    // States (worktrees only)
    if widths.states > 0 {
        line.pad_to(positions.states);
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
    }

    // Path (worktrees only)
    if widths.path > 0 {
        line.pad_to(positions.path);
        if let Some(info) = worktree_info {
            let path_str = shorten_path(&info.worktree.path, &layout.common_prefix);
            let path_text = format!("{:width$}", path_str, width = widths.path);
            if let Some(style) = text_style {
                line.push_styled(path_text, style);
            } else {
                line.push_raw(path_text);
            }
        } else {
            push_blank(&mut line, widths.path);
        }
    }

    // Upstream tracking
    if widths.upstream.total > 0 {
        line.pad_to(positions.upstream);
        if let Some((_remote_name, upstream_ahead, upstream_behind)) = upstream.active() {
            let dim_deletion = DELETION.dimmed();
            // TODO: Handle show_remote_names when implemented
            append_line(
                &mut line,
                format_arrow_column(
                    upstream_ahead,
                    upstream_behind,
                    &widths.upstream,
                    ADDITION,
                    dim_deletion,
                ),
            );
        } else {
            push_blank(&mut line, widths.upstream.total);
        }
    }

    // Age (Time)
    if widths.time > 0 {
        line.pad_to(positions.time);
        let time_str = format!(
            "{:width$}",
            format_relative_time(commit.timestamp),
            width = widths.time
        );
        line.push_styled(time_str, Style::new().dimmed());
    }

    // CI status
    if widths.ci_status > 0 {
        line.pad_to(positions.ci_status);
        if let Some(pr_status) = item.pr_status() {
            let mut ci_segment = format_ci_status(pr_status);
            ci_segment.pad_to(widths.ci_status);
            append_line(&mut line, ci_segment);
        } else {
            push_blank(&mut line, widths.ci_status);
        }
    }

    // Commit (short HEAD) - always dimmed (reference info)
    if widths.commit > 0 {
        line.pad_to(positions.commit);
        line.push_styled(short_head, Style::new().dimmed());
    }

    // Message
    if widths.message > 0 {
        line.pad_to(positions.message);
        let msg = truncate_at_word_boundary(&commit.commit_message, layout.max_message_len);
        let msg_start = line.width();
        line.push_styled(msg, Style::new().dimmed());
        // Pad to correct visual width (not character count - important for unicode!)
        line.pad_to(msg_start + widths.message);
    }

    println!("{}", line.render());
}

#[cfg(test)]
mod tests {
    use super::*;
    use worktrunk::styling::StyledLine;

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

    #[test]
    fn test_arrow_column_alignment_invariant() {
        // Test that arrow columns maintain consistent width regardless of values
        // This ensures down arrows always appear at the same horizontal position
        use super::super::layout::DiffWidths;
        use super::format_arrow_column;
        use worktrunk::styling::{ADDITION, DELETION};

        let widths = DiffWidths {
            total: 7, // "↑99 ↓99" = 1+2+1+1+2 = 7
            added_digits: 2,
            deleted_digits: 2,
        };

        let dim_deletion = DELETION.dimmed();

        // All these cases should produce identical width (vertical alignment)
        let test_cases = vec![
            (0, 0, "both zero"),
            (1, 0, "only ahead"),
            (0, 1, "only behind"),
            (1, 1, "both single digit"),
            (99, 99, "both max digits"),
            (5, 44, "mixed digits"),
        ];

        for (ahead, behind, description) in test_cases {
            let result = format_arrow_column(ahead, behind, &widths, ADDITION, dim_deletion);
            assert_eq!(
                result.width(),
                7,
                "Arrow column ({ahead}, {behind}) [{description}] should always be width 7"
            );
        }
    }

    #[test]
    fn test_arrow_column_with_header_wider_than_data() {
        // Reproduces the actual bug: when header is wider than data
        // - No ahead values (max_ahead_digits = 0)
        // - Max behind is 50 (max_behind_digits = 2)
        // - data_width = 1 + 0 + 1 + 1 + 2 = 5
        // - header "Commits" = 7
        // - total = max(5, 7) = 7
        use super::super::layout::DiffWidths;
        use super::format_arrow_column;
        use worktrunk::styling::{ADDITION, DELETION};

        let widths = DiffWidths {
            total: 7,          // To fit "Commits" header
            added_digits: 0,   // No ahead values
            deleted_digits: 2, // Max behind is 50
        };

        let dim_deletion = DELETION.dimmed();

        // Empty column should be 7 spaces
        let empty = format_arrow_column(0, 0, &widths, ADDITION, dim_deletion);
        assert_eq!(empty.width(), 7, "Empty column should be 7 spaces");

        // Column with only behind should also be 7!
        let behind_only = format_arrow_column(0, 50, &widths, ADDITION, dim_deletion);
        assert_eq!(
            behind_only.width(),
            7,
            "Column with behind=50 should be 7 chars (currently {} - THIS IS THE BUG)",
            behind_only.width()
        );
    }
}
