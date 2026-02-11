//! Progressive table renderer using crossterm for direct terminal control.
//!
//! This module provides a progressive table renderer that updates rows in-place
//! as data arrives, using crossterm for cursor control. Unlike indicatif's
//! MultiProgress, this renderer:
//!
//! - Uses our own escape-aware width calculations (StyledLine, truncate_visible)
//! - Supports OSC-8 hyperlinks correctly
//! - Has predictable cursor behavior based on our rendering logic

use crossterm::{
    ExecutableCommand,
    cursor::{MoveToColumn, MoveUp},
    terminal::{Clear, ClearType},
};
use std::io::{IsTerminal, Write, stdout};

use crate::display::truncate_visible;

/// Progressive table that updates rows in-place using crossterm cursor control.
///
/// The table structure is:
/// - Header row (column labels)
/// - N data rows (one per worktree/branch)
/// - Spacer (blank line)
/// - Footer (loading status / summary)
///
/// Data mutation (`update_row`, `update_footer`) is separate from rendering (`flush`).
/// Call `flush()` after updates to write changes to the terminal.
pub struct ProgressiveTable {
    /// Previously rendered content for each line (header + rows + spacer + footer)
    lines: Vec<String>,
    /// Maximum width for content (terminal width - safety margin)
    max_width: usize,
    /// Number of data rows visible in skeleton (not counting header, spacer, footer).
    /// May be less than `total_row_count` when terminal is too short.
    row_count: usize,
    /// Total number of data rows (including those not shown in skeleton)
    total_row_count: usize,
    /// Whether output is going to a TTY
    is_tty: bool,
    /// Lines that have been modified since last flush
    dirty: Vec<usize>,
    /// Whether the skeleton was printed (only true in TTY mode after render_skeleton)
    rendered: bool,
}

impl ProgressiveTable {
    /// Create a new progressive table with the given structure.
    ///
    /// Call `render_skeleton()` after construction to print the skeleton table.
    ///
    /// # Arguments
    /// * `header` - The header line content
    /// * `skeletons` - Initial content for each data row (skeleton with known data)
    /// * `initial_footer` - Initial footer message
    /// * `max_width` - Maximum content width (for truncation)
    pub fn new(
        header: String,
        skeletons: Vec<String>,
        initial_footer: String,
        max_width: usize,
    ) -> Self {
        // Only check terminal height when stdout is a TTY. terminal_size() falls
        // back to stderr/stdin, so it can return Some even for piped stdout.
        let term_height = if stdout().is_terminal() {
            terminal_size::terminal_size().map(|(_, h)| h.0 as usize)
        } else {
            None
        };
        Self::new_with_height(header, skeletons, initial_footer, max_width, term_height)
    }

    fn new_with_height(
        header: String,
        skeletons: Vec<String>,
        initial_footer: String,
        max_width: usize,
        terminal_height: Option<usize>,
    ) -> Self {
        let is_tty = stdout().is_terminal();
        let total_row_count = skeletons.len();

        // Limit visible rows to fit in terminal: header + rows + spacer + footer = rows + 3
        // Reserve one extra line for the cursor position after printing.
        // Only limit when we have height info — None means non-TTY or unknown.
        let visible_row_count = terminal_height
            .map(|h| total_row_count.min(h.saturating_sub(4)))
            .unwrap_or(total_row_count);

        // Build initial lines: header + visible rows + spacer + footer
        let mut lines = Vec::with_capacity(visible_row_count + 3);
        lines.push(truncate_visible(&header, max_width));

        for skeleton in skeletons.into_iter().take(visible_row_count) {
            lines.push(truncate_visible(&skeleton, max_width));
        }

        // Spacer (blank line)
        lines.push(String::new());

        // Footer
        lines.push(truncate_visible(&initial_footer, max_width));

        Self {
            lines,
            max_width,
            row_count: visible_row_count,
            total_row_count,
            is_tty,
            dirty: Vec::new(),
            rendered: false,
        }
    }

    /// Print the skeleton table to stdout (TTY only).
    ///
    /// Idempotent: calling multiple times has no effect after the first render.
    pub fn render_skeleton(&mut self) -> std::io::Result<()> {
        if self.is_tty && !self.rendered {
            self.print_all()?;
            self.rendered = true;
        }
        Ok(())
    }

    /// Print all lines to stdout.
    fn print_all(&self) -> std::io::Result<()> {
        let mut stdout = stdout();
        for line in &self.lines {
            writeln!(stdout, "{}", line)?;
        }
        stdout.flush()
    }

    /// Update a data row at the given index.
    ///
    /// This only updates the internal state. Call `flush()` to render changes.
    ///
    /// # Arguments
    /// * `row_idx` - Index of the data row (0-based, not counting header)
    /// * `content` - New content for the row
    ///
    /// # Returns
    /// `true` if the content changed, `false` if unchanged or out of bounds.
    pub fn update_row(&mut self, row_idx: usize, content: String) -> bool {
        if row_idx >= self.row_count {
            return false;
        }

        let truncated = truncate_visible(&content, self.max_width);

        // Line index: header (0) + row_idx
        let line_idx = row_idx + 1;

        // Skip if content hasn't changed
        if self.lines[line_idx] == truncated {
            return false;
        }

        self.lines[line_idx] = truncated;
        // Only mark dirty if we've rendered (otherwise nothing on screen to redraw)
        if self.rendered {
            self.dirty.push(line_idx);
        }
        true
    }

    /// Update the footer message.
    ///
    /// This only updates the internal state. Call `flush()` to render changes.
    ///
    /// # Returns
    /// `true` if the content changed, `false` if unchanged.
    pub fn update_footer(&mut self, content: String) -> bool {
        let truncated = truncate_visible(&content, self.max_width);

        // Footer is the last line
        let footer_idx = self.lines.len() - 1;

        // Skip if content hasn't changed
        if self.lines[footer_idx] == truncated {
            return false;
        }

        self.lines[footer_idx] = truncated;
        // Only mark dirty if we've rendered (otherwise nothing on screen to redraw)
        if self.rendered {
            self.dirty.push(footer_idx);
        }
        true
    }

    /// Flush pending changes to the terminal.
    ///
    /// Redraws all lines that have been modified since the last flush.
    /// No-op if nothing is dirty (which includes the case where we haven't rendered yet).
    pub fn flush(&mut self) -> std::io::Result<()> {
        if self.dirty.is_empty() {
            return Ok(());
        }

        // Defense-in-depth: dirty should only be non-empty if we're in TTY mode and rendered.
        // The update_* methods gate on `self.rendered`, which is only set when is_tty is true.
        debug_assert!(
            self.is_tty && self.rendered,
            "dirty list should only be non-empty after render_skeleton in TTY mode"
        );

        // Take ownership of dirty indices to avoid borrow conflict with redraw_line
        for line_idx in std::mem::take(&mut self.dirty) {
            self.redraw_line(line_idx)?;
        }

        Ok(())
    }

    /// Redraw a specific line by moving cursor up, clearing, and printing.
    fn redraw_line(&self, line_idx: usize) -> std::io::Result<()> {
        let mut stdout = stdout();

        // Calculate how many lines up from current position
        // Current position is after the footer (last line)
        let lines_up = self.lines.len() - line_idx;

        // Move cursor up to the target line
        if lines_up > 0 {
            stdout.execute(MoveUp(lines_up as u16))?;
        }

        // Move to column 0 and clear the line
        stdout.execute(MoveToColumn(0))?;
        stdout.execute(Clear(ClearType::CurrentLine))?;

        // Print the new content
        write!(stdout, "{}", self.lines[line_idx])?;

        // Move cursor back to the end (after footer)
        // We need to move down (lines_up) lines, but since we printed one line
        // without newline, we need to print newlines to get back
        for _ in 0..lines_up {
            writeln!(stdout)?;
        }

        stdout.flush()
    }

    /// Finalize the table with final row content and footer.
    ///
    /// For the normal case, updates rows in-place and redraws the footer.
    /// When overflowing (skeleton showed a subset of rows), erases the skeleton
    /// and prints the complete table — the output scrolls naturally, avoiding
    /// the `MoveUp`-into-scrollback problem.
    pub fn finalize(
        &mut self,
        final_rows: Vec<String>,
        final_footer: String,
    ) -> std::io::Result<()> {
        if self.row_count < self.total_row_count {
            // Overflow: erase skeleton, print complete table (scrolls naturally)
            debug_assert!(
                self.rendered,
                "overflow finalize should only be called after render_skeleton"
            );
            let mut stdout = stdout();
            stdout.execute(MoveUp(self.lines.len() as u16))?;
            stdout.execute(MoveToColumn(0))?;
            stdout.execute(Clear(ClearType::FromCursorDown))?;
            writeln!(stdout, "{}", self.lines[0])?; // header (unchanged)
            for row in &final_rows {
                writeln!(stdout, "{}", truncate_visible(row, self.max_width))?;
            }
            writeln!(stdout)?;
            writeln!(
                stdout,
                "{}",
                truncate_visible(&final_footer, self.max_width)
            )?;
            stdout.flush()
        } else {
            // Normal: update rows in-place + footer
            for (idx, row) in final_rows.into_iter().enumerate() {
                self.update_row(idx, row);
            }
            self.update_footer(final_footer);
            self.flush()
        }
    }

    /// Check if output is going to a TTY.
    pub fn is_tty(&self) -> bool {
        self.is_tty
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_and_updates_rows() {
        let header = "header".to_string();
        let skeletons = vec!["row0".to_string(), "row1".to_string()];
        let footer = "loading".to_string();

        let mut table =
            ProgressiveTable::new(header.clone(), skeletons.clone(), footer.clone(), 80);

        // header + 2 rows + spacer + footer
        assert_eq!(table.lines.len(), 5);
        assert_eq!(table.lines[0], header);
        assert_eq!(table.lines[1], skeletons[0]);
        assert_eq!(table.lines[2], skeletons[1]);
        assert!(table.lines[3].is_empty());
        assert_eq!(table.lines[4], footer);

        // No-op when index out of bounds (returns false)
        assert!(!table.update_row(5, "ignored".into()));

        // Update row content and verify it changed
        assert!(table.update_row(1, "row1-updated".into()));
        assert_eq!(table.lines[2], "row1-updated");

        // Updating with identical content returns false (no change)
        let before = table.lines[2].clone();
        assert!(!table.update_row(1, before.clone()));
        assert_eq!(table.lines[2], before);

        // Footer update
        assert!(table.update_footer("done".into()));
        assert_eq!(table.lines.last().unwrap(), "done");
    }

    #[test]
    fn test_truncation_applied() {
        let long_header = "this is a very long header that exceeds width".to_string();
        let skeletons = vec!["short".to_string()];
        let footer = "loading...".to_string();

        let table = ProgressiveTable::new(long_header.clone(), skeletons, footer, 20);

        // Header should be truncated (shorter than original)
        assert!(
            table.lines[0].len() < long_header.len(),
            "Header '{}' should be shorter than '{}'",
            table.lines[0],
            long_header
        );
    }

    #[test]
    fn test_update_footer_no_change() {
        let header = "header".to_string();
        let skeletons = vec!["row0".to_string()];
        let footer = "loading".to_string();

        let mut table = ProgressiveTable::new(header, skeletons, footer.clone(), 80);

        // First footer should match
        assert_eq!(table.lines.last().unwrap(), &footer);

        // Update with same content returns false (no change)
        assert!(!table.update_footer(footer.clone()));
        assert_eq!(table.lines.last().unwrap(), &footer);
    }

    #[test]
    fn test_is_tty_returns_value() {
        let table = ProgressiveTable::new(
            "header".to_string(),
            vec!["row".to_string()],
            "footer".to_string(),
            80,
        );

        // In test environment, stdout is typically not a TTY
        let _ = table.is_tty();
    }

    #[test]
    fn test_row_count_tracking() {
        let table = ProgressiveTable::new(
            "h".to_string(),
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
            "f".to_string(),
            80,
        );

        assert_eq!(table.row_count, 3);
        assert_eq!(table.total_row_count, 3);
        assert_eq!(table.row_count, table.total_row_count);
    }

    #[test]
    fn test_update_row_bounds_check() {
        let mut table = ProgressiveTable::new(
            "header".to_string(),
            vec!["row0".to_string(), "row1".to_string()],
            "footer".to_string(),
            80,
        );

        // Out-of-bounds update returns false
        assert!(!table.update_row(10, "should be ignored".to_string()));

        // Original rows should be unchanged
        assert_eq!(table.lines[1], "row0");
        assert_eq!(table.lines[2], "row1");
    }

    #[test]
    fn test_finalize_without_render() {
        let mut table = ProgressiveTable::new(
            "header".to_string(),
            vec!["row".to_string()],
            "loading...".to_string(),
            80,
        );

        // Without render_skeleton(), finalize updates footer but doesn't print
        table
            .finalize(vec!["row-final".to_string()], "Complete!".to_string())
            .unwrap();

        // Footer IS updated (the data changes), but no output since not rendered
        assert_eq!(table.lines.last().unwrap(), "Complete!");
        assert_eq!(table.lines[1], "row-final");
    }

    #[test]
    fn test_dirty_tracking_before_render() {
        let mut table = ProgressiveTable::new(
            "header".to_string(),
            vec!["row0".to_string(), "row1".to_string()],
            "footer".to_string(),
            80,
        );

        // Initially no dirty lines
        assert!(table.dirty.is_empty());

        // Before render_skeleton, updates modify data but don't mark dirty
        // (nothing on screen to redraw)
        assert!(table.update_row(0, "updated".into()));
        assert!(table.dirty.is_empty());
        assert_eq!(table.lines[1], "updated"); // Data IS updated

        assert!(table.update_footer("new footer".into()));
        assert!(table.dirty.is_empty());
        assert_eq!(table.lines.last().unwrap(), "new footer"); // Data IS updated

        // Flush is a no-op (nothing dirty)
        table.flush().unwrap();
        assert!(table.dirty.is_empty());
    }

    #[test]
    fn test_dirty_tracking_after_render() {
        let mut table = ProgressiveTable::new(
            "header".to_string(),
            vec!["row0".to_string(), "row1".to_string()],
            "footer".to_string(),
            80,
        );

        // Simulate TTY render state (in tests, is_tty is false so render_skeleton is a no-op,
        // but we can manually set both flags to test dirty tracking while maintaining invariant)
        table.is_tty = true;
        table.rendered = true;

        // After render, updates mark lines as dirty
        table.update_row(0, "updated".into());
        assert_eq!(table.dirty, vec![1]); // line_idx = row_idx + 1

        table.update_footer("new footer".into());
        assert_eq!(table.dirty, vec![1, 4]); // footer is last line
    }

    #[test]
    fn overflow_limits_visible_rows() {
        // 10 rows, terminal height 8 → visible = 8 - 4 = 4
        let skeletons: Vec<String> = (0..10).map(|i| format!("row{i}")).collect();
        let table = ProgressiveTable::new_with_height(
            "header".into(),
            skeletons,
            "loading".into(),
            80,
            Some(8),
        );

        assert_eq!(table.row_count, 4);
        assert_eq!(table.total_row_count, 10);
        assert!(table.row_count < table.total_row_count);
        // header + 4 visible rows + spacer + footer = 7 lines
        assert_eq!(table.lines.len(), 7);
        assert_eq!(table.lines[0], "header");
        assert_eq!(table.lines[1], "row0");
        assert_eq!(table.lines[4], "row3");
        assert_eq!(table.lines[5], ""); // spacer
        assert_eq!(table.lines[6], "loading");
    }

    #[test]
    fn no_overflow_when_fits() {
        // 3 rows, terminal height 20 → visible = 3 (fits easily)
        let skeletons = vec!["a".into(), "b".into(), "c".into()];
        let table = ProgressiveTable::new_with_height(
            "header".into(),
            skeletons,
            "loading".into(),
            80,
            Some(20),
        );

        assert_eq!(table.row_count, 3);
        assert_eq!(table.total_row_count, 3);
        assert_eq!(table.row_count, table.total_row_count);
    }

    #[test]
    fn overflow_boundary_exact_fit() {
        // 5 rows need height 5+4=9, terminal height 9 → fits exactly, no overflow
        let skeletons: Vec<String> = (0..5).map(|i| format!("row{i}")).collect();
        let table = ProgressiveTable::new_with_height(
            "header".into(),
            skeletons,
            "loading".into(),
            80,
            Some(9),
        );

        assert_eq!(table.row_count, 5);
        assert_eq!(table.row_count, table.total_row_count);
    }

    #[test]
    fn overflow_boundary_one_short() {
        // 5 rows need height 5+4=9, terminal height 8 → overflow, visible = 4
        let skeletons: Vec<String> = (0..5).map(|i| format!("row{i}")).collect();
        let table = ProgressiveTable::new_with_height(
            "header".into(),
            skeletons,
            "loading".into(),
            80,
            Some(8),
        );

        assert_eq!(table.row_count, 4);
        assert_eq!(table.total_row_count, 5);
        assert!(table.row_count < table.total_row_count);
    }

    #[test]
    fn overflow_hidden_rows_are_noop() {
        let skeletons: Vec<String> = (0..10).map(|i| format!("row{i}")).collect();
        let mut table = ProgressiveTable::new_with_height(
            "header".into(),
            skeletons,
            "loading".into(),
            80,
            Some(8),
        );

        // Can update visible rows (0..4)
        assert!(table.update_row(0, "updated0".into()));
        assert_eq!(table.lines[1], "updated0");

        // Hidden rows (4..10) are no-ops
        assert!(!table.update_row(4, "should-be-ignored".into()));
        assert!(!table.update_row(9, "should-be-ignored".into()));
    }

    #[test]
    fn overflow_very_small_terminal() {
        // Terminal too small for any rows: height 3 → visible = 0
        let skeletons: Vec<String> = (0..5).map(|i| format!("row{i}")).collect();
        let table = ProgressiveTable::new_with_height(
            "header".into(),
            skeletons,
            "loading".into(),
            80,
            Some(3),
        );

        assert_eq!(table.row_count, 0);
        assert_eq!(table.total_row_count, 5);
        assert!(table.row_count < table.total_row_count);
        // header + 0 rows + spacer + footer = 3 lines
        assert_eq!(table.lines.len(), 3);
    }

    #[test]
    fn no_height_info_shows_all_rows() {
        // No terminal height info → show all rows (non-TTY or unknown)
        let skeletons: Vec<String> = (0..10).map(|i| format!("row{i}")).collect();
        let table = ProgressiveTable::new_with_height(
            "header".into(),
            skeletons,
            "loading".into(),
            80,
            None,
        );

        assert_eq!(table.row_count, 10);
        assert_eq!(table.total_row_count, 10);
        assert_eq!(table.row_count, table.total_row_count);
    }
}
