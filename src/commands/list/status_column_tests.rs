//! Unit tests for Status column git symbol rendering
//!
//! These tests verify the grid-based rendering of git status symbols in `StatusSymbols.render_with_mask()`.
//!
//! ## Grid-Based Rendering Model
//!
//! - Each symbol type has a fixed position (0a, 0b, 0c, 0d, 1, 2, 3)
//! - Only positions used by at least one row are included (position mask)
//! - Rendering creates a grid: first position in mask = column 0
//! - Each position maps to exactly one column
//! - **Multiple symbols from the same position** appear together in that column (e.g., "?!+" all at position 3)
//! - Symbols fill their position's column, empty positions get spaces
//! - Example: mask [0b, 3] creates 2-column grid:
//!   - Row with ≡ at 0b: "≡ " (≡ at col 0, space at col 1)
//!   - Row with ! at 3:  " !" (space at col 0, ! at col 1)
//!   - Row with ≡!+ at 0b and 3: "≡!+" (≡ at col 0, !+ at col 1)
//!
//! ## User Status Alignment
//!
//! User status rendering happens in the rendering layer (`render.rs`), not in `StatusSymbols`.
//! User status aligns at column position `max_git_symbols_width` for both worktrees and branches.
//! Integration tests verify user status behavior (see `with_user_status.snap`).
//!
//! Each test specifies the exact expected output to make the target behavior explicit.

#[cfg(test)]
mod status_column_rendering_tests {
    /// Test 1: Single symbol at position 0b (branch state)
    //      Row 1: ≡ (synced with remote)
    //      Expected: "≡"
    #[test]
    fn test_single_symbol_position_0b() {
        use super::super::model::{BranchState, StatusSymbols};

        // Symbols: [≡]
        // Max git width: 1
        // User status: None
        // Expected: "≡" (no padding, no user status)
        let symbols = StatusSymbols {
            branch_state: BranchState::MatchesMain,
            ..Default::default()
        };

        let mask = symbols.position_mask();
        let result = symbols.render_with_mask(&mask);

        assert_eq!(result, "≡");
    }

    /// Test 2: Single symbol at position 3 (working tree)
    //      Row 1: ! (uncommitted changes)
    //      Expected: "!"
    #[test]
    fn test_single_symbol_position_3() {
        use super::super::model::StatusSymbols;

        // Symbols: [!]
        // Max git width: 1
        // User status: None
        // Expected: "!" (no padding, no user status)
        let symbols = StatusSymbols {
            working_tree: "!".to_string(),
            ..Default::default()
        };

        let mask = symbols.position_mask();
        let result = symbols.render_with_mask(&mask);

        assert_eq!(result, "!");
    }

    /// Test 3: Two symbols at different positions create alignment grid
    /// Mask [0b, 3] creates 2-column grid:
    ///   - Column 0 = position 0b
    ///   - Column 1 = position 3
    //      Row 1: ≡ (position 0b)
    //      Row 2: ! (position 3)
    //      Expected:
    //      Row 1: "≡ " (≡ in col 0, space in col 1)
    //      Row 2: " !" (space in col 0, ! in col 1)
    #[test]
    fn test_two_different_positions_align() {
        use super::super::model::{BranchState, StatusSymbols};

        // Position mask: [0b, 3] → 2-column grid
        // Row 1: symbol at position 0b (col 0)
        // Row 2: symbol at position 3 (col 1)

        // Create mask from row that has both positions
        let mask_builder = StatusSymbols {
            branch_state: BranchState::MatchesMain,
            working_tree: "!".to_string(),
            ..Default::default()
        };
        let mask = mask_builder.position_mask();

        // Row 1: only position 0b
        let row1 = StatusSymbols {
            branch_state: BranchState::MatchesMain,
            ..Default::default()
        };
        assert_eq!(row1.render_with_mask(&mask), "≡ ");

        // Row 2: only position 3
        let row2 = StatusSymbols {
            working_tree: "!".to_string(),
            ..Default::default()
        };
        assert_eq!(row2.render_with_mask(&mask), " !");
    }

    /// Test 4: Same position symbols - single column grid
    /// Mask [3] creates 1-column grid:
    ///   - Column 0 = position 3
    //      Row 1: ! (position 3)
    //      Row 2: ? (position 3)
    //      Expected:
    //      Row 1: "!" (col 0 filled with !)
    //      Row 2: "?" (col 0 filled with ?)
    #[test]
    fn test_same_position_symbols_no_padding() {
        use super::super::model::StatusSymbols;

        // Position mask: [3] → 1-column grid
        // Both rows have symbol at position 3 (col 0)

        // Mask includes only position 3
        let mask_builder = StatusSymbols {
            working_tree: "!".to_string(),
            ..Default::default()
        };
        let mask = mask_builder.position_mask();

        // Row 1: ! at position 3
        let row1 = StatusSymbols {
            working_tree: "!".to_string(),
            ..Default::default()
        };
        assert_eq!(row1.render_with_mask(&mask), "!");

        // Row 2: ? at position 3
        let row2 = StatusSymbols {
            working_tree: "?".to_string(),
            ..Default::default()
        };
        assert_eq!(row2.render_with_mask(&mask), "?");
    }

    /// Test 5: Multiple symbols in one row fill multiple columns
    /// Mask [0b, 3] creates 2-column grid:
    ///   - Column 0 = position 0b
    ///   - Column 1 = position 3
    //      Row 1: ≡? (both positions filled)
    //      Expected: "≡?" (col 0=≡, col 1=?)
    #[test]
    fn test_multiple_symbols_one_row() {
        use super::super::model::{BranchState, StatusSymbols};

        // Position mask: [0b, 3] → 2-column grid
        // Row 1: symbols at both positions
        // Expected: "≡?" (both columns filled, no spaces)
        let row = StatusSymbols {
            branch_state: BranchState::MatchesMain,
            working_tree: "?".to_string(),
            ..Default::default()
        };

        let mask = row.position_mask();
        assert_eq!(row.render_with_mask(&mask), "≡?");
    }

    /// Test 6: Grid with some columns empty
    /// Mask [0b, 3] creates 2-column grid:
    ///   - Column 0 = position 0b
    ///   - Column 1 = position 3
    //      Row 1: ≡ (only position 0b)
    //      Row 2: ≡! (both positions)
    //      Expected:
    //      Row 1: "≡ " (col 0=≡, col 1=space)
    //      Row 2: "≡!" (col 0=≡, col 1=!)
    #[test]
    fn test_multiple_symbols_with_position_gap() {
        use super::super::model::{BranchState, StatusSymbols};

        // Position mask: [0b, 3] → 2-column grid
        // Row 1: col 0 filled, col 1 empty
        // Row 2: both columns filled

        // Create mask from row with both positions
        let mask_builder = StatusSymbols {
            branch_state: BranchState::MatchesMain,
            working_tree: "!".to_string(),
            ..Default::default()
        };
        let mask = mask_builder.position_mask();

        // Row 1: only position 0b
        let row1 = StatusSymbols {
            branch_state: BranchState::MatchesMain,
            ..Default::default()
        };
        assert_eq!(row1.render_with_mask(&mask), "≡ ");

        // Row 2: both positions
        let row2 = StatusSymbols {
            branch_state: BranchState::MatchesMain,
            working_tree: "!".to_string(),
            ..Default::default()
        };
        assert_eq!(row2.render_with_mask(&mask), "≡!");
    }

    /// Test 12: Empty status (no symbols, no user status)
    //      Expected: ""
    #[test]
    fn test_empty_status() {
        use super::super::model::StatusSymbols;

        // Git symbols: None
        // User status: None
        // Expected: "" (empty string)
        let symbols = StatusSymbols::default();
        let mask = symbols.position_mask();

        assert_eq!(symbols.render_with_mask(&mask), "");
    }

    /// Test 14: Three positions create 3-column grid
    /// Mask [0b, 1, 3] creates 3-column grid:
    ///   - Column 0 = position 0b
    ///   - Column 1 = position 1
    ///   - Column 2 = position 3
    //      Row 1: ≡ (position 0b)
    //      Row 2: ↓ (position 1)
    //      Row 3: ! (position 3)
    //      Expected:
    //      Row 1: "≡  " (col 0=≡, col 1=space, col 2=space)
    //      Row 2: " ↓ " (col 0=space, col 1=↓, col 2=space)
    //      Row 3: "  !" (col 0=space, col 1=space, col 2=!)
    #[test]
    fn test_three_different_positions() {
        use super::super::model::{BranchState, MainDivergence, StatusSymbols};

        // Position mask: [0b, 1, 3] → 3-column grid
        // Create mask from all three positions
        let mask_builder = StatusSymbols {
            branch_state: BranchState::MatchesMain,
            main_divergence: MainDivergence::Behind,
            working_tree: "!".to_string(),
            ..Default::default()
        };
        let mask = mask_builder.position_mask();

        // Row 1: only position 0b
        let row1 = StatusSymbols {
            branch_state: BranchState::MatchesMain,
            ..Default::default()
        };
        assert_eq!(row1.render_with_mask(&mask), "≡  ");

        // Row 2: only position 1
        let row2 = StatusSymbols {
            main_divergence: MainDivergence::Behind,
            ..Default::default()
        };
        assert_eq!(row2.render_with_mask(&mask), " ↓ ");

        // Row 3: only position 3
        let row3 = StatusSymbols {
            working_tree: "!".to_string(),
            ..Default::default()
        };
        assert_eq!(row3.render_with_mask(&mask), "  !");
    }

    /// Test 15: Adjacent positions (0b and 0c)
    //      Row 1: ≡↻ (position 0b + 0c, adjacent)
    //      Expected: "≡↻"
    #[test]
    fn test_adjacent_positions_no_space() {
        use super::super::model::{BranchState, GitOperation, StatusSymbols};

        // Symbols: [≡↻]
        // Position mask: 0b + 0c (adjacent)
        // Expected: "≡↻" (no space between adjacent positions)
        let row = StatusSymbols {
            branch_state: BranchState::MatchesMain,
            git_operation: GitOperation::Rebase,
            ..Default::default()
        };

        let mask = row.position_mask();
        assert_eq!(row.render_with_mask(&mask), "≡↻");
    }

    /// Test 16: Non-adjacent positions with all filled
    //      Row 1: ≡!+ (position 0b + 3 + working tree continuation)
    //      Expected: "≡!+"
    #[test]
    fn test_all_positions_filled() {
        use super::super::model::{BranchState, StatusSymbols};

        // Symbols: [≡!+]
        // Position mask: 0b + 3 (+ continuation)
        // Expected: "≡!+" (no spaces, all positions filled)
        let row = StatusSymbols {
            branch_state: BranchState::MatchesMain,
            working_tree: "!+".to_string(), // Multiple symbols at position 3
            ..Default::default()
        };

        let mask = row.position_mask();
        assert_eq!(row.render_with_mask(&mask), "≡!+");
    }

    /// Test 17: Real-world complex case 1
    //      Row 1: ≡? + 🤖 (synced branch + working tree untracked + user status)
    //      Row 2: ! (uncommitted changes only)
    //      Row 3: ↓!+ (behind main + uncommitted + working tree added)
    //      Expected:
    //      Row 1: "≡?🤖"
    //      Row 2: " !"
    //      Row 3: "↓!+"
    #[test]
    fn test_real_world_complex_1() {
        use super::super::model::{BranchState, MainDivergence, StatusSymbols};

        // Position mask: 0b + 1 + 3 (3-column grid)
        // Row 1: 0b=≡, 3=?, user=🤖 (user status tested separately)
        // Row 2: 3=!
        // Row 3: 1=↓, 3=!+

        // Create mask from all positions
        let mask_builder = StatusSymbols {
            branch_state: BranchState::MatchesMain,
            main_divergence: MainDivergence::Behind,
            working_tree: "!+".to_string(),
            ..Default::default()
        };
        let mask = mask_builder.position_mask();

        // Row 1: position 0b and 3 (note: user status alignment tested separately)
        let row1 = StatusSymbols {
            branch_state: BranchState::MatchesMain,
            working_tree: "?".to_string(),
            ..Default::default()
        };
        assert_eq!(row1.render_with_mask(&mask), "≡ ?");

        // Row 2: only position 3
        let row2 = StatusSymbols {
            working_tree: "!".to_string(),
            ..Default::default()
        };
        assert_eq!(row2.render_with_mask(&mask), "  !");

        // Row 3: position 1 and 3
        let row3 = StatusSymbols {
            main_divergence: MainDivergence::Behind,
            working_tree: "!+".to_string(),
            ..Default::default()
        };
        assert_eq!(row3.render_with_mask(&mask), " ↓!+");
    }

    /// Test 18: THE FAILING TEST - 2-column grid with partial fills
    /// Mask [0b, 3] creates 2-column grid:
    ///   - Column 0 = position 0b
    ///   - Column 1 = position 3
    //      Row 1: ≡? (synced + untracked)
    //      Row 2: ! (uncommitted only)
    //      Expected:
    //      Row 1: "≡?" (col 0=≡, col 1=?)
    //      Row 2: " !" (col 0=space, col 1=!)
    #[test]
    fn test_real_world_extreme_diffs() {
        use super::super::model::{BranchState, StatusSymbols};

        // This is the actual failing test case from spacing_edge_cases
        // Position mask: [0b, 3] → 2-column grid
        // Row 1 (huge): symbols at positions 0b (≡) and 3 (?)
        // Row 2 (tiny): symbol at position 3 (!) only

        // Create mask from row with both positions
        let mask_builder = StatusSymbols {
            branch_state: BranchState::MatchesMain,
            working_tree: "?".to_string(),
            ..Default::default()
        };
        let mask = mask_builder.position_mask();

        // Row 1: both columns filled
        let row1 = StatusSymbols {
            branch_state: BranchState::MatchesMain,
            working_tree: "?".to_string(),
            ..Default::default()
        };
        assert_eq!(row1.render_with_mask(&mask), "≡?");

        // Row 2: only position 3 (col 0 empty→space, col 1 filled)
        let row2 = StatusSymbols {
            working_tree: "!".to_string(),
            ..Default::default()
        };
        assert_eq!(row2.render_with_mask(&mask), " !");
    }

    /// Test 20: Position mask creates minimal grid
    /// All positions available: 0a, 0b, 0c, 0d, 1, 2, 3 (7 total)
    /// Used positions: 0b, 3 (2 used)
    /// Mask [0b, 3] creates 2-column grid (NOT 7-column):
    ///   - Column 0 = position 0b
    ///   - Column 1 = position 3
    //      Row 1: ≡ (only position 0b used)
    //      Row 2: ! (only position 3 used)
    //      Expected:
    //      Row 1: "≡ " (2 chars: col 0=≡, col 1=space)
    //      Row 2: " !" (2 chars: col 0=space, col 1=!)
    //      NOT:
    //      Row 1: "≡      " (7 chars with spaces for all positions)
    #[test]
    fn test_position_mask_removes_unused_positions() {
        use super::super::model::{BranchState, StatusSymbols};

        // Position mask: [0b, 3] → 2-column grid
        // Only used positions create columns
        // Expected: 2-char width (NOT 7-char for all possible positions)

        // Create mask from positions 0b and 3 only
        let mask_builder = StatusSymbols {
            branch_state: BranchState::MatchesMain,
            working_tree: "!".to_string(),
            ..Default::default()
        };
        let mask = mask_builder.position_mask();

        // Row 1: only position 0b
        let row1 = StatusSymbols {
            branch_state: BranchState::MatchesMain,
            ..Default::default()
        };
        let result1 = row1.render_with_mask(&mask);
        assert_eq!(result1, "≡ ");
        assert_eq!(result1.chars().count(), 2); // 2 chars, not 7

        // Row 2: only position 3
        let row2 = StatusSymbols {
            working_tree: "!".to_string(),
            ..Default::default()
        };
        let result2 = row2.render_with_mask(&mask);
        assert_eq!(result2, " !");
        assert_eq!(result2.chars().count(), 2); // 2 chars, not 7
    }
}
