//! Column layout and priority allocation for the list command.
//!
//! # Status Column Structure
//!
//! The Status column uses a unified position-based grid system for all status
//! indicators including user-defined status.
//!
//! ## Unified Position Grid
//!
//! All status indicators use position-based alignment with selective rendering.
//! See [`super::model::StatusSymbols`] for the complete symbol list and categories.
//!
//! Only positions used by at least one row are included (position mask):
//! - Within those positions, symbols align vertically for scannability
//! - Empty positions render as single space for grid alignment
//! - No leading spaces before the first symbol
//!
//! Example with working_tree, main_state, and user_marker used:
//! ```text
//! Row 1: "   _🤖"   (working=space, main=_, user=🤖)
//! Row 2: "?! _  "   (working=?!, main=_, user=space)
//! Row 3: "    💬"   (working=space, main=space, user=💬)
//! ```
//!
//! ## Width Calculation
//!
//! ```text
//! status_width = max(rendered_width_across_all_items)
//! ```
//!
//! The width is calculated by rendering each item's status with the position
//! mask and taking the maximum width.
//!
//! ## Why This Design?
//!
//! **Single canonical system:**
//! - One alignment mechanism for all status indicators
//! - User marker treated consistently with git symbols
//!
//! **Eliminates wasted space:**
//! - Position mask removes columns for symbols that appear in zero rows
//! - User marker only takes space when present
//!
//! **Maintains alignment:**
//! - All symbols align vertically at their positions (vertical scannability)
//! - Grid adapts to minimize width based on active positions
//!
//! # Priority System Design
//!
//! ## Priority Scoring Model
//!
//! The allocation system uses a **priority scoring model**:
//! ```text
//! final_priority = base_priority + empty_penalty
//! ```
//!
//! **Base priorities** (1-11) are determined by **user need hierarchy** - what questions users need
//! answered when scanning worktrees:
//! - 1: Branch (identity - "what is this?")
//! - 2: Working diff (critical - "do I need to commit?")
//! - 3: Ahead/behind (critical - "am I out of sync?")
//! - 4-10: Context (work volume, states, path, time, CI, etc.)
//! - 11: Message (nice-to-have, space-hungry)
//!
//! **Empty penalty**: +10 if column has no data (only header)
//! - Empty working_diff: 2 + 10 = priority 12
//! - Empty ahead/behind: 3 + 10 = priority 13
//! - etc.
//!
//! This creates two effective priority tiers:
//! - **Tier 1 (priorities 1-11)**: Columns with actual data
//! - **Tier 2 (priorities 12-21)**: Empty columns (visual consistency)
//!
//! The empty penalty is large (+10) but not infinite, so empty columns maintain their relative
//! ordering (empty working_diff still ranks higher than empty ci_status) for visual consistency.
//!
//! ## Why This Design?
//!
//! **Problem**: Terminal width is limited. We must decide what to show.
//!
//! **Goals**:
//! 1. Show critical data (uncommitted changes, sync status) at any terminal width
//! 2. Show nice-to-have data (message, commit hash) when space allows
//! 3. Maintain visual consistency - empty columns in predictable positions at wide widths
//!
//! **Key decision**: Message sits at the boundary (priority 11). Empty columns (priority 12+)
//! rank below message, so:
//! - Narrow terminals: Data columns + message (hide empty columns)
//! - Wide terminals: Data columns + message + empty columns (visual consistency)
//!
//! ## Limitation: Progressive Mode
//!
//! The empty penalty system requires knowing whether columns have data, but progressive rendering
//! computes layout before data arrives. Currently we assume most columns have data (optimistic),
//! which means empty penalties don't apply in progressive mode.
//!
//! Exceptions that we can compute instantly from items:
//! - `path`: true only if any worktree has `branch_worktree_mismatch` (computed from items)
//! - `branch_diff`/`ci_status`: false if their required task is skipped
//!
//! Other columns (status, working_diff, ahead_behind, upstream) require expensive git operations,
//! so we assume they have data until proven otherwise.
//!
//! ## Special Cases
//!
//! Three columns have non-standard behavior that extends beyond the basic two-tier model:
//!
//! 1. **BranchDiff** - Visibility gate (`show_full` flag)
//!    - Hidden by default as too noisy for typical usage
//!    - Only allocated when `show_full=true` (match guard skips if false)
//!
//! 2. **CiStatus** - Visibility gate (`fetch_ci` flag)
//!    - Only shown when `fetch_ci=true` (when CI data was requested)
//!    - Bypasses the tier system entirely when `fetch_ci=false`
//!    - Within the visibility gate, follows normal two-tier priority (priority 9 with data, 19 when empty)
//!
//! 3. **Message** - Flexible sizing with post-allocation expansion
//!    - Allocated at priority 11 with flexible width (min 20, preferred 50)
//!    - After all columns allocated (including empty ones), expands up to max 100 using leftover space
//!    - Two-step process ensures critical columns get space before message grows
//!
//! ## Implementation
//!
//! The code implements this using a centralized registry and priority-based allocation:
//!
//! ```rust
//! // Build candidates from centralized COLUMN_SPECS registry
//! let mut candidates: Vec<ColumnCandidate> = COLUMN_SPECS
//!     .iter()
//!     .filter(|spec| /* visibility gates: show_full, fetch_ci */)
//!     .map(|spec| ColumnCandidate {
//!         spec,
//!         priority: if spec.kind.has_data(&data_flags) {
//!             spec.base_priority
//!         } else {
//!             spec.base_priority + EMPTY_PENALTY
//!         }
//!     })
//!     .collect();
//!
//! // Sort by final priority
//! candidates.sort_by_key(|candidate| candidate.priority);
//!
//! // Allocate columns in priority order, building pending list
//! for candidate in candidates {
//!     if candidate.spec.kind == ColumnKind::Message {
//!         // Special handling: flexible width (min 20, preferred 50)
//!     } else if let Some(ideal) = candidate.spec.kind.ideal(...) {
//!         if let allocated = try_allocate(&mut remaining, ideal.width, ...) {
//!             pending.push(PendingColumn { spec: candidate.spec, width: allocated, format: ideal.format });
//!         }
//!     }
//! }
//!
//! // Message post-allocation expansion (uses truly leftover space)
//! if let Some(message_col) = pending.iter_mut().find(|col| col.spec.kind == ColumnKind::Message) {
//!     message_col.width += remaining.min(MAX_MESSAGE - message_col.width);
//! }
//! ```
//!
//! **Benefits**:
//! - Column metadata centralized in `COLUMN_SPECS` registry (single source of truth)
//! - Priority calculation explicit (base_priority + conditional EMPTY_PENALTY)
//! - Single unified allocation loop (no phase duplication)
//! - Easy to understand: build candidates → sort by priority → allocate → expand message
//! - Extensible: can add new modifiers (terminal width bonus, user config) without restructuring
//!
//! ## Helper Functions
//!
//! - `fit_header()`: Ensures column width ≥ header width to prevent overflow
//! - `try_allocate()`: Attempts to allocate space, returns 0 if insufficient

use crate::display::get_terminal_width;
use anstyle::Style;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use unicode_width::UnicodeWidthStr;
use worktrunk::styling::{ADDITION, DELETION, Stream, supports_hyperlinks};

use super::collect::TaskKind;
use super::collect_progressive_impl::parse_port_from_url;
use super::columns::{COLUMN_SPECS, ColumnKind, ColumnSpec, column_display_index};

// Re-export DiffVariant for external use (e.g., select command)
pub use super::columns::DiffVariant;

/// Width of short commit hash display (first 8 hex characters)
const COMMIT_HASH_WIDTH: usize = 8;

/// Ensures a column width is at least as wide as its header.
///
/// This is the general solution for preventing header overflow: pass the header
/// string and the calculated data width, and this returns the larger of the two.
///
/// For empty columns (data_width = 0), returns header width. This allows empty
/// columns to be allocated at low priority (base_priority + EMPTY_PENALTY) for
/// visual consistency on wide terminals.
fn fit_header(header: &str, data_width: usize) -> usize {
    use unicode_width::UnicodeWidthStr;
    data_width.max(header.width())
}

/// Helper: Try to allocate space for a column. Returns the allocated width if successful.
/// Updates `remaining` by subtracting the allocated width + spacing.
/// If is_first is true, doesn't require spacing before the column.
///
/// The spacing is consumed from the budget (subtracted from `remaining`) but not returned
/// as part of the column's width, since the spacing appears before the column content.
fn try_allocate(
    remaining: &mut usize,
    ideal_width: usize,
    spacing: usize,
    is_first: bool,
) -> usize {
    if ideal_width == 0 {
        return 0;
    }
    let required = if is_first {
        ideal_width
    } else {
        ideal_width + spacing // Gap before column + column content
    };
    if *remaining < required {
        return 0;
    }
    *remaining = remaining.saturating_sub(required);
    ideal_width // Return just the column width
}

/// Width information for two-part columns: diffs ("+128 -147") and arrows ("↑6 ↓1")
#[derive(Clone, Copy, Debug)]
pub struct DiffWidths {
    pub total: usize,
    pub positive_digits: usize, // First part: +/↑/⇡
    pub negative_digits: usize, // Second part: -/↓/⇣
}

#[derive(Clone, Debug)]
pub struct ColumnWidths {
    pub branch: usize,
    pub status: usize, // Includes both git status symbols and user-defined status
    pub time: usize,
    pub url: usize,
    pub ci_status: usize,
    pub message: usize,
    pub ahead_behind: DiffWidths,
    pub working_diff: DiffWidths,
    pub branch_diff: DiffWidths,
    pub upstream: DiffWidths,
}

/// Tracks which columns have actual data (vs just headers)
#[derive(Clone, Copy, Debug)]
pub struct ColumnDataFlags {
    pub status: bool, // True if any item has git status symbols or user-defined status
    pub working_diff: bool,
    pub ahead_behind: bool,
    pub branch_diff: bool,
    pub upstream: bool,
    pub url: bool,
    pub ci_status: bool,
    pub path: bool, // True if any worktree has branch_worktree_mismatch
}

/// Layout metadata including position mask for Status column
#[derive(Clone, Debug)]
pub struct LayoutMetadata {
    pub widths: ColumnWidths,
    pub data_flags: ColumnDataFlags,
    pub status_position_mask: super::model::PositionMask,
}

const EMPTY_PENALTY: u8 = 10;

#[derive(Clone, Copy, Debug)]
pub struct DiffDisplayConfig {
    pub variant: DiffVariant,
    pub positive_style: Style,
    pub negative_style: Style,
    pub always_show_zeros: bool,
}

impl DiffDisplayConfig {
    /// Format diff values with fixed-width alignment for tabular display.
    ///
    /// Numbers are right-aligned within a 3-digit column width.
    /// Returns empty spaces if both values are zero (unless `always_show_zeros` is set).
    #[cfg(unix)] // Only used by select command which is unix-only
    pub fn format_aligned(&self, positive: usize, negative: usize) -> String {
        const DIGITS: usize = 3;
        let positive_width = 1 + DIGITS; // symbol + digits
        let negative_width = 1 + DIGITS;
        let total_width = positive_width + 1 + negative_width; // with separator

        let config = DiffColumnConfig {
            positive_digits: DIGITS,
            negative_digits: DIGITS,
            total_width,
            display: *self,
        };

        config.render_segment(positive, negative).render()
    }

    /// Format diff values as plain text with ANSI colors (no fixed-width alignment).
    ///
    /// Returns `None` if both values are zero (unless `always_show_zeros` is set).
    /// Format: `+N -M` with appropriate colors for each component.
    pub fn format_plain(&self, positive: usize, negative: usize) -> Option<String> {
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

#[derive(Clone, Copy)]
pub(super) struct DiffSymbols {
    pub(super) positive: &'static str,
    pub(super) negative: &'static str,
}

impl DiffVariant {
    pub(super) fn symbols(self) -> DiffSymbols {
        match self {
            DiffVariant::Signs => DiffSymbols {
                positive: "+",
                negative: "-",
            },
            DiffVariant::Arrows => DiffSymbols {
                positive: "↑",
                negative: "↓",
            },
            DiffVariant::UpstreamArrows => DiffSymbols {
                positive: "⇡",
                negative: "⇣",
            },
        }
    }
}

impl ColumnKind {
    pub fn diff_display_config(self) -> Option<DiffDisplayConfig> {
        match self {
            ColumnKind::WorkingDiff | ColumnKind::BranchDiff => Some(DiffDisplayConfig {
                variant: DiffVariant::Signs,
                positive_style: ADDITION,
                negative_style: DELETION,
                always_show_zeros: false,
            }),
            ColumnKind::AheadBehind => Some(DiffDisplayConfig {
                variant: DiffVariant::Arrows,
                positive_style: ADDITION,
                negative_style: DELETION.dimmed(),
                always_show_zeros: false,
            }),
            ColumnKind::Upstream => Some(DiffDisplayConfig {
                variant: DiffVariant::UpstreamArrows,
                positive_style: ADDITION,
                negative_style: DELETION.dimmed(),
                always_show_zeros: false, // 0/0 case handled specially with | symbol
            }),
            _ => None,
        }
    }

    /// Format diff-style values as plain text with ANSI colors (for json-pretty).
    pub(crate) fn format_diff_plain(self, positive: usize, negative: usize) -> Option<String> {
        let config = self.diff_display_config()?;
        config.format_plain(positive, negative)
    }

    pub fn has_data(self, flags: &ColumnDataFlags) -> bool {
        match self {
            ColumnKind::Gutter => true, // Always present (shows @ ^ + or space)
            ColumnKind::Branch => true,
            ColumnKind::Status => flags.status,
            ColumnKind::WorkingDiff => flags.working_diff,
            ColumnKind::AheadBehind => flags.ahead_behind,
            ColumnKind::BranchDiff => flags.branch_diff,
            ColumnKind::Path => flags.path,
            ColumnKind::Upstream => flags.upstream,
            ColumnKind::Url => flags.url,
            ColumnKind::Time => true,
            ColumnKind::CiStatus => flags.ci_status,
            ColumnKind::Commit => true,
            ColumnKind::Message => true,
        }
    }

    /// Returns the ideal (width, format) for this column, or None if width is 0 or Message.
    fn ideal(
        self,
        widths: &ColumnWidths,
        max_path_width: usize,
        commit_width: usize,
    ) -> Option<(usize, ColumnFormat)> {
        let text = |w: usize| (w > 0).then_some((w, ColumnFormat::Text));
        let diff = |dw: DiffWidths| -> Option<(usize, ColumnFormat)> {
            if dw.total == 0 {
                return None;
            }
            let display = self.diff_display_config()?;
            Some((
                dw.total,
                ColumnFormat::Diff(DiffColumnConfig {
                    positive_digits: dw.positive_digits,
                    negative_digits: dw.negative_digits,
                    total_width: dw.total,
                    display,
                }),
            ))
        };

        match self {
            ColumnKind::Gutter => text(2), // Fixed width: symbol (1 char) + space (1 char)
            ColumnKind::Branch => text(widths.branch),
            ColumnKind::Status => text(widths.status),
            ColumnKind::Path => text(max_path_width),
            ColumnKind::Time => text(widths.time),
            ColumnKind::Url => text(widths.url),
            ColumnKind::CiStatus => text(widths.ci_status),
            ColumnKind::Commit => text(commit_width),
            ColumnKind::Message => None,
            ColumnKind::WorkingDiff => diff(widths.working_diff),
            ColumnKind::AheadBehind => diff(widths.ahead_behind),
            ColumnKind::BranchDiff => diff(widths.branch_diff),
            ColumnKind::Upstream => diff(widths.upstream),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum ColumnFormat {
    Text,
    Diff(DiffColumnConfig),
}

#[derive(Clone, Copy, Debug)]
pub struct DiffColumnConfig {
    pub positive_digits: usize,
    pub negative_digits: usize,
    pub total_width: usize,
    pub display: DiffDisplayConfig,
}

#[derive(Clone, Debug)]
pub struct ColumnLayout {
    pub kind: ColumnKind,
    pub header: &'static str,
    pub start: usize,
    pub width: usize,
    pub format: ColumnFormat,
}

pub struct LayoutConfig {
    pub columns: Vec<ColumnLayout>,
    pub main_worktree_path: PathBuf,
    pub max_message_len: usize,
    pub hidden_column_count: usize,
    pub status_position_mask: super::model::PositionMask,
}

#[derive(Clone, Copy)]
struct ColumnCandidate<'a> {
    spec: &'a ColumnSpec,
    priority: u8,
}

#[derive(Clone, Copy)]
struct PendingColumn<'a> {
    spec: &'a ColumnSpec,
    width: usize,
    format: ColumnFormat,
}

/// Estimate URL column width using heuristics.
///
/// When hyperlinks are supported, URLs display as `:PORT` (6 chars for 5-digit ports).
/// Otherwise, estimates full URL width from template structure.
fn estimate_url_width(url_template: Option<&str>, hyperlinks_supported: bool) -> usize {
    let Some(template) = url_template else {
        return 0;
    };

    // When hyperlinks are supported, URLs with ports display as ":XXXXX" (6 chars)
    if hyperlinks_supported {
        // Check for port patterns: template variables or static ports
        if template.contains("hash_port")
            || template.contains(":{{")
            || parse_port_from_url(template).is_some()
        {
            return 6; // ":12345"
        }
    }

    // Fallback: estimate full URL width from template structure
    // {{ branch | hash_port }} becomes a 5-digit port (10000-19999)
    // {{ branch }} becomes the branch name (unknown length, use 10 as average)
    template
        .replace("{{ branch | hash_port }}", "12345")
        .replace("{{ branch }}", "feature-xx")
        .len()
}

/// Build pre-allocated column width estimates.
///
/// Uses generous fixed allocations for expensive-to-compute columns (status, diffs, time, CI)
/// that handle overflow with compact notation (K suffix). This provides consistent layout
/// without requiring a data scan.
fn build_estimated_widths(
    max_branch: usize,
    skip_tasks: &HashSet<TaskKind>,
    has_branch_worktree_mismatch: bool,
    url_width: usize,
) -> LayoutMetadata {
    // Fixed widths for slow columns (require expensive git operations)
    // Values exceeding these widths use compact notation (K suffix)
    //
    // Status column: Must match PositionMask::FULL width for consistent alignment
    // PositionMask::FULL allocates: 1+1+1+1+1+1+2 = 8 chars (7 positions)
    let status_fixed = fit_header(ColumnKind::Status.header(), 8);
    let working_diff_fixed = fit_header(ColumnKind::WorkingDiff.header(), 9); // "+999 -999"
    let ahead_behind_fixed = fit_header(ColumnKind::AheadBehind.header(), 7); // "↑99 ↓99"
    let branch_diff_fixed = fit_header(ColumnKind::BranchDiff.header(), 9); // "+999 -999"
    let upstream_fixed = fit_header(ColumnKind::Upstream.header(), 7); // "↑99 ↓99"
    let age_estimate = 4; // "11mo" (short format)
    let ci_estimate = fit_header(ColumnKind::CiStatus.header(), 1); // Single indicator symbol

    // Assume columns will have data (better to show and hide than to not show).
    // This is a limitation of progressive mode - we can't know which columns have data
    // before the data arrives, so empty penalties don't apply properly.
    //
    // Exceptions that we can compute instantly from items:
    // - path: true only if any worktree has branch_worktree_mismatch
    // - branch_diff/ci_status: false if their required task is skipped
    let data_flags = ColumnDataFlags {
        status: true,
        working_diff: true,
        ahead_behind: true,
        branch_diff: !skip_tasks.contains(&TaskKind::BranchDiff),
        upstream: true,
        url: !skip_tasks.contains(&TaskKind::UrlStatus),
        ci_status: !skip_tasks.contains(&TaskKind::CiStatus),
        path: has_branch_worktree_mismatch,
    };

    // URL width estimated from template + longest branch (or fallback)
    // When url_width is 0 (no template), don't allocate any space for URL column
    let url_estimate = if url_width > 0 {
        fit_header(ColumnKind::Url.header(), url_width)
    } else {
        0
    };

    let widths = ColumnWidths {
        branch: max_branch,
        status: status_fixed,
        time: age_estimate,
        url: url_estimate,
        ci_status: ci_estimate,
        message: 50, // Will be flexible during allocation
        // Commit counts (Arrows): compact notation, 2 digits covers up to 99
        ahead_behind: DiffWidths {
            total: ahead_behind_fixed,
            positive_digits: 2,
            negative_digits: 2,
        },
        // Line diffs (Signs): show full numbers, 3 digits covers up to 999
        working_diff: DiffWidths {
            total: working_diff_fixed,
            positive_digits: 3,
            negative_digits: 3,
        },
        branch_diff: DiffWidths {
            total: branch_diff_fixed,
            positive_digits: 3,
            negative_digits: 3,
        },
        // Upstream (Arrows): compact notation, 2 digits covers up to 99
        upstream: DiffWidths {
            total: upstream_fixed,
            positive_digits: 2,
            negative_digits: 2,
        },
    };

    LayoutMetadata {
        widths,
        data_flags,
        status_position_mask: super::model::PositionMask::FULL,
    }
}

/// Allocate columns using priority-based allocation logic.
///
/// This is the core allocation algorithm used by `calculate_layout_from_basics()`
/// with pre-allocated width estimates for expensive-to-compute columns.
fn allocate_columns_with_priority(
    metadata: &LayoutMetadata,
    skip_tasks: &HashSet<TaskKind>,
    max_path_width: usize,
    commit_width: usize,
    terminal_width: usize,
    main_worktree_path: PathBuf,
) -> LayoutConfig {
    let spacing = 2;
    let mut remaining = terminal_width;

    // Build candidates with priorities
    // Filter out columns whose required task is being skipped
    let mut candidates: Vec<ColumnCandidate> = COLUMN_SPECS
        .iter()
        .filter(|spec| {
            spec.requires_task
                .is_none_or(|task| !skip_tasks.contains(&task))
        })
        .map(|spec| ColumnCandidate {
            spec,
            priority: if spec.kind.has_data(&metadata.data_flags) {
                spec.base_priority
            } else {
                spec.base_priority + EMPTY_PENALTY
            },
        })
        .collect();

    candidates.sort_by_key(|candidate| candidate.priority);

    // Store which candidates have data for later calculation of hidden columns
    let candidates_with_data: Vec<_> = candidates
        .iter()
        .map(|c| (c.spec.kind, c.spec.kind.has_data(&metadata.data_flags)))
        .collect();

    const MIN_MESSAGE: usize = 10;
    const MAX_MESSAGE: usize = 100;

    let mut pending: Vec<PendingColumn> = Vec::new();

    // Helper: check if spacing should be skipped (first column, or previous was Gutter)
    let needs_spacing = |pending: &[PendingColumn]| -> bool {
        if pending.is_empty() {
            return false;
        }
        // No gap after Gutter - its content includes the spacing
        if pending.last().map(|c| c.spec.kind) == Some(ColumnKind::Gutter) {
            return false;
        }
        true
    };

    // Allocate columns in priority order
    for candidate in candidates {
        let spec = candidate.spec;

        // Special handling for Message column
        if spec.kind == ColumnKind::Message {
            let spacing_cost = if needs_spacing(&pending) { spacing } else { 0 };

            if remaining <= spacing_cost {
                continue;
            }

            let available = remaining - spacing_cost;
            let mut message_width = 0;

            // Allocate at minimum width initially. Post-allocation expansion will
            // bring it up to preferred/max width after empty columns have a chance
            // to be allocated.
            if available >= MIN_MESSAGE {
                message_width = MIN_MESSAGE.min(metadata.widths.message);
            }

            if message_width > 0 {
                remaining = remaining.saturating_sub(message_width + spacing_cost);
                pending.push(PendingColumn {
                    spec,
                    width: message_width,
                    format: ColumnFormat::Text,
                });
            }

            continue;
        }

        // For non-message columns
        let Some((ideal_width, format)) =
            spec.kind
                .ideal(&metadata.widths, max_path_width, commit_width)
        else {
            continue;
        };

        let skip_spacing = !needs_spacing(&pending);
        let allocated = try_allocate(&mut remaining, ideal_width, spacing, skip_spacing);
        if allocated > 0 {
            pending.push(PendingColumn {
                spec,
                width: allocated,
                format,
            });
        }
    }

    // Expand message column with leftover space
    let mut max_message_len = 0;
    if let Some(message_col) = pending
        .iter_mut()
        .find(|col| col.spec.kind == ColumnKind::Message)
    {
        if message_col.width < MAX_MESSAGE && remaining > 0 {
            let expansion = remaining.min(MAX_MESSAGE - message_col.width);
            message_col.width += expansion;
        }
        max_message_len = message_col.width;
    }

    // Sort by display order to maintain correct visual order
    pending.sort_by_key(|col| column_display_index(col.spec.kind));

    // Build final column layouts with positions
    let gap = 2;
    let mut position = 0;
    let mut columns = Vec::new();

    for col in pending {
        let start = if columns.is_empty() {
            0
        } else {
            // No gap after gutter column - its content includes the spacing
            let prev_was_gutter = columns
                .last()
                .map(|c: &ColumnLayout| c.kind == ColumnKind::Gutter)
                .unwrap_or(false);
            if prev_was_gutter {
                position
            } else {
                position + gap
            }
        };
        position = start + col.width;

        columns.push(ColumnLayout {
            kind: col.spec.kind,
            header: col.spec.kind.header(),
            start,
            width: col.width,
            format: col.format,
        });
    }

    // Count how many columns were hidden (not allocated).
    // This includes both data columns and empty columns that could show with more width.
    let allocated_kinds: std::collections::HashSet<_> =
        columns.iter().map(|col| col.kind).collect();
    let hidden_column_count = candidates_with_data
        .iter()
        .filter(|(kind, _has_data)| !allocated_kinds.contains(kind))
        .count();

    LayoutConfig {
        columns,
        main_worktree_path,
        max_message_len,
        hidden_column_count,
        status_position_mask: metadata.status_position_mask,
    }
}

/// Calculate responsive layout from basic worktree info.
///
/// Uses pre-allocated width estimates for expensive-to-compute columns (status, diffs, time, CI).
/// This is faster than scanning all data and provides consistent layout between buffered and
/// progressive modes. Values exceeding estimates use compact notation (K suffix).
///
/// Fast to compute from actual data:
/// - Branch names (from worktrees and standalone branches)
/// - Paths (relative to main worktree)
///
/// Pre-allocated estimates (generous to minimize truncation):
/// - Status: 8 chars (PositionMask::FULL, 7 positions)
/// - Working diff: 9 chars ("+999 -999")
/// - Ahead/behind: 7 chars ("↑99 ↓99")
/// - Branch diff: 9 chars ("+999 -999")
/// - Upstream: 7 chars ("↑99 ↓99")
/// - Age: 4 chars ("11mo" short format)
/// - CI: 1 char (indicator symbol)
/// - Message: flexible (20-100 chars)
/// - URL: estimated from template + longest branch
pub fn calculate_layout_from_basics(
    items: &[super::model::ListItem],
    skip_tasks: &HashSet<TaskKind>,
    main_worktree_path: &Path,
    url_template: Option<&str>,
) -> LayoutConfig {
    calculate_layout_with_width(
        items,
        skip_tasks,
        get_terminal_width(),
        main_worktree_path,
        url_template,
    )
}

/// Calculate layout with explicit width (for contexts like skim where available width differs)
pub fn calculate_layout_with_width(
    items: &[super::model::ListItem],
    skip_tasks: &HashSet<TaskKind>,
    terminal_width: usize,
    main_worktree_path: &Path,
    url_template: Option<&str>,
) -> LayoutConfig {
    // Calculate actual widths for things we know
    // Include branch names from both worktrees and standalone branches
    let longest_branch = items
        .iter()
        .filter_map(|item| item.branch.as_deref())
        .max_by_key(|b| b.width());

    let max_branch = longest_branch.map(|b| b.width()).unwrap_or(0);
    let max_branch = fit_header(ColumnKind::Branch.header(), max_branch);

    let path_data_width = items
        .iter()
        .filter_map(|item| item.worktree_path())
        .map(|path| {
            use crate::display::shorten_path;
            shorten_path(path.as_path(), main_worktree_path).width()
        })
        .max()
        .unwrap_or(0);
    let max_path_width = fit_header(ColumnKind::Path.header(), path_data_width);

    // Check if any worktree has a branch-worktree mismatch.
    // Path column is only useful when there's a mismatch; otherwise it's redundant with branch.
    let has_branch_worktree_mismatch = items
        .iter()
        .filter_map(|item| item.worktree_data())
        .any(|data| data.branch_worktree_mismatch);

    // Estimate URL width from template (heuristic, no expansion needed)
    let url_width = estimate_url_width(url_template, supports_hyperlinks(Stream::Stdout));

    // Build pre-allocated width estimates (same as buffered mode)
    let metadata = build_estimated_widths(
        max_branch,
        skip_tasks,
        has_branch_worktree_mismatch,
        url_width,
    );

    let commit_width = fit_header(ColumnKind::Commit.header(), COMMIT_HASH_WIDTH);

    allocate_columns_with_priority(
        &metadata,
        skip_tasks,
        max_path_width,
        commit_width,
        terminal_width,
        main_worktree_path.to_path_buf(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::list::columns::ColumnKind;
    use std::path::PathBuf;
    use worktrunk::git::LineDiff;

    #[test]
    fn test_fit_header() {
        // Data wider than header - return data width
        assert_eq!(fit_header("Age", 10), 10);

        // Header wider than data - return header width
        assert_eq!(fit_header("Branch", 3), 6);

        // Empty data - return header width
        assert_eq!(fit_header("Status", 0), 6);

        // Equal widths
        assert_eq!(fit_header("Path", 4), 4);
    }

    #[test]
    fn test_try_allocate() {
        // First column doesn't need spacing
        let mut remaining = 100;
        let allocated = try_allocate(&mut remaining, 20, 2, true);
        assert_eq!(allocated, 20);
        assert_eq!(remaining, 80);

        // Subsequent columns need spacing
        let allocated = try_allocate(&mut remaining, 15, 2, false);
        assert_eq!(allocated, 15);
        assert_eq!(remaining, 63); // 80 - 15 - 2

        // Zero width returns 0
        let mut remaining = 50;
        assert_eq!(try_allocate(&mut remaining, 0, 2, false), 0);
        assert_eq!(remaining, 50);

        // Insufficient space returns 0
        let mut remaining = 10;
        assert_eq!(try_allocate(&mut remaining, 20, 2, false), 0);
        assert_eq!(remaining, 10);
    }

    #[test]
    fn test_column_kind_has_data() {
        let all_true = ColumnDataFlags {
            status: true,
            working_diff: true,
            ahead_behind: true,
            branch_diff: true,
            upstream: true,
            url: true,
            ci_status: true,
            path: true,
        };
        let all_false = ColumnDataFlags {
            status: false,
            working_diff: false,
            ahead_behind: false,
            branch_diff: false,
            upstream: false,
            url: false,
            ci_status: false,
            path: false,
        };

        // Always-have-data columns
        assert!(ColumnKind::Gutter.has_data(&all_false));
        assert!(ColumnKind::Branch.has_data(&all_false));
        assert!(ColumnKind::Time.has_data(&all_false));
        assert!(ColumnKind::Commit.has_data(&all_false));
        assert!(ColumnKind::Message.has_data(&all_false));

        // Flag-dependent columns
        assert!(ColumnKind::Status.has_data(&all_true));
        assert!(!ColumnKind::Status.has_data(&all_false));
        assert!(ColumnKind::WorkingDiff.has_data(&all_true));
        assert!(!ColumnKind::WorkingDiff.has_data(&all_false));
        assert!(ColumnKind::AheadBehind.has_data(&all_true));
        assert!(!ColumnKind::AheadBehind.has_data(&all_false));
        assert!(ColumnKind::BranchDiff.has_data(&all_true));
        assert!(!ColumnKind::BranchDiff.has_data(&all_false));
        assert!(ColumnKind::Upstream.has_data(&all_true));
        assert!(!ColumnKind::Upstream.has_data(&all_false));
        assert!(ColumnKind::Url.has_data(&all_true));
        assert!(!ColumnKind::Url.has_data(&all_false));
        assert!(ColumnKind::CiStatus.has_data(&all_true));
        assert!(!ColumnKind::CiStatus.has_data(&all_false));
        assert!(ColumnKind::Path.has_data(&all_true));
        assert!(!ColumnKind::Path.has_data(&all_false));
    }

    #[test]
    fn test_column_kind_diff_display_config() {
        // Diff columns have config
        assert!(ColumnKind::WorkingDiff.diff_display_config().is_some());
        assert!(ColumnKind::BranchDiff.diff_display_config().is_some());
        assert!(ColumnKind::AheadBehind.diff_display_config().is_some());
        assert!(ColumnKind::Upstream.diff_display_config().is_some());

        // Non-diff columns don't have config
        assert!(ColumnKind::Branch.diff_display_config().is_none());
        assert!(ColumnKind::Status.diff_display_config().is_none());
        assert!(ColumnKind::Path.diff_display_config().is_none());
        assert!(ColumnKind::Time.diff_display_config().is_none());
        assert!(ColumnKind::Message.diff_display_config().is_none());
        assert!(ColumnKind::Commit.diff_display_config().is_none());
        assert!(ColumnKind::CiStatus.diff_display_config().is_none());

        // Check variants
        let working = ColumnKind::WorkingDiff.diff_display_config().unwrap();
        assert!(matches!(working.variant, DiffVariant::Signs));

        let ahead = ColumnKind::AheadBehind.diff_display_config().unwrap();
        assert!(matches!(ahead.variant, DiffVariant::Arrows));

        let upstream = ColumnKind::Upstream.diff_display_config().unwrap();
        assert!(matches!(upstream.variant, DiffVariant::UpstreamArrows));
    }

    #[test]
    fn test_column_kind_ideal() {
        let widths = ColumnWidths {
            branch: 15,
            status: 8,
            time: 4,
            url: 0,
            ci_status: 2,
            message: 50,
            ahead_behind: DiffWidths {
                total: 7,
                positive_digits: 2,
                negative_digits: 2,
            },
            working_diff: DiffWidths {
                total: 9,
                positive_digits: 3,
                negative_digits: 3,
            },
            branch_diff: DiffWidths {
                total: 9,
                positive_digits: 3,
                negative_digits: 3,
            },
            upstream: DiffWidths {
                total: 7,
                positive_digits: 2,
                negative_digits: 2,
            },
        };

        // Text columns return (width, ColumnFormat::Text)
        let (w, fmt) = ColumnKind::Gutter.ideal(&widths, 20, 8).unwrap();
        assert_eq!(w, 2);
        assert!(matches!(fmt, ColumnFormat::Text));

        let (w, fmt) = ColumnKind::Branch.ideal(&widths, 20, 8).unwrap();
        assert_eq!(w, 15);
        assert!(matches!(fmt, ColumnFormat::Text));

        let (w, fmt) = ColumnKind::Status.ideal(&widths, 20, 8).unwrap();
        assert_eq!(w, 8);
        assert!(matches!(fmt, ColumnFormat::Text));

        let (w, fmt) = ColumnKind::Path.ideal(&widths, 20, 8).unwrap();
        assert_eq!(w, 20);
        assert!(matches!(fmt, ColumnFormat::Text));

        let (w, fmt) = ColumnKind::Time.ideal(&widths, 20, 8).unwrap();
        assert_eq!(w, 4);
        assert!(matches!(fmt, ColumnFormat::Text));

        let (w, fmt) = ColumnKind::Commit.ideal(&widths, 20, 8).unwrap();
        assert_eq!(w, 8);
        assert!(matches!(fmt, ColumnFormat::Text));

        // Message returns None (handled specially)
        assert!(ColumnKind::Message.ideal(&widths, 20, 8).is_none());

        // Diff columns return (width, ColumnFormat::Diff(_))
        let (w, fmt) = ColumnKind::WorkingDiff.ideal(&widths, 20, 8).unwrap();
        assert_eq!(w, 9);
        assert!(matches!(fmt, ColumnFormat::Diff(_)));

        let (w, fmt) = ColumnKind::AheadBehind.ideal(&widths, 20, 8).unwrap();
        assert_eq!(w, 7);
        assert!(matches!(fmt, ColumnFormat::Diff(_)));

        // Zero width returns None
        let zero_widths = ColumnWidths {
            branch: 0,
            status: 0,
            time: 0,
            url: 0,
            ci_status: 0,
            message: 0,
            ahead_behind: DiffWidths {
                total: 0,
                positive_digits: 0,
                negative_digits: 0,
            },
            working_diff: DiffWidths {
                total: 0,
                positive_digits: 0,
                negative_digits: 0,
            },
            branch_diff: DiffWidths {
                total: 0,
                positive_digits: 0,
                negative_digits: 0,
            },
            upstream: DiffWidths {
                total: 0,
                positive_digits: 0,
                negative_digits: 0,
            },
        };
        assert!(ColumnKind::Branch.ideal(&zero_widths, 0, 0).is_none());
        assert!(ColumnKind::WorkingDiff.ideal(&zero_widths, 0, 0).is_none());
    }

    #[test]
    fn test_pre_allocated_width_estimates() {
        // Test that build_estimated_widths() returns correct pre-allocated estimates
        // Empty skip set means all tasks are computed (equivalent to --full)
        // has_branch_worktree_mismatch=true to test the path flag is passed through
        // url_width=0 since we're not testing URL column here
        let metadata = build_estimated_widths(20, &HashSet::new(), true, 0);
        let widths = metadata.widths;

        // Line diffs (Signs variant: +/-) allocate 3 digits for 100-999 range
        // Format: "+999 -999" = 1+3+1+1+3 = 9, header "HEAD±" is 5, so total is 9
        assert_eq!(
            widths.working_diff.total, 9,
            "Working diff should pre-allocate for '+999 -999' (9 chars)"
        );
        assert_eq!(
            widths.working_diff.positive_digits, 3,
            "Pre-allocated for 3-digit positive count"
        );
        assert_eq!(
            widths.working_diff.negative_digits, 3,
            "Pre-allocated for 3-digit negative count"
        );

        // Branch diff also uses Signs variant when show_full=true
        // Format: "+999 -999" = 9, header "main…±" is 6, so total is 9
        assert_eq!(
            widths.branch_diff.total, 9,
            "Branch diff should pre-allocate for '+999 -999' (9 chars)"
        );
        assert_eq!(
            widths.branch_diff.positive_digits, 3,
            "Pre-allocated for 3-digit positive count"
        );
        assert_eq!(
            widths.branch_diff.negative_digits, 3,
            "Pre-allocated for 3-digit negative count"
        );

        // Commit counts (Arrows variant: ↑↓) use compact notation, allocate 2 digits
        // Format: "↑99 ↓99" = 1+2+1+1+2 = 7, header "main↕" is 5, so total is 7
        assert_eq!(
            widths.ahead_behind.total, 7,
            "Ahead/behind should pre-allocate for '↑99 ↓99' (7 chars)"
        );
        assert_eq!(
            widths.ahead_behind.positive_digits, 2,
            "Pre-allocated for 2-digit positive count (uses compact notation)"
        );
        assert_eq!(
            widths.ahead_behind.negative_digits, 2,
            "Pre-allocated for 2-digit negative count (uses compact notation)"
        );

        // Upstream also uses Arrows variant
        // Format: "↑99 ↓99" = 7, header "Remote⇅" is 7, so total is 7
        assert_eq!(
            widths.upstream.total, 7,
            "Upstream should pre-allocate for '↑99 ↓99' (7 chars)"
        );
        assert_eq!(
            widths.upstream.positive_digits, 2,
            "Pre-allocated for 2-digit positive count"
        );
        assert_eq!(
            widths.upstream.negative_digits, 2,
            "Pre-allocated for 2-digit negative count"
        );
    }

    #[test]
    fn test_visible_columns_follow_gap_rule() {
        use crate::commands::list::model::{
            AheadBehind, BranchDiffTotals, CommitDetails, DisplayFields, GitOperationState,
            ItemKind, ListItem, StatusSymbols, UpstreamStatus, WorktreeData,
        };

        // Create test data with specific widths to verify position calculation
        let item = ListItem {
            head: "abc12345".to_string(),
            branch: Some("feature".to_string()),
            commit: Some(CommitDetails {
                timestamp: 1234567890,
                commit_message: "Test commit message".to_string(),
            }),
            counts: Some(AheadBehind {
                ahead: 5,
                behind: 10,
            }),
            branch_diff: Some(BranchDiffTotals {
                diff: LineDiff::from((200, 30)),
            }),
            committed_trees_match: Some(false),
            has_file_changes: Some(true),
            would_merge_add: None,
            is_ancestor: None,
            is_orphan: None,
            upstream: Some(UpstreamStatus {
                remote: Some("origin".to_string()),
                ahead: 4,
                behind: 2,
            }),
            pr_status: None,
            url: None,
            url_active: None,
            status_symbols: Some(StatusSymbols::default()),
            display: DisplayFields::default(),
            kind: ItemKind::Worktree(Box::new(WorktreeData {
                path: PathBuf::from("/test/path"),
                detached: false,
                locked: None,
                prunable: None,
                working_tree_diff: Some(LineDiff::from((100, 50))),
                git_operation: GitOperationState::None,
                is_main: false,
                is_current: false,
                is_previous: false,
                branch_worktree_mismatch: false,
                working_diff_display: None,
            })),
        };

        let items = vec![item];
        let skip_tasks: HashSet<TaskKind> = [TaskKind::BranchDiff, TaskKind::CiStatus]
            .into_iter()
            .collect();
        let main_worktree_path = PathBuf::from("/test");
        let layout = calculate_layout_from_basics(&items, &skip_tasks, &main_worktree_path, None);

        assert!(
            !layout.columns.is_empty(),
            "At least one column should be visible"
        );

        let mut columns_iter = layout.columns.iter();
        let first = columns_iter.next().expect("gutter column should exist");
        assert_eq!(
            first.kind,
            ColumnKind::Gutter,
            "Gutter column should be first"
        );
        assert_eq!(first.start, 0, "Gutter should begin at position 0");

        let mut previous_end = first.start + first.width;
        let mut prev_kind = first.kind;
        for column in columns_iter {
            // No gap after gutter column - its content includes the spacing
            let expected_gap = if prev_kind == ColumnKind::Gutter {
                0
            } else {
                2
            };
            assert_eq!(
                column.start,
                previous_end + expected_gap,
                "Columns should be separated by expected gap (0 after gutter, 2 otherwise)"
            );
            previous_end = column.start + column.width;
            prev_kind = column.kind;
        }

        // Path may or may not be visible depending on terminal width
        // At narrow widths (80 columns default in tests), Path may not fit
        if let Some(path_column) = layout
            .columns
            .iter()
            .find(|col| col.kind == ColumnKind::Path)
        {
            assert!(path_column.width > 0, "Path column must have width > 0");
        }
    }

    #[test]
    fn test_column_positions_with_empty_columns() {
        use crate::commands::list::model::{
            AheadBehind, BranchDiffTotals, CommitDetails, DisplayFields, GitOperationState,
            ItemKind, ListItem, StatusSymbols, UpstreamStatus, WorktreeData,
        };

        // Create minimal data - most columns will be empty
        let item = ListItem {
            head: "abc12345".to_string(),
            branch: Some("main".to_string()),
            commit: Some(CommitDetails {
                timestamp: 1234567890,
                commit_message: "Test".to_string(),
            }),
            counts: Some(AheadBehind {
                ahead: 0,
                behind: 0,
            }),
            branch_diff: Some(BranchDiffTotals {
                diff: LineDiff::default(),
            }),
            committed_trees_match: Some(false),
            has_file_changes: Some(true),
            would_merge_add: None,
            is_ancestor: None,
            is_orphan: None,
            upstream: Some(UpstreamStatus::default()),
            pr_status: None,
            url: None,
            url_active: None,
            status_symbols: Some(StatusSymbols::default()),
            display: DisplayFields::default(),
            kind: ItemKind::Worktree(Box::new(WorktreeData {
                path: PathBuf::from("/test"),
                detached: false,
                locked: None,
                prunable: None,
                working_tree_diff: Some(LineDiff::default()),
                git_operation: GitOperationState::None,
                is_main: true, // Primary worktree: no ahead/behind shown
                is_current: false,
                is_previous: false,
                branch_worktree_mismatch: false,
                working_diff_display: None,
            })),
        };

        let items = vec![item];
        let skip_tasks: HashSet<TaskKind> = [TaskKind::BranchDiff, TaskKind::CiStatus]
            .into_iter()
            .collect();
        let main_worktree_path = PathBuf::from("/home/user/project");
        let layout = calculate_layout_from_basics(&items, &skip_tasks, &main_worktree_path, None);

        assert!(
            layout
                .columns
                .first()
                .map(|col| col.kind == ColumnKind::Gutter && col.start == 0)
                .unwrap_or(false),
            "Gutter column should start at position 0"
        );

        // Path visibility depends on terminal width and column priorities
        // At narrow widths (80 columns default in tests), Path may not fit
    }

    #[test]
    fn test_estimate_url_width_no_template() {
        assert_eq!(estimate_url_width(None, false), 0);
        assert_eq!(estimate_url_width(None, true), 0);
    }

    #[test]
    fn test_estimate_url_width_with_hash_port() {
        let template = "http://localhost:{{ branch | hash_port }}";

        // Without hyperlinks: full URL width from template expansion
        // Replaces {{ branch | hash_port }} with "12345" → "http://localhost:12345" = 22
        assert_eq!(estimate_url_width(Some(template), false), 22);

        // With hyperlinks: compact port display ":12345" = 6
        assert_eq!(estimate_url_width(Some(template), true), 6);
    }

    #[test]
    fn test_estimate_url_width_with_branch_variable() {
        // Template with branch variable but no port
        let template = "http://localhost/{{ branch }}";

        // Without hyperlinks: full URL width from template expansion
        // Replaces {{ branch }} with "feature-xx" → "http://localhost/feature-xx" = 27
        assert_eq!(estimate_url_width(Some(template), false), 27);

        // With hyperlinks: no port pattern, so still uses template estimation
        assert_eq!(estimate_url_width(Some(template), true), 27);
    }

    #[test]
    fn test_estimate_url_width_static_template() {
        let template = "http://localhost:3000";

        // Without hyperlinks: template length = 21
        assert_eq!(estimate_url_width(Some(template), false), 21);

        // With hyperlinks: has static port, compact display ":3000" = 6
        assert_eq!(estimate_url_width(Some(template), true), 6);
    }

    #[test]
    fn test_estimate_url_width_port_pattern() {
        let template = "http://localhost:{{ port }}";

        // Without hyperlinks: template length (no branch/hash_port replacement)
        assert_eq!(estimate_url_width(Some(template), false), template.len());

        // With hyperlinks: has ":{{" pattern, compact display = 6
        assert_eq!(estimate_url_width(Some(template), true), 6);
    }
}
