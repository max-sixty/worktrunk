use crate::display::{format_relative_time_short, shorten_path, truncate_at_word_boundary};
use anstyle::{AnsiColor, Color, Style};
use std::path::Path;
use unicode_width::UnicodeWidthStr;
use worktrunk::styling::{CURRENT, StyledLine};

use super::ci_status::{CiSource, CiStatus, PrStatus};
use super::columns::{ColumnKind, DiffVariant};
use super::layout::{
    ColumnFormat, ColumnLayout, DiffColumnConfig, DiffDisplayConfig, LayoutConfig,
};
use super::model::{
    AheadBehind, CommitDetails, ListItem, PositionMask, UpstreamStatus, WorktreeData,
};
use worktrunk::git::LineDiff;

/// Compute style for branch/path based on worktree state
fn position_style(is_current: bool, is_main: bool, default: Style) -> Style {
    if is_current {
        CURRENT.bold()
    } else if is_main {
        Style::new()
            .fg_color(Some(Color::Ansi(AnsiColor::Cyan)))
            .bold()
    } else {
        default
    }
}

impl DiffDisplayConfig {
    fn format_plain(&self, positive: usize, negative: usize) -> Option<String> {
        if !self.always_show_zeros && positive == 0 && negative == 0 {
            return None;
        }

        let symbols = self.variant.symbols();

        let mut parts = Vec::with_capacity(2);

        if positive > 0 || self.always_show_zeros {
            parts.push(format!(
                "{}{}{}{}",
                self.positive_style,
                symbols.positive,
                positive,
                self.positive_style.render_reset()
            ));
        }

        if negative > 0 || self.always_show_zeros {
            parts.push(format!(
                "{}{}{}{}",
                self.negative_style,
                symbols.negative,
                negative,
                self.negative_style.render_reset()
            ));
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" "))
        }
    }
}

impl ColumnKind {
    /// Format diff-style values as plain text with ANSI colors (for json-pretty).
    pub(crate) fn format_diff_plain(self, positive: usize, negative: usize) -> Option<String> {
        let config = self.diff_display_config()?;

        config.format_plain(positive, negative)
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

    /// Get indicator symbol and style for rendering
    fn indicator_and_style(&self) -> (&'static str, Style) {
        let indicator = match self.source {
            CiSource::PullRequest => "●",
            CiSource::Branch => "○",
        };

        let style = if self.url.is_some() {
            self.style().underline()
        } else {
            self.style()
        };

        (indicator, style)
    }

    fn render_indicator(&self) -> StyledLine {
        let mut segment = StyledLine::new();
        let (indicator, style) = self.indicator_and_style();

        if let Some(ref url) = self.url {
            use worktrunk::styling::hyperlink;
            let styled_indicator = format!(
                "{}{}{}",
                style,
                hyperlink(indicator, url),
                style.render_reset()
            );
            segment.push_raw(styled_indicator);
        } else {
            segment.push_styled(indicator.to_string(), style);
        }

        segment
    }
}

#[derive(Clone, Copy)]
struct DiffSymbols {
    positive: &'static str,
    negative: &'static str,
}

impl DiffVariant {
    fn symbols(self) -> DiffSymbols {
        match self {
            DiffVariant::Signs => DiffSymbols {
                positive: "+",
                negative: "-",
            },
            DiffVariant::Arrows => DiffSymbols {
                positive: "↑",
                negative: "↓",
            },
        }
    }
}

impl DiffColumnConfig {
    /// Check if a value exceeds the allocated digit width
    fn exceeds_width(value: usize, digits: usize) -> bool {
        if digits == 0 {
            return value > 0;
        }
        let max_value = 10_usize.pow(digits as u32) - 1;
        value > max_value
    }

    /// Check if a subcolumn value should be rendered (non-zero or explicitly showing zeros)
    fn should_render(value: usize, always_show_zeros: bool) -> bool {
        value > 0 || (value == 0 && always_show_zeros)
    }

    /// Format a value using compact notation (K for thousands, optionally C for hundreds)
    ///
    /// Returns (formatted_string, uses_compact_notation)
    ///
    /// For line diffs (Signs): Shows full numbers in 100-999 range, uses K for thousands
    /// For commit counts (Arrows): Uses C for hundreds, K for thousands
    ///
    /// Note: Uses integer division for approximation (intentional truncation):
    /// - 648 / 100 = 6 → "6C" (represents ~600)
    /// - 1999 / 1000 = 1 → "1K" (represents ~1000)
    ///
    /// Examples (Signs):  100 -> ("100", false), 648 -> ("648", false), 1000 -> ("1K", true)
    /// Examples (Arrows): 100 -> ("1C", true),   648 -> ("6C", true),   1000 -> ("1K", true)
    fn format_overflow(value: usize, variant: DiffVariant) -> (String, bool) {
        if value >= 10_000 {
            // Cap at 9K to maintain 2-char limit (indicates "very large")
            ("9K".to_string(), true)
        } else if value >= 1_000 {
            (format!("{}K", value / 1_000), true)
        } else if value >= 100 {
            match variant {
                // Line diffs: show full number (user prefers precision over compactness)
                DiffVariant::Signs => (value.to_string(), false),
                // Commit counts: use C abbreviation
                DiffVariant::Arrows => (format!("{}C", value / 100), true),
            }
        } else {
            (value.to_string(), false)
        }
    }

    /// Render a subcolumn value with symbol and padding to fixed width
    /// Numbers are right-aligned on the ones column (e.g., " +2", "+53")
    /// For compact notation (C/K suffix), renders bold (e.g., bold "+6C", bold "+5K")
    fn render_subcolumn(
        segment: &mut StyledLine,
        symbol: &str,
        value: usize,
        width: usize,
        style: Style,
        overflow: bool,
        variant: DiffVariant,
    ) {
        let (value_str, is_compact) = if overflow {
            Self::format_overflow(value, variant)
        } else {
            (value.to_string(), false)
        };
        let content_len = 1 + value_str.len(); // symbol + digits
        let padding_needed = width.saturating_sub(content_len);

        // Add left padding for right-alignment
        if padding_needed > 0 {
            segment.push_raw(" ".repeat(padding_needed));
        }

        // Add styled content - bold entire value if using compact notation (C/K suffix)
        // to emphasize approximation
        if is_compact {
            segment.push_styled(format!("{}{}", symbol, value_str), style.bold());
        } else {
            segment.push_styled(format!("{}{}", symbol, value_str), style);
        }
    }

    fn render_segment(&self, positive: usize, negative: usize) -> StyledLine {
        let symbols = self.display.variant.symbols();
        let mut segment = StyledLine::new();

        // Check for overflow
        let positive_overflow = Self::exceeds_width(positive, self.added_digits);
        let negative_overflow = Self::exceeds_width(negative, self.deleted_digits);

        if positive == 0 && negative == 0 && !self.display.always_show_zeros {
            segment.push_raw(" ".repeat(self.total_width));
            return segment;
        }

        let positive_width = 1 + self.added_digits;
        let negative_width = 1 + self.deleted_digits;

        // Fixed content width ensures vertical alignment of subcolumns
        let content_width = positive_width + 1 + negative_width;
        let total_padding = self.total_width.saturating_sub(content_width);

        // Add leading padding for right-alignment
        if total_padding > 0 {
            segment.push_raw(" ".repeat(total_padding));
        }

        // Render positive (added) subcolumn
        if Self::should_render(positive, self.display.always_show_zeros) {
            Self::render_subcolumn(
                &mut segment,
                symbols.positive,
                positive,
                positive_width,
                self.display.positive_style,
                positive_overflow,
                self.display.variant,
            );
        } else {
            // Empty positive subcolumn - add spaces to maintain alignment
            segment.push_raw(" ".repeat(positive_width));
        }

        // Always add separator to maintain fixed layout (early return handles empty case)
        segment.push_raw(" ");

        // Render negative (deleted) subcolumn
        if Self::should_render(negative, self.display.always_show_zeros) {
            Self::render_subcolumn(
                &mut segment,
                symbols.negative,
                negative,
                negative_width,
                self.display.negative_style,
                negative_overflow,
                self.display.variant,
            );
        } else {
            // Empty negative subcolumn - add spaces to maintain alignment
            segment.push_raw(" ".repeat(negative_width));
        }

        segment
    }
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
            let cell = render_cell(column);
            let cell_width = cell.width();

            // Debug: Log if cell exceeds its allocated width
            if cell_width > column.width {
                log::debug!(
                    "Cell overflow: column={:?} allocated={} actual={} excess={}",
                    column.kind,
                    column.width,
                    cell_width,
                    cell_width - column.width
                );
            }

            line.extend(cell);

            // Pad to end of column (unless it's the last column)
            if index != last_index {
                line.pad_to(column.start + column.width);
            }
        }

        let final_width = line.width();
        log::debug!("Rendered line width: {}", final_width);

        line
    }

    pub fn format_header_line(&self) -> String {
        let style = Style::new().bold();
        let line = self.render_line(|column| {
            let mut cell = StyledLine::new();
            if !column.header.is_empty() {
                // Diff columns have right-aligned values, so right-align headers too
                let is_diff_column = matches!(column.format, ColumnFormat::Diff(_));

                if is_diff_column {
                    // Right-align header within column width
                    let header_width = column.header.width();
                    if header_width < column.width {
                        let padding = column.width - header_width;
                        cell.push_raw(" ".repeat(padding));
                    }
                }

                cell.push_styled(column.header.to_string(), style);
            }
            cell
        });

        line.render()
    }

    pub fn format_list_item_line(
        &self,
        item: &ListItem,
        current_worktree_path: Option<&std::path::PathBuf>,
    ) -> String {
        let ctx = ListRowContext::new(item, current_worktree_path);
        let line = self.render_line(|column| {
            column.render_cell(
                &ctx,
                &self.status_position_mask,
                &self.common_prefix,
                self.max_message_len,
            )
        });

        line.render()
    }

    /// Render a skeleton row showing known data (branch, path) with placeholders for other columns
    pub fn format_skeleton_row(&self, item: &super::model::ListItem, is_current: bool) -> String {
        use crate::display::shorten_path;

        let branch = item.branch_name();
        let is_main = item.is_main();
        let shortened_path = item
            .worktree_path()
            .map(|p| shorten_path(p, &self.common_prefix))
            .unwrap_or_default();

        let dim = Style::new().dimmed();
        let spinner = "⋯"; // Placeholder character

        let line = self.render_line(|col| {
            let mut cell = StyledLine::new();

            match col.kind {
                ColumnKind::Gutter => {
                    // Show actual gutter symbol even in skeleton
                    let has_worktree = item.worktree_path().is_some();
                    let symbol = if has_worktree {
                        if is_current {
                            "@ "
                        } else if is_main {
                            "^ "
                        } else {
                            "+ "
                        }
                    } else {
                        "  "
                    };
                    cell.push_raw(symbol.to_string());
                }
                ColumnKind::Branch => {
                    // Show actual branch name
                    cell.push_styled(branch, position_style(is_current, is_main, dim));
                    cell.pad_to(col.width);
                }
                ColumnKind::Path => {
                    // Show actual path
                    cell.push_styled(&shortened_path, position_style(is_current, is_main, dim));
                    cell.pad_to(col.width);
                }
                ColumnKind::Commit => {
                    // Show actual commit hash (always available)
                    let head = item.head();
                    let short_head = &head[..8.min(head.len())];
                    cell.push_styled(short_head, dim);
                }
                _ => {
                    // Show spinner for data columns
                    cell.push_styled(spinner, dim);
                    cell.pad_to(col.width);
                }
            }

            cell
        });

        line.render()
    }
}

struct ListRowContext<'a> {
    item: &'a ListItem,
    worktree_data: Option<&'a WorktreeData>,
    counts: AheadBehind,
    branch_diff: LineDiff,
    upstream: UpstreamStatus,
    commit: CommitDetails,
    head: &'a str,
    text_style: Option<Style>,
    is_current: bool,
}

impl<'a> ListRowContext<'a> {
    fn new(item: &'a ListItem, current_worktree_path: Option<&std::path::PathBuf>) -> Self {
        let worktree_data = item.worktree_data();
        let counts = item.counts();
        let commit = item.commit_details();
        let branch_diff = item.branch_diff().diff;
        let upstream = item.upstream();
        let head = item.head();

        let is_current = worktree_data
            .and_then(|data| current_worktree_path.map(|p| p == &data.path))
            .unwrap_or(false);

        let mut ctx = Self {
            item,
            worktree_data,
            counts,
            branch_diff,
            upstream,
            commit,
            head,
            text_style: None,
            is_current,
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
        let base_style = self.worktree_data.and_then(|data| {
            let is_current = current_worktree_path
                .map(|p| p == &data.path)
                .unwrap_or(false);
            match (is_current, data.is_main) {
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

        config.render_segment(positive, negative)
    }

    fn render_cell(
        &self,
        ctx: &ListRowContext,
        status_mask: &PositionMask,
        common_prefix: &Path,
        max_message_len: usize,
    ) -> StyledLine {
        match self.kind {
            ColumnKind::Gutter => {
                let mut cell = StyledLine::new();
                let symbol = if let Some(data) = ctx.worktree_data {
                    // Worktree: priority is current > main > regular
                    if ctx.is_current {
                        "@ " // Current worktree
                    } else if data.is_main {
                        "^ " // Main worktree
                    } else {
                        "+ " // Regular worktree
                    }
                } else {
                    "  " // Branch without worktree (two spaces to match width)
                };
                cell.push_raw(symbol.to_string());
                cell
            }
            ColumnKind::Branch => {
                let mut cell = StyledLine::new();
                let text = ctx.item.branch.as_deref().unwrap_or("-");
                if let Some(style) = ctx.text_style {
                    cell.push_styled(text.to_string(), style);
                } else {
                    cell.push_raw(text.to_string());
                }
                cell.truncate_to_width(self.width)
            }
            ColumnKind::Status => {
                let mut cell = StyledLine::new();

                // Render status symbols (works for both worktrees and branches)
                if let Some(ref status_symbols) = ctx.item.status_symbols {
                    cell.push_raw(status_symbols.render_with_mask(status_mask));
                } else if ctx.worktree_data.is_some() {
                    // Show spinner while status is being computed (worktrees only)
                    cell.push_styled("⋯", Style::new().dimmed());
                }

                // Truncate if exceeds column width, then pad
                let mut cell = cell.truncate_to_width(self.width);
                cell.pad_to(self.width);
                cell
            }
            ColumnKind::WorkingDiff => {
                let Some(diff) = ctx
                    .worktree_data
                    .and_then(|data| data.working_tree_diff.as_ref())
                else {
                    return StyledLine::new();
                };
                self.render_diff_cell(diff.added, diff.deleted)
            }
            ColumnKind::AheadBehind => {
                if ctx.item.is_main() {
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
                if ctx.item.is_main() {
                    return StyledLine::new();
                }
                self.render_diff_cell(ctx.branch_diff.added, ctx.branch_diff.deleted)
            }
            ColumnKind::Path => {
                let Some(data) = ctx.worktree_data else {
                    return StyledLine::new();
                };
                let mut cell = StyledLine::new();
                let path_str = shorten_path(&data.path, common_prefix);
                if let Some(style) = ctx.text_style {
                    cell.push_styled(path_str, style);
                } else {
                    cell.push_raw(path_str);
                }
                cell.truncate_to_width(self.width)
            }
            ColumnKind::Upstream => {
                let Some((_, ahead, behind)) = ctx.upstream.active() else {
                    return StyledLine::new();
                };
                self.render_diff_cell(ahead, behind)
            }
            ColumnKind::Time => {
                let mut cell = StyledLine::new();

                // Show spinner if commit details haven't loaded yet
                if ctx.worktree_data.is_some() && ctx.item.commit.is_none() {
                    cell.push_styled("⋯", Style::new().dimmed());
                } else {
                    let time_str = format_relative_time_short(ctx.commit.timestamp);
                    cell.push_styled(time_str, Style::new().dimmed());
                }

                cell
            }
            ColumnKind::CiStatus => {
                // Check display field first for pending indicators during progressive rendering
                if ctx.worktree_data.is_some()
                    && let Some(ref ci_display) = ctx.item.display.ci_status_display
                {
                    let mut cell = StyledLine::new();
                    // ci_status_display contains pre-formatted ANSI text (either actual status or "⋯")
                    cell.push_raw(ci_display.clone());
                    return cell;
                }

                // pr_status is Option<Option<PrStatus>>:
                // - None = not loaded yet (show spinner)
                // - Some(None) = loaded, no CI (show nothing)
                // - Some(Some(status)) = loaded with CI (show status)
                match ctx.item.pr_status() {
                    None => {
                        // Not loaded yet - show spinner
                        let mut cell = StyledLine::new();
                        cell.push_styled("⋯", Style::new().dimmed());
                        cell
                    }
                    Some(None) => {
                        // Loaded, no CI - show nothing
                        StyledLine::new()
                    }
                    Some(Some(pr_status)) => {
                        // Loaded with CI - show status
                        pr_status.render_indicator()
                    }
                }
            }
            ColumnKind::Commit => {
                let mut cell = StyledLine::new();
                cell.push_styled(ctx.short_head().to_string(), Style::new().dimmed());
                cell
            }
            ColumnKind::Message => {
                let mut cell = StyledLine::new();

                // Show spinner if commit details haven't loaded yet
                if ctx.worktree_data.is_some() && ctx.item.commit.is_none() {
                    cell.push_styled("⋯", Style::new().dimmed());
                } else {
                    let msg =
                        truncate_at_word_boundary(&ctx.commit.commit_message, max_message_len);
                    cell.push_styled(msg, Style::new().dimmed());
                }

                cell
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::list::layout::DiffDisplayConfig;
    use worktrunk::styling::{ADDITION, DELETION, StyledLine};

    fn format_diff_like_column(
        positive: usize,
        negative: usize,
        config: DiffColumnConfig,
    ) -> StyledLine {
        config.render_segment(positive, negative)
    }

    #[test]
    fn test_format_diff_column_pads_to_total_width() {
        use super::super::columns::DiffVariant;

        // Case 1: Single-digit diffs with total=6 (to fit "WT +/-" header)
        let total = 6;
        let result = format_diff_like_column(
            1,
            1,
            DiffColumnConfig {
                added_digits: 1,
                deleted_digits: 1,
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
        let total = 8;
        let result = format_diff_like_column(
            10,
            50,
            DiffColumnConfig {
                added_digits: 2,
                deleted_digits: 2,
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
        let total = 9;
        let result = format_diff_like_column(
            100,
            50,
            DiffColumnConfig {
                added_digits: 3,
                deleted_digits: 2,
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
        let total = 6;
        let result = format_diff_like_column(
            0,
            0,
            DiffColumnConfig {
                added_digits: 1,
                deleted_digits: 1,
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

        let total = 6;

        let result = format_diff_like_column(
            1,
            1,
            DiffColumnConfig {
                added_digits: 1,
                deleted_digits: 1,
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
        let clean = worktrunk::styling::strip_ansi_codes(&rendered);

        // Should be " +1 -1" (with leading space for right-alignment)
        assert_eq!(clean, " +1 -1", "Diff should be right-aligned");
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
        let clean = worktrunk::styling::strip_ansi_codes(&rendered);
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

        let total = 7;

        let dim_deletion = DELETION.dimmed();
        let cases = [(0, 0), (1, 0), (0, 1), (1, 1), (99, 99), (5, 44)];

        for (ahead, behind) in cases {
            let result = format_diff_like_column(
                ahead,
                behind,
                DiffColumnConfig {
                    added_digits: 2,
                    deleted_digits: 2,
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

        let total = 7;

        let dim_deletion = DELETION.dimmed();

        let empty = format_diff_like_column(
            0,
            0,
            DiffColumnConfig {
                added_digits: 0,
                deleted_digits: 2,
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
                added_digits: 0,
                deleted_digits: 2,
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

        let total = 7;

        let dim_deletion = DELETION.dimmed();

        // With always_show_zeros=false, (0, 0) renders as blank
        let without = format_diff_like_column(
            0,
            0,
            DiffColumnConfig {
                added_digits: 1,
                deleted_digits: 1,
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
        let clean_without = worktrunk::styling::strip_ansi_codes(&rendered_without);
        assert_eq!(clean_without, "       ", "Should render as blank");

        // With always_show_zeros=true, (0, 0) renders as "↑0 ↓0"
        let with = format_diff_like_column(
            0,
            0,
            DiffColumnConfig {
                added_digits: 1,
                deleted_digits: 1,
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
        let clean_with = worktrunk::styling::strip_ansi_codes(&rendered_with);
        assert_eq!(
            clean_with, "  ↑0 ↓0",
            "Should render ↑0 ↓0 with padding (right-aligned)"
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

    #[test]
    fn test_diff_column_numeric_right_alignment() {
        use super::super::columns::DiffVariant;

        // Test that numbers are right-aligned on the ones column
        // When we have 2-digit allocation but use 1-digit values, they should have leading space
        let total = 8; // 3 (added) + 1 (separator) + 3 (deleted) + 1 (leading padding)

        // Test case 1: (53, 7) - large added, small deleted
        let result1 = format_diff_like_column(
            53,
            7,
            DiffColumnConfig {
                added_digits: 2, // Allocates 3 chars: "+NN"
                deleted_digits: 2,
                total_width: total,
                display: DiffDisplayConfig {
                    variant: DiffVariant::Signs,
                    positive_style: ADDITION,
                    negative_style: DELETION,
                    always_show_zeros: false,
                },
            },
        );
        let rendered1 = result1.render();
        let clean1 = worktrunk::styling::strip_ansi_codes(&rendered1);
        assert_eq!(clean1, " +53  -7", "Should be ' +53  -7'");

        // Test case 2: (33, 23) - both medium
        let result2 = format_diff_like_column(
            33,
            23,
            DiffColumnConfig {
                added_digits: 2,
                deleted_digits: 2,
                total_width: total,
                display: DiffDisplayConfig {
                    variant: DiffVariant::Signs,
                    positive_style: ADDITION,
                    negative_style: DELETION,
                    always_show_zeros: false,
                },
            },
        );
        let rendered2 = result2.render();
        let clean2 = worktrunk::styling::strip_ansi_codes(&rendered2);
        assert_eq!(clean2, " +33 -23", "Should be ' +33 -23'");

        // Test case 3: (2, 2) - both small (needs padding)
        let result3 = format_diff_like_column(
            2,
            2,
            DiffColumnConfig {
                added_digits: 2,
                deleted_digits: 2,
                total_width: total,
                display: DiffDisplayConfig {
                    variant: DiffVariant::Signs,
                    positive_style: ADDITION,
                    negative_style: DELETION,
                    always_show_zeros: false,
                },
            },
        );
        let rendered3 = result3.render();
        let clean3 = worktrunk::styling::strip_ansi_codes(&rendered3);
        assert_eq!(clean3, "  +2  -2", "Should be '  +2  -2'");

        // Verify vertical alignment: the ones digits should be in the same column
        // The ones digit should be at position 3 for all cases (with 2-digit allocation)
        // ' +53  -7' -> position 3 is '3'
        // ' +33 -23' -> position 3 is '3' (second '3', the ones digit)
        // '  +2  -2' -> position 3 is '2'
        let ones_pos = 3;
        assert_eq!(
            clean1.chars().nth(ones_pos).unwrap(),
            '3',
            "Ones digit of 53 should be at position {ones_pos}"
        );
        assert_eq!(
            clean2.chars().nth(ones_pos).unwrap(),
            '3',
            "Ones digit of 33 should be at position {ones_pos}"
        );
        assert_eq!(
            clean3.chars().nth(ones_pos).unwrap(),
            '2',
            "Ones digit of 2 should be at position {ones_pos}"
        );
    }

    #[test]
    fn test_diff_column_overflow_handling() {
        use super::super::columns::DiffVariant;

        // Test overflow with Signs variant (+ and -)
        // Allocated: 3 digits for added, 3 digits for deleted (total width 9)
        // Max value: 999
        let total = 9;

        // Case 1: Value just within limit (should render normally)
        let result = format_diff_like_column(
            999,
            999,
            DiffColumnConfig {
                added_digits: 3,
                deleted_digits: 3,
                total_width: total,
                display: DiffDisplayConfig {
                    variant: DiffVariant::Signs,
                    positive_style: ADDITION,
                    negative_style: DELETION,
                    always_show_zeros: false,
                },
            },
        );
        assert_eq!(result.width(), total);
        assert!(result.render().contains("999"));

        // Case 2: Positive overflow (1000 exceeds 3 digits)
        // Should show: "+1K -500" (positive with K suffix, negative normal)
        let overflow_result = format_diff_like_column(
            1000,
            500,
            DiffColumnConfig {
                added_digits: 3,
                deleted_digits: 3,
                total_width: total,
                display: DiffDisplayConfig {
                    variant: DiffVariant::Signs,
                    positive_style: ADDITION,
                    negative_style: DELETION,
                    always_show_zeros: false,
                },
            },
        );
        assert_eq!(overflow_result.width(), total);
        let rendered = overflow_result.render();
        assert!(
            rendered.contains("+1") && rendered.contains('K'),
            "Positive overflow should show +1K (may have styling), got: {}",
            rendered
        );
        assert!(
            rendered.contains("500"),
            "Negative value should show normally when positive overflows, got: {}",
            rendered
        );

        // Case 3: Negative overflow
        // Should show: "+500 -1K" (positive normal, negative with K suffix)
        let overflow_result2 = format_diff_like_column(
            500,
            1000,
            DiffColumnConfig {
                added_digits: 3,
                deleted_digits: 3,
                total_width: total,
                display: DiffDisplayConfig {
                    variant: DiffVariant::Signs,
                    positive_style: ADDITION,
                    negative_style: DELETION,
                    always_show_zeros: false,
                },
            },
        );
        assert_eq!(overflow_result2.width(), total);
        let rendered2 = overflow_result2.render();
        assert!(
            rendered2.contains("500"),
            "Positive value should show normally when negative overflows, got: {}",
            rendered2
        );
        assert!(
            rendered2.contains("-1") && rendered2.contains('K'),
            "Negative overflow should show -1K (may have styling), got: {}",
            rendered2
        );

        // Case 4: Extreme overflow (>= 10K values cap at 9K for 2-char limit)
        let extreme_overflow = format_diff_like_column(
            100_000,
            200_000,
            DiffColumnConfig {
                added_digits: 3,
                deleted_digits: 3,
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
            extreme_overflow.width(),
            total,
            "100K overflow should fit in allocated width"
        );
        let extreme_rendered = extreme_overflow.render();
        assert!(
            extreme_rendered.contains("+9") && extreme_rendered.contains('K'),
            "100K+ overflow should cap at +9K (may have styling), got: {}",
            extreme_rendered
        );
        assert!(
            extreme_rendered.contains("-9") && extreme_rendered.contains('K'),
            "100K+ overflow should cap at -9K (may have styling), got: {}",
            extreme_rendered
        );

        // Test overflow with Arrows variant (↑ and ↓)
        let arrow_total = 7;

        // Case 5: Arrow positive overflow (100 exceeds 2 digits, max is 99)
        // Should show with K suffix (not repeated symbols)
        let arrow_overflow = format_diff_like_column(
            1000, // Use larger value to show K suffix
            50,
            DiffColumnConfig {
                added_digits: 2,
                deleted_digits: 2,
                total_width: arrow_total,
                display: DiffDisplayConfig {
                    variant: DiffVariant::Arrows,
                    positive_style: ADDITION,
                    negative_style: DELETION,
                    always_show_zeros: false,
                },
            },
        );
        assert_eq!(arrow_overflow.width(), arrow_total);
        let arrow_rendered = arrow_overflow.render();
        assert!(
            arrow_rendered.contains("↑1") && arrow_rendered.contains('K'),
            "Arrow positive overflow should show ↑1K (may have styling), got: {}",
            arrow_rendered
        );
        assert!(
            arrow_rendered.contains("50"),
            "Negative value should show normally when positive overflows, got: {}",
            arrow_rendered
        );

        // Case 6: Arrow negative overflow
        // Should show with K suffix
        let arrow_overflow2 = format_diff_like_column(
            50,
            1000, // Use larger value to show K suffix
            DiffColumnConfig {
                added_digits: 2,
                deleted_digits: 2,
                total_width: arrow_total,
                display: DiffDisplayConfig {
                    variant: DiffVariant::Arrows,
                    positive_style: ADDITION,
                    negative_style: DELETION,
                    always_show_zeros: false,
                },
            },
        );
        assert_eq!(arrow_overflow2.width(), arrow_total);
        let arrow_rendered2 = arrow_overflow2.render();
        assert!(
            arrow_rendered2.contains("50"),
            "Positive value should show normally when negative overflows, got: {}",
            arrow_rendered2
        );
        assert!(
            arrow_rendered2.contains("↓1") && arrow_rendered2.contains('K'),
            "Arrow negative overflow should show ↓1K (may have styling), got: {}",
            arrow_rendered2
        );
    }
}
