//! Preview mode and layout management.
//!
//! Tracks the active preview tab (in process memory) and auto-detects the
//! preview layout for the interactive selector.

use std::sync::atomic::{AtomicU8, Ordering};

/// Preview modes for the interactive selector
///
/// Each mode shows a different aspect of the selected row:
/// 1. WorkingTree: Uncommitted changes (git diff HEAD --stat)
/// 2. Log: Commit history since diverging from the default branch (git log with merge-base)
/// 3. BranchDiff: Line diffs since the merge-base with the default branch (git diff --stat DEFAULT…)
/// 4. UpstreamDiff: Diff vs upstream tracking branch (ahead/behind)
/// 5. Summary: LLM-generated branch summary (requires [commit.generation] config)
/// 6. Pr: The selected row's PR/MR, rendered from already-fetched data (no network)
/// 7. Comments: The PR/MR's comment thread (background forge fetch on `--prs` rows)
///
/// A mode whose content is structurally absent for the current row is rendered
/// de-emphasized in the tab bar (see `TabAvailability` / `render_preview_tabs`):
/// tab 4 when the branch has no upstream, tab 5 when summaries are disabled,
/// the working-tree/branch-diff/upstream/summary tabs on a `--prs` row (no
/// local worktree), tab 6 on a worktree row (PR previews render only on `--prs`
/// rows), and tab 7 (comments) on a worktree row (comments are fetched only for
/// `--prs` rows).
///
/// Loosely aligned with `wt list` columns, though not a perfect match:
/// - Tab 1 corresponds to "HEAD±" column
/// - Tab 2 shows commits (related to "main↕" counts)
/// - Tab 3 corresponds to "main…± (--full)" column
/// - Tab 4 corresponds to "Remote⇅" column
/// - Tab 6 corresponds to the "CI" column's PR/MR
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum PreviewMode {
    WorkingTree = 1,
    Log = 2,
    BranchDiff = 3,
    UpstreamDiff = 4,
    Summary = 5,
    Pr = 6,
    Comments = 7,
}

impl PreviewMode {
    pub(super) fn from_u8(n: u8) -> Self {
        match n {
            2 => Self::Log,
            3 => Self::BranchDiff,
            4 => Self::UpstreamDiff,
            5 => Self::Summary,
            6 => Self::Pr,
            7 => Self::Comments,
            _ => Self::WorkingTree,
        }
    }

    /// The next tab, wrapping `Comments` → `WorkingTree` (tab key).
    pub(super) fn next(self) -> Self {
        Self::from_u8(if self as u8 >= 7 { 1 } else { self as u8 + 1 })
    }

    /// The previous tab, wrapping `WorkingTree` → `Comments` (shift-tab key).
    pub(super) fn prev(self) -> Self {
        Self::from_u8(if self as u8 <= 1 { 7 } else { self as u8 - 1 })
    }
}

/// Typical terminal character aspect ratio (width/height).
///
/// Terminal characters are taller than wide - typically around 0.5 (twice as tall as wide).
/// This varies by font, but 0.5 is a reasonable default for monospace fonts.
const CHAR_ASPECT_RATIO: f64 = 0.5;

/// Skim uses this percentage of terminal height.
pub(super) const SKIM_HEIGHT_PERCENT: usize = 90;

/// Lines reserved for skim chrome (header + prompt/margins).
pub(super) const LIST_CHROME_LINES: usize = 4;

/// Minimum preview lines to keep usable even with many items.
pub(super) const MIN_PREVIEW_LINES: usize = 5;

/// Minimum list rows to keep selectable even on a short terminal.
pub(super) const MIN_VISIBLE_ITEMS: usize = 3;

/// Preview width as percentage of terminal width (for Right layout).
const PREVIEW_WIDTH_PERCENT: usize = 50;

/// Minimum terminal columns for side-by-side (Right) layout.
///
/// Below this width, the list panel in Right layout is too narrow
/// for branch names to be readable. Fall back to Down layout instead.
const MIN_COLS_FOR_RIGHT_LAYOUT: f64 = 80.0;

/// Skim's usable height: the share of the terminal it actually paints
/// (`SkimOptions::height("90%")`).
///
/// The single home for the `SKIM_HEIGHT_PERCENT` conversion. Every site that
/// needs the available row budget — both layout arms in `dimensions_for`, the
/// picker's `num_items_estimate` cap, and the preview half-page scroll — derives
/// it here, so the definition can't drift between them.
pub(super) fn available_height(term_height: usize) -> usize {
    term_height * SKIM_HEIGHT_PERCENT / 100
}

/// Maximum list rows the Down layout shows before scrolling.
///
/// The list may claim up to half of skim's area (`available / 2`); the preview
/// keeps the other half, so visible rows scale with terminal height instead of a
/// fixed ceiling. Integer division truncates the list's half toward the preview —
/// a deliberate preview-favoring tie-break. Floored at `MIN_VISIBLE_ITEMS` so the
/// cap stays usable on a short terminal; on a terminal too short to fit both this
/// floor and `MIN_PREVIEW_LINES`, `dimensions_for` lets the preview floor win and
/// skim clamps the remainder. The single source of truth for the cap: both
/// `dimensions_for` and the picker's `num_items_estimate` short-circuit gate on it,
/// applying one formula at both sites.
pub(super) fn max_visible_items(available: usize) -> usize {
    let rows = (available / 2).saturating_sub(LIST_CHROME_LINES);
    rows.max(MIN_VISIBLE_ITEMS)
}

/// Preview layout orientation for the interactive selector
///
/// Preview window position (auto-detected at startup based on terminal dimensions)
///
/// - Right: Preview on the right side (50% width) - better for wide terminals
/// - Down: Preview below the list - better for tall/vertical monitors
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum PreviewLayout {
    #[default]
    Right,
    Down,
}

impl PreviewLayout {
    /// Auto-detect layout based on terminal dimensions.
    ///
    /// Terminal dimensions are in characters, not pixels. Since characters are
    /// typically twice as tall as wide (~0.5 aspect ratio), we correct for this
    /// when calculating the effective aspect ratio.
    ///
    /// Example: 180 cols × 136 rows
    /// - Raw ratio: 180/136 = 1.32 (appears landscape)
    /// - Effective: 1.32 × 0.5 = 0.66 (actually portrait!)
    ///
    /// Returns Down for portrait (effective ratio < 1.0), Right for landscape.
    /// Also returns Down when the terminal is too narrow for side-by-side layout,
    /// even if the aspect ratio suggests landscape (e.g. 60×24 on a phone).
    pub(super) fn auto_detect() -> Self {
        let (cols, rows) = terminal_size::terminal_size()
            .map(|(terminal_size::Width(w), terminal_size::Height(h))| (w as f64, h as f64))
            .unwrap_or((80.0, 24.0));

        Self::for_dimensions(cols, rows)
    }

    /// Determine layout for given terminal dimensions (cols × rows).
    fn for_dimensions(cols: f64, rows: f64) -> Self {
        // Too narrow for side-by-side — branch names won't fit in half the width
        if cols < MIN_COLS_FOR_RIGHT_LAYOUT {
            return Self::Down;
        }

        // Effective aspect ratio accounting for character shape
        let effective_ratio = (cols / rows) * CHAR_ASPECT_RATIO;

        if effective_ratio < 1.0 {
            Self::Down
        } else {
            Self::Right
        }
    }

    /// Calculate the preview window spec for skim.
    /// Derives dimensions from `preview_dimensions` to ensure consistency.
    pub(super) fn to_preview_window_spec(self, num_items: usize) -> String {
        let (width, height) = self.preview_dimensions(num_items);
        match self {
            Self::Right => format!("right:{}", width),
            Self::Down => format!("down:{}", height),
        }
    }

    /// Calculate preview dimensions (width, height) in characters.
    /// Single source of truth for preview sizing — used by both skim config
    /// and background pre-computation. Reads the live terminal size, then
    /// delegates the pure layout math to `dimensions_for`.
    pub(super) fn preview_dimensions(self, num_items: usize) -> (usize, usize) {
        let (term_width, term_height) = terminal_size::terminal_size()
            .map(|(terminal_size::Width(w), terminal_size::Height(h))| (w as usize, h as usize))
            .unwrap_or((80, 24));

        self.dimensions_for(term_width, term_height, num_items)
    }

    /// Pure layout math behind `preview_dimensions`, with the terminal size
    /// passed in so the split is testable without a live TTY.
    ///
    /// Right keeps a fixed 50%/90% split independent of `num_items`. Down lets
    /// the list grow to `max_visible_items(available)` rows (half of skim's
    /// area) and hands the rest to the preview, floored at `MIN_PREVIEW_LINES`
    /// and clamped to never exceed `available`.
    fn dimensions_for(
        self,
        term_width: usize,
        term_height: usize,
        num_items: usize,
    ) -> (usize, usize) {
        match self {
            Self::Right => {
                let width = term_width * PREVIEW_WIDTH_PERCENT / 100;
                let height = available_height(term_height);
                (width, height)
            }
            Self::Down => {
                let width = term_width;
                let available = available_height(term_height);
                let list_lines = LIST_CHROME_LINES + num_items.min(max_visible_items(available));
                let remaining = available.saturating_sub(list_lines);
                let height = remaining.max(MIN_PREVIEW_LINES).min(available);
                (width, height)
            }
        }
    }
}

/// The active preview tab, shared across the picker session.
///
/// One picker runs per process, so a single process-wide value is the source of
/// truth: `WorktreeSkimItem::preview` / `PrSkimItem::preview` read it to choose
/// what to render, and the keymap's `Action::Custom` callbacks (installed in
/// `super::install_preview_tab_keybindings`) write it on alt-1…alt-7 / tab /
/// shift-tab, then re-run the preview.
///
/// It lives in memory rather than on disk. Tab switching used to write a digit
/// to a per-process temp file via `echo`/`tr`/`mv` keybind commands, because
/// skim runs keybind commands through a shell and a shell command was the only
/// way to react to a keypress. The native `Action::Custom` path removes that
/// constraint — and with it the file, whose `tr`/`mv` commands skim's Windows
/// shell (cmd.exe) cannot run.
static PREVIEW_MODE: AtomicU8 = AtomicU8::new(PreviewMode::WorkingTree as u8);

pub(super) struct PreviewStateData;

impl PreviewStateData {
    /// The active preview tab.
    pub(super) fn read_mode() -> PreviewMode {
        PreviewMode::from_u8(PREVIEW_MODE.load(Ordering::Relaxed))
    }

    /// Jump to a specific tab (alt-1…alt-7).
    pub(super) fn set_mode(mode: PreviewMode) {
        PREVIEW_MODE.store(mode as u8, Ordering::Relaxed);
    }

    /// Cycle to the next (`forward`) / previous tab, wrapping around (tab /
    /// shift-tab). Runs on skim's single event-loop thread, so the
    /// load-then-store needs no compare-and-swap.
    pub(super) fn rotate(forward: bool) {
        let current = Self::read_mode();
        Self::set_mode(if forward {
            current.next()
        } else {
            current.prev()
        });
    }
}

/// Per-session preview state: the auto-detected initial layout. Constructing it
/// also resets [`PreviewStateData`] to the working-tree tab so every picker
/// opens on the same tab.
pub(super) struct PreviewState {
    pub(super) initial_layout: PreviewLayout,
}

impl PreviewState {
    pub(super) fn new() -> Self {
        PreviewStateData::set_mode(PreviewMode::WorkingTree);
        Self {
            initial_layout: PreviewLayout::auto_detect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_preview_mode_from_u8() {
        assert_eq!(PreviewMode::from_u8(1), PreviewMode::WorkingTree);
        assert_eq!(PreviewMode::from_u8(2), PreviewMode::Log);
        assert_eq!(PreviewMode::from_u8(3), PreviewMode::BranchDiff);
        assert_eq!(PreviewMode::from_u8(4), PreviewMode::UpstreamDiff);
        assert_eq!(PreviewMode::from_u8(5), PreviewMode::Summary);
        assert_eq!(PreviewMode::from_u8(6), PreviewMode::Pr);
        assert_eq!(PreviewMode::from_u8(7), PreviewMode::Comments);
        // Invalid values default to WorkingTree
        assert_eq!(PreviewMode::from_u8(0), PreviewMode::WorkingTree);
        assert_eq!(PreviewMode::from_u8(99), PreviewMode::WorkingTree);
    }

    #[test]
    fn test_preview_layout_to_preview_window_spec() {
        // Right uses absolute width derived from terminal size
        let spec = PreviewLayout::Right.to_preview_window_spec(10);
        assert!(spec.starts_with("right:"));

        // Down calculates based on item count
        let spec = PreviewLayout::Down.to_preview_window_spec(5);
        assert!(spec.starts_with("down:"));
    }

    #[test]
    fn test_available_height_is_skim_height_percent() {
        // The single home for skim's 90%-of-terminal conversion.
        assert_eq!(available_height(0), 0);
        assert_eq!(available_height(24), 21); // 24 * 90 / 100
        assert_eq!(available_height(50), 45);
        assert_eq!(available_height(100), 90);
    }

    #[test]
    fn test_max_visible_items_scales_with_height() {
        // available -> cap. The list claims half of skim's area minus chrome,
        // floored at MIN_VISIBLE_ITEMS(3). The floor binds until available/2
        // exceeds chrome by 3 (available >= 14); above that the cap grows
        // linearly with the terminal instead of the former fixed 12.
        let cases = [
            (0, 3),
            (8, 3),    // 4 - 4 = 0 -> floor
            (12, 3),   // 6 - 4 = 2 -> floor
            (14, 3),   // 7 - 4 = 3 -> floor boundary
            (16, 4),   // 8 - 4 = 4 -> floor releases
            (24, 8),   // 12 - 4
            (28, 10),  // 14 - 4
            (36, 14),  // 18 - 4
            (72, 32),  // 36 - 4
            (100, 46), // 50 - 4
            (216, 104),
        ];
        for (available, expected) in cases {
            assert_eq!(
                max_visible_items(available),
                expected,
                "max_visible_items({available})"
            );
        }
    }

    #[test]
    fn test_down_preview_dimensions_scenarios() {
        // (term_height, num_items) -> preview lines, for the balanced 50/50
        // split. Width is irrelevant to the Down height, so pass a fixed 80.
        // Mirrors the design's scenario grid: visible rows scale with height
        // (h=80/n=30 -> 30 rows, h=120/n=100 -> 50 rows) while the preview
        // keeps ~half and never collapses to its floor on a roomy terminal.
        let cases = [
            (20, 3, 11),
            (20, 12, 9),
            (20, 30, 9),
            (20, 100, 9),
            (24, 3, 14),
            (24, 12, 11),
            (24, 30, 11),
            (24, 100, 11),
            (40, 3, 29),
            (40, 12, 20),
            (40, 30, 18),
            (40, 100, 18),
            (50, 3, 38),
            (50, 12, 29),
            (50, 30, 23),
            (50, 100, 23),
            (80, 3, 65),
            (80, 12, 56),
            (80, 30, 38),
            (80, 100, 36),
            (120, 3, 101),
            (120, 12, 92),
            (120, 30, 74),
            (120, 100, 54),
        ];
        for (term_height, num_items, expected_preview) in cases {
            let (width, preview) = PreviewLayout::Down.dimensions_for(80, term_height, num_items);
            assert_eq!(width, 80, "Down width is the full terminal width");
            assert_eq!(
                preview, expected_preview,
                "Down preview height for {term_height}h x {num_items} items"
            );
            // The list region is whatever skim has left after the preview.
            // It must hold the chrome plus exactly the visible rows we sized
            // for, never more than there are items, and never overflow skim.
            let available = term_height * SKIM_HEIGHT_PERCENT / 100;
            assert!(preview <= available, "preview must fit skim's area");
            let list_region = available - preview;
            let visible = list_region.saturating_sub(LIST_CHROME_LINES);
            assert!(visible <= num_items, "never show more rows than items");
        }
    }

    #[test]
    fn test_down_preview_no_phantom_rows_when_empty() {
        // With no worktrees yet, the list reserves only its chrome — the
        // MIN_VISIBLE_ITEMS floor must not invent rows that don't exist.
        let term_height = 50;
        let available = term_height * SKIM_HEIGHT_PERCENT / 100;
        let (_, preview) = PreviewLayout::Down.dimensions_for(80, term_height, 0);
        assert_eq!(available - preview, LIST_CHROME_LINES);
    }

    #[test]
    fn test_down_preview_dimensions_never_panic_on_tiny_terminals() {
        // Saturating math must hold for degenerate terminals and absurd item
        // counts: the preview band always fits within skim's area.
        for term_height in [0, 1, 2, 3, 8, 10, 12, 14] {
            for num_items in [0, 1, 1000] {
                let available = term_height * SKIM_HEIGHT_PERCENT / 100;
                let (_, preview) = PreviewLayout::Down.dimensions_for(80, term_height, num_items);
                assert!(
                    preview <= available,
                    "preview {preview} exceeds available {available} at {term_height}h"
                );
            }
        }
    }

    #[test]
    fn test_down_preview_saturates_at_cap() {
        // The picker's num_items_estimate short-circuit stops counting at
        // max_visible_items(available). This is sound only if any item count
        // at or above the cap yields the same preview spec as the cap itself.
        for term_height in [24, 40, 80, 120] {
            let available = term_height * SKIM_HEIGHT_PERCENT / 100;
            let cap = max_visible_items(available);
            let at_cap = PreviewLayout::Down.dimensions_for(80, term_height, cap);
            assert_eq!(
                at_cap,
                PreviewLayout::Down.dimensions_for(80, term_height, cap + 1),
                "cap+1 must match cap at {term_height}h"
            );
            assert_eq!(
                at_cap,
                PreviewLayout::Down.dimensions_for(80, term_height, 100_000),
                "a huge count must match cap at {term_height}h"
            );
        }
    }

    #[test]
    fn test_preview_mode_rotation() {
        // tab cycles forward through all seven tabs and wraps; shift-tab is the
        // exact inverse. These drive the tab / shift-tab keybindings.
        let forward: Vec<PreviewMode> =
            std::iter::successors(Some(PreviewMode::WorkingTree), |m| {
                let next = m.next();
                (next != PreviewMode::WorkingTree).then_some(next)
            })
            .collect();
        assert_eq!(
            forward,
            vec![
                PreviewMode::WorkingTree,
                PreviewMode::Log,
                PreviewMode::BranchDiff,
                PreviewMode::UpstreamDiff,
                PreviewMode::Summary,
                PreviewMode::Pr,
                PreviewMode::Comments,
            ]
        );
        assert_eq!(PreviewMode::Comments.next(), PreviewMode::WorkingTree);
        assert_eq!(PreviewMode::WorkingTree.prev(), PreviewMode::Comments);
        for mode in forward {
            assert_eq!(mode.next().prev(), mode);
        }
    }

    #[test]
    fn test_layout_for_dimensions_wide_terminal() {
        // Standard wide terminal: landscape aspect ratio → Right
        assert_eq!(
            PreviewLayout::for_dimensions(120.0, 40.0),
            PreviewLayout::Right
        );
    }

    #[test]
    fn test_layout_for_dimensions_portrait_terminal() {
        // Tall terminal: portrait aspect ratio → Down
        // 180/136 * 0.5 = 0.66 < 1.0
        assert_eq!(
            PreviewLayout::for_dimensions(180.0, 136.0),
            PreviewLayout::Down
        );
    }

    #[test]
    fn test_layout_for_dimensions_narrow_terminal_forces_down() {
        // Narrow terminal (e.g. phone): landscape ratio but too few columns for
        // side-by-side layout — branch names would be hidden in half-width list.
        // 60/24 * 0.5 = 1.25 (landscape ratio), but 60 cols < 80 minimum → Down
        assert_eq!(
            PreviewLayout::for_dimensions(60.0, 24.0),
            PreviewLayout::Down
        );

        // Even narrower
        assert_eq!(
            PreviewLayout::for_dimensions(40.0, 20.0),
            PreviewLayout::Down
        );
    }

    #[test]
    fn test_layout_for_dimensions_boundary() {
        // Exactly at the minimum → Right (if aspect ratio allows)
        assert_eq!(
            PreviewLayout::for_dimensions(80.0, 24.0),
            PreviewLayout::Right
        );

        // Just below → Down
        assert_eq!(
            PreviewLayout::for_dimensions(79.0, 24.0),
            PreviewLayout::Down
        );
    }
}
