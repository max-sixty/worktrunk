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
//! Example with branch_state, working_tree, and user_status used:
//! ```text
//! Row 1: "≡   🤖"   (branch=≡, working=space, user=🤖)
//! Row 2: "≡?!   "   (branch=≡, working=?!, user=space)
//! Row 3: "  💬"     (branch=space, working=space, user=💬)
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
//! - User status treated consistently with git symbols
//!
//! **Eliminates wasted space:**
//! - Position mask removes columns for symbols that appear in zero rows
//! - User status only takes space when present
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

use crate::display::{find_common_prefix, get_terminal_width};
use anstyle::Style;
use std::path::{Path, PathBuf};
use unicode_width::UnicodeWidthStr;
use worktrunk::styling::{ADDITION, DELETION};

use super::columns::{COLUMN_SPECS, ColumnKind, ColumnSpec, DiffVariant};

/// Width of short commit hash display (first 8 hex characters)
const COMMIT_HASH_WIDTH: usize = 8;

/// Column header labels - single source of truth for all column headers.
/// Both layout calculations and rendering use these constants.
pub const HEADER_GUTTER: &str = ""; // No header for gutter (type indicator column)
pub const HEADER_BRANCH: &str = "Branch";
pub const HEADER_STATUS: &str = "Status";
pub const HEADER_WORKING_DIFF: &str = "HEAD±";
pub const HEADER_AHEAD_BEHIND: &str = "main↕";
pub const HEADER_BRANCH_DIFF: &str = "main…±";
pub const HEADER_PATH: &str = "Path";
pub const HEADER_UPSTREAM: &str = "Remote⇅";
pub const HEADER_AGE: &str = "Age";
pub const HEADER_CI: &str = "CI";
pub const HEADER_COMMIT: &str = "Commit";
pub const HEADER_MESSAGE: &str = "Message";

/// Get terminal width for list rendering.
pub fn get_safe_list_width() -> usize {
    get_terminal_width()
}

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
/// - For diff columns: added_digits/deleted_digits refer to line change counts
/// - For arrow columns: added_digits/deleted_digits refer to ahead/behind commit counts
#[derive(Clone, Copy, Debug)]
pub struct DiffWidths {
    pub total: usize,
    pub added_digits: usize,   // First part: + for diffs, ↑ for arrows
    pub deleted_digits: usize, // Second part: - for diffs, ↓ for arrows
}

#[derive(Clone, Debug)]
pub struct ColumnWidths {
    pub branch: usize,
    pub status: usize, // Includes both git status symbols and user-defined status
    pub time: usize,
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
    pub ci_status: bool,
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
                variant: DiffVariant::Arrows,
                positive_style: ADDITION,
                negative_style: DELETION.dimmed(),
                always_show_zeros: true,
            }),
            _ => None,
        }
    }

    pub fn has_data(self, flags: &ColumnDataFlags) -> bool {
        match self {
            ColumnKind::Gutter => true, // Always present (shows @ ^ + or space)
            ColumnKind::Branch => true,
            ColumnKind::Status => flags.status,
            ColumnKind::WorkingDiff => flags.working_diff,
            ColumnKind::AheadBehind => flags.ahead_behind,
            ColumnKind::BranchDiff => flags.branch_diff,
            ColumnKind::Path => true,
            ColumnKind::Upstream => flags.upstream,
            ColumnKind::Time => true,
            ColumnKind::CiStatus => flags.ci_status,
            ColumnKind::Commit => true,
            ColumnKind::Message => true,
        }
    }

    fn ideal(
        self,
        widths: &ColumnWidths,
        max_path_width: usize,
        commit_width: usize,
    ) -> Option<ColumnIdeal> {
        match self {
            ColumnKind::Gutter => ColumnIdeal::text(2), // Fixed width: symbol (1 char) + space (1 char)
            ColumnKind::Branch => ColumnIdeal::text(widths.branch),
            ColumnKind::Status => ColumnIdeal::text(widths.status),
            ColumnKind::Path => ColumnIdeal::text(max_path_width),
            ColumnKind::Time => ColumnIdeal::text(widths.time),
            ColumnKind::CiStatus => ColumnIdeal::text(widths.ci_status),
            ColumnKind::Commit => ColumnIdeal::text(commit_width),
            ColumnKind::Message => None,
            ColumnKind::WorkingDiff => {
                ColumnIdeal::diff(widths.working_diff, ColumnKind::WorkingDiff)
            }
            ColumnKind::AheadBehind => {
                ColumnIdeal::diff(widths.ahead_behind, ColumnKind::AheadBehind)
            }
            ColumnKind::BranchDiff => ColumnIdeal::diff(widths.branch_diff, ColumnKind::BranchDiff),
            ColumnKind::Upstream => ColumnIdeal::diff(widths.upstream, ColumnKind::Upstream),
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
    pub added_digits: usize,
    pub deleted_digits: usize,
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
    pub common_prefix: PathBuf,
    pub max_message_len: usize,
    pub hidden_nonempty_count: usize,
    pub status_position_mask: super::model::PositionMask,
}

#[derive(Clone, Copy, Debug)]
struct ColumnIdeal {
    width: usize,
    format: ColumnFormat,
}

impl ColumnIdeal {
    fn text(width: usize) -> Option<Self> {
        if width == 0 {
            None
        } else {
            Some(Self {
                width,
                format: ColumnFormat::Text,
            })
        }
    }

    fn diff(widths: DiffWidths, kind: ColumnKind) -> Option<Self> {
        if widths.total == 0 {
            return None;
        }

        let display = kind.diff_display_config()?;

        Some(Self {
            width: widths.total,
            format: ColumnFormat::Diff(DiffColumnConfig {
                added_digits: widths.added_digits,
                deleted_digits: widths.deleted_digits,
                total_width: widths.total,
                display,
            }),
        })
    }
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

/// Build pre-allocated column width estimates.
///
/// Uses generous fixed allocations for expensive-to-compute columns (status, diffs, time, CI)
/// that handle overflow with compact notation (K suffix). This provides consistent layout
/// without requiring a data scan.
fn build_estimated_widths(max_branch: usize, show_full: bool, fetch_ci: bool) -> LayoutMetadata {
    // Fixed widths for slow columns (require expensive git operations)
    // Values exceeding these widths use compact notation (K suffix)
    //
    // Status column: Must match PositionMask::FULL width for consistent alignment
    // PositionMask::FULL allocates: 1+1+1+1+1+1+1+2 = 9 chars
    let status_fixed = fit_header(HEADER_STATUS, 9);
    let working_diff_fixed = fit_header(HEADER_WORKING_DIFF, 9); // "+999 -999"
    let ahead_behind_fixed = fit_header(HEADER_AHEAD_BEHIND, 7); // "↑99 ↓99"
    let branch_diff_fixed = fit_header(HEADER_BRANCH_DIFF, 9); // "+999 -999"
    let upstream_fixed = fit_header(HEADER_UPSTREAM, 7); // "↑99 ↓99"
    let age_estimate = 4; // "11mo" (short format)
    let ci_estimate = fit_header(HEADER_CI, 1); // Single indicator symbol

    // Assume columns will have data (better to show and hide than to not show)
    let data_flags = ColumnDataFlags {
        status: true,
        working_diff: true,
        ahead_behind: true,
        branch_diff: show_full,
        upstream: true,
        ci_status: fetch_ci,
    };

    let widths = ColumnWidths {
        branch: max_branch,
        status: status_fixed,
        time: age_estimate,
        ci_status: ci_estimate,
        message: 50, // Will be flexible during allocation
        // Commit counts (Arrows): compact notation, 2 digits covers up to 99
        ahead_behind: DiffWidths {
            total: ahead_behind_fixed,
            added_digits: 2,
            deleted_digits: 2,
        },
        // Line diffs (Signs): show full numbers, 3 digits covers up to 999
        working_diff: DiffWidths {
            total: working_diff_fixed,
            added_digits: 3,
            deleted_digits: 3,
        },
        branch_diff: DiffWidths {
            total: branch_diff_fixed,
            added_digits: 3,
            deleted_digits: 3,
        },
        // Upstream (Arrows): compact notation, 2 digits covers up to 99
        upstream: DiffWidths {
            total: upstream_fixed,
            added_digits: 2,
            deleted_digits: 2,
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
    show_full: bool,
    fetch_ci: bool,
    max_path_width: usize,
    commit_width: usize,
    terminal_width: usize,
    common_prefix: PathBuf,
) -> LayoutConfig {
    let spacing = 2;
    let mut remaining = terminal_width;

    // Build candidates with priorities
    let mut candidates: Vec<ColumnCandidate> = COLUMN_SPECS
        .iter()
        .filter(|spec| {
            (!spec.requires_show_full || show_full) && (!spec.requires_fetch_ci || fetch_ci)
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

    const MIN_MESSAGE: usize = 20;
    const PREFERRED_MESSAGE: usize = 50;
    const MAX_MESSAGE: usize = 100;

    let mut pending: Vec<PendingColumn> = Vec::new();

    // Allocate columns in priority order
    for candidate in candidates {
        let spec = candidate.spec;

        // Special handling for Message column
        if spec.kind == ColumnKind::Message {
            let is_first = pending.is_empty();
            let spacing_cost = if is_first { 0 } else { spacing };

            if remaining <= spacing_cost {
                continue;
            }

            let available = remaining - spacing_cost;
            let mut message_width = 0;

            if available >= PREFERRED_MESSAGE {
                message_width = PREFERRED_MESSAGE.min(metadata.widths.message);
            } else if available >= MIN_MESSAGE {
                message_width = available.min(metadata.widths.message);
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
        let Some(ideal) = spec
            .kind
            .ideal(&metadata.widths, max_path_width, commit_width)
        else {
            continue;
        };

        let allocated = try_allocate(&mut remaining, ideal.width, spacing, pending.is_empty());
        if allocated > 0 {
            pending.push(PendingColumn {
                spec,
                width: allocated,
                format: ideal.format,
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

    // Sort by display index to maintain correct visual order
    pending.sort_by_key(|col| col.spec.display_index);

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
            header: col.spec.header,
            start,
            width: col.width,
            format: col.format,
        });
    }

    // Count how many non-empty columns were hidden (not allocated)
    let allocated_kinds: std::collections::HashSet<_> =
        columns.iter().map(|col| col.kind).collect();
    let hidden_nonempty_count = candidates_with_data
        .iter()
        .filter(|(kind, has_data)| !allocated_kinds.contains(kind) && *has_data)
        .count();

    LayoutConfig {
        columns,
        common_prefix,
        max_message_len,
        hidden_nonempty_count,
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
/// - Paths (with common prefix removed)
///
/// Pre-allocated estimates (generous to minimize truncation):
/// - Status: 9 chars (PositionMask::FULL)
/// - Working diff: 9 chars ("+999 -999")
/// - Ahead/behind: 7 chars ("↑99 ↓99")
/// - Branch diff: 9 chars ("+999 -999")
/// - Upstream: 7 chars ("↑99 ↓99")
/// - Age: 4 chars ("11mo" short format)
/// - CI: 1 char (indicator symbol)
/// - Message: flexible (20-100 chars)
pub fn calculate_layout_from_basics(
    items: &[super::model::ListItem],
    show_full: bool,
    fetch_ci: bool,
) -> LayoutConfig {
    let terminal_width = get_safe_list_width();

    // Calculate common prefix from worktree paths
    let paths: Vec<&Path> = items
        .iter()
        .filter_map(|item| item.worktree_path())
        .map(|p| p.as_path())
        .collect();
    let common_prefix = find_common_prefix(&paths);

    // Calculate actual widths for things we know
    // Include branch names from both worktrees and standalone branches
    let max_branch = items
        .iter()
        .filter_map(|item| item.branch.as_deref())
        .map(|b| b.width())
        .max()
        .unwrap_or(0);

    let max_branch = fit_header(HEADER_BRANCH, max_branch);

    let path_data_width = items
        .iter()
        .filter_map(|item| item.worktree_path())
        .map(|path| {
            use crate::display::shorten_path;
            shorten_path(path.as_path(), &common_prefix).width()
        })
        .max()
        .unwrap_or(0);
    let max_path_width = fit_header(HEADER_PATH, path_data_width);

    // Build pre-allocated width estimates (same as buffered mode)
    let metadata = build_estimated_widths(max_branch, show_full, fetch_ci);

    let commit_width = fit_header(HEADER_COMMIT, COMMIT_HASH_WIDTH);

    allocate_columns_with_priority(
        &metadata,
        show_full,
        fetch_ci,
        max_path_width,
        commit_width,
        terminal_width,
        common_prefix,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::list::columns::ColumnKind;
    use std::path::PathBuf;
    use worktrunk::git::LineDiff;

    #[test]
    fn test_pre_allocated_width_estimates() {
        // Test that build_estimated_widths() returns correct pre-allocated estimates
        let metadata = build_estimated_widths(20, true, true);
        let widths = metadata.widths;

        // Line diffs (Signs variant: +/-) allocate 3 digits for 100-999 range
        // Format: "+999 -999" = 1+3+1+1+3 = 9, header "HEAD±" is 5, so total is 9
        assert_eq!(
            widths.working_diff.total, 9,
            "Working diff should pre-allocate for '+999 -999' (9 chars)"
        );
        assert_eq!(
            widths.working_diff.added_digits, 3,
            "Pre-allocated for 3-digit added count"
        );
        assert_eq!(
            widths.working_diff.deleted_digits, 3,
            "Pre-allocated for 3-digit deleted count"
        );

        // Branch diff also uses Signs variant when show_full=true
        // Format: "+999 -999" = 9, header "main…±" is 6, so total is 9
        assert_eq!(
            widths.branch_diff.total, 9,
            "Branch diff should pre-allocate for '+999 -999' (9 chars)"
        );
        assert_eq!(
            widths.branch_diff.added_digits, 3,
            "Pre-allocated for 3-digit added count"
        );
        assert_eq!(
            widths.branch_diff.deleted_digits, 3,
            "Pre-allocated for 3-digit deleted count"
        );

        // Commit counts (Arrows variant: ↑↓) use compact notation, allocate 2 digits
        // Format: "↑99 ↓99" = 1+2+1+1+2 = 7, header "main↕" is 5, so total is 7
        assert_eq!(
            widths.ahead_behind.total, 7,
            "Ahead/behind should pre-allocate for '↑99 ↓99' (7 chars)"
        );
        assert_eq!(
            widths.ahead_behind.added_digits, 2,
            "Pre-allocated for 2-digit ahead count (uses compact notation)"
        );
        assert_eq!(
            widths.ahead_behind.deleted_digits, 2,
            "Pre-allocated for 2-digit behind count (uses compact notation)"
        );

        // Upstream also uses Arrows variant
        // Format: "↑99 ↓99" = 7, header "Remote⇅" is 7, so total is 7
        assert_eq!(
            widths.upstream.total, 7,
            "Upstream should pre-allocate for '↑99 ↓99' (7 chars)"
        );
        assert_eq!(
            widths.upstream.added_digits, 2,
            "Pre-allocated for 2-digit ahead count"
        );
        assert_eq!(
            widths.upstream.deleted_digits, 2,
            "Pre-allocated for 2-digit behind count"
        );
    }

    #[test]
    fn test_visible_columns_follow_gap_rule() {
        use crate::commands::list::model::{
            AheadBehind, BranchDiffTotals, CommitDetails, DisplayFields, ItemKind, ListItem,
            StatusSymbols, UpstreamStatus, WorktreeData,
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
            upstream: Some(UpstreamStatus::from_parts(Some("origin".to_string()), 4, 2)),
            pr_status: None,
            status_symbols: Some(StatusSymbols::default()),
            display: DisplayFields::default(),
            kind: ItemKind::Worktree(Box::new(WorktreeData {
                path: PathBuf::from("/test/path"),
                bare: false,
                detached: false,
                locked: None,
                prunable: None,
                working_tree_diff: Some(LineDiff::from((100, 50))),
                working_tree_diff_with_main: Some(Some(LineDiff::default())),
                worktree_state: None,
                is_main: false,
                working_diff_display: None,
            })),
        };

        let items = vec![item];
        let layout = calculate_layout_from_basics(&items, false, false);

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

        let path_column = layout
            .columns
            .iter()
            .find(|col| col.kind == ColumnKind::Path)
            .expect("Path column must be present");
        assert!(path_column.width > 0, "Path column must have width > 0");
    }

    #[test]
    fn test_column_positions_with_empty_columns() {
        use crate::commands::list::model::{
            AheadBehind, BranchDiffTotals, CommitDetails, DisplayFields, ItemKind, ListItem,
            StatusSymbols, UpstreamStatus, WorktreeData,
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
            upstream: Some(UpstreamStatus::default()),
            pr_status: None,
            status_symbols: Some(StatusSymbols::default()),
            display: DisplayFields::default(),
            kind: ItemKind::Worktree(Box::new(WorktreeData {
                path: PathBuf::from("/test"),
                bare: false,
                detached: false,
                locked: None,
                prunable: None,
                working_tree_diff: Some(LineDiff::default()),
                working_tree_diff_with_main: Some(Some(LineDiff::default())),
                worktree_state: None,
                is_main: true, // Primary worktree: no ahead/behind shown
                working_diff_display: None,
            })),
        };

        let items = vec![item];
        let layout = calculate_layout_from_basics(&items, false, false);

        assert!(
            layout
                .columns
                .first()
                .map(|col| col.kind == ColumnKind::Gutter && col.start == 0)
                .unwrap_or(false),
            "Gutter column should start at position 0"
        );

        // Columns with data should always be visible (Branch, Path, Time, Commit, Message)
        let path_visible = layout
            .columns
            .iter()
            .any(|col| col.kind == ColumnKind::Path);
        assert!(path_visible, "Path should always be visible (has data)");

        // Empty columns may or may not be visible depending on terminal width
        // They have low priority (base_priority + EMPTY_PENALTY) so they're allocated
        // only if space remains after higher-priority columns
    }
}
