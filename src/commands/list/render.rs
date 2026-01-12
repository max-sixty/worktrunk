use crate::display::{format_relative_time_short, shorten_path, truncate_to_width};
use anstyle::Style;
use std::path::Path;
use unicode_width::UnicodeWidthStr;
use worktrunk::styling::{Stream, StyledLine, hyperlink_stdout, supports_hyperlinks};

use super::ci_status::PrStatus;
use super::collect_progressive_impl::parse_port_from_url;
use super::columns::{ColumnKind, DiffVariant};
use super::layout::{ColumnFormat, ColumnLayout, DiffColumnConfig, LayoutConfig};
use super::model::{
    AheadBehind, CommitDetails, ListItem, PositionMask, UpstreamStatus, WorktreeData,
};
use worktrunk::git::LineDiff;

impl PrStatus {
    /// Render indicator as a StyledLine for table column rendering.
    ///
    /// Uses OSC 8 hyperlinks when the terminal supports them; falls back to
    /// plain colored indicator otherwise.
    fn render_indicator(&self) -> StyledLine {
        let mut segment = StyledLine::new();
        let include_link = supports_hyperlinks(Stream::Stdout);
        segment.push_raw(self.format_indicator(include_link));
        segment
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
    /// Values >= 10,000 display as "∞" to indicate "very large" without false precision.
    ///
    /// Examples (Signs):  100 -> ("100", false), 648 -> ("648", false), 1000 -> ("1K", true)
    /// Examples (Arrows): 100 -> ("1C", true),   648 -> ("6C", true),   1000 -> ("1K", true)
    fn format_overflow(value: usize, variant: DiffVariant) -> (String, bool) {
        if value >= 10_000 {
            // Use ∞ for extreme values to avoid false precision (9K could be 9K or 900K)
            ("∞".to_string(), true)
        } else if value >= 1_000 {
            (format!("{}K", value / 1_000), true)
        } else if value >= 100 {
            match variant {
                // Line diffs: show full number (user prefers precision over compactness)
                DiffVariant::Signs => (value.to_string(), false),
                // Commit counts: use C abbreviation
                DiffVariant::Arrows | DiffVariant::UpstreamArrows => {
                    (format!("{}C", value / 100), true)
                }
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
        let content_len = 1 + value_str.width(); // symbol + display width
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

    /// Render diff values as a StyledLine with fixed-width alignment.
    ///
    /// Numbers are right-aligned within their allocated digit width.
    /// Use this for tabular display where columns must align vertically.
    pub fn render_segment(&self, positive: usize, negative: usize) -> StyledLine {
        let symbols = self.display.variant.symbols();
        let mut segment = StyledLine::new();

        // Check for overflow
        let positive_overflow = Self::exceeds_width(positive, self.positive_digits);
        let negative_overflow = Self::exceeds_width(negative, self.negative_digits);

        if positive == 0 && negative == 0 && !self.display.always_show_zeros {
            segment.push_raw(" ".repeat(self.total_width));
            return segment;
        }

        let positive_width = 1 + self.positive_digits;
        let negative_width = 1 + self.negative_digits;

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
        self.render_header_line().render()
    }

    /// Render header line as StyledLine (for extracting both plain and styled text)
    pub fn render_header_line(&self) -> StyledLine {
        let style = Style::new().bold();
        self.render_line(|column| {
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
        })
    }

    pub fn format_list_item_line(&self, item: &ListItem, previous_branch: Option<&str>) -> String {
        self.render_list_item_line(item, previous_branch).render()
    }

    /// Render list item line as StyledLine (for extracting both plain and styled text)
    pub fn render_list_item_line(
        &self,
        item: &ListItem,
        previous_branch: Option<&str>,
    ) -> StyledLine {
        let ctx = ListRowContext::new(item, previous_branch);
        self.render_line(|column| {
            column.render_cell(
                &ctx,
                &self.status_position_mask,
                &self.main_worktree_path,
                self.max_message_len,
            )
        })
    }

    /// Render a skeleton row showing known data (branch, path) with placeholders for other columns.
    ///
    /// Used for both worktrees and branch-only items; branch-only rows render an empty path
    /// and a blank gutter placeholder.
    pub fn format_skeleton_row(&self, item: &super::model::ListItem) -> String {
        use crate::display::shorten_path;

        let branch = item.branch_name();
        let wt_data = item.worktree_data();
        let shortened_path = item
            .worktree_path()
            .map(|p| shorten_path(p, &self.main_worktree_path))
            .unwrap_or_default();

        let dim = Style::new().dimmed();
        let spinner = "⋯"; // Placeholder character

        let line = self.render_line(|col| {
            let mut cell = StyledLine::new();

            match col.kind {
                ColumnKind::Gutter => {
                    // Skeleton shows placeholder gutter - actual symbols appear when data loads.
                    // This allows deferring previous_branch lookup until after skeleton.
                    let symbol = if wt_data.is_some() {
                        "· " // Placeholder for worktrees
                    } else {
                        "  " // Branch without worktree (two spaces to match width)
                    };
                    cell.push_styled(symbol, dim);
                }
                ColumnKind::Branch => {
                    // Show actual branch name (no dim - start normal, gray out later if removable)
                    cell.push_raw(branch.to_string());
                    cell.pad_to(col.width);
                }
                ColumnKind::Path => {
                    // Show actual path (no dim - start normal, gray out later if removable)
                    cell.push_raw(&shortened_path);
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
    /// None means task was skipped (show `…`), Some means computed (may be zero)
    branch_diff: Option<LineDiff>,
    upstream: UpstreamStatus,
    commit: CommitDetails,
    head: &'a str,
    text_style: Option<Style>,
    is_current: bool,
    is_previous: bool,
}

impl<'a> ListRowContext<'a> {
    fn new(item: &'a ListItem, previous_branch: Option<&str>) -> Self {
        let worktree_data = item.worktree_data();
        let counts = item.counts();
        let commit = item.commit_details();
        let branch_diff = item.branch_diff().map(|bd| bd.diff);
        let upstream = item.upstream();
        let head = item.head();

        // Use stored values for worktrees, compute for branches
        let is_current = worktree_data.is_some_and(|d| d.is_current);
        let is_previous = worktree_data.map(|d| d.is_previous).unwrap_or_else(|| {
            // Branches don't have WorktreeData, compute from previous_branch
            previous_branch.is_some_and(|prev| item.branch.as_deref() == Some(prev))
        });

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
            is_previous,
        };

        ctx.text_style = ctx.compute_text_style();
        ctx
    }

    fn short_head(&self) -> &str {
        &self.head[..8.min(self.head.len())]
    }

    fn compute_text_style(&self) -> Option<Style> {
        // No special styling for current worktree - gutter symbol (@) and top position
        // already communicate it. Only dim removable worktrees.
        if self.item.should_dim() {
            Some(Style::new().dimmed())
        } else {
            None
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
        main_worktree_path: &Path,
        max_message_len: usize,
    ) -> StyledLine {
        match self.kind {
            ColumnKind::Gutter => {
                let mut cell = StyledLine::new();
                let symbol = if let Some(data) = ctx.worktree_data {
                    // Priority: @ (current) > ^ (main) > - (previous) > + (regular)
                    if ctx.is_current {
                        "@ " // Current worktree
                    } else if data.is_main {
                        "^ " // Main worktree
                    } else if ctx.is_previous {
                        "- " // Previous worktree (wt switch -)
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
                } else {
                    // Show spinner while status is being computed (both worktrees and branches)
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
                match ctx.branch_diff {
                    Some(diff) => self.render_diff_cell(diff.added, diff.deleted),
                    None => {
                        // Task was skipped — show ellipsis to indicate "not computed"
                        let mut cell = StyledLine::new();
                        let padding = self.width.saturating_sub(1);
                        cell.push_raw(" ".repeat(padding));
                        cell.push_styled("…", Style::new().dimmed());
                        cell
                    }
                }
            }
            ColumnKind::Path => {
                let Some(data) = ctx.worktree_data else {
                    return StyledLine::new();
                };
                let mut cell = StyledLine::new();
                let path_str = shorten_path(&data.path, main_worktree_path);
                if let Some(style) = ctx.text_style {
                    cell.push_styled(path_str, style);
                } else {
                    cell.push_raw(path_str);
                }
                cell.truncate_to_width(self.width)
            }
            ColumnKind::Upstream => {
                let Some(active) = ctx.upstream.active() else {
                    return StyledLine::new();
                };
                // Show centered | when in sync instead of ⇡0  ⇣0
                // Note: This duplicates the InSync check from Divergence::Special, but
                // checking counts directly is simpler than threading the enum through.
                if active.ahead == 0 && active.behind == 0 {
                    let mut cell = StyledLine::new();
                    // Center the symbol in the column width
                    let padding_left = (self.width.saturating_sub(1)) / 2;
                    cell.push_raw(" ".repeat(padding_left));
                    cell.push_styled("|", Style::new().dimmed());
                    return cell;
                }
                self.render_diff_cell(active.ahead, active.behind)
            }
            ColumnKind::Time => {
                let mut cell = StyledLine::new();

                // Show spinner if commit details haven't loaded yet (for both worktrees and branches)
                if ctx.item.commit.is_none() {
                    cell.push_styled("⋯", Style::new().dimmed());
                } else {
                    let time_str = format_relative_time_short(ctx.commit.timestamp);
                    cell.push_styled(time_str, Style::new().dimmed());
                }

                cell
            }
            ColumnKind::Url => {
                // URL column: shows dev server URL from project config template
                // - When hyperlinks supported: show ":port" as clickable link
                // - When hyperlinks not supported: show full URL
                // - dim if not available/active
                // - normal if available and active
                match (&ctx.item.url, ctx.item.url_active) {
                    (None, _) => StyledLine::new(), // No URL configured
                    (Some(url), Some(true)) => {
                        // Active: normal styling
                        let mut cell = StyledLine::new();
                        cell.push_raw(format_url_cell(url));
                        cell.truncate_to_width(self.width)
                    }
                    (Some(url), _) => {
                        // Not active or unknown: dim styling
                        let mut cell = StyledLine::new();
                        let formatted = format_url_cell(url);
                        // Apply dim to the formatted text (which may contain OSC 8 sequences)
                        cell.push_styled(formatted, Style::new().dimmed());
                        cell.truncate_to_width(self.width)
                    }
                }
            }
            ColumnKind::CiStatus => {
                // Check display field first for pending indicators during progressive rendering
                // (works for both worktrees and branches)
                if let Some(ref ci_display) = ctx.item.display.ci_status_display {
                    let mut cell = StyledLine::new();
                    // ci_status_display contains pre-formatted ANSI text (either actual status or "⋯")
                    cell.push_raw(ci_display.clone());
                    return cell;
                }

                // pr_status is Option<Option<PrStatus>>:
                // - None = not loaded yet (show spinner)
                // - Some(None) = loaded, no CI (show nothing)
                // - Some(Some(status)) = loaded with CI (show status)
                match &ctx.item.pr_status {
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

                // Show spinner if commit details haven't loaded yet (for both worktrees and branches)
                if ctx.item.commit.is_none() {
                    cell.push_styled("⋯", Style::new().dimmed());
                } else {
                    let msg = truncate_to_width(&ctx.commit.commit_message, max_message_len);
                    cell.push_styled(msg, Style::new().dimmed());
                }

                cell
            }
        }
    }
}

/// Format URL cell with optional hyperlink.
///
/// When the terminal supports OSC 8 hyperlinks, shows just the port (e.g., `:3000`)
/// as a clickable link. Otherwise, shows the full URL.
fn format_url_cell(url: &str) -> String {
    if supports_hyperlinks(Stream::Stdout) {
        // Extract port from URL for compact display
        if let Some(port) = parse_port_from_url(url) {
            return hyperlink_stdout(url, &format!(":{port}"));
        }
    }
    // Fallback: show full URL
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::list::layout::DiffDisplayConfig;
    use ansi_str::AnsiStr;
    use worktrunk::styling::{ADDITION, DELETION, StyledLine};

    fn format_diff_like_column(
        positive: usize,
        negative: usize,
        config: DiffColumnConfig,
    ) -> StyledLine {
        config.render_segment(positive, negative)
    }

    #[test]
    #[cfg(unix)] // format_aligned is unix-only
    fn test_format_aligned_produces_fixed_width_output() {
        use super::super::columns::DiffVariant;

        let config = DiffDisplayConfig {
            variant: DiffVariant::Signs,
            positive_style: ADDITION,
            negative_style: DELETION,
            always_show_zeros: false,
        };

        // Test various values
        let result1 = config.format_aligned(310, 112);
        let result2 = config.format_aligned(54, 63);
        let result3 = config.format_aligned(9, 3);

        // All should have the same width (3 + 1 + 3 + 1 + 3 = 9 chars for "+NNN -NNN")
        let clean1 = result1.ansi_strip().into_owned();
        let clean2 = result2.ansi_strip().into_owned();
        let clean3 = result3.ansi_strip().into_owned();

        assert_eq!(
            clean1.len(),
            clean2.len(),
            "All aligned outputs should have same width"
        );
        assert_eq!(
            clean2.len(),
            clean3.len(),
            "All aligned outputs should have same width"
        );

        // Verify right-alignment: smaller numbers have leading spaces
        assert!(
            clean2.starts_with(' '),
            "54 should have leading space: '{}'",
            clean2
        );
        assert!(
            clean3.starts_with(' '),
            "9 should have leading spaces: '{}'",
            clean3
        );
    }

    #[test]
    #[cfg(unix)] // format_aligned is unix-only
    fn test_format_aligned_handles_single_side() {
        use super::super::columns::DiffVariant;

        let config = DiffDisplayConfig {
            variant: DiffVariant::Signs,
            positive_style: ADDITION,
            negative_style: DELETION,
            always_show_zeros: false,
        };

        // Insertions only
        let ins_only = config.format_aligned(447, 0);
        let clean_ins = ins_only.ansi_strip().into_owned();
        assert!(
            clean_ins.contains("+447"),
            "Should contain +447: '{}'",
            clean_ins
        );

        // Deletions only
        let del_only = config.format_aligned(0, 5);
        let clean_del = del_only.ansi_strip().into_owned();
        assert!(
            clean_del.contains("-5"),
            "Should contain -5: '{}'",
            clean_del
        );

        // Both should have same total width
        assert_eq!(
            clean_ins.len(),
            clean_del.len(),
            "Single-side outputs should match width"
        );
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
                positive_digits: 1,
                negative_digits: 1,
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
                positive_digits: 2,
                negative_digits: 2,
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
                positive_digits: 3,
                negative_digits: 2,
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
                positive_digits: 1,
                negative_digits: 1,
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
                positive_digits: 1,
                negative_digits: 1,
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
        let clean = rendered.ansi_strip().into_owned();

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
        let clean = rendered.ansi_strip().into_owned();
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
        line1.push_styled(branch_ja.to_string(), Style::new().bold());
        line1.pad_to(20); // Pad to width 20

        assert_eq!(line1.width(), 20);

        // Case 2: Regular ASCII branch
        let branch_ascii = "feature-test";
        assert_eq!(branch_ascii.width(), 12);

        let mut line2 = StyledLine::new();
        line2.push_styled(branch_ascii.to_string(), Style::new().bold());
        line2.pad_to(20);

        assert_eq!(line2.width(), 20);

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
                    positive_digits: 2,
                    negative_digits: 2,
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
                positive_digits: 0,
                negative_digits: 2,
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
                positive_digits: 0,
                negative_digits: 2,
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
                positive_digits: 1,
                negative_digits: 1,
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
        let clean_without = rendered_without.ansi_strip().into_owned();
        assert_eq!(clean_without, "       ");

        // With always_show_zeros=true, (0, 0) renders as "↑0 ↓0"
        let with = format_diff_like_column(
            0,
            0,
            DiffColumnConfig {
                positive_digits: 1,
                negative_digits: 1,
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
        let clean_with = rendered_with.ansi_strip().into_owned();
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

        assert_eq!(line1.width(), 6);

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

        assert_eq!(line2.width(), 6);

        // Both should have the same visual width after padding
        assert_eq!(
            line1.width(),
            line2.width(),
            "Unicode and ASCII status should pad to same visual width"
        );

        // Case 3: Complex status with multiple emoji (git symbols + user marker)
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

        assert_eq!(line3.width(), 10);
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
                positive_digits: 2, // Allocates 3 chars: "+NN"
                negative_digits: 2,
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
        let clean1 = rendered1.ansi_strip().into_owned();
        assert_eq!(clean1, " +53  -7");

        // Test case 2: (33, 23) - both medium
        let result2 = format_diff_like_column(
            33,
            23,
            DiffColumnConfig {
                positive_digits: 2,
                negative_digits: 2,
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
        let clean2 = rendered2.ansi_strip().into_owned();
        assert_eq!(clean2, " +33 -23");

        // Test case 3: (2, 2) - both small (needs padding)
        let result3 = format_diff_like_column(
            2,
            2,
            DiffColumnConfig {
                positive_digits: 2,
                negative_digits: 2,
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
        let clean3 = rendered3.ansi_strip().into_owned();
        assert_eq!(clean3, "  +2  -2");

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
                positive_digits: 3,
                negative_digits: 3,
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
                positive_digits: 3,
                negative_digits: 3,
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
        assert!(rendered.contains("+1") && rendered.contains('K'));
        assert!(rendered.contains("500"));

        // Case 3: Negative overflow
        // Should show: "+500 -1K" (positive normal, negative with K suffix)
        let overflow_result2 = format_diff_like_column(
            500,
            1000,
            DiffColumnConfig {
                positive_digits: 3,
                negative_digits: 3,
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
        assert!(rendered2.contains("500"));
        assert!(rendered2.contains("-1") && rendered2.contains('K'));

        // Case 4: Extreme overflow (>= 10K values show ∞ to avoid false precision)
        let extreme_overflow = format_diff_like_column(
            100_000,
            200_000,
            DiffColumnConfig {
                positive_digits: 3,
                negative_digits: 3,
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
        assert!(extreme_rendered.contains("+∞"));
        assert!(extreme_rendered.contains("-∞"));

        // Test overflow with Arrows variant (↑ and ↓)
        let arrow_total = 7;

        // Case 5: Arrow positive overflow (100 exceeds 2 digits, max is 99)
        // Should show with K suffix (not repeated symbols)
        let arrow_overflow = format_diff_like_column(
            1000, // Use larger value to show K suffix
            50,
            DiffColumnConfig {
                positive_digits: 2,
                negative_digits: 2,
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
        assert!(arrow_rendered.contains("↑1") && arrow_rendered.contains('K'));
        assert!(arrow_rendered.contains("50"));

        // Case 6: Arrow negative overflow
        // Should show with K suffix
        let arrow_overflow2 = format_diff_like_column(
            50,
            1000, // Use larger value to show K suffix
            DiffColumnConfig {
                positive_digits: 2,
                negative_digits: 2,
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
        assert!(arrow_rendered2.contains("50"));
        assert!(arrow_rendered2.contains("↓1") && arrow_rendered2.contains('K'));
    }
}
