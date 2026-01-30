//! Status symbol types for rendering worktree and branch state.
//!
//! These types handle the visual representation of various states in the
//! status column of `wt list` output.

use super::state::{Divergence, MainState, OperationState, WorktreeState};

/// Tracks which status symbol positions are actually used across all items
/// and the maximum width needed for each position.
///
/// This allows the Status column to:
/// 1. Only allocate space for positions that have data
/// 2. Pad each position to a consistent width for vertical alignment
///
/// Stores maximum character width for each of 7 positions (including user marker).
/// A width of 0 means the position is unused.
#[derive(Debug, Clone, Copy, Default)]
pub struct PositionMask {
    /// Maximum width for each position: [0, 1, 2, 3, 4, 5, 6]
    /// 0 = position unused, >0 = max characters needed
    widths: [usize; 7],
}

impl PositionMask {
    // Render order indices (0-6) - symbols appear in this order left-to-right
    // Working tree split into 3 fixed positions for vertical alignment
    pub(crate) const STAGED: usize = 0; // + (staged changes)
    pub(crate) const MODIFIED: usize = 1; // ! (modified files)
    pub(crate) const UNTRACKED: usize = 2; // ? (untracked files)
    pub(crate) const WORKTREE_STATE: usize = 3; // Worktree: ✘⤴⤵/⚑⊟⊞
    pub(crate) const MAIN_STATE: usize = 4; // Main relationship: ^✗_⊂↕↑↓
    pub(crate) const UPSTREAM_DIVERGENCE: usize = 5; // Remote: |⇅⇡⇣
    pub(crate) const USER_MARKER: usize = 6;

    /// Full mask with all positions enabled (for JSON output and progressive rendering)
    /// Allocates realistic widths based on common symbol sizes to ensure proper grid alignment
    pub const FULL: Self = Self {
        widths: [
            1, // STAGED: + (1 char)
            1, // MODIFIED: ! (1 char)
            1, // UNTRACKED: ? (1 char)
            1, // WORKTREE_STATE: ✘⤴⤵/⚑⊟⊞ (1 char, priority: conflicts > rebase > merge > branch_worktree_mismatch > prunable > locked > branch)
            1, // MAIN_STATE: ^✗_–⊂↕↑↓ (1 char, priority: is_main > would_conflict > empty > same_commit > integrated > diverged > ahead > behind)
            1, // UPSTREAM_DIVERGENCE: |⇡⇣⇅ (1 char)
            2, // USER_MARKER: single emoji or two chars (allocate 2)
        ],
    };

    /// Get the allocated width for a position
    pub(crate) fn width(&self, pos: usize) -> usize {
        self.widths[pos]
    }
}

/// Working tree changes as structured booleans
///
/// This is the canonical internal representation. Display strings are derived from this.
#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub struct WorkingTreeStatus {
    pub staged: bool,
    pub modified: bool,
    pub untracked: bool,
    pub renamed: bool,
    pub deleted: bool,
}

impl WorkingTreeStatus {
    /// Create from git status parsing results
    pub fn new(
        staged: bool,
        modified: bool,
        untracked: bool,
        renamed: bool,
        deleted: bool,
    ) -> Self {
        Self {
            staged,
            modified,
            untracked,
            renamed,
            deleted,
        }
    }

    /// Returns true if any changes are present
    pub fn is_dirty(&self) -> bool {
        self.staged || self.modified || self.untracked || self.renamed || self.deleted
    }

    /// Format as display string for JSON serialization and raw output (e.g., "+!?").
    ///
    /// For styled terminal rendering, use `StatusSymbols::styled_symbols()` instead.
    pub fn to_symbols(self) -> String {
        let mut s = String::with_capacity(5);
        if self.staged {
            s.push('+');
        }
        if self.modified {
            s.push('!');
        }
        if self.untracked {
            s.push('?');
        }
        if self.renamed {
            s.push('»');
        }
        if self.deleted {
            s.push('✘');
        }
        s
    }
}

/// Structured status symbols for aligned rendering
///
/// Symbols are categorized to enable vertical alignment in table output.
/// Display order (left to right):
/// - Working tree: +, !, ? (staged, modified, untracked - NOT mutually exclusive)
/// - Worktree state: ✘, ⤴, ⤵, /, ⚑, ⊟, ⊞ (operations + location)
/// - Main state: ^, ✗, _, ⊂, ↕, ↑, ↓ (relationship to default branch - single-stroke vertical arrows)
/// - Upstream divergence: |, ⇅, ⇡, ⇣ (relationship to remote - vertical arrows)
/// - User marker: custom labels, emoji
///
/// ## Mutual Exclusivity
///
/// **Worktree state (operations take priority over location):**
/// Priority: ✘ > ⤴ > ⤵ > ⚑ > ⊟ > ⊞ > /
/// - ✘: Actual conflicts (must resolve)
/// - ⤴: Rebase in progress
/// - ⤵: Merge in progress
/// - ⚑: Branch-worktree mismatch
/// - ⊟: Prunable (directory missing)
/// - ⊞: Locked worktree
/// - /: Branch without worktree
///
/// **Main state (single position with priority):**
/// Priority: ^ > ✗ > _ > – > ⊂ > ↕ > ↑ > ↓
/// - ^: This IS the main worktree
/// - ✗: Would conflict if merged to default branch
/// - _: Same commit as default branch, clean working tree (removable)
/// - –: Same commit as default branch, uncommitted changes (NOT removable)
/// - ⊂: Content integrated (removable)
/// - ↕: Diverged from default branch
/// - ↑: Ahead of default branch
/// - ↓: Behind default branch
///
/// **Upstream divergence (enforced by type system):**
/// - |: In sync with remote
/// - ⇅: Diverged from remote
/// - ⇡: Ahead of remote
/// - ⇣: Behind remote
///
/// **NOT mutually exclusive (can co-occur):**
/// - Working tree symbols (+!?): Can have multiple types of changes
#[derive(Debug, Clone, Default)]
pub struct StatusSymbols {
    /// Main branch relationship state (single position, horizontal arrows)
    /// Priority: IsMain (^) > WouldConflict (✗) > Empty (_) > SameCommit (–) > Integrated (⊂) > Diverged (↕) > Ahead (↑) > Behind (↓)
    pub(crate) main_state: MainState,

    /// Worktree operation and location state (single position)
    /// Operations (✘⤴⤵) take priority over location states (/⚑⊟⊞)
    pub(crate) operation_state: OperationState,

    /// Worktree location state: / for branches, ⚑⊟⊞ for worktrees
    pub(crate) worktree_state: WorktreeState,

    /// Remote/upstream divergence state (mutually exclusive)
    pub(crate) upstream_divergence: Divergence,

    /// Working tree changes (NOT mutually exclusive, can have multiple)
    pub(crate) working_tree: WorkingTreeStatus,

    /// User-defined status annotation (custom labels, e.g., 💬, 🤖)
    pub(crate) user_marker: Option<String>,
}

impl StatusSymbols {
    /// Render symbols with selective alignment based on position mask
    ///
    /// Only includes positions present in the mask. This ensures vertical
    /// scannability - each symbol type appears at the same column position
    /// across all rows, while minimizing wasted space.
    ///
    /// See [`StatusSymbols`] struct doc for symbol categories.
    pub fn render_with_mask(&self, mask: &PositionMask) -> String {
        use worktrunk::styling::StyledLine;

        let mut result = String::with_capacity(64);

        if self.is_empty() {
            return result;
        }

        // Grid-based rendering: each position gets a fixed width for vertical alignment.
        // CRITICAL: Always use PositionMask::FULL for consistent spacing between progressive and final rendering.
        // The mask provides the maximum width needed for each position across all rows.
        // Accept wider Status column with whitespace as tradeoff for perfect alignment.
        for (pos, styled_content, has_data) in self.styled_symbols() {
            let allocated_width = mask.width(pos);

            if has_data {
                // Use StyledLine to handle width calculation (strips ANSI codes automatically)
                let mut segment = StyledLine::new();
                segment.push_raw(styled_content);
                segment.pad_to(allocated_width);
                result.push_str(&segment.render());
            } else {
                // Fill empty position with spaces for alignment
                for _ in 0..allocated_width {
                    result.push(' ');
                }
            }
        }

        result
    }

    /// Check if symbols are empty
    pub fn is_empty(&self) -> bool {
        self.main_state == MainState::None
            && self.operation_state == OperationState::None
            && self.worktree_state == WorktreeState::None
            && self.upstream_divergence == Divergence::None
            && !self.working_tree.is_dirty()
            && self.user_marker.is_none()
    }

    /// Render status symbols in compact form for statusline (no grid alignment).
    ///
    /// Uses the same styled symbols as `render_with_mask()`, just without padding.
    pub fn format_compact(&self) -> String {
        self.styled_symbols()
            .into_iter()
            .filter_map(|(_, styled, has_data)| has_data.then_some(styled))
            .collect()
    }

    /// Build styled symbols array with position indices.
    ///
    /// Returns: `[(position_mask, styled_string, has_data); 7]`
    ///
    /// Order: working_tree (+!?) → main_state → upstream_divergence → worktree_state → user_marker
    ///
    /// Styling follows semantic meaning:
    /// - Cyan: Working tree changes (activity indicator)
    /// - Red: Conflicts (blocking problems)
    /// - Yellow: Git operations, would_conflict, locked/prunable (states needing attention)
    /// - Dimmed: Main state symbols, divergence arrows, branch indicator (informational)
    pub(crate) fn styled_symbols(&self) -> [(usize, String, bool); 7] {
        use color_print::cformat;

        // Working tree symbols split into 3 fixed columns for vertical alignment
        let style_working = |has: bool, sym: char| -> (String, bool) {
            if has {
                (cformat!("<cyan>{sym}</>"), true)
            } else {
                (String::new(), false)
            }
        };
        let (staged_str, has_staged) = style_working(self.working_tree.staged, '+');
        let (modified_str, has_modified) = style_working(self.working_tree.modified, '!');
        let (untracked_str, has_untracked) = style_working(self.working_tree.untracked, '?');

        // Main state (merged column: ^✗_⊂↕↑↓)
        let (main_state_str, has_main_state) = self
            .main_state
            .styled()
            .map_or((String::new(), false), |s| (s, true));

        // Upstream divergence (|⇅⇡⇣)
        let (upstream_divergence_str, has_upstream_divergence) = self
            .upstream_divergence
            .styled()
            .map_or((String::new(), false), |s| (s, true));

        // Worktree state: operations (✘⤴⤵) take priority over location (/⚑⊟⊞)
        let (worktree_str, has_worktree) = if self.operation_state != OperationState::None {
            // Operation state takes priority
            (self.operation_state.styled().unwrap_or_default(), true)
        } else {
            // Fall back to location state
            match self.worktree_state {
                WorktreeState::None => (String::new(), false),
                // Branch indicator (/) is informational (dimmed)
                WorktreeState::Branch => (cformat!("<dim>{}</>", self.worktree_state), true),
                // Branch-worktree mismatch (⚑) is a stronger warning (red)
                WorktreeState::BranchWorktreeMismatch => {
                    (cformat!("<red>{}</>", self.worktree_state), true)
                }
                // Other worktree attrs (⊟⊞) are warnings (yellow)
                _ => (cformat!("<yellow>{}</>", self.worktree_state), true),
            }
        };

        let user_marker_str = self.user_marker.as_deref().unwrap_or("").to_string();

        // CRITICAL: Display order must match position indices for correct rendering.
        // Order: Working tree (0-2) → Worktree (3) → Main (4) → Remote (5) → User (6)
        [
            (PositionMask::STAGED, staged_str, has_staged),
            (PositionMask::MODIFIED, modified_str, has_modified),
            (PositionMask::UNTRACKED, untracked_str, has_untracked),
            (PositionMask::WORKTREE_STATE, worktree_str, has_worktree),
            (PositionMask::MAIN_STATE, main_state_str, has_main_state),
            (
                PositionMask::UPSTREAM_DIVERGENCE,
                upstream_divergence_str,
                has_upstream_divergence,
            ),
            (
                PositionMask::USER_MARKER,
                user_marker_str,
                self.user_marker.is_some(),
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_working_tree_status_is_dirty() {
        // Empty status is not dirty
        assert!(!WorkingTreeStatus::default().is_dirty());

        // Each flag individually makes it dirty
        assert!(WorkingTreeStatus::new(true, false, false, false, false).is_dirty());
        assert!(WorkingTreeStatus::new(false, true, false, false, false).is_dirty());
        assert!(WorkingTreeStatus::new(false, false, true, false, false).is_dirty());
        assert!(WorkingTreeStatus::new(false, false, false, true, false).is_dirty());
        assert!(WorkingTreeStatus::new(false, false, false, false, true).is_dirty());

        // Multiple flags
        assert!(WorkingTreeStatus::new(true, true, true, true, true).is_dirty());
    }

    #[test]
    fn test_working_tree_status_to_symbols() {
        // Empty
        assert_eq!(WorkingTreeStatus::default().to_symbols(), "");

        // Individual symbols
        assert_eq!(
            WorkingTreeStatus::new(true, false, false, false, false).to_symbols(),
            "+"
        );
        assert_eq!(
            WorkingTreeStatus::new(false, true, false, false, false).to_symbols(),
            "!"
        );
        assert_eq!(
            WorkingTreeStatus::new(false, false, true, false, false).to_symbols(),
            "?"
        );
        assert_eq!(
            WorkingTreeStatus::new(false, false, false, true, false).to_symbols(),
            "»"
        );
        assert_eq!(
            WorkingTreeStatus::new(false, false, false, false, true).to_symbols(),
            "✘"
        );

        // Combined symbols (order: staged, modified, untracked, renamed, deleted)
        assert_eq!(
            WorkingTreeStatus::new(true, true, false, false, false).to_symbols(),
            "+!"
        );
        assert_eq!(
            WorkingTreeStatus::new(true, true, true, false, false).to_symbols(),
            "+!?"
        );
        assert_eq!(
            WorkingTreeStatus::new(true, true, true, true, true).to_symbols(),
            "+!?»✘"
        );
    }

    #[test]
    fn test_status_symbols_is_empty() {
        let symbols = StatusSymbols::default();
        assert!(symbols.is_empty());

        let symbols = StatusSymbols {
            main_state: MainState::Ahead,
            ..Default::default()
        };
        assert!(!symbols.is_empty());

        let symbols = StatusSymbols {
            operation_state: OperationState::Rebase,
            ..Default::default()
        };
        assert!(!symbols.is_empty());

        let symbols = StatusSymbols {
            worktree_state: WorktreeState::Locked,
            ..Default::default()
        };
        assert!(!symbols.is_empty());

        let symbols = StatusSymbols {
            upstream_divergence: Divergence::Ahead,
            ..Default::default()
        };
        assert!(!symbols.is_empty());

        let symbols = StatusSymbols {
            working_tree: WorkingTreeStatus::new(true, false, false, false, false),
            ..Default::default()
        };
        assert!(!symbols.is_empty());

        let symbols = StatusSymbols {
            user_marker: Some("🔥".to_string()),
            ..Default::default()
        };
        assert!(!symbols.is_empty());
    }

    #[test]
    fn test_status_symbols_format_compact() {
        // Empty symbols
        let symbols = StatusSymbols::default();
        assert_eq!(symbols.format_compact(), "");

        // Single symbol
        let symbols = StatusSymbols {
            main_state: MainState::Ahead,
            ..Default::default()
        };
        let compact = symbols.format_compact();
        assert!(compact.contains("↑"));

        // Multiple symbols
        let symbols = StatusSymbols {
            working_tree: WorkingTreeStatus::new(true, true, false, false, false),
            main_state: MainState::Ahead,
            ..Default::default()
        };
        let compact = symbols.format_compact();
        assert!(compact.contains("+"));
        assert!(compact.contains("!"));
        assert!(compact.contains("↑"));
    }

    #[test]
    fn test_status_symbols_render_with_mask() {
        let symbols = StatusSymbols {
            main_state: MainState::Ahead,
            ..Default::default()
        };
        let rendered = symbols.render_with_mask(&PositionMask::FULL);
        // Should have fixed-width output with spacing
        assert!(!rendered.is_empty());
        assert!(rendered.contains("↑"));
    }

    #[test]
    fn test_position_mask_width() {
        let mask = PositionMask::FULL;
        // Check expected widths for each position
        assert_eq!(mask.width(PositionMask::STAGED), 1);
        assert_eq!(mask.width(PositionMask::MODIFIED), 1);
        assert_eq!(mask.width(PositionMask::UNTRACKED), 1);
        assert_eq!(mask.width(PositionMask::WORKTREE_STATE), 1);
        assert_eq!(mask.width(PositionMask::MAIN_STATE), 1);
        assert_eq!(mask.width(PositionMask::UPSTREAM_DIVERGENCE), 1);
        assert_eq!(mask.width(PositionMask::USER_MARKER), 2);
    }

    #[test]
    fn test_position_mask_default() {
        let mask = PositionMask::default();
        // Default has all widths at 0
        for i in 0..7 {
            assert_eq!(mask.width(i), 0);
        }
    }
}
