//! Column layout and priority allocation for the list command.
//!
//! # Status Column Structure
//!
//! The Status column combines two subcolumns without a 2-space column gap:
//!
//! ```text
//! Status Column = [Git Status Symbols] + [User Status]
//!                 ↑                       ↑
//!                 Variable width          Aligned subcolumn
//!                 (position mask)         (fixed position)
//! ```
//!
//! ## Git Status Symbols (Variable Width)
//!
//! Git status symbols use position-based alignment with selective rendering:
//! - Only positions used by at least one row are included (position mask)
//! - Within those positions, symbols align vertically for scannability
//! - Empty positions between symbols get spacing for alignment
//! - No leading spaces before the first symbol
//!
//! Example with positions 0b (≡) and 3 (?!) used:
//! ```text
//! Row 1: "≡ "     (position 0b filled, position 3 empty → space)
//! Row 2: "≡?"     (position 0b filled, position 3 filled)
//! ```
//!
//! ## User Status Subcolumn (Aligned)
//!
//! User-defined status aligns at a fixed position within the Status column:
//! - Starts immediately after max git symbols width (no extra gap)
//! - All user statuses align vertically regardless of git symbols
//! - Creates visual separation between git state and user labels
//!
//! Example:
//! ```text
//! Git Symbols  User Status
//! ≡            ⏸            (git symbols padded to max width)
//! ≡?           🤖           (user status aligns at fixed position)
//! ↓!+                       (no user status)
//! ```
//!
//! ## Width Calculation
//!
//! ```text
//! status_width = max_git_symbols_width + max_user_status_width
//! ```
//!
//! Where:
//! - `max_git_symbols_width` = maximum rendered width using position mask
//! - `max_user_status_width` = maximum width of user-defined status strings
//!
//! ## Why This Design?
//!
//! **Eliminates wasted space:**
//! - Position mask removes columns for symbols that appear in zero rows
//! - No 2-space column gap between git and user status
//!
//! **Maintains alignment:**
//! - Git symbols align at their positions (vertical scannability)
//! - User status aligns in its own subcolumn (visual consistency)
//!
//! **Example comparison:**
//!
//! BAD (no alignment):
//! ```text
//! ≡⏸
//! ≡?🤖
//! ```
//!
//! GOOD (aligned subcolumns):
//! ```text
//! ≡ ⏸
//! ≡?🤖
//! ```
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
//!     } else if let Some(ideal) = ideal_for_column(candidate.spec, ...) {
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
//! - `calculate_diff_width()`: Computes width for diff-style columns ("+added -deleted")
//! - `fit_header()`: Ensures column width ≥ header width to prevent overflow
//! - `try_allocate()`: Attempts to allocate space, returns 0 if insufficient

use crate::display::{find_common_prefix, get_terminal_width};
use anstyle::Style;
use std::path::{Path, PathBuf};
use unicode_width::UnicodeWidthStr;
use worktrunk::styling::{ADDITION, DELETION};

use super::{
    columns::{COLUMN_SPECS, ColumnKind, ColumnSpec, DiffVariant},
    model::ListItem,
};

/// Width of short commit hash display (first 8 hex characters)
const COMMIT_HASH_WIDTH: usize = 8;

/// Column header labels - single source of truth for all column headers.
/// Both layout calculations and rendering use these constants.
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

/// Calculates width for a diff-style column (format: "+added -deleted" or "↑ahead ↓behind").
///
/// Returns DiffWidths with:
/// - total: width including header minimum ("+{added} -{deleted}"), or just header width if no data
/// - added_digits/deleted_digits: number of digits for each part
///
/// Empty columns (both digits = 0) get header width and are allocated at low priority
/// (base_priority + EMPTY_PENALTY) for visual consistency on wide terminals.
fn calculate_diff_width(header: &str, added_digits: usize, deleted_digits: usize) -> DiffWidths {
    let has_data = added_digits > 0 || deleted_digits > 0;
    let data_width = if has_data {
        1 + added_digits + 1 + 1 + deleted_digits // "+added -deleted"
    } else {
        0 // fit_header will use header width for empty columns
    };
    let total = fit_header(header, data_width);

    DiffWidths {
        total,
        added_digits,
        deleted_digits,
    }
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
    /// Maximum width of git status symbols (for padding before user status subcolumn)
    pub max_git_symbols_width: usize,
}

const EMPTY_PENALTY: u8 = 10;

#[derive(Clone, Copy, Debug)]
pub struct DiffDigits {
    pub added: usize,
    pub deleted: usize,
}

impl From<DiffWidths> for DiffDigits {
    fn from(widths: DiffWidths) -> Self {
        Self {
            added: widths.added_digits,
            deleted: widths.deleted_digits,
        }
    }
}

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
}

#[derive(Clone, Copy, Debug)]
pub enum ColumnFormat {
    Text,
    Diff(DiffColumnConfig),
}

#[derive(Clone, Copy, Debug)]
pub struct DiffColumnConfig {
    pub digits: DiffDigits,
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
    /// Maximum width of git status symbols (for padding before user status subcolumn)
    pub max_git_symbols_width: usize,
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
                digits: widths.into(),
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

fn ideal_for_column(
    spec: &ColumnSpec,
    widths: &ColumnWidths,
    max_path_width: usize,
    commit_width: usize,
) -> Option<ColumnIdeal> {
    match spec.kind {
        ColumnKind::Branch => ColumnIdeal::text(widths.branch),
        ColumnKind::Status => ColumnIdeal::text(widths.status),
        ColumnKind::Path => ColumnIdeal::text(max_path_width),
        ColumnKind::Time => ColumnIdeal::text(widths.time),
        ColumnKind::CiStatus => ColumnIdeal::text(widths.ci_status),
        ColumnKind::Commit => ColumnIdeal::text(commit_width),
        ColumnKind::Message => None,
        ColumnKind::WorkingDiff => ColumnIdeal::diff(widths.working_diff, ColumnKind::WorkingDiff),
        ColumnKind::AheadBehind => ColumnIdeal::diff(widths.ahead_behind, ColumnKind::AheadBehind),
        ColumnKind::BranchDiff => ColumnIdeal::diff(widths.branch_diff, ColumnKind::BranchDiff),
        ColumnKind::Upstream => ColumnIdeal::diff(widths.upstream, ColumnKind::Upstream),
    }
}

pub fn calculate_column_widths(items: &[ListItem], fetch_ci: bool) -> LayoutMetadata {
    // Track maximum data widths (headers are enforced via fit_header() later)
    let mut max_branch = 0;
    let mut max_time = 0;
    let mut max_message = 0;

    // Track diff component widths separately
    let mut max_wt_added_digits = 0;
    let mut max_wt_deleted_digits = 0;
    let mut max_br_added_digits = 0;
    let mut max_br_deleted_digits = 0;

    // Track ahead/behind digit widths separately for alignment
    let mut max_ahead_digits = 0;
    let mut max_behind_digits = 0;
    let mut max_upstream_ahead_digits = 0;
    let mut max_upstream_behind_digits = 0;

    // Track which status positions are used across all items
    let mut status_position_mask = super::model::PositionMask::default();

    for item in items {
        let commit = item.commit_details();
        let counts = item.counts();
        let branch_diff = item.branch_diff().diff;
        let upstream = item.upstream();
        let worktree_info = item.worktree_info();

        // Branch name
        max_branch = max_branch.max(item.branch_name().width());

        // Status column: git status symbols (worktrees only)
        // Position mask is collected here for selective rendering
        if let Some(info) = worktree_info {
            // Collect position usage from this item's status symbols
            let item_mask = info.status_symbols.position_mask();
            status_position_mask.merge(&item_mask);
        }

        // Time
        let time_str = crate::display::format_relative_time(commit.timestamp);
        max_time = max_time.max(time_str.width());

        // Message (truncate to 50 chars max)
        let msg_len = commit.commit_message.chars().take(50).count();
        max_message = max_message.max(msg_len);

        // Ahead/behind (only for non-primary items) - track digits separately
        if !item.is_primary() && (counts.ahead > 0 || counts.behind > 0) {
            max_ahead_digits = max_ahead_digits.max(counts.ahead.to_string().len());
            max_behind_digits = max_behind_digits.max(counts.behind.to_string().len());
        }

        // Working tree diff (worktrees only) - track digits separately
        if let Some(info) = worktree_info
            && !info.working_tree_diff.is_empty()
        {
            max_wt_added_digits =
                max_wt_added_digits.max(info.working_tree_diff.added.to_string().len());
            max_wt_deleted_digits =
                max_wt_deleted_digits.max(info.working_tree_diff.deleted.to_string().len());
        }

        // Branch diff (only for non-primary items) - track digits separately
        if !item.is_primary() && !branch_diff.is_empty() {
            max_br_added_digits = max_br_added_digits.max(branch_diff.added.to_string().len());
            max_br_deleted_digits =
                max_br_deleted_digits.max(branch_diff.deleted.to_string().len());
        }

        // Upstream tracking - track digits only (not remote name yet)
        if let Some((_remote_name, upstream_ahead, upstream_behind)) = upstream.active() {
            max_upstream_ahead_digits =
                max_upstream_ahead_digits.max(upstream_ahead.to_string().len());
            max_upstream_behind_digits =
                max_upstream_behind_digits.max(upstream_behind.to_string().len());
        }
    }

    // Calculate diff widths using helper (format: "+left -right")
    let working_diff = calculate_diff_width(
        HEADER_WORKING_DIFF,
        max_wt_added_digits,
        max_wt_deleted_digits,
    );
    let branch_diff = calculate_diff_width(
        HEADER_BRANCH_DIFF,
        max_br_added_digits,
        max_br_deleted_digits,
    );
    let ahead_behind =
        calculate_diff_width(HEADER_AHEAD_BEHIND, max_ahead_digits, max_behind_digits);

    // Upstream (format: "↑n ↓n", TODO: add remote name when show_remote_names is implemented)
    let upstream = calculate_diff_width(
        HEADER_UPSTREAM,
        max_upstream_ahead_digits,
        max_upstream_behind_digits,
    );

    // Calculate Status column width: git symbols + user status (as aligned subcolumns)
    // Git symbols width: maximum rendered width using position mask (variable per row)
    let max_git_symbols = items
        .iter()
        .filter_map(|item| item.worktree_info())
        .map(|info| {
            info.status_symbols
                .render_with_mask(&status_position_mask)
                .width()
        })
        .max()
        .unwrap_or(0);

    // User status width: maximum user-defined status width (for alignment subcolumn)
    let max_user_status = items
        .iter()
        .filter_map(|item| {
            if let Some(info) = item.worktree_info() {
                info.user_status.as_ref().map(|s| s.width())
            } else if let ListItem::Branch(branch_info) = item {
                branch_info.user_status.as_ref().map(|s| s.width())
            } else {
                None
            }
        })
        .max()
        .unwrap_or(0);

    // Total Status width = git symbols + user status (no extra gap between them)
    let status_data_width = max_git_symbols + max_user_status;

    // For Status column: always fit header to prevent header overflow
    // Even though we want narrow columns, we can't make them narrower than the header
    let has_status_data = status_data_width > 0;
    let final_status = fit_header(HEADER_STATUS, status_data_width);

    // CI status column: Always 2 chars wide
    // Only show if we attempted to fetch CI data (regardless of whether any items have status)
    let has_ci_status = fetch_ci && items.iter().any(|item| item.pr_status().is_some());
    let ci_status_width = 2; // Fixed width

    let widths = ColumnWidths {
        branch: fit_header(HEADER_BRANCH, max_branch),
        status: final_status,
        time: fit_header(HEADER_AGE, max_time),
        ci_status: fit_header(HEADER_CI, ci_status_width),
        message: fit_header(HEADER_MESSAGE, max_message),
        ahead_behind,
        working_diff,
        branch_diff,
        upstream,
    };

    let data_flags = ColumnDataFlags {
        status: has_status_data,
        working_diff: working_diff.added_digits > 0 || working_diff.deleted_digits > 0,
        ahead_behind: ahead_behind.added_digits > 0 || ahead_behind.deleted_digits > 0,
        branch_diff: branch_diff.added_digits > 0 || branch_diff.deleted_digits > 0,
        upstream: upstream.added_digits > 0 || upstream.deleted_digits > 0,
        ci_status: has_ci_status,
    };

    LayoutMetadata {
        widths,
        data_flags,
        status_position_mask,
        max_git_symbols_width: max_git_symbols,
    }
}

/// Calculate responsive layout based on terminal width
pub fn calculate_responsive_layout(
    items: &[ListItem],
    show_full: bool,
    fetch_ci: bool,
) -> LayoutConfig {
    let terminal_width = get_terminal_width();
    let paths: Vec<&Path> = items
        .iter()
        .filter_map(|item| item.worktree_path().map(|path| path.as_path()))
        .collect();
    let common_prefix = find_common_prefix(&paths);

    // Calculate ideal column widths and track which columns have data
    let metadata = calculate_column_widths(items, fetch_ci);
    let ideal_widths = metadata.widths;
    let data_flags = metadata.data_flags;
    let status_position_mask = metadata.status_position_mask;
    let max_git_symbols_width = metadata.max_git_symbols_width;

    // Calculate actual maximum path width (after common prefix removal)
    let path_data_width = items
        .iter()
        .filter_map(|item| item.worktree_path())
        .map(|path| {
            use crate::display::shorten_path;
            use unicode_width::UnicodeWidthStr;
            shorten_path(path.as_path(), &common_prefix).width()
        })
        .max()
        .unwrap_or(0);
    let max_path_width = fit_header(HEADER_PATH, path_data_width);

    let commit_width = fit_header(HEADER_COMMIT, COMMIT_HASH_WIDTH);

    let spacing = 2;
    let mut remaining = terminal_width;

    let mut candidates: Vec<ColumnCandidate> = COLUMN_SPECS
        .iter()
        .filter(|spec| {
            (!spec.requires_show_full || show_full) && (!spec.requires_fetch_ci || fetch_ci)
        })
        .map(|spec| ColumnCandidate {
            spec,
            priority: if spec.kind.has_data(&data_flags) {
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
        .map(|c| (c.spec.kind, c.spec.kind.has_data(&data_flags)))
        .collect();

    const MIN_MESSAGE: usize = 20;
    const PREFERRED_MESSAGE: usize = 50;
    const MAX_MESSAGE: usize = 100;

    let mut pending: Vec<PendingColumn> = Vec::new();

    for candidate in candidates {
        let spec = candidate.spec;

        if spec.kind == ColumnKind::Message {
            let is_first = pending.is_empty();
            let spacing_cost = if is_first { 0 } else { spacing };

            if remaining <= spacing_cost {
                continue;
            }

            let available = remaining - spacing_cost;
            let mut message_width = 0;

            if available >= PREFERRED_MESSAGE {
                message_width = PREFERRED_MESSAGE.min(ideal_widths.message);
            } else if available >= MIN_MESSAGE {
                message_width = available.min(ideal_widths.message);
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

        let Some(ideal) = ideal_for_column(spec, &ideal_widths, max_path_width, commit_width)
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

    pending.sort_by_key(|col| col.spec.display_index);

    let gap = 2;
    let mut position = 0;
    let mut columns = Vec::new();

    for col in pending {
        let start = if columns.is_empty() {
            0
        } else {
            position + gap
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
        status_position_mask,
        max_git_symbols_width,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::list::columns::ColumnKind;
    use std::path::PathBuf;
    use worktrunk::git::LineDiff;

    #[test]
    fn test_column_width_calculation_with_unicode() {
        use crate::commands::list::model::{
            AheadBehind, BranchDiffTotals, CommitDetails, DisplayFields, StatusSymbols,
            UpstreamStatus, WorktreeInfo,
        };

        let info1 = WorktreeInfo {
            path: PathBuf::from("/test"),
            head: "abc123".to_string(),
            branch: Some("main".to_string()),
            bare: false,
            detached: false,
            locked: None,
            prunable: None,
            commit: CommitDetails {
                timestamp: 0,
                commit_message: "Test".to_string(),
            },
            counts: AheadBehind {
                ahead: 3,
                behind: 2,
            },
            working_tree_diff: LineDiff::from((100, 50)),
            working_tree_diff_with_main: Some(LineDiff::default()),
            branch_diff: BranchDiffTotals {
                diff: LineDiff::from((200, 30)),
            },
            is_primary: false,
            upstream: UpstreamStatus::from_parts(Some("origin".to_string()), 4, 0),
            worktree_state: None,
            pr_status: None,
            has_conflicts: false,
            status_symbols: StatusSymbols::default(),
            user_status: None,
            display: DisplayFields::default(),
            working_diff_display: None,
        };

        let metadata = calculate_column_widths(&[super::ListItem::Worktree(info1)], false);
        let widths = metadata.widths;

        // "↑3 ↓2" has format "↑3 ↓2" = 1+1+1+1+1 = 5, header "main↕" is also 5
        assert_eq!(
            widths.ahead_behind.total, 5,
            "Ahead/behind column should fit header 'main↕' (width 5)"
        );
        assert_eq!(widths.ahead_behind.added_digits, 1, "3 has 1 digit");
        assert_eq!(widths.ahead_behind.deleted_digits, 1, "2 has 1 digit");

        // "+100 -50" has width 8, but header "HEAD±" is 5, so column width is 8
        assert_eq!(
            widths.working_diff.total, 8,
            "Working diff column should fit header 'HEAD±' (width 5)"
        );
        assert_eq!(widths.working_diff.added_digits, 3, "100 has 3 digits");
        assert_eq!(widths.working_diff.deleted_digits, 2, "50 has 2 digits");

        // "+200 -30" has width 8, header "main…±" is 6, so column width is 8
        assert_eq!(
            widths.branch_diff.total, 8,
            "Branch diff column should fit header 'main…±' (width 6)"
        );
        assert_eq!(widths.branch_diff.added_digits, 3, "200 has 3 digits");
        assert_eq!(widths.branch_diff.deleted_digits, 2, "30 has 2 digits");

        // Upstream: "↑4 ↓0" = "↑" (1) + "4" (1) + " " (1) + "↓" (1) + "0" (1) = 5, but header "Remote⇅" = 7
        assert_eq!(
            widths.upstream.total, 7,
            "Upstream column should fit header 'Remote⇅' (width 7)"
        );
        assert_eq!(widths.upstream.added_digits, 1, "4 has 1 digit");
        assert_eq!(widths.upstream.deleted_digits, 1, "0 has 1 digit");
    }

    #[test]
    fn test_visible_columns_follow_gap_rule() {
        use crate::commands::list::model::{
            AheadBehind, BranchDiffTotals, CommitDetails, DisplayFields, StatusSymbols,
            UpstreamStatus, WorktreeInfo,
        };

        // Create test data with specific widths to verify position calculation
        let info = WorktreeInfo {
            path: PathBuf::from("/test/path"),
            head: "abc12345".to_string(),
            branch: Some("feature".to_string()),
            bare: false,
            detached: false,
            locked: None,
            prunable: None,
            commit: CommitDetails {
                timestamp: 1234567890,
                commit_message: "Test commit message".to_string(),
            },
            counts: AheadBehind {
                ahead: 5,
                behind: 10,
            },
            working_tree_diff: LineDiff::from((100, 50)),
            working_tree_diff_with_main: Some(LineDiff::default()),
            branch_diff: BranchDiffTotals {
                diff: LineDiff::from((200, 30)),
            },
            is_primary: false,
            upstream: UpstreamStatus::from_parts(Some("origin".to_string()), 4, 2),
            worktree_state: None,
            pr_status: None,
            has_conflicts: false,
            status_symbols: StatusSymbols::default(),
            user_status: None,
            display: DisplayFields::default(),
            working_diff_display: None,
        };

        let items = vec![super::ListItem::Worktree(info)];
        let layout = calculate_responsive_layout(&items, false, false);

        assert!(
            !layout.columns.is_empty(),
            "At least one column should be visible"
        );

        let mut columns_iter = layout.columns.iter();
        let first = columns_iter.next().expect("branch column should exist");
        assert_eq!(
            first.kind,
            ColumnKind::Branch,
            "Branch column should be first"
        );
        assert_eq!(first.start, 0, "Branch should begin at position 0");

        let mut previous_end = first.start + first.width;
        for column in columns_iter {
            assert_eq!(
                column.start,
                previous_end + 2,
                "Columns should be separated by a 2-space gap"
            );
            previous_end = column.start + column.width;
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
            AheadBehind, BranchDiffTotals, CommitDetails, DisplayFields, StatusSymbols,
            UpstreamStatus, WorktreeInfo,
        };

        // Create minimal data - most columns will be empty
        let info = WorktreeInfo {
            path: PathBuf::from("/test"),
            head: "abc12345".to_string(),
            branch: Some("main".to_string()),
            bare: false,
            detached: false,
            locked: None,
            prunable: None,
            commit: CommitDetails {
                timestamp: 1234567890,
                commit_message: "Test".to_string(),
            },
            counts: AheadBehind {
                ahead: 0,
                behind: 0,
            },
            working_tree_diff: LineDiff::default(),
            working_tree_diff_with_main: Some(LineDiff::default()),
            branch_diff: BranchDiffTotals {
                diff: LineDiff::default(),
            },
            is_primary: true, // Primary worktree: no ahead/behind shown
            upstream: UpstreamStatus::default(),
            worktree_state: None,
            pr_status: None,
            has_conflicts: false,
            status_symbols: StatusSymbols::default(),
            user_status: None,
            display: DisplayFields::default(),
            working_diff_display: None,
        };

        let items = vec![super::ListItem::Worktree(info)];
        let layout = calculate_responsive_layout(&items, false, false);

        assert!(
            layout
                .columns
                .first()
                .map(|col| col.kind == ColumnKind::Branch && col.start == 0)
                .unwrap_or(false),
            "Branch column should start at position 0"
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

    #[test]
    fn test_consecutive_empty_columns_have_low_priority() {
        use crate::commands::list::model::{
            AheadBehind, BranchDiffTotals, CommitDetails, DisplayFields, StatusSymbols,
            UpstreamStatus, WorktreeInfo,
        };

        // Create data where multiple consecutive columns are empty:
        // visible(branch) → empty(working_diff) → empty(ahead_behind) → empty(branch_diff)
        // → empty(states) → visible(path)
        let info = WorktreeInfo {
            path: PathBuf::from("/test/worktree"),
            head: "abc12345".to_string(),
            branch: Some("feature-x".to_string()),
            bare: false,
            detached: false,
            locked: None,
            prunable: None,
            commit: CommitDetails {
                timestamp: 1234567890,
                commit_message: "Test commit".to_string(),
            },
            counts: AheadBehind {
                ahead: 0,
                behind: 0,
            },
            working_tree_diff: LineDiff::default(), // Empty: no dirty changes
            working_tree_diff_with_main: Some(LineDiff::default()),
            branch_diff: BranchDiffTotals {
                diff: LineDiff::default(),
            }, // Empty: no diff
            is_primary: true, // Empty: no ahead/behind for primary
            upstream: UpstreamStatus::default(), // Empty: no upstream
            worktree_state: None, // Empty: no state
            pr_status: None,
            has_conflicts: false,
            status_symbols: StatusSymbols::default(),
            user_status: None,
            display: DisplayFields::default(),
            working_diff_display: None,
        };

        let items = vec![super::ListItem::Worktree(info)];
        let layout = calculate_responsive_layout(&items, false, false);

        let path_visible = layout
            .columns
            .iter()
            .any(|col| col.kind == ColumnKind::Path);
        assert!(
            path_visible,
            "Path should be visible (has data, priority 7)"
        );

        let message_visible = layout
            .columns
            .iter()
            .any(|col| col.kind == ColumnKind::Message);
        assert!(
            message_visible,
            "Message should be allocated before empty columns (priority 12 < empty columns)"
        );

        // Empty columns (priority 12+) may or may not be visible depending on terminal width.
        // They rank lower than message (priority 12), so message allocates first.
    }
}
