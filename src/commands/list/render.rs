use crate::display::{format_relative_time, shorten_path, truncate_at_word_boundary};
use anstyle::{AnsiColor, Color, Style};
use worktrunk::styling::{ADDITION, CURRENT, DELETION, StyledLine, println};

use super::ci_status::{CiStatus, PrStatus};
use super::columns::{ColumnKind, DiffVariant};
use super::layout::{ColumnFormat, DiffDigits, LayoutConfig};
use super::model::ListItem;

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

#[derive(Clone, Copy)]
enum ValueAlign {
    Left,
    Right,
}

#[derive(Clone, Copy)]
struct DiffRenderConfig {
    positive_symbol: &'static str,
    negative_symbol: &'static str,
    align: ValueAlign,
}

fn diff_render_config(variant: DiffVariant) -> DiffRenderConfig {
    match variant {
        DiffVariant::Signs => DiffRenderConfig {
            positive_symbol: "+",
            negative_symbol: "-",
            align: ValueAlign::Right,
        },
        DiffVariant::Arrows => DiffRenderConfig {
            positive_symbol: "↑",
            negative_symbol: "↓",
            align: ValueAlign::Left,
        },
    }
}

struct DiffColumnConfig {
    digits: DiffDigits,
    total_width: usize,
    variant: DiffVariant,
    positive_style: Style,
    negative_style: Style,
    always_show_zeros: bool,
}

fn format_diff_like_column(
    positive: usize,
    negative: usize,
    config: DiffColumnConfig,
) -> StyledLine {
    let render_config = diff_render_config(config.variant);
    let mut segment = StyledLine::new();

    if positive == 0 && negative == 0 && !config.always_show_zeros {
        segment.push_raw(" ".repeat(config.total_width));
        return segment;
    }

    let positive_width = 1 + config.digits.added;
    let negative_width = 1 + config.digits.deleted;
    let content_width = positive_width + 1 + negative_width;
    let extra_padding = config.total_width.saturating_sub(content_width);

    if matches!(render_config.align, ValueAlign::Right) && extra_padding > 0 {
        segment.push_raw(" ".repeat(extra_padding));
    }

    if positive > 0 || (positive == 0 && config.always_show_zeros) {
        let value = format!("{}{}", render_config.positive_symbol, positive);
        let formatted = match render_config.align {
            ValueAlign::Right => format!("{:>width$}", value, width = positive_width),
            ValueAlign::Left => format!("{:<width$}", value, width = positive_width),
        };
        segment.push_styled(formatted, config.positive_style);
    } else {
        segment.push_raw(" ".repeat(positive_width));
    }

    segment.push_raw(" ");

    if negative > 0 || (negative == 0 && config.always_show_zeros) {
        let value = format!("{}{}", render_config.negative_symbol, negative);
        let formatted = match render_config.align {
            ValueAlign::Right => format!("{:>width$}", value, width = negative_width),
            ValueAlign::Left => format!("{:<width$}", value, width = negative_width),
        };
        segment.push_styled(formatted, config.negative_style);
    } else {
        segment.push_raw(" ".repeat(negative_width));
    }

    if matches!(render_config.align, ValueAlign::Left) && extra_padding > 0 {
        segment.pad_to(segment.width() + extra_padding);
    }

    if segment.width() < config.total_width {
        segment.pad_to(config.total_width);
    }

    segment
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

/// Format CI status indicator using the statusline.sh color scheme
fn format_ci_status(pr_status: &PrStatus) -> StyledLine {
    let mut segment = StyledLine::new();
    let style = ci_status_style(pr_status);
    segment.push_styled("●".to_string(), style);
    segment
}

pub fn format_all_states(item: &ListItem) -> String {
    let mut states = Vec::new();

    // Add conflicts state (applies to both worktrees and branches)
    if item.has_conflicts() {
        states.push("(conflicts)".to_string());
    }

    // Worktree-specific states
    if let Some(info) = item.worktree_info() {
        // State priority: "matches main" takes precedence over "no commits"
        // since it's more specific (working tree identical to main)
        if !info.is_primary && info.working_tree_diff_with_main == Some((0, 0)) {
            states.push("(matches main)".to_string());
        } else if !info.is_primary && item.counts().ahead == 0 && info.working_tree_diff == (0, 0) {
            // Only show "no commits" if working tree doesn't match main
            states.push("(no commits)".to_string());
        }

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
    }

    states.join(" ")
}

pub fn format_header_line(layout: &LayoutConfig) {
    let style = Style::new();
    let mut line = StyledLine::new();

    for (i, column) in layout.columns.iter().enumerate() {
        line.pad_to(column.start);
        let is_last = i == layout.columns.len() - 1;
        let header = if is_last {
            column.header.to_string()
        } else {
            format!("{:width$}", column.header, width = column.width)
        };
        line.push_styled(header, style);
    }

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

/// Check if a branch/worktree is potentially removable
///
/// Dims rows for worktrees/branches that likely don't contain unique work
/// beyond what's already in the main branch. This helps focus attention on
/// worktrees that contain work.
///
/// Dims when (using OR logic):
/// - No commits AND clean working tree (ahead == 0 AND working_tree_diff == (0, 0)):
///   The worktree has no commits ahead and no uncommitted changes
/// - Working tree matches main (working_tree_diff_with_main == Some((0, 0))):
///   The working tree contents are identical to main, regardless of commit history
///
/// Either condition alone is sufficient to dim, as both indicate "no unique work here".
fn is_potentially_removable(item: &ListItem) -> bool {
    if item.is_primary() {
        return false;
    }

    let counts = item.counts();

    if let Some(info) = item.worktree_info() {
        // Condition 1: No commits ahead AND no uncommitted changes
        let no_commits_and_clean = counts.ahead == 0 && info.working_tree_diff == (0, 0);

        // Condition 2: Working tree matches main (regardless of commit history)
        let matches_main = info.working_tree_diff_with_main == Some((0, 0));

        no_commits_and_clean || matches_main
    } else {
        // For branches without worktrees, just check if no commits ahead
        counts.ahead == 0
    }
}

/// Render a list item (worktree or branch) as a formatted line
pub fn format_list_item_line(
    item: &ListItem,
    layout: &LayoutConfig,
    current_worktree_path: Option<&std::path::PathBuf>,
) {
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

    // Override styling if removable (dim the row, preserving existing color)
    let text_style = if removable {
        Some(text_style.unwrap_or_default().dimmed())
    } else {
        text_style
    };

    let mut line = StyledLine::new();
    let num_columns = layout.columns.len();
    for (i, column) in layout.columns.iter().enumerate() {
        line.pad_to(column.start);
        let is_last = i == num_columns - 1;

        match (column.kind, column.format) {
            (ColumnKind::Branch, _) => {
                let branch_text = if is_last {
                    item.branch_name().to_string()
                } else {
                    format!("{:width$}", item.branch_name(), width = column.width)
                };
                if let Some(style) = text_style {
                    line.push_styled(branch_text, style);
                } else {
                    line.push_raw(branch_text);
                }
            }
            (ColumnKind::Status, _) => {
                if let Some(info) = worktree_info {
                    let status_text = if is_last {
                        info.status_symbols.clone()
                    } else {
                        format!("{:width$}", info.status_symbols, width = column.width)
                    };
                    if let Some(style) = text_style {
                        line.push_styled(status_text, style);
                    } else {
                        line.push_raw(status_text);
                    }
                } else if !is_last {
                    push_blank(&mut line, column.width);
                }
            }
            (ColumnKind::WorkingDiff, ColumnFormat::Diff { digits, variant }) => {
                if let Some(info) = worktree_info {
                    let (wt_added, wt_deleted) = info.working_tree_diff;
                    let segment = format_diff_like_column(
                        wt_added,
                        wt_deleted,
                        DiffColumnConfig {
                            digits,
                            total_width: column.width,
                            variant,
                            positive_style: ADDITION,
                            negative_style: DELETION,
                            always_show_zeros: false,
                        },
                    );
                    append_line(&mut line, segment);
                } else if !is_last {
                    push_blank(&mut line, column.width);
                }
            }
            (ColumnKind::AheadBehind, ColumnFormat::Diff { digits, variant }) => {
                if !item.is_primary() && (counts.ahead > 0 || counts.behind > 0) {
                    let dim_deletion = DELETION.dimmed();
                    let segment = format_diff_like_column(
                        counts.ahead,
                        counts.behind,
                        DiffColumnConfig {
                            digits,
                            total_width: column.width,
                            variant,
                            positive_style: ADDITION,
                            negative_style: dim_deletion,
                            always_show_zeros: false,
                        },
                    );
                    append_line(&mut line, segment);
                } else if !is_last {
                    push_blank(&mut line, column.width);
                }
            }
            (ColumnKind::BranchDiff, ColumnFormat::Diff { digits, variant }) => {
                if !item.is_primary() {
                    let segment = format_diff_like_column(
                        branch_diff.0,
                        branch_diff.1,
                        DiffColumnConfig {
                            digits,
                            total_width: column.width,
                            variant,
                            positive_style: ADDITION,
                            negative_style: DELETION,
                            always_show_zeros: false,
                        },
                    );
                    append_line(&mut line, segment);
                } else if !is_last {
                    push_blank(&mut line, column.width);
                }
            }
            (ColumnKind::States, _) => {
                let states = format_all_states(item);
                if !states.is_empty() {
                    let states_text = if is_last {
                        states
                    } else {
                        format!("{:width$}", states, width = column.width)
                    };
                    if let Some(style) = text_style {
                        line.push_styled(states_text, style);
                    } else {
                        line.push_raw(states_text);
                    }
                } else if !is_last {
                    push_blank(&mut line, column.width);
                }
            }
            (ColumnKind::Path, _) => {
                if let Some(info) = worktree_info {
                    let path_str = shorten_path(&info.worktree.path, &layout.common_prefix);
                    let path_text = if is_last {
                        path_str
                    } else {
                        format!("{:width$}", path_str, width = column.width)
                    };
                    if let Some(style) = text_style {
                        line.push_styled(path_text, style);
                    } else {
                        line.push_raw(path_text);
                    }
                } else if !is_last {
                    push_blank(&mut line, column.width);
                }
            }
            (ColumnKind::Upstream, ColumnFormat::Diff { digits, variant }) => {
                if let Some((_remote_name, upstream_ahead, upstream_behind)) = upstream.active() {
                    let dim_deletion = DELETION.dimmed();
                    let segment = format_diff_like_column(
                        upstream_ahead,
                        upstream_behind,
                        DiffColumnConfig {
                            digits,
                            total_width: column.width,
                            variant,
                            positive_style: ADDITION,
                            negative_style: dim_deletion,
                            always_show_zeros: true,
                        },
                    );
                    append_line(&mut line, segment);
                } else if !is_last {
                    push_blank(&mut line, column.width);
                }
            }
            (ColumnKind::Time, _) => {
                let time_str = if is_last {
                    format_relative_time(commit.timestamp)
                } else {
                    format!(
                        "{:width$}",
                        format_relative_time(commit.timestamp),
                        width = column.width
                    )
                };
                line.push_styled(time_str, Style::new().dimmed());
            }
            (ColumnKind::CiStatus, _) => {
                if let Some(pr_status) = item.pr_status() {
                    let mut ci_segment = format_ci_status(pr_status);
                    if !is_last {
                        ci_segment.pad_to(column.width);
                    }
                    append_line(&mut line, ci_segment);
                } else if !is_last {
                    push_blank(&mut line, column.width);
                }
            }
            (ColumnKind::Commit, _) => {
                let commit_text = if is_last {
                    short_head.to_string()
                } else {
                    format!("{:width$}", short_head, width = column.width)
                };
                line.push_styled(commit_text, Style::new().dimmed());
            }
            (ColumnKind::Message, _) => {
                let msg = truncate_at_word_boundary(&commit.commit_message, layout.max_message_len);
                let msg_start = line.width();
                line.push_styled(msg, Style::new().dimmed());
                if !is_last {
                    line.pad_to(msg_start + column.width);
                }
            }
            // Fallback for diff columns when format is unexpectedly Text
            (_, _) => {
                if !is_last {
                    push_blank(&mut line, column.width);
                }
            }
        }
    }

    println!("{}", line.render());
}

#[cfg(test)]
mod tests {
    use super::*;
    use worktrunk::styling::StyledLine;

    #[test]
    fn test_format_diff_column_pads_to_total_width() {
        use super::super::columns::DiffVariant;

        // Case 1: Single-digit diffs with total=6 (to fit "WT +/-" header)
        let digits = DiffDigits {
            added: 1,
            deleted: 1,
        };
        let total = 6;
        let result = format_diff_like_column(
            1,
            1,
            DiffColumnConfig {
                digits,
                total_width: total,
                variant: DiffVariant::Signs,
                positive_style: ADDITION,
                negative_style: DELETION,
                always_show_zeros: false,
            },
        );
        assert_eq!(
            result.width(),
            total,
            "Diff '+1 -1' should be padded to 6 chars"
        );

        // Case 2: Two-digit diffs with total=8
        let digits = DiffDigits {
            added: 2,
            deleted: 2,
        };
        let total = 8;
        let result = format_diff_like_column(
            10,
            50,
            DiffColumnConfig {
                digits,
                total_width: total,
                variant: DiffVariant::Signs,
                positive_style: ADDITION,
                negative_style: DELETION,
                always_show_zeros: false,
            },
        );
        assert_eq!(
            result.width(),
            total,
            "Diff '+10 -50' should be padded to 8 chars"
        );

        // Case 3: Asymmetric digit counts with total=9
        let digits = DiffDigits {
            added: 3,
            deleted: 2,
        };
        let total = 9;
        let result = format_diff_like_column(
            100,
            50,
            DiffColumnConfig {
                digits,
                total_width: total,
                variant: DiffVariant::Signs,
                positive_style: ADDITION,
                negative_style: DELETION,
                always_show_zeros: false,
            },
        );
        assert_eq!(
            result.width(),
            total,
            "Diff '+100 -50' should be padded to 9 chars"
        );

        // Case 4: Zero diff should also pad to total width
        let digits = DiffDigits {
            added: 1,
            deleted: 1,
        };
        let total = 6;
        let result = format_diff_like_column(
            0,
            0,
            DiffColumnConfig {
                digits,
                total_width: total,
                variant: DiffVariant::Signs,
                positive_style: ADDITION,
                negative_style: DELETION,
                always_show_zeros: false,
            },
        );
        assert_eq!(result.width(), total, "Empty diff should be 6 spaces");
    }

    #[test]
    fn test_format_diff_column_right_alignment() {
        // Test that diff values are right-aligned within the total width
        use super::super::columns::DiffVariant;

        let digits = DiffDigits {
            added: 1,
            deleted: 1,
        };
        let total = 6;

        let result = format_diff_like_column(
            1,
            1,
            DiffColumnConfig {
                digits,
                total_width: total,
                variant: DiffVariant::Signs,
                positive_style: ADDITION,
                negative_style: DELETION,
                always_show_zeros: false,
            },
        );
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
    fn test_arrow_variant_alignment_invariant() {
        use super::super::columns::DiffVariant;
        use worktrunk::styling::{ADDITION, DELETION};

        let digits = DiffDigits {
            added: 2,
            deleted: 2,
        };
        let total = 7;

        let dim_deletion = DELETION.dimmed();
        let cases = [(0, 0), (1, 0), (0, 1), (1, 1), (99, 99), (5, 44)];

        for (ahead, behind) in cases {
            let result = format_diff_like_column(
                ahead,
                behind,
                DiffColumnConfig {
                    digits,
                    total_width: total,
                    variant: DiffVariant::Arrows,
                    positive_style: ADDITION,
                    negative_style: dim_deletion,
                    always_show_zeros: false,
                },
            );
            assert_eq!(result.width(), total);
        }
    }

    #[test]
    fn test_arrow_variant_respects_header_width() {
        use super::super::columns::DiffVariant;
        use worktrunk::styling::{ADDITION, DELETION};

        let digits = DiffDigits {
            added: 0,
            deleted: 2,
        };
        let total = 7;

        let dim_deletion = DELETION.dimmed();

        let empty = format_diff_like_column(
            0,
            0,
            DiffColumnConfig {
                digits,
                total_width: total,
                variant: DiffVariant::Arrows,
                positive_style: ADDITION,
                negative_style: dim_deletion,
                always_show_zeros: false,
            },
        );
        assert_eq!(empty.width(), total);

        let behind_only = format_diff_like_column(
            0,
            50,
            DiffColumnConfig {
                digits,
                total_width: total,
                variant: DiffVariant::Arrows,
                positive_style: ADDITION,
                negative_style: dim_deletion,
                always_show_zeros: false,
            },
        );
        assert_eq!(behind_only.width(), total);
    }

    #[test]
    fn test_always_show_zeros_renders_zero_values() {
        use super::super::columns::DiffVariant;
        use worktrunk::styling::{ADDITION, DELETION};

        let digits = DiffDigits {
            added: 1,
            deleted: 1,
        };
        let total = 7;

        let dim_deletion = DELETION.dimmed();

        // With always_show_zeros=false, (0, 0) renders as blank
        let without = format_diff_like_column(
            0,
            0,
            DiffColumnConfig {
                digits,
                total_width: total,
                variant: DiffVariant::Arrows,
                positive_style: ADDITION,
                negative_style: dim_deletion,
                always_show_zeros: false,
            },
        );
        assert_eq!(without.width(), total);
        let rendered_without = without.render();
        let ansi_escape = regex::Regex::new(r"\x1b\[[0-9;]*m").unwrap();
        let clean_without = ansi_escape.replace_all(&rendered_without, "");
        assert_eq!(clean_without.as_ref(), "       ", "Should render as blank");

        // With always_show_zeros=true, (0, 0) renders as "↑0 ↓0"
        let with = format_diff_like_column(
            0,
            0,
            DiffColumnConfig {
                digits,
                total_width: total,
                variant: DiffVariant::Arrows,
                positive_style: ADDITION,
                negative_style: dim_deletion,
                always_show_zeros: true,
            },
        );
        assert_eq!(with.width(), total);
        let rendered_with = with.render();
        let clean_with = ansi_escape.replace_all(&rendered_with, "");
        assert_eq!(
            clean_with.as_ref(),
            "↑0 ↓0  ",
            "Should render ↑0 ↓0 with padding"
        );
    }
}
