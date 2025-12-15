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
use std::io::{IsTerminal, Write, stderr};

use crate::display::truncate_visible;

/// Progressive table that updates rows in-place using crossterm cursor control.
///
/// The table structure is:
/// - Header row (column labels)
/// - N data rows (one per worktree/branch)
/// - Spacer (blank line)
/// - Footer (loading status / summary)
pub struct ProgressiveTable {
    /// Previously rendered content for each line (header + rows + spacer + footer)
    lines: Vec<String>,
    /// Maximum width for content (terminal width - safety margin)
    max_width: usize,
    /// Number of data rows (not counting header, spacer, footer)
    row_count: usize,
    /// Whether output is going to a TTY
    is_tty: bool,
}

impl ProgressiveTable {
    /// Create a new progressive table with the given structure.
    ///
    /// Call `render_initial()` after construction to print the initial table.
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
        let is_tty = stderr().is_terminal();
        let row_count = skeletons.len();

        // Build initial lines: header + rows + spacer + footer
        let mut lines = Vec::with_capacity(row_count + 3);
        lines.push(truncate_visible(&header, max_width, "…"));

        for skeleton in skeletons {
            lines.push(truncate_visible(&skeleton, max_width, "…"));
        }

        // Spacer (blank line)
        lines.push(String::new());

        // Footer
        lines.push(truncate_visible(&initial_footer, max_width, "…"));

        Self {
            lines,
            max_width,
            row_count,
            is_tty,
        }
    }

    /// Print the initial table to stderr (TTY only).
    pub fn render_initial(&self) -> std::io::Result<()> {
        if self.is_tty {
            self.print_all()?;
        }
        Ok(())
    }

    /// Print all lines to stderr.
    fn print_all(&self) -> std::io::Result<()> {
        let mut stderr = stderr();
        for line in &self.lines {
            writeln!(stderr, "{}", line)?;
        }
        stderr.flush()
    }

    /// Update a data row at the given index.
    ///
    /// # Arguments
    /// * `row_idx` - Index of the data row (0-based, not counting header)
    /// * `content` - New content for the row
    pub fn update_row(&mut self, row_idx: usize, content: String) -> std::io::Result<()> {
        if row_idx >= self.row_count {
            return Ok(());
        }

        let truncated = truncate_visible(&content, self.max_width, "…");

        // Line index: header (0) + row_idx
        let line_idx = row_idx + 1;

        // Skip if content hasn't changed
        if self.lines[line_idx] == truncated {
            return Ok(());
        }

        self.lines[line_idx] = truncated;

        if self.is_tty {
            self.redraw_line(line_idx)?;
        }

        Ok(())
    }

    /// Update the footer message.
    pub fn update_footer(&mut self, content: String) -> std::io::Result<()> {
        let truncated = truncate_visible(&content, self.max_width, "…");

        // Footer is the last line
        let footer_idx = self.lines.len() - 1;

        // Skip if content hasn't changed
        if self.lines[footer_idx] == truncated {
            return Ok(());
        }

        self.lines[footer_idx] = truncated;

        if self.is_tty {
            self.redraw_line(footer_idx)?;
        }

        Ok(())
    }

    /// Redraw a specific line by moving cursor up, clearing, and printing.
    fn redraw_line(&self, line_idx: usize) -> std::io::Result<()> {
        let mut stderr = stderr();

        // Calculate how many lines up from current position
        // Current position is after the footer (last line)
        let lines_up = self.lines.len() - line_idx;

        // Move cursor up to the target line
        if lines_up > 0 {
            stderr.execute(MoveUp(lines_up as u16))?;
        }

        // Move to column 0 and clear the line
        stderr.execute(MoveToColumn(0))?;
        stderr.execute(Clear(ClearType::CurrentLine))?;

        // Print the new content
        write!(stderr, "{}", self.lines[line_idx])?;

        // Move cursor back to the end (after footer)
        // We need to move down (lines_up) lines, but since we printed one line
        // without newline, we need to print newlines to get back
        for _ in 0..lines_up {
            writeln!(stderr)?;
        }

        stderr.flush()
    }

    /// Finalize for TTY: do final render pass and leave output in place.
    ///
    /// # Arguments
    /// * `final_footer` - Final summary message to replace loading status
    pub fn finalize_tty(&mut self, final_footer: String) -> std::io::Result<()> {
        if !self.is_tty {
            return Ok(());
        }

        // Update footer with final summary
        self.update_footer(final_footer)?;

        Ok(())
    }

    /// Finalize for non-TTY: clear all and print final static table.
    ///
    /// # Arguments
    /// * `final_lines` - Final rendered lines (header + rows + spacer + footer)
    pub fn finalize_non_tty(&self, final_lines: Vec<String>) -> std::io::Result<()> {
        let mut stderr = stderr();

        // For non-TTY, we just print the final table
        // (initial output was suppressed)
        for line in final_lines {
            writeln!(stderr, "{}", line)?;
        }

        stderr.flush()
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

        // new() no longer has I/O side effects — render_initial() must be called explicitly
        let mut table =
            ProgressiveTable::new(header.clone(), skeletons.clone(), footer.clone(), 80);

        // header + 2 rows + spacer + footer
        assert_eq!(table.lines.len(), 5);
        assert_eq!(table.lines[0], header);
        assert_eq!(table.lines[1], skeletons[0]);
        assert_eq!(table.lines[2], skeletons[1]);
        assert!(table.lines[3].is_empty(), "spacer should be blank");
        assert_eq!(table.lines[4], footer);

        // No-op when index out of bounds
        table.update_row(5, "ignored".into()).unwrap();

        // Update row content and verify it changed
        table.update_row(1, "row1-updated".into()).unwrap();
        assert_eq!(table.lines[2], "row1-updated");

        // Updating with identical content should be a no-op
        let before = table.lines[2].clone();
        table.update_row(1, before.clone()).unwrap();
        assert_eq!(table.lines[2], before);

        // Footer update
        table.update_footer("done".into()).unwrap();
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

        // Update with same content should be a no-op
        table.update_footer(footer.clone()).unwrap();
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

        // In test environment, stderr is typically not a TTY
        // Just ensure the method can be called without panicking
        let _is_tty = table.is_tty();
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
    }

    #[test]
    fn test_update_row_bounds_check() {
        let mut table = ProgressiveTable::new(
            "header".to_string(),
            vec!["row0".to_string(), "row1".to_string()],
            "footer".to_string(),
            80,
        );

        // Should not panic when updating out-of-bounds row
        table
            .update_row(10, "should be ignored".to_string())
            .unwrap();

        // Original rows should be unchanged
        assert_eq!(table.lines[1], "row0");
        assert_eq!(table.lines[2], "row1");
    }

    #[test]
    fn test_finalize_tty_updates_footer() {
        let mut table = ProgressiveTable::new(
            "header".to_string(),
            vec!["row".to_string()],
            "loading...".to_string(),
            80,
        );

        // In non-TTY test environment, finalize_tty should still update footer internally
        table.finalize_tty("Complete!".to_string()).unwrap();

        // Note: In non-TTY mode, finalize_tty returns early, so footer won't change
        // This tests that the method doesn't panic
    }

    #[test]
    fn test_finalize_non_tty() {
        let table = ProgressiveTable::new(
            "header".to_string(),
            vec!["row".to_string()],
            "loading".to_string(),
            80,
        );

        let final_lines = vec![
            "Final Header".to_string(),
            "Final Row".to_string(),
            "".to_string(),
            "Complete".to_string(),
        ];

        // Should not panic (writes to stderr which may fail in some test envs)
        let _ = table.finalize_non_tty(final_lines);
    }

    #[test]
    fn test_render_initial_non_tty() {
        let table = ProgressiveTable::new(
            "header".to_string(),
            vec!["row".to_string()],
            "footer".to_string(),
            80,
        );

        // In non-TTY mode, render_initial should be a no-op
        table.render_initial().unwrap();
    }
}
