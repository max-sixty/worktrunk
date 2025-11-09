use crate::display::{format_relative_time, shorten_path, truncate_at_word_boundary};
use anstyle::{AnsiColor, Color, Style};
use std::path::Path;
use worktrunk::styling::{CURRENT, StyledLine, println};

use super::ci_status::{CiStatus, PrStatus};
use super::columns::{ColumnKind, DiffVariant};
use super::layout::{
    ColumnFormat, ColumnLayout, DiffColumnConfig, DiffDisplayConfig, LayoutConfig,
};
use super::model::{
    AheadBehind, CommitDetails, ListItem, PositionMask, UpstreamStatus, WorktreeInfo,
};

fn format_plain_diff_from_config(
    positive: usize,
    negative: usize,
    config: DiffDisplayConfig,
) -> Option<String> {
    if !config.always_show_zeros && positive == 0 && negative == 0 {
        return None;
    }

    let (positive_symbol, negative_symbol) = match config.variant {
        DiffVariant::Signs => ("+", "-"),
        DiffVariant::Arrows => ("↑", "↓"),
    };

    let mut parts = Vec::with_capacity(2);

    if positive > 0 || config.always_show_zeros {
        parts.push(format!(
            "{}{}{}{}",
            config.positive_style,
            positive_symbol,
            positive,
            config.positive_style.render_reset()
        ));
    }

    if negative > 0 || config.always_show_zeros {
        parts.push(format!(
            "{}{}{}{}",
            config.negative_style,
            negative_symbol,
            negative,
            config.negative_style.render_reset()
        ));
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

impl ColumnKind {
    /// Format diff-style values as plain text with ANSI colors (for json-pretty).
    pub(crate) fn format_diff_plain(self, positive: usize, negative: usize) -> Option<String> {
        let config = self.diff_display_config()?;

        format_plain_diff_from_config(positive, negative, config)
    }
}

impl PrStatus {
    /// Determine the style for a CI status (color + optional dimming)
    fn style(&self) -> Style {
        let color = match self.ci_status {
            CiStatus::Passed => AnsiColor::Green,
            CiStatus::Running => AnsiColor::Blue,
            CiStatus::Failed => AnsiColor::Red,
            CiStatus::Conflicts => AnsiColor::Yellow,
            CiStatus::NoCI => AnsiColor::BrightBlack,
        };

        if self.is_stale {
            Style::new().fg_color(Some(Color::Ansi(color))).dimmed()
        } else {
            Style::new().fg_color(Some(Color::Ansi(color)))
        }
    }

    /// Format CI status as plain text with ANSI colors (for json-pretty)
    pub fn format_plain(&self) -> String {
        let style = self.style();

        let status_str = match self.ci_status {
            CiStatus::Passed => "passed",
            CiStatus::Running => "running",
            CiStatus::Failed => "failed",
            CiStatus::Conflicts => "conflicts",
            CiStatus::NoCI => "no-ci",
        };

        format!("{}● {}{}", style, status_str, style.render_reset())
    }

    fn render_indicator(&self) -> StyledLine {
        let mut segment = StyledLine::new();
        segment.push_styled("●".to_string(), self.style());
        segment
    }
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

fn format_diff_like_column(
    positive: usize,
    negative: usize,
    config: DiffColumnConfig,
) -> StyledLine {
    let render_config = diff_render_config(config.display.variant);
    let mut segment = StyledLine::new();

    if positive == 0 && negative == 0 && !config.display.always_show_zeros {
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

    if positive > 0 || (positive == 0 && config.display.always_show_zeros) {
        let value = format!("{}{}", render_config.positive_symbol, positive);
        let formatted = match render_config.align {
            ValueAlign::Right => format!("{:>width$}", value, width = positive_width),
            ValueAlign::Left => format!("{:<width$}", value, width = positive_width),
        };
        segment.push_styled(formatted, config.display.positive_style);
    } else {
        segment.push_raw(" ".repeat(positive_width));
    }

    segment.push_raw(" ");

    if negative > 0 || (negative == 0 && config.display.always_show_zeros) {
        let value = format!("{}{}", render_config.negative_symbol, negative);
        let formatted = match render_config.align {
            ValueAlign::Right => format!("{:>width$}", value, width = negative_width),
            ValueAlign::Left => format!("{:<width$}", value, width = negative_width),
        };
        segment.push_styled(formatted, config.display.negative_style);
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

impl LayoutConfig {
    fn render_line<F>(&self, mut render_cell: F) -> StyledLine
    where
        F: FnMut(&ColumnLayout) -> StyledLine,
    {
        let mut line = StyledLine::new();
        if self.columns.is_empty() {
            return line;
        }

        let last_index = self.columns.len() - 1;

        for (index, column) in self.columns.iter().enumerate() {
            line.pad_to(column.start);
            line.extend(render_cell(column));

            if index != last_index {
                line.pad_to(column.start + column.width);
            }
        }

        line
    }

    pub fn format_header_line(&self) {
        let style = Style::new().bold();
        let line = self.render_line(|column| {
            let mut cell = StyledLine::new();
            if !column.header.is_empty() {
                cell.push_styled(column.header.to_string(), style);
            }
            cell
        });

        println!("{}", line.render());
    }

    pub fn format_list_item_line(
        &self,
        item: &ListItem,
        current_worktree_path: Option<&std::path::PathBuf>,
    ) {
        let ctx = ListRowContext::new(item, current_worktree_path);
        let line = self.render_line(|column| {
            column.render_cell(
                &ctx,
                &self.status_position_mask,
                self.max_git_symbols_width,
                &self.common_prefix,
                self.max_message_len,
            )
        });

        println!("{}", line.render());
    }
}

struct ListRowContext<'a> {
    item: &'a ListItem,
    worktree_info: Option<&'a WorktreeInfo>,
    counts: &'a AheadBehind,
    branch_diff: (usize, usize),
    upstream: &'a UpstreamStatus,
    commit: &'a CommitDetails,
    head: &'a str,
    text_style: Option<Style>,
}

impl<'a> ListRowContext<'a> {
    fn new(item: &'a ListItem, current_worktree_path: Option<&std::path::PathBuf>) -> Self {
        let worktree_info = item.worktree_info();
        let counts = item.counts();
        let commit = item.commit_details();
        let branch_diff = item.branch_diff().diff;
        let upstream = item.upstream();
        let head = item.head();

        let mut ctx = Self {
            item,
            worktree_info,
            counts,
            branch_diff,
            upstream,
            commit,
            head,
            text_style: None,
        };

        ctx.text_style = ctx.compute_text_style(current_worktree_path);
        ctx
    }

    fn short_head(&self) -> &str {
        &self.head[..8.min(self.head.len())]
    }

    fn compute_text_style(
        &self,
        current_worktree_path: Option<&std::path::PathBuf>,
    ) -> Option<Style> {
        let base_style = self.worktree_info.and_then(|info| {
            let is_current = current_worktree_path
                .map(|p| p == &info.worktree.path)
                .unwrap_or(false);
            match (is_current, info.is_primary) {
                (true, _) => Some(CURRENT),
                (_, true) => Some(Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan)))),
                _ => None,
            }
        });

        if self.item.is_potentially_removable() {
            Some(base_style.unwrap_or_default().dimmed())
        } else {
            base_style
        }
    }
}

impl ColumnLayout {
    fn render_diff_cell(&self, positive: usize, negative: usize) -> StyledLine {
        let ColumnFormat::Diff(config) = self.format else {
            return StyledLine::new();
        };

        debug_assert_eq!(config.total_width, self.width);

        format_diff_like_column(positive, negative, config)
    }

    fn render_cell(
        &self,
        ctx: &ListRowContext,
        status_mask: &PositionMask,
        max_git_symbols_width: usize,
        common_prefix: &Path,
        max_message_len: usize,
    ) -> StyledLine {
        match self.kind {
            ColumnKind::Branch => {
                let mut cell = StyledLine::new();
                let text = ctx.item.branch_name().to_string();
                if let Some(style) = ctx.text_style {
                    cell.push_styled(text, style);
                } else {
                    cell.push_raw(text);
                }
                cell
            }
            ColumnKind::Status => {
                let mut cell = StyledLine::new();

                if let Some(info) = ctx.worktree_info {
                    let git_symbols_start = cell.width();
                    cell.push_raw(info.status_symbols.render_with_mask(status_mask));

                    if let Some(ref user_status) = info.user_status {
                        cell.pad_to(git_symbols_start + max_git_symbols_width);
                        cell.push_raw(user_status.clone());
                    }
                } else if let ListItem::Branch(branch_info) = ctx.item
                    && let Some(ref user_status) = branch_info.user_status
                {
                    let git_symbols_start = cell.width();
                    cell.pad_to(git_symbols_start + max_git_symbols_width);
                    cell.push_raw(user_status.clone());
                }

                cell
            }
            ColumnKind::WorkingDiff => {
                let Some((added, deleted)) = ctx.worktree_info.map(|info| info.working_tree_diff)
                else {
                    return StyledLine::new();
                };
                self.render_diff_cell(added, deleted)
            }
            ColumnKind::AheadBehind => {
                if ctx.item.is_primary() {
                    return StyledLine::new();
                }
                let ahead = ctx.counts.ahead;
                let behind = ctx.counts.behind;
                if ahead == 0 && behind == 0 {
                    return StyledLine::new();
                }
                self.render_diff_cell(ahead, behind)
            }
            ColumnKind::BranchDiff => {
                if ctx.item.is_primary() {
                    return StyledLine::new();
                }
                self.render_diff_cell(ctx.branch_diff.0, ctx.branch_diff.1)
            }
            ColumnKind::Path => {
                let Some(info) = ctx.worktree_info else {
                    return StyledLine::new();
                };
                let mut cell = StyledLine::new();
                let path_str = shorten_path(&info.worktree.path, common_prefix);
                if let Some(style) = ctx.text_style {
                    cell.push_styled(path_str, style);
                } else {
                    cell.push_raw(path_str);
                }
                cell
            }
            ColumnKind::Upstream => {
                let Some((_, ahead, behind)) = ctx.upstream.active() else {
                    return StyledLine::new();
                };
                self.render_diff_cell(ahead, behind)
            }
            ColumnKind::Time => {
                let mut cell = StyledLine::new();
                let time_str = format_relative_time(ctx.commit.timestamp);
                cell.push_styled(time_str, Style::new().dimmed());
                cell
            }
            ColumnKind::CiStatus => {
                let Some(pr_status) = ctx.item.pr_status() else {
                    return StyledLine::new();
                };
                pr_status.render_indicator()
            }
            ColumnKind::Commit => {
                let mut cell = StyledLine::new();
                cell.push_styled(ctx.short_head().to_string(), Style::new().dimmed());
                cell
            }
            ColumnKind::Message => {
                let mut cell = StyledLine::new();
                let msg = truncate_at_word_boundary(&ctx.commit.commit_message, max_message_len);
                cell.push_styled(msg, Style::new().dimmed());
                cell
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::list::layout::{DiffDigits, DiffDisplayConfig};
    use worktrunk::styling::{ADDITION, DELETION, StyledLine};

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
                display: DiffDisplayConfig {
                    variant: DiffVariant::Signs,
                    positive_style: ADDITION,
                    negative_style: DELETION,
                    always_show_zeros: false,
                },
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
                display: DiffDisplayConfig {
                    variant: DiffVariant::Signs,
                    positive_style: ADDITION,
                    negative_style: DELETION,
                    always_show_zeros: false,
                },
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
                display: DiffDisplayConfig {
                    variant: DiffVariant::Signs,
                    positive_style: ADDITION,
                    negative_style: DELETION,
                    always_show_zeros: false,
                },
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
                display: DiffDisplayConfig {
                    variant: DiffVariant::Signs,
                    positive_style: ADDITION,
                    negative_style: DELETION,
                    always_show_zeros: false,
                },
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
                display: DiffDisplayConfig {
                    variant: DiffVariant::Signs,
                    positive_style: ADDITION,
                    negative_style: DELETION,
                    always_show_zeros: false,
                },
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
                    display: DiffDisplayConfig {
                        variant: DiffVariant::Arrows,
                        positive_style: ADDITION,
                        negative_style: dim_deletion,
                        always_show_zeros: false,
                    },
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
                display: DiffDisplayConfig {
                    variant: DiffVariant::Arrows,
                    positive_style: ADDITION,
                    negative_style: dim_deletion,
                    always_show_zeros: false,
                },
            },
        );
        assert_eq!(empty.width(), total);

        let behind_only = format_diff_like_column(
            0,
            50,
            DiffColumnConfig {
                digits,
                total_width: total,
                display: DiffDisplayConfig {
                    variant: DiffVariant::Arrows,
                    positive_style: ADDITION,
                    negative_style: dim_deletion,
                    always_show_zeros: false,
                },
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
                display: DiffDisplayConfig {
                    variant: DiffVariant::Arrows,
                    positive_style: ADDITION,
                    negative_style: dim_deletion,
                    always_show_zeros: false,
                },
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
                display: DiffDisplayConfig {
                    variant: DiffVariant::Arrows,
                    positive_style: ADDITION,
                    negative_style: dim_deletion,
                    always_show_zeros: true,
                },
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

    #[test]
    fn test_status_column_padding_with_emoji() {
        use unicode_width::UnicodeWidthStr;

        // Test that status column with emoji is padded correctly using visual width
        // This reproduces the issue where "↑🤖" was misaligned

        // Case 1: Status with emoji (↑ is 1 column, 🤖 is 2 columns = 3 total)
        let status_with_emoji = "↑🤖";
        assert_eq!(
            status_with_emoji.width(),
            3,
            "Status '↑🤖' should have visual width 3"
        );

        let mut line1 = StyledLine::new();
        let status_start = line1.width(); // 0
        line1.push_raw(status_with_emoji.to_string());
        line1.pad_to(status_start + 6); // Pad to width 6 (typical Status column width)

        assert_eq!(line1.width(), 6, "Status column with emoji should pad to 6");

        // Case 2: Status with only ASCII symbols (↑ is 1 column = 1 total)
        let status_ascii = "↑";
        assert_eq!(
            status_ascii.width(),
            1,
            "Status '↑' should have visual width 1"
        );

        let mut line2 = StyledLine::new();
        let status_start2 = line2.width();
        line2.push_raw(status_ascii.to_string());
        line2.pad_to(status_start2 + 6);

        assert_eq!(line2.width(), 6, "Status column with ASCII should pad to 6");

        // Both should have the same visual width after padding
        assert_eq!(
            line1.width(),
            line2.width(),
            "Unicode and ASCII status should pad to same visual width"
        );

        // Case 3: Complex status with multiple emoji (git symbols + user status)
        let complex_status = "↑⇡🤖📝";
        // ↑ (1) + ⇡ (1) + 🤖 (2) + 📝 (2) = 6 visual columns
        assert_eq!(
            complex_status.width(),
            6,
            "Complex status should have visual width 6"
        );

        let mut line3 = StyledLine::new();
        let status_start3 = line3.width();
        line3.push_raw(complex_status.to_string());
        line3.pad_to(status_start3 + 10); // Pad to width 10

        assert_eq!(line3.width(), 10, "Complex status should pad to 10");
    }
}
