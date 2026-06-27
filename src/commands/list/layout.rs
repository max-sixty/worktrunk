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
//! **Base priorities** (0-13) are determined by **user need hierarchy** - what questions users need
//! answered when scanning worktrees:
//! - 0: Gutter (always present)
//! - 1: Branch (identity - "what is this?")
//! - 2-4: Critical (status, working diff, ahead/behind)
//! - 5-12: Context (CI, branch diff, path, upstream, URL, summary, commit, time)
//! - 13: Message (nice-to-have, space-hungry)
//!
//! **Empty penalty**: +10 if column has no data (only header)
//! - Empty working_diff: 3 + 10 = priority 13
//! - Empty ahead/behind: 4 + 10 = priority 14
//! - etc.
//!
//! This creates two effective priority tiers:
//! - **Tier 1 (priorities 0-13)**: Columns with actual data
//! - **Tier 2 (priorities 13-23)**: Empty columns (visual consistency)
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
//! **Key decision**: Message sits at the boundary (priority 13). Empty columns (priority 13+)
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
//! Some columns have non-standard behavior that extends beyond the basic two-tier model:
//!
//! 1. **CiStatus** and **Summary** - Visibility gate (`show_full` flag)
//!    - Both require `show_full=true` — they reach off-machine (CI status over
//!      the network, branch summaries via the LLM), so they're hidden by default
//!    - Gated via `skip_tasks`: when `show_full=false`, their `TaskKind` is in
//!      `skip_tasks` and the column is filtered out entirely (bypasses the tier
//!      system)
//!    - **BranchDiff** (`main…±`) is pure local git, so it is *not* gated — it
//!      shows by default and follows the normal two-tier priority (6/16);
//!      CiStatus is 5/15
//!
//! 2. **Low-priority columns** yield to Summary
//!    - Columns with effective priority > Summary's (10) are dropped to reclaim
//!      space, with thresholds based on priority distance:
//!      - Within 4 levels (Commit 11, Time 12, Message 13): Summary < 50
//!      - Beyond 4 levels (e.g., no-data Path 7+10=17): Summary < 70 (MAX)
//!    - Highest priority value drops first; no-data columns qualify via EMPTY_PENALTY
//!
//! 3. **Summary** - Flexible sizing with post-allocation expansion
//!    - Allocated at priority 10 with minimum width 10
//!    - After all columns allocated, expands up to 70 using leftover space
//!    - Reclaims no-data and low-priority columns as needed
//!    - Expands BEFORE Message, so Summary gets priority for space
//!
//! 4. **Message** - Flexible sizing, gated on Summary readability
//!    - Allocated at priority 13 with minimum width 10
//!    - **Only kept if Summary reaches 50 chars** — below that, Summary needs
//!      all flexible space and Message is dropped (its space reclaimed for Summary)
//!    - After Summary expansion, expands up to max 100 using remaining leftover space
//!
//! ## Implementation
//!
//! The code implements this using a centralized registry and priority-based allocation:
//!
//! ```rust
//! // Build candidates from centralized COLUMN_SPECS registry
//! let mut candidates: Vec<ColumnCandidate> = COLUMN_SPECS
//!     .iter()
//!     .filter(|spec| /* visibility gate: skip_tasks */)
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
//! // Post-allocation expansion: Summary first, then Message with leftovers
//! if let Some(summary_col) = pending.iter_mut().find(|col| col.spec.kind == ColumnKind::Summary) {
//!     summary_col.width += remaining.min(MAX_SUMMARY - summary_col.width);
//! }
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

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anstyle::Style;
use unicode_width::UnicodeWidthStr;
use worktrunk::styling::{ADDITION, DELETION, Stream, supports_hyperlinks};

use crate::display::shorten_path;

use super::collect::{TaskKind, parse_port_from_url};
use super::columns::{COLUMN_SPECS, ColumnKind, ColumnSpec, column_display_index};
use super::custom_columns::ResolvedCustomColumn;

// Re-export DiffVariant for external use (e.g., picker module)
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
    data_width.max(header.width())
}

/// Helper: Try to allocate space for a column. Returns the allocated width if successful.
/// Updates `remaining` by subtracting the allocated width + spacing.
/// If is_first is true, doesn't require spacing before the column.
///
/// When `min_width` is provided, the column can shrink below its ideal width (down to
/// `min_width`) instead of being dropped entirely. This prevents high-priority columns
/// like Branch from disappearing on narrow terminals.
///
/// The spacing is consumed from the budget (subtracted from `remaining`) but not returned
/// as part of the column's width, since the spacing appears before the column content.
fn try_allocate(
    remaining: &mut usize,
    ideal_width: usize,
    min_width: Option<usize>,
    spacing: usize,
    is_first: bool,
) -> usize {
    if ideal_width == 0 {
        return 0;
    }
    let spacing_cost = if is_first { 0 } else { spacing };

    // Try ideal width first
    if *remaining >= ideal_width + spacing_cost {
        *remaining -= ideal_width + spacing_cost;
        return ideal_width;
    }

    // Fall back to whatever fits above min_width
    if let Some(min) = min_width
        && *remaining >= min + spacing_cost
    {
        let width = *remaining - spacing_cost;
        *remaining = 0;
        return width;
    }

    0
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
    pub ahead_behind: DiffWidths,
    pub working_diff: DiffWidths,
    pub branch_diff: DiffWidths,
    pub upstream: DiffWidths,
    /// Measured widths for `[list.custom-columns]` columns, indexed like the
    /// resolved column list. Values are final before layout (no estimates);
    /// 0 means the column is empty for every row and is excluded entirely.
    pub custom: Vec<usize>,
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
}

impl DiffDisplayConfig {
    /// Format diff values with fixed-width alignment for tabular display.
    ///
    /// Numbers are right-aligned within a 3-digit column width.
    /// Returns empty spaces if both values are zero.
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
    /// Returns `None` if both values are zero.
    /// Format: `+N -M` with appropriate colors for each component.
    pub fn format_plain(&self, positive: usize, negative: usize) -> Option<String> {
        if positive == 0 && negative == 0 {
            return None;
        }

        let symbols = self.variant.symbols();
        let mut parts = Vec::with_capacity(2);

        if positive > 0 {
            parts.push(format!(
                "{}{}{}{}",
                self.positive_style,
                symbols.positive,
                positive,
                self.positive_style.render_reset()
            ));
        }

        if negative > 0 {
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
            }),
            ColumnKind::AheadBehind => Some(DiffDisplayConfig {
                variant: DiffVariant::Arrows,
                positive_style: ADDITION,
                negative_style: DELETION.dimmed(),
            }),
            ColumnKind::Upstream => Some(DiffDisplayConfig {
                variant: DiffVariant::UpstreamArrows,
                positive_style: ADDITION,
                negative_style: DELETION.dimmed(),
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
            ColumnKind::Summary => true, // Placeholder shown until data arrives
            ColumnKind::Message => true,
            // Custom values are final before layout (nothing arrives later),
            // so all-empty columns are excluded from candidates instead of
            // taking the EMPTY_PENALTY path built for still-loading data.
            ColumnKind::Custom(_) => true,
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
            ColumnKind::Summary => None, // Flexible: handled specially in allocation loop
            ColumnKind::Message => None,
            ColumnKind::WorkingDiff => diff(widths.working_diff),
            ColumnKind::AheadBehind => diff(widths.ahead_behind),
            ColumnKind::BranchDiff => diff(widths.branch_diff),
            ColumnKind::Upstream => diff(widths.upstream),
            ColumnKind::Custom(i) => text(widths.custom.get(i as usize).copied().unwrap_or(0)),
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
    /// Borrowed from [`ColumnKind::header`] for built-ins; owned for custom
    /// columns, whose header is the resolved column name.
    pub header: std::borrow::Cow<'static, str>,
    pub start: usize,
    pub width: usize,
    pub format: ColumnFormat,
}

#[derive(Clone)]
pub struct LayoutConfig {
    pub columns: Vec<ColumnLayout>,
    pub main_worktree_path: PathBuf,
    pub max_message_len: usize,
    pub max_summary_len: usize,
    pub hidden_column_count: usize,
    pub status_position_mask: super::model::PositionMask,
}

/// `Send + Sync` snapshot of the column geometry: `(kind, start, width)` per
/// visible column, in display order. `LayoutConfig` itself can't cross
/// threads, so renderers running outside `collect` — the picker's `--prs`
/// thread — take this snapshot to place their cells on the same grid as the
/// worktree rows.
#[derive(Clone, Debug, Default)]
pub struct ColumnGrid {
    pub columns: Vec<GridColumn>,
}

#[derive(Clone, Copy, Debug)]
pub struct GridColumn {
    pub kind: ColumnKind,
    pub start: usize,
    pub width: usize,
}

impl ColumnGrid {
    pub fn column(&self, kind: ColumnKind) -> Option<GridColumn> {
        self.columns.iter().copied().find(|col| col.kind == kind)
    }
}

impl LayoutConfig {
    pub fn column_grid(&self) -> ColumnGrid {
        ColumnGrid {
            columns: self
                .columns
                .iter()
                .map(|col| GridColumn {
                    kind: col.kind,
                    start: col.start,
                    width: col.width,
                })
                .collect(),
        }
    }
}

/// The user's column configuration for one render: which columns to show and
/// the resolved custom columns available to render.
///
/// `selected` is `None` for the default column set (every built-in, with custom
/// columns appended in resolution order). `Some(order)` renders exactly those
/// columns in that order — `order` may name built-ins *and* custom columns
/// (`ColumnKind::Custom(i)`, indexing `custom`), and anything not listed is
/// hidden. `custom` always carries the resolved `[list.custom-columns]` so their
/// widths and headers are available; `selected` decides which of them render.
#[derive(Clone, Copy)]
pub struct ColumnSelection<'a> {
    pub selected: Option<&'a [ColumnKind]>,
    pub custom: &'a [ResolvedCustomColumn],
}

#[derive(Clone, Copy)]
struct ColumnCandidate<'a> {
    spec: &'a ColumnSpec,
    priority: u8,
}

#[derive(Clone, Copy)]
struct PendingColumn<'a> {
    spec: &'a ColumnSpec,
    priority: u8,
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
    max_pr_number: Option<u64>,
    custom_widths: Vec<usize>,
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
    // CI column: PR/MR reference ("#3035"), sized from the cached largest
    // number seen; "#9999" before the first fetch populates the cache. A
    // number that outgrows the estimate renders as the bare `#` indicator
    // (`PrStatus::format_cell`) — the layout never resizes mid-render — and
    // the ratcheted cache sizes the next invocation correctly.
    let ci_estimate = fit_header(
        ColumnKind::CiStatus.header(),
        super::ci_status::pr_ref_width(max_pr_number.unwrap_or(9999)),
    );

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
        custom: custom_widths,
    };

    LayoutMetadata {
        widths,
        data_flags,
        status_position_mask: super::model::PositionMask::FULL,
    }
}

/// Final visual ordering key for a column.
///
/// Without a `[list] columns` selection this defers to the static registry
/// order ([`column_display_index`]). With a selection, the configured order
/// wins for every column: Gutter pins first, then each selected column — built-in
/// or custom — follows its position in `order`. The candidate filter drops any
/// column not in the selection, so the `usize::MAX` sentinel only covers that
/// unreachable case; `+ 1` runs only on a real position, so it can't overflow.
fn display_sort_key(kind: ColumnKind, selected: Option<&[ColumnKind]>) -> (usize, usize) {
    let Some(order) = selected else {
        return column_display_index(kind);
    };
    if kind == ColumnKind::Gutter {
        return (0, 0);
    }
    order
        .iter()
        .position(|&k| k == kind)
        .map_or((usize::MAX, 0), |position| (position + 1, 0))
}

/// Allocate columns using priority-based allocation logic.
///
/// This is the core allocation algorithm used by `calculate_layout_with_width()`
/// with pre-allocated width estimates for expensive-to-compute columns.
fn allocate_columns_with_priority(
    metadata: &LayoutMetadata,
    skip_tasks: &HashSet<TaskKind>,
    max_path_width: usize,
    commit_width: usize,
    terminal_width: usize,
    main_worktree_path: PathBuf,
    columns: ColumnSelection,
) -> LayoutConfig {
    let ColumnSelection {
        custom: custom_columns,
        selected,
    } = columns;
    let spacing = 2;
    let mut remaining = terminal_width;

    // Custom columns join the candidate pool with their configured priority.
    // Width 0 means empty for every row — excluded entirely (values are
    // final, so unlike still-loading built-ins nothing can arrive later). When
    // `[list] columns` selects a subset, a custom column shows only if it was
    // named (by header → `Custom(i)`); without a selection every custom joins.
    let custom_specs: Vec<ColumnSpec> = custom_columns
        .iter()
        .enumerate()
        .filter(|&(i, _)| metadata.widths.custom.get(i).copied().unwrap_or(0) > 0)
        .filter(|&(i, _)| match selected {
            Some(order) => order.contains(&ColumnKind::Custom(i as u8)),
            None => true,
        })
        .map(|(i, column)| ColumnSpec::new(ColumnKind::Custom(i as u8), column.priority, None))
        .collect();

    // Build candidates with priorities.
    // - Filter out columns whose required task is being skipped.
    // - When `[list] columns` selects a subset, keep only the chosen built-ins
    //   (Gutter is structural — always kept). This filters WITHOUT renumbering
    //   base_priority, so the Summary drop loop and EMPTY_PENALTY tiers below
    //   stay correct: the allocator sorts by priority and never indexes by it,
    //   so a sparse retained set just works. Custom columns honor the same
    //   selection (filtered into `custom_specs` above), so a selection lists
    //   built-ins and customs in one ordered set.
    let mut candidates: Vec<ColumnCandidate> = COLUMN_SPECS
        .iter()
        .filter(|spec| {
            spec.requires_task
                .is_none_or(|task| !skip_tasks.contains(&task))
        })
        .filter(|spec| match selected {
            Some(order) => spec.kind == ColumnKind::Gutter || order.contains(&spec.kind),
            None => true,
        })
        .chain(custom_specs.iter())
        .map(|spec| ColumnCandidate {
            spec,
            priority: if spec.kind.has_data(&metadata.data_flags) {
                spec.base_priority
            } else {
                spec.base_priority + EMPTY_PENALTY
            },
        })
        .collect();

    // Stable sort: a custom column tying a built-in's priority allocates
    // after it (customs are appended to the candidate list).
    candidates.sort_by_key(|candidate| candidate.priority);

    // Store candidate kinds for later calculation of hidden columns
    let candidate_kinds: Vec<_> = candidates.iter().map(|c| c.spec.kind).collect();

    const MIN_SUMMARY: usize = 10;
    const MAX_SUMMARY: usize = 70;
    const MIN_MESSAGE: usize = 10;
    const MAX_MESSAGE: usize = 100;
    // Low-priority columns (Commit, Time, Message) are only shown when Summary
    // reaches this width — below that, Summary needs the space to be readable.
    const SUMMARY_THRESHOLD_FOR_LOW_PRIORITY: usize = 50;

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

        // Flexible columns: allocate at minimum, expand post-loop
        if matches!(spec.kind, ColumnKind::Summary | ColumnKind::Message) {
            let min_width = match spec.kind {
                ColumnKind::Summary => MIN_SUMMARY,
                _ => MIN_MESSAGE,
            };
            let spacing_cost = if needs_spacing(&pending) { spacing } else { 0 };
            if remaining > spacing_cost {
                let available = remaining - spacing_cost;
                if available >= min_width {
                    remaining = remaining.saturating_sub(min_width + spacing_cost);
                    pending.push(PendingColumn {
                        spec,
                        priority: candidate.priority,
                        width: min_width,
                        format: ColumnFormat::Text,
                    });
                }
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

        let is_first = !needs_spacing(&pending);
        let min_width = if spec.shrinkable {
            Some(spec.kind.header().width().max(1))
        } else {
            None
        };
        let allocated = try_allocate(&mut remaining, ideal_width, min_width, spacing, is_first);
        if allocated > 0 {
            pending.push(PendingColumn {
                spec,
                priority: candidate.priority,
                width: allocated,
                format,
            });
        }
    }

    // Post-allocation expansion: Summary first, then Message with leftovers.
    // Low-priority columns (Commit, Time, Message) are dropped when Summary
    // hasn't reached SUMMARY_THRESHOLD_FOR_LOW_PRIORITY (50).
    let mut max_summary_len = 0;
    if let Some(summary_col) = pending
        .iter_mut()
        .find(|col| col.spec.kind == ColumnKind::Summary)
    {
        if summary_col.width < MAX_SUMMARY && remaining > 0 {
            let expansion = remaining.min(MAX_SUMMARY - summary_col.width);
            summary_col.width += expansion;
            remaining -= expansion;
        }
        max_summary_len = summary_col.width;
    }

    // Drop low-priority columns to give Summary more space. Columns with
    // effective priority > Summary's (10) are dropped based on priority distance:
    // - Within 4 levels (Commit 11, Time 12, Message 13): dropped when Summary < 50
    // - Beyond 4 levels (e.g., no-data Path 17): dropped when Summary < MAX_SUMMARY
    // No-data columns naturally qualify via EMPTY_PENALTY (e.g., Path 7+10=17).
    let summary_priority = ColumnKind::Summary.priority();
    while max_summary_len > 0 && max_summary_len < MAX_SUMMARY {
        let drop_pos = pending
            .iter()
            .enumerate()
            .filter(|(_, col)| {
                if col.spec.kind == ColumnKind::Summary || col.priority <= summary_priority {
                    return false;
                }
                let gap = col.priority - summary_priority;
                let threshold = if gap <= 4 {
                    SUMMARY_THRESHOLD_FOR_LOW_PRIORITY
                } else {
                    MAX_SUMMARY
                };
                max_summary_len < threshold
            })
            .max_by_key(|(_, col)| col.priority)
            .map(|(i, _)| i);

        let Some(pos) = drop_pos else { break };

        let reclaimed = pending[pos].width + spacing;
        pending.remove(pos);
        remaining += reclaimed;

        if let Some(summary_col) = pending
            .iter_mut()
            .find(|col| col.spec.kind == ColumnKind::Summary)
        {
            let expansion = remaining.min(MAX_SUMMARY - summary_col.width);
            summary_col.width += expansion;
            remaining -= expansion;
            max_summary_len = summary_col.width;
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

    // Sort by display order to maintain correct visual order. With an explicit
    // `[list] columns` selection the configured order wins (Gutter stays first);
    // otherwise the static registry order applies.
    pending.sort_by_key(|col| display_sort_key(col.spec.kind, selected));

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

        let header = match col.spec.kind {
            ColumnKind::Custom(i) => {
                std::borrow::Cow::Owned(custom_columns[i as usize].name.clone())
            }
            kind => std::borrow::Cow::Borrowed(kind.header()),
        };
        columns.push(ColumnLayout {
            kind: col.spec.kind,
            header,
            start,
            width: col.width,
            format: col.format,
        });
    }

    // Count how many columns were hidden (not allocated).
    // This includes both data columns and empty columns that could show with more width.
    let allocated_kinds: std::collections::HashSet<_> =
        columns.iter().map(|col| col.kind).collect();
    let hidden_column_count = candidate_kinds
        .iter()
        .filter(|kind| !allocated_kinds.contains(kind))
        .count();

    LayoutConfig {
        columns,
        main_worktree_path,
        max_message_len,
        max_summary_len,
        hidden_column_count,
        status_position_mask: metadata.status_position_mask,
    }
}

/// Calculate responsive layout for an explicit terminal width.
///
/// (Contexts like skim pass a width smaller than the full terminal because the
/// list only gets part of it; the rest belongs to the preview pane.)
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
/// - CI: sized from `max_pr_number` (the cached largest PR/MR number seen,
///   e.g. "#3035"); 5 chars ("#9999") before the first fetch populates it
/// - Message: flexible (20-100 chars)
/// - URL: estimated from template + longest branch
pub fn calculate_layout_with_width(
    items: &[super::model::ListItem],
    skip_tasks: &HashSet<TaskKind>,
    terminal_width: usize,
    main_worktree_path: &Path,
    url_template: Option<&str>,
    max_pr_number: Option<u64>,
    columns: ColumnSelection,
) -> LayoutConfig {
    let custom_columns = columns.custom;
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
        .map(|path| shorten_path(path.as_path(), main_worktree_path).width())
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

    // Custom column widths are measured, not estimated: values were expanded
    // before layout. A column empty on every row stays 0 and is excluded.
    let custom_widths: Vec<usize> = custom_columns
        .iter()
        .enumerate()
        .map(|(i, column)| {
            let widest_value = items
                .iter()
                .filter_map(|item| item.custom_values.get(i))
                .map(|value| value.width())
                .max()
                .unwrap_or(0);
            if widest_value == 0 {
                0
            } else {
                fit_header(&column.name, widest_value.min(column.max_width))
            }
        })
        .collect();

    // Build pre-allocated width estimates (same as buffered mode)
    let metadata = build_estimated_widths(
        max_branch,
        skip_tasks,
        has_branch_worktree_mismatch,
        url_width,
        max_pr_number,
        custom_widths,
    );

    let commit_width = fit_header(ColumnKind::Commit.header(), COMMIT_HASH_WIDTH);

    allocate_columns_with_priority(
        &metadata,
        skip_tasks,
        max_path_width,
        commit_width,
        terminal_width,
        main_worktree_path.to_path_buf(),
        columns,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::terminal_width;
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
        let allocated = try_allocate(&mut remaining, 20, None, 2, true);
        assert_eq!(allocated, 20);
        assert_eq!(remaining, 80);

        // Subsequent columns need spacing
        let allocated = try_allocate(&mut remaining, 15, None, 2, false);
        assert_eq!(allocated, 15);
        assert_eq!(remaining, 63); // 80 - 15 - 2

        // Zero width returns 0
        let mut remaining = 50;
        assert_eq!(try_allocate(&mut remaining, 0, None, 2, false), 0);
        assert_eq!(remaining, 50);

        // Insufficient space returns 0 (no min_width)
        let mut remaining = 10;
        assert_eq!(try_allocate(&mut remaining, 20, None, 2, false), 0);
        assert_eq!(remaining, 10);
    }

    #[test]
    fn test_try_allocate_with_min_width() {
        // Ideal fits: allocate ideal
        let mut remaining = 30;
        let allocated = try_allocate(&mut remaining, 20, Some(6), 2, false);
        assert_eq!(allocated, 20);
        assert_eq!(remaining, 8); // 30 - 20 - 2

        // Ideal doesn't fit, but min does: allocate whatever fits
        let mut remaining = 15;
        let allocated = try_allocate(&mut remaining, 20, Some(6), 2, false);
        assert_eq!(allocated, 13); // 15 - 2 spacing = 13 available
        assert_eq!(remaining, 0);

        // Neither ideal nor min fits: return 0
        let mut remaining = 5;
        let allocated = try_allocate(&mut remaining, 20, Some(6), 2, false);
        assert_eq!(allocated, 0);
        assert_eq!(remaining, 5);

        // First column with min_width (no spacing cost)
        let mut remaining = 10;
        let allocated = try_allocate(&mut remaining, 20, Some(6), 2, true);
        assert_eq!(allocated, 10); // all remaining space
        assert_eq!(remaining, 0);
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
            custom: Vec::new(),
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

        // Flexible columns return None (handled specially in allocation loop)
        assert!(ColumnKind::Summary.ideal(&widths, 20, 8).is_none());
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
            custom: Vec::new(),
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
        let metadata = build_estimated_widths(20, &HashSet::new(), true, 0, None, Vec::new());
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

        // CI: no cached PR number → fallback estimate fits "#9999"
        assert_eq!(widths.ci_status, 5, "CI fallback should fit '#9999'");
    }

    #[test]
    fn test_ci_column_width_from_max_pr_number() {
        // Cached largest number sizes the column: "#12345" → 6
        let metadata =
            build_estimated_widths(20, &HashSet::new(), false, 0, Some(12345), Vec::new());
        assert_eq!(metadata.widths.ci_status, 6);

        // Never below header width ("CI" → 2)
        let metadata = build_estimated_widths(20, &HashSet::new(), false, 0, Some(1), Vec::new());
        assert_eq!(metadata.widths.ci_status, 2);
    }

    #[test]
    fn test_visible_columns_follow_gap_rule() {
        use crate::commands::list::model::{
            ActiveGitOperation, AheadBehind, BranchDiffTotals, CommitDetails, ItemKind, ListItem,
            StatusSymbols, UpstreamStatus, WorktreeData,
        };

        // Create test data with specific widths to verify position calculation
        let item = ListItem {
            head: "abc12345".to_string(),
            short_sha: "abc1234".to_string(),
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
            is_patch_id_match: None,
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
            summary: None,
            has_merge_tree_conflicts: None,
            user_marker: None,
            status_symbols: StatusSymbols::default(),
            statusline: None,
            custom_values: Vec::new(),
            kind: ItemKind::Worktree(Box::new(WorktreeData {
                path: PathBuf::from("/test/path"),
                detached: false,
                locked: None,
                prunable: None,
                working_tree_diff: Some(LineDiff::from((100, 50))),
                working_tree_status: None,
                has_conflicts: None,
                has_working_tree_conflicts: None,
                git_operation: Some(ActiveGitOperation::None),
                is_main: false,
                is_current: false,
                is_previous: false,
                branch_worktree_mismatch: false,
            })),
        };

        let items = vec![item];
        let skip_tasks: HashSet<TaskKind> = [TaskKind::BranchDiff, TaskKind::CiStatus]
            .into_iter()
            .collect();
        let main_worktree_path = PathBuf::from("/test");
        let layout = calculate_layout_with_width(
            &items,
            &skip_tasks,
            terminal_width().expect("COLUMNS=80 is set in .cargo/config.toml"),
            &main_worktree_path,
            None,
            None,
            ColumnSelection {
                custom: &[],
                selected: None,
            },
        );

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
            ActiveGitOperation, AheadBehind, BranchDiffTotals, CommitDetails, ItemKind, ListItem,
            StatusSymbols, UpstreamStatus, WorktreeData,
        };

        // Create minimal data - most columns will be empty
        let item = ListItem {
            head: "abc12345".to_string(),
            short_sha: "abc1234".to_string(),
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
            is_patch_id_match: None,
            is_ancestor: None,
            is_orphan: None,
            upstream: Some(UpstreamStatus::default()),
            pr_status: None,
            url: None,
            url_active: None,
            summary: None,
            has_merge_tree_conflicts: None,
            user_marker: None,
            status_symbols: StatusSymbols::default(),
            statusline: None,
            custom_values: Vec::new(),
            kind: ItemKind::Worktree(Box::new(WorktreeData {
                path: PathBuf::from("/test"),
                detached: false,
                locked: None,
                prunable: None,
                working_tree_diff: Some(LineDiff::default()),
                working_tree_status: None,
                has_conflicts: None,
                has_working_tree_conflicts: None,
                git_operation: Some(ActiveGitOperation::None),
                is_main: true, // Primary worktree: no ahead/behind shown
                is_current: false,
                is_previous: false,
                branch_worktree_mismatch: false,
            })),
        };

        let items = vec![item];
        let skip_tasks: HashSet<TaskKind> = [TaskKind::BranchDiff, TaskKind::CiStatus]
            .into_iter()
            .collect();
        let main_worktree_path = PathBuf::from("/home/user/project");
        let layout = calculate_layout_with_width(
            &items,
            &skip_tasks,
            terminal_width().expect("COLUMNS=80 is set in .cargo/config.toml"),
            &main_worktree_path,
            None,
            None,
            ColumnSelection {
                custom: &[],
                selected: None,
            },
        );

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

    // --- Flexible column (Summary/Message) allocation tests ---

    /// Helper: create a minimal ListItem for layout tests.
    fn make_test_item(branch: &str) -> super::super::model::ListItem {
        use crate::commands::list::model::{
            ActiveGitOperation, ItemKind, StatusSymbols, WorktreeData,
        };
        super::super::model::ListItem {
            head: "abc12345".to_string(),
            short_sha: "abc1234".to_string(),
            branch: Some(branch.to_string()),
            commit: None,
            counts: None,
            branch_diff: None,
            committed_trees_match: None,
            has_file_changes: None,
            would_merge_add: None,
            is_patch_id_match: None,
            is_ancestor: None,
            is_orphan: None,
            upstream: None,
            pr_status: None,
            url: None,
            url_active: None,
            summary: None,
            has_merge_tree_conflicts: None,
            user_marker: None,
            status_symbols: StatusSymbols::default(),
            statusline: None,
            custom_values: Vec::new(),
            kind: ItemKind::Worktree(Box::new(WorktreeData {
                path: PathBuf::from("/test/wt"),
                detached: false,
                locked: None,
                prunable: None,
                working_tree_diff: None,
                working_tree_status: None,
                has_conflicts: None,
                has_working_tree_conflicts: None,
                git_operation: Some(ActiveGitOperation::None),
                is_main: false,
                is_current: false,
                is_previous: false,
                branch_worktree_mismatch: false,
            })),
        }
    }

    /// Helper: compute layout with explicit terminal width and skip_tasks.
    fn layout_at_width(width: usize, skip_tasks: &HashSet<TaskKind>) -> LayoutConfig {
        let items = vec![make_test_item("feature-branch")];
        calculate_layout_with_width(
            &items,
            skip_tasks,
            width,
            Path::new("/test"),
            None,
            None,
            ColumnSelection {
                custom: &[],
                selected: None,
            },
        )
    }

    /// Default skip_tasks for non-full mode (CI and Summary skipped; BranchDiff
    /// runs and the `main…±` column shows by default).
    fn non_full_skip_tasks() -> HashSet<TaskKind> {
        [TaskKind::CiStatus, TaskKind::SummaryGenerate]
            .into_iter()
            .collect()
    }

    /// Full mode skip_tasks (nothing skipped).
    fn full_skip_tasks() -> HashSet<TaskKind> {
        HashSet::new()
    }

    fn find_column(layout: &LayoutConfig, kind: ColumnKind) -> Option<&ColumnLayout> {
        layout.columns.iter().find(|c| c.kind == kind)
    }

    #[test]
    fn test_selected_columns_filter_and_reorder() {
        // Select a subset in a non-default order. At a wide width nothing drops,
        // so the visible set is exactly Gutter (always) + the selection, in the
        // configured order (Gutter pinned first) — not the static registry order.
        let items = vec![make_test_item("feature-branch")];
        let selected = [ColumnKind::Time, ColumnKind::Branch, ColumnKind::Commit];
        let layout = calculate_layout_with_width(
            &items,
            &full_skip_tasks(),
            300,
            Path::new("/test"),
            None,
            None,
            ColumnSelection {
                custom: &[],
                selected: Some(&selected),
            },
        );

        let kinds: Vec<ColumnKind> = layout.columns.iter().map(|c| c.kind).collect();
        assert_eq!(
            kinds,
            vec![
                ColumnKind::Gutter,
                ColumnKind::Time,
                ColumnKind::Branch,
                ColumnKind::Commit
            ],
            "only Gutter + selected columns appear, in the configured order"
        );
        // Deselected built-ins are absent and don't inflate the hidden count
        // (they were never candidates).
        assert!(find_column(&layout, ColumnKind::Status).is_none());
        assert!(find_column(&layout, ColumnKind::Message).is_none());
        assert_eq!(layout.hidden_column_count, 0);
    }

    #[test]
    fn test_selected_columns_include_and_hide_custom() {
        // A selection lists built-ins and custom columns in one ordered set: a
        // named custom renders interleaved at its configured position, while a
        // custom omitted from a non-empty selection is hidden.
        let mut item = make_test_item("feature-branch");
        item.custom_values = vec!["TICKET-1".to_string(), "max".to_string()];
        let items = vec![item];
        let custom = [
            ResolvedCustomColumn {
                name: "Ticket".to_string(),
                template: "{{ vars.ticket }}".to_string(),
                max_width: 40,
                priority: 9,
            },
            ResolvedCustomColumn {
                name: "Owner".to_string(),
                template: "{{ vars.owner }}".to_string(),
                max_width: 40,
                priority: 9,
            },
        ];
        // Place Ticket (custom 0) between two built-ins; omit Owner (custom 1).
        let selected = [
            ColumnKind::Branch,
            ColumnKind::Custom(0),
            ColumnKind::Commit,
        ];
        let layout = calculate_layout_with_width(
            &items,
            &full_skip_tasks(),
            300,
            Path::new("/test"),
            None,
            None,
            ColumnSelection {
                custom: &custom,
                selected: Some(&selected),
            },
        );

        let kinds: Vec<ColumnKind> = layout.columns.iter().map(|c| c.kind).collect();
        assert_eq!(
            kinds,
            vec![
                ColumnKind::Gutter,
                ColumnKind::Branch,
                ColumnKind::Custom(0),
                ColumnKind::Commit,
            ],
            "a named custom renders interleaved at its configured position"
        );
        assert!(
            find_column(&layout, ColumnKind::Custom(1)).is_none(),
            "a custom omitted from a non-empty selection is hidden"
        );
    }

    #[test]
    fn test_default_columns_append_custom() {
        // Without a selection (the default set), every custom column appends in
        // resolution order — the selection filter only applies when present.
        let mut item = make_test_item("feature-branch");
        item.custom_values = vec!["TICKET-1".to_string()];
        let items = vec![item];
        let custom = [ResolvedCustomColumn {
            name: "Ticket".to_string(),
            template: "{{ vars.ticket }}".to_string(),
            max_width: 40,
            priority: 9,
        }];
        let layout = calculate_layout_with_width(
            &items,
            &full_skip_tasks(),
            300,
            Path::new("/test"),
            None,
            None,
            ColumnSelection {
                custom: &custom,
                selected: None,
            },
        );
        assert!(
            find_column(&layout, ColumnKind::Custom(0)).is_some(),
            "custom columns append in the default (no-selection) set"
        );
    }

    #[test]
    fn test_selected_columns_cannot_force_a_gated_column() {
        // Selection narrows the candidate set; it can't force on a column gated
        // off by skip_tasks. `ci` needs --full: with CiStatus skipped, selecting
        // it leaves a gutter-only table (the skip_tasks filter runs first).
        let items = vec![make_test_item("feature-branch")];
        let selected = [ColumnKind::CiStatus];
        let gated = calculate_layout_with_width(
            &items,
            &non_full_skip_tasks(),
            200,
            Path::new("/test"),
            None,
            None,
            ColumnSelection {
                custom: &[],
                selected: Some(&selected),
            },
        );
        assert!(
            find_column(&gated, ColumnKind::CiStatus).is_none(),
            "a gated column stays hidden even when selected"
        );
        let kinds: Vec<ColumnKind> = gated.columns.iter().map(|c| c.kind).collect();
        assert_eq!(kinds, vec![ColumnKind::Gutter], "only the gutter survives");

        // Once the gate opens (full mode, nothing skipped), the same selection
        // renders CI.
        let full = calculate_layout_with_width(
            &items,
            &full_skip_tasks(),
            200,
            Path::new("/test"),
            None,
            None,
            ColumnSelection {
                custom: &[],
                selected: Some(&selected),
            },
        );
        assert!(
            find_column(&full, ColumnKind::CiStatus).is_some(),
            "the selected column renders once its task is no longer skipped"
        );
    }

    #[test]
    fn test_selected_columns_drop_by_base_priority() {
        // Selection controls membership/order, not drop priority: on a narrow
        // terminal the least important *selected* column drops by base_priority
        // (Message=13 > Branch=1), independent of configured order.
        let items = vec![make_test_item("a-fairly-long-feature-branch-name")];
        let selected = [ColumnKind::Message, ColumnKind::Branch];
        let layout = calculate_layout_with_width(
            &items,
            &full_skip_tasks(),
            24,
            Path::new("/test"),
            None,
            None,
            ColumnSelection {
                custom: &[],
                selected: Some(&selected),
            },
        );
        assert!(
            find_column(&layout, ColumnKind::Branch).is_some(),
            "Branch (priority 1) survives a narrow width"
        );
        assert!(
            find_column(&layout, ColumnKind::Message).is_none(),
            "Message (priority 13) drops first despite being listed first"
        );
    }

    #[test]
    fn test_summary_absent_when_skipped() {
        // Non-full mode: SummaryGenerate in skip_tasks → no Summary column
        let layout = layout_at_width(200, &non_full_skip_tasks());

        assert!(
            find_column(&layout, ColumnKind::Summary).is_none(),
            "Summary should not appear when SummaryGenerate is skipped"
        );
        assert_eq!(layout.max_summary_len, 0);

        // Message should still be present and get leftover space
        assert!(find_column(&layout, ColumnKind::Message).is_some());
        assert!(layout.max_message_len > 0);
    }

    #[test]
    fn test_summary_present_in_full_mode() {
        let layout = layout_at_width(200, &full_skip_tasks());

        assert!(
            find_column(&layout, ColumnKind::Summary).is_some(),
            "Summary should appear in full mode"
        );
        assert!(layout.max_summary_len > 0);
    }

    #[test]
    fn test_summary_expands_before_message() {
        // At a moderate width, Summary should expand toward its max (70)
        // before Message gets leftover space.
        let layout = layout_at_width(200, &full_skip_tasks());

        let summary = find_column(&layout, ColumnKind::Summary);
        let message = find_column(&layout, ColumnKind::Message);

        assert!(summary.is_some(), "Summary should be allocated");
        assert!(message.is_some(), "Message should be allocated");

        let summary_width = summary.unwrap().width;
        let message_width = message.unwrap().width;

        // Summary should have expanded beyond its minimum of 10
        assert!(
            summary_width > 10,
            "Summary should expand beyond minimum: got {summary_width}"
        );
        assert_eq!(layout.max_summary_len, summary_width);
        assert_eq!(layout.max_message_len, message_width);
    }

    #[test]
    fn test_summary_capped_at_max() {
        // Very wide terminal: Summary should cap at MAX_SUMMARY (70)
        let layout = layout_at_width(500, &full_skip_tasks());

        let summary = find_column(&layout, ColumnKind::Summary).unwrap();
        assert_eq!(summary.width, 70, "Summary should cap at MAX_SUMMARY (70)");
        assert_eq!(layout.max_summary_len, 70);
    }

    #[test]
    fn test_message_capped_at_max() {
        // Very wide terminal: Message should cap at MAX_MESSAGE (100)
        let layout = layout_at_width(500, &full_skip_tasks());

        let message = find_column(&layout, ColumnKind::Message).unwrap();
        assert_eq!(
            message.width, 100,
            "Message should cap at MAX_MESSAGE (100)"
        );
        assert_eq!(layout.max_message_len, 100);
    }

    #[test]
    fn test_message_gets_more_space_when_summary_skipped() {
        // Compare Message width with and without Summary
        let with_summary = layout_at_width(200, &full_skip_tasks());
        let without_summary = layout_at_width(200, &non_full_skip_tasks());

        let msg_with = find_column(&with_summary, ColumnKind::Message)
            .unwrap()
            .width;
        let msg_without = find_column(&without_summary, ColumnKind::Message)
            .unwrap()
            .width;

        // Without Summary, Message should get more space (or equal if both maxed)
        assert!(
            msg_without >= msg_with,
            "Message should get at least as much space without Summary: \
             with={msg_with}, without={msg_without}"
        );
    }

    #[test]
    fn test_summary_display_order() {
        // Summary should appear between BranchDiff and Upstream in display order
        let layout = layout_at_width(500, &full_skip_tasks());

        let kinds: Vec<ColumnKind> = layout.columns.iter().map(|c| c.kind).collect();

        if let Some(summary_pos) = kinds.iter().position(|k| *k == ColumnKind::Summary) {
            // Summary should come after BranchDiff (if present) and before Upstream (if present)
            if let Some(branch_diff_pos) = kinds.iter().position(|k| *k == ColumnKind::BranchDiff) {
                assert!(
                    summary_pos > branch_diff_pos,
                    "Summary should appear after BranchDiff"
                );
            }
            if let Some(upstream_pos) = kinds.iter().position(|k| *k == ColumnKind::Upstream) {
                assert!(
                    summary_pos < upstream_pos,
                    "Summary should appear before Upstream"
                );
            }
        } else {
            panic!("Summary column should be present at width 500");
        }
    }

    #[test]
    fn test_low_priority_columns_gated_on_summary_threshold() {
        // Probe widths: when Summary is present but < 50, Commit/Time/Message must be absent.
        // At wide widths where Summary >= 50, they can appear.
        let mut found_below = false;
        for width in 80..200 {
            let l = layout_at_width(width, &full_skip_tasks());
            if let Some(s) = find_column(&l, ColumnKind::Summary)
                && s.width < 50
            {
                found_below = true;
                assert!(
                    find_column(&l, ColumnKind::Commit).is_none(),
                    "Commit present at width {width} with Summary {}",
                    s.width
                );
                assert!(
                    find_column(&l, ColumnKind::Time).is_none(),
                    "Time present at width {width} with Summary {}",
                    s.width
                );
                assert!(
                    find_column(&l, ColumnKind::Message).is_none(),
                    "Message present at width {width} with Summary {}",
                    s.width
                );
            }
        }
        assert!(found_below, "no width produced Summary < 50");

        // At 200, Summary is well above threshold and all columns appear.
        let l = layout_at_width(200, &full_skip_tasks());
        assert!(find_column(&l, ColumnKind::Summary).unwrap().width >= 50);
        assert!(find_column(&l, ColumnKind::Commit).is_some());
        assert!(find_column(&l, ColumnKind::Time).is_some());
        assert!(find_column(&l, ColumnKind::Message).is_some());
    }

    #[test]
    fn test_narrow_terminal_drops_flexible_columns() {
        // At a very narrow width, neither Summary nor Message should fit
        // after the critical fixed columns are allocated.
        let layout = layout_at_width(40, &full_skip_tasks());

        // At 40 chars, only Gutter (2) + Branch (~14) can fit
        assert!(
            find_column(&layout, ColumnKind::Summary).is_none(),
            "Summary should not fit at 40 chars"
        );
        assert!(
            find_column(&layout, ColumnKind::Message).is_none(),
            "Message should not fit at 40 chars"
        );
    }

    /// Helper: create a test item with a specific worktree path and no mismatch.
    fn make_test_item_at(branch: &str, path: &str) -> super::super::model::ListItem {
        use crate::commands::list::model::{
            ActiveGitOperation, ItemKind, StatusSymbols, WorktreeData,
        };
        super::super::model::ListItem {
            head: "abc12345".to_string(),
            short_sha: "abc1234".to_string(),
            branch: Some(branch.to_string()),
            commit: None,
            counts: None,
            branch_diff: None,
            committed_trees_match: None,
            has_file_changes: None,
            would_merge_add: None,
            is_patch_id_match: None,
            is_ancestor: None,
            is_orphan: None,
            upstream: None,
            pr_status: None,
            url: None,
            url_active: None,
            summary: None,
            has_merge_tree_conflicts: None,
            user_marker: None,
            status_symbols: StatusSymbols::default(),
            statusline: None,
            custom_values: Vec::new(),
            kind: ItemKind::Worktree(Box::new(WorktreeData {
                path: PathBuf::from(path),
                detached: false,
                locked: None,
                prunable: None,
                working_tree_diff: None,
                working_tree_status: None,
                has_conflicts: None,
                has_working_tree_conflicts: None,
                git_operation: Some(ActiveGitOperation::None),
                is_main: false,
                is_current: false,
                is_previous: false,
                branch_worktree_mismatch: false,
            })),
        }
    }

    /// When paths are consistent (no mismatch), Path should yield space to Summary.
    ///
    /// Scenario: 4 worktrees with `../agents.*` sibling paths (longest ~28 chars),
    /// full mode, moderate terminal width. Path is redundant (paths are predictable
    /// from branch names) and should not reduce Summary's readability.
    ///
    /// At very wide terminals both columns coexist. At moderate widths where space
    /// is constrained, Summary should be preferred over Path.
    #[test]
    fn test_path_yields_to_summary_when_no_mismatch() {
        // Mirrors the user's real setup: worktrees at sibling dirs with consistent naming
        let items = vec![
            {
                let mut item = make_test_item_at("main", "/test/worktrunk");
                if let super::super::model::ItemKind::Worktree(ref mut data) = item.kind {
                    data.is_main = true;
                }
                item
            },
            make_test_item_at("hourly-maintenance", "/test/agents.hourly-maintenance"),
            make_test_item_at("lab-continue", "/test/agents.lab-continue"),
            make_test_item_at("dry-run-pager", "/test/agents.dry-run-pager"),
        ];
        let main_path = Path::new("/test/worktrunk");

        // Full mode: all columns enabled
        let skip = full_skip_tasks();

        // At very wide terminals: both Path and Summary coexist
        let layout_wide = calculate_layout_with_width(
            &items,
            &skip,
            300,
            main_path,
            None,
            None,
            ColumnSelection {
                custom: &[],
                selected: None,
            },
        );
        assert!(
            find_column(&layout_wide, ColumnKind::Summary).is_some(),
            "Summary should be present at 300"
        );
        assert!(
            find_column(&layout_wide, ColumnKind::Path).is_some(),
            "Path should be present at 300 (infinite space → show everything)"
        );

        // At moderate widths (170): Summary should reach at least 50 chars.
        // Currently Path eats ~30 chars from Summary's expansion budget,
        // leaving Summary at ~48 and dropping Message entirely.
        let layout_170 = calculate_layout_with_width(
            &items,
            &skip,
            170,
            main_path,
            None,
            None,
            ColumnSelection {
                custom: &[],
                selected: None,
            },
        );
        let summary_170 = find_column(&layout_170, ColumnKind::Summary)
            .expect("Summary should be present at 170")
            .width;
        assert!(
            summary_170 >= 50,
            "Summary should reach at least 50 at width 170 when paths are consistent: got {summary_170}"
        );
    }

    /// Snapshot test rendering the motivating case: 4 worktrees with consistent
    /// paths, full mode, 170-char terminal. Verifies Summary gets adequate space
    /// by dropping redundant Path column.
    #[test]
    fn test_snapshot_path_yields_to_summary() {
        use crate::commands::list::model::{
            ActiveGitOperation, AheadBehind, BranchDiffTotals, CommitDetails, ItemKind,
            StatusSymbols, UpstreamStatus, WorktreeData,
        };
        use worktrunk::git::LineDiff;

        let ts = 1742500000; // fixed timestamp for reproducible "Age"

        let make_item = |branch: &str,
                         path: &str,
                         is_main: bool,
                         is_current: bool,
                         ahead: usize,
                         behind: usize,
                         diff: Option<(usize, usize)>,
                         summary: Option<&str>,
                         upstream: bool|
         -> super::super::model::ListItem {
            let counts = if is_main {
                None
            } else {
                Some(AheadBehind { ahead, behind })
            };
            let branch_diff = diff.map(|(a, d)| BranchDiffTotals {
                diff: LineDiff::from((a, d)),
            });
            let upstream_status = upstream.then(|| UpstreamStatus {
                remote: Some("origin".to_string()),
                ahead: 0,
                behind: 0,
            });
            super::super::model::ListItem {
                head: "a620bcfe".to_string(),
                short_sha: "a620bcf".to_string(),
                branch: Some(branch.to_string()),
                commit: Some(CommitDetails {
                    timestamp: ts,
                    commit_message: "Some commit message".to_string(),
                }),
                counts,
                branch_diff,
                committed_trees_match: None,
                has_file_changes: None,
                would_merge_add: None,
                is_patch_id_match: None,
                is_ancestor: None,
                is_orphan: None,
                upstream: upstream_status,
                pr_status: Some(None), // loaded, no CI
                url: None,
                url_active: None,
                summary: Some(summary.map(|s| s.to_string())),
                has_merge_tree_conflicts: None,
                user_marker: None,
                status_symbols: StatusSymbols::default(),
                statusline: None,
                custom_values: Vec::new(),
                kind: ItemKind::Worktree(Box::new(WorktreeData {
                    path: PathBuf::from(path),
                    detached: false,
                    locked: None,
                    prunable: None,
                    working_tree_diff: Some(LineDiff::default()),
                    working_tree_status: None,
                    has_conflicts: None,
                    has_working_tree_conflicts: None,
                    git_operation: Some(ActiveGitOperation::None),
                    is_main,
                    is_current,
                    is_previous: false,
                    branch_worktree_mismatch: false,
                })),
            }
        };

        let items = vec![
            make_item(
                "main",
                "/test/worktrunk",
                true,
                true,
                0,
                0,
                None,
                None,
                true,
            ),
            make_item(
                "hourly-maintenance",
                "/test/agents.hourly-maintenance",
                false,
                false,
                2,
                0,
                None,
                None,
                false,
            ),
            make_item(
                "lab-continue",
                "/test/agents.lab-continue",
                false,
                false,
                1,
                2,
                Some((28, 1)),
                Some("Add extend and block insert in Markdown parser"),
                true,
            ),
            make_item(
                "dry-run-pager",
                "/test/agents.dry-run-pager",
                false,
                false,
                3,
                1,
                None,
                None,
                true,
            ),
        ];
        let main_path = Path::new("/test/worktrunk");
        let skip = full_skip_tasks();

        let layout = calculate_layout_with_width(
            &items,
            &skip,
            170,
            main_path,
            None,
            None,
            ColumnSelection {
                custom: &[],
                selected: None,
            },
        );

        let mut lines = Vec::new();
        lines.push(layout.render_header_line().plain_text());
        for item in &items {
            lines.push(
                layout
                    .render_list_item_line(item, super::super::render::PLACEHOLDER)
                    .plain_text(),
            );
        }
        let table = lines.join("\n");
        insta::assert_snapshot!(table);
    }

    #[test]
    fn test_branch_column_never_dropped() {
        // Branch is shrinkable: it should always be present, even at narrow widths
        // where its ideal width (longest branch name) doesn't fit.
        let items = vec![make_test_item(
            "feature/very-long-branch-name-that-exceeds-available-space",
        )];
        let skip = non_full_skip_tasks();
        let main_path = Path::new("/test");

        // At 30 cols, ideal branch width (~57) can't fit, but Branch should still
        // be allocated at a reduced width rather than dropped.
        let layout = calculate_layout_with_width(
            &items,
            &skip,
            30,
            main_path,
            None,
            None,
            ColumnSelection {
                custom: &[],
                selected: None,
            },
        );
        let branch = find_column(&layout, ColumnKind::Branch);
        assert!(
            branch.is_some(),
            "Branch column should never be dropped, even at 30 cols"
        );
        let branch_width = branch.unwrap().width;
        assert!(
            branch_width >= 6,
            "Branch should be at least header width (6): got {branch_width}"
        );

        // At 80 cols, Branch should fit comfortably
        let layout = calculate_layout_with_width(
            &items,
            &skip,
            80,
            main_path,
            None,
            None,
            ColumnSelection {
                custom: &[],
                selected: None,
            },
        );
        let branch = find_column(&layout, ColumnKind::Branch).unwrap();
        assert!(
            branch.width > 6,
            "Branch should have more than header width at 80 cols"
        );
    }

    #[test]
    fn test_summary_skipped_preserves_other_full_columns() {
        // Even with SummaryGenerate skipped, other full-mode columns should still appear
        let mut skip_only_summary: HashSet<TaskKind> = HashSet::new();
        skip_only_summary.insert(TaskKind::SummaryGenerate);

        let layout = layout_at_width(300, &skip_only_summary);

        assert!(
            find_column(&layout, ColumnKind::Summary).is_none(),
            "Summary should be skipped"
        );
        assert!(
            find_column(&layout, ColumnKind::BranchDiff).is_some(),
            "BranchDiff should still appear"
        );
        assert!(
            find_column(&layout, ColumnKind::CiStatus).is_some(),
            "CiStatus should still appear"
        );
        assert!(
            find_column(&layout, ColumnKind::Message).is_some(),
            "Message should still appear"
        );
    }

    fn custom_column(name: &str, max_width: usize, priority: u8) -> ResolvedCustomColumn {
        ResolvedCustomColumn {
            name: name.to_string(),
            template: String::new(),
            max_width,
            priority,
        }
    }

    #[test]
    fn test_custom_columns_measured_width_and_position() {
        let columns = [
            custom_column("Ticket", 10, 9),
            custom_column("WideHeaderName", 40, 9),
        ];
        let mut item = make_test_item("feature");
        item.custom_values = vec!["JIRA-1234-overflows".to_string(), "x".to_string()];
        let items = vec![item];

        let layout = calculate_layout_with_width(
            &items,
            &non_full_skip_tasks(),
            300,
            Path::new("/test"),
            None,
            None,
            ColumnSelection {
                custom: &columns,
                selected: None,
            },
        );

        // Value wider than max_width clamps to it
        let ticket = find_column(&layout, ColumnKind::Custom(0)).expect("Ticket allocated");
        assert_eq!(ticket.header, "Ticket");
        assert_eq!(ticket.width, 10);

        // Value narrower than the header floors at header width
        let wide = find_column(&layout, ColumnKind::Custom(1)).expect("second column allocated");
        assert_eq!(wide.width, "WideHeaderName".len());

        // Display position: custom columns keep resolution order and render
        // before Commit
        let pos = |kind| layout.columns.iter().position(|c| c.kind == kind).unwrap();
        assert!(pos(ColumnKind::Custom(0)) < pos(ColumnKind::Custom(1)));
        assert!(pos(ColumnKind::Custom(1)) < pos(ColumnKind::Commit));
    }

    #[test]
    fn test_custom_column_empty_everywhere_is_excluded() {
        let columns = [custom_column("Ticket", 40, 9)];
        let mut item = make_test_item("feature");
        item.custom_values = vec![String::new()];
        let items = vec![item];

        // Skip UrlStatus like collect does when no URL template is
        // configured; otherwise the zero-width Url candidate counts as a
        // hidden column and obscures the assertion below.
        let mut skip_tasks = non_full_skip_tasks();
        skip_tasks.insert(TaskKind::UrlStatus);

        let layout = calculate_layout_with_width(
            &items,
            &skip_tasks,
            300,
            Path::new("/test"),
            None,
            None,
            ColumnSelection {
                custom: &columns,
                selected: None,
            },
        );

        // Values are final before layout, so an all-empty column is excluded
        // entirely — not allocated and not counted as hidden
        assert!(find_column(&layout, ColumnKind::Custom(0)).is_none());
        assert_eq!(layout.hidden_column_count, 0);
    }

    #[test]
    fn test_custom_column_dropped_on_narrow_terminal() {
        let columns = [custom_column("Ticket", 40, 9)];
        let mut item = make_test_item("feature");
        item.custom_values = vec!["JIRA-1234".to_string()];
        let items = vec![item];

        let narrow = calculate_layout_with_width(
            &items,
            &non_full_skip_tasks(),
            30,
            Path::new("/test"),
            None,
            None,
            ColumnSelection {
                custom: &columns,
                selected: None,
            },
        );

        // Priority 9 loses to the core columns when space runs out, and the
        // unallocated candidate counts toward the hidden-column footer
        assert!(find_column(&narrow, ColumnKind::Custom(0)).is_none());
        assert!(find_column(&narrow, ColumnKind::Branch).is_some());
        assert!(narrow.hidden_column_count > 0);
    }
}
