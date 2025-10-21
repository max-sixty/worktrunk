//! Consolidated styling module for terminal output.
//!
//! This module uses the anstyle ecosystem:
//! - anstream for auto-detecting color support
//! - anstyle for composable styling
//! - Semantic style constants for domain-specific use

use anstyle::{AnsiColor, Color, Style};
use unicode_width::UnicodeWidthStr;

// ============================================================================
// Re-exports from anstream (auto-detecting output)
// ============================================================================

/// Auto-detecting println that respects NO_COLOR, CLICOLOR_FORCE, and terminal capabilities
pub use anstream::println;

/// Auto-detecting eprintln that respects NO_COLOR, CLICOLOR_FORCE, and terminal capabilities
pub use anstream::eprintln;

/// Auto-detecting print that respects NO_COLOR, CLICOLOR_FORCE, and terminal capabilities
pub use anstream::print;

/// Auto-detecting eprint that respects NO_COLOR, CLICOLOR_FORCE, and terminal capabilities
pub use anstream::eprint;

// ============================================================================
// Re-exports from anstyle (for composition)
// ============================================================================

/// Re-export Style for users who want to compose custom styles
pub use anstyle::Style as AnstyleStyle;

// ============================================================================
// Semantic Style Constants
// ============================================================================

/// Error style (red) - use as `{ERROR}text{ERROR:#}`
pub const ERROR: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Red)));

// ============================================================================
// Message Emojis
// ============================================================================

/// Error emoji - use with ERROR style: `eprintln!("{ERROR_EMOJI} {ERROR}message{ERROR:#}");`
pub const ERROR_EMOJI: &str = "❌";

/// Warning emoji - use with WARNING style: `eprintln!("{WARNING_EMOJI} {WARNING}message{WARNING:#}");`
pub const WARNING_EMOJI: &str = "🟡";

/// Hint emoji - use with HINT style: `println!("{HINT_EMOJI} {HINT}message{HINT:#}");`
pub const HINT_EMOJI: &str = "💡";

/// Warning style (yellow) - use as `{WARNING}text{WARNING:#}`
pub const WARNING: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow)));

/// Hint style (dimmed) - use as `{HINT}text{HINT:#}`
pub const HINT: Style = Style::new().dimmed();

/// Current worktree style (magenta + bold)
pub const CURRENT: Style = Style::new()
    .bold()
    .fg_color(Some(Color::Ansi(AnsiColor::Magenta)));

/// Addition style for diffs (green)
pub const ADDITION: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Green)));

/// Deletion style for diffs (red)
pub const DELETION: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Red)));

// ============================================================================
// Styled Output Types
// ============================================================================

/// A piece of text with an optional style
#[derive(Clone, Debug)]
pub struct StyledString {
    pub text: String,
    pub style: Option<Style>,
}

impl StyledString {
    pub fn new(text: impl Into<String>, style: Option<Style>) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }

    pub fn raw(text: impl Into<String>) -> Self {
        Self::new(text, None)
    }

    pub fn styled(text: impl Into<String>, style: Style) -> Self {
        Self::new(text, Some(style))
    }

    /// Returns the visual width (unicode-aware, no ANSI codes)
    pub fn width(&self) -> usize {
        self.text.width()
    }

    /// Renders to a string with ANSI escape codes
    pub fn render(&self) -> String {
        if let Some(style) = &self.style {
            format!("{}{}{}", style.render(), self.text, style.render_reset())
        } else {
            self.text.clone()
        }
    }
}

/// A line composed of multiple styled strings
#[derive(Clone, Debug, Default)]
pub struct StyledLine {
    pub segments: Vec<StyledString>,
}

impl StyledLine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a raw (unstyled) segment
    pub fn push_raw(&mut self, text: impl Into<String>) {
        self.segments.push(StyledString::raw(text));
    }

    /// Add a styled segment
    pub fn push_styled(&mut self, text: impl Into<String>, style: Style) {
        self.segments.push(StyledString::styled(text, style));
    }

    /// Add a segment (StyledString)
    pub fn push(&mut self, segment: StyledString) {
        self.segments.push(segment);
    }

    /// Pad with spaces to reach a specific width
    pub fn pad_to(&mut self, target_width: usize) {
        let current_width = self.width();
        if current_width < target_width {
            self.push_raw(" ".repeat(target_width - current_width));
        }
    }

    /// Returns the total visual width
    pub fn width(&self) -> usize {
        self.segments.iter().map(|s| s.width()).sum()
    }

    /// Renders the entire line with ANSI escape codes
    pub fn render(&self) -> String {
        self.segments.iter().map(|s| s.render()).collect()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // StyledString tests
    #[test]
    fn test_styled_string_width() {
        // ASCII strings
        let s = StyledString::raw("hello");
        assert_eq!(s.width(), 5);

        // Unicode arrows
        let s = StyledString::raw("↑3 ↓2");
        assert_eq!(
            s.width(),
            5,
            "↑3 ↓2 should have width 5, not {}",
            s.text.len()
        );

        // Mixed Unicode
        let s = StyledString::raw("日本語");
        assert_eq!(s.width(), 6); // CJK characters are typically width 2

        // Emoji
        let s = StyledString::raw("🎉");
        assert_eq!(s.width(), 2); // Emoji are typically width 2
    }

    // StyledLine tests
    #[test]
    fn test_styled_line_width() {
        let mut line = StyledLine::new();
        line.push_raw("Branch");
        line.push_raw("  ");
        line.push_raw("↑3 ↓2");

        // "Branch" (6) + "  " (2) + "↑3 ↓2" (5) = 13
        assert_eq!(line.width(), 13, "Line width should be 13");
    }

    #[test]
    fn test_styled_line_padding() {
        let mut line = StyledLine::new();
        line.push_raw("test");
        assert_eq!(line.width(), 4);

        line.pad_to(10);
        assert_eq!(line.width(), 10, "After padding to 10, width should be 10");

        // Padding when already at target should not change width
        line.pad_to(10);
        assert_eq!(line.width(), 10, "Padding again should not change width");
    }

    #[test]
    fn test_sparse_column_padding() {
        // Build simplified lines to test sparse column padding
        let mut line1 = StyledLine::new();
        line1.push_raw(format!("{:8}", "branch-a"));
        line1.push_raw("  ");
        // Has ahead/behind
        line1.push_raw(format!("{:5}", "↑3 ↓2"));
        line1.push_raw("  ");

        let mut line2 = StyledLine::new();
        line2.push_raw(format!("{:8}", "branch-b"));
        line2.push_raw("  ");
        // No ahead/behind, should pad with spaces
        line2.push_raw(" ".repeat(5));
        line2.push_raw("  ");

        // Both lines should have same width up to this point
        assert_eq!(
            line1.width(),
            line2.width(),
            "Rows with and without sparse column data should have same width"
        );
    }
}
