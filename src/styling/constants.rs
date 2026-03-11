//! Style constants and symbols for terminal output
//!
//! # Styling with color-print
//!
//! Use `cformat!` with HTML-like tags for all user-facing messages:
//!
//! ```
//! use color_print::cformat;
//!
//! // Simple styling
//! let msg = cformat!("<green>Success message</>");
//!
//! // Nested styles - bold inherits green
//! let branch = "feature";
//! let msg = cformat!("<green>Removed branch <bold>{branch}</> successfully</>");
//!
//! // Semantic mapping:
//! // - Errors: <red>...</>
//! // - Warnings: <yellow>...</>
//! // - Hints: <dim>...</>
//! // - Progress: <cyan>...</>
//! // - Success: <green>...</>
//! // - Inline references (in hints): <underline>...</>
//! ```
//!
//! # anstyle constants
//!
//! A few `Style` constants remain for programmatic use with `StyledLine` and
//! table rendering where computed styles are needed at runtime.

use anstyle::{AnsiColor, Color, Style};
use color_print::cstr;

// ============================================================================
// Programmatic Style Constants (for StyledLine, tables, computed styles)
// ============================================================================

/// Addition style for diffs (green) - used in table rendering
pub const ADDITION: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Green)));

/// Deletion style for diffs (red) - used in table rendering
pub const DELETION: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Red)));

/// Gutter style for quoted content (commands, config, error details)
///
/// We wanted the dimmest/most subtle background that works on both dark and light
/// terminals. BrightWhite was the best we could find among basic ANSI colors, but
/// we're open to better ideas. Options considered:
/// - Black/BrightBlack: too dark on light terminals
/// - Reverse video: just flips which terminal looks good
/// - 256-color grays: better but not universally supported
/// - No background: loses the visual separation we want
pub const GUTTER: Style = Style::new().bg_color(Some(Color::Ansi(AnsiColor::BrightWhite)));

/// Default width for help text rendering when terminal width is unknown.
/// Used in both the CLI binary and tests for consistent output in docs.
pub const DEFAULT_HELP_WIDTH: usize = 98;

// ============================================================================
// Message Symbols
// ============================================================================
//
// Single-width Unicode symbols for message prefixes with embedded colors.
// Using `cstr!` to create colored `&'static str` constants that work everywhere.

/// Progress symbol (cyan ◎)
pub const PROGRESS_SYMBOL: &str = cstr!("<cyan>◎</>");

/// Success symbol (green ✓)
pub const SUCCESS_SYMBOL: &str = cstr!("<green>✓</>");

/// Error symbol (red ✗)
pub const ERROR_SYMBOL: &str = cstr!("<red>✗</>");

/// Warning symbol (yellow ▲)
pub const WARNING_SYMBOL: &str = cstr!("<yellow>▲</>");

/// Hint symbol (dim ↳)
pub const HINT_SYMBOL: &str = cstr!("<dim>↳</>");

/// Info symbol (dim ○) - for neutral status
pub const INFO_SYMBOL: &str = cstr!("<dim>○</>");

/// Prompt symbol (cyan ❯) - for questions requiring user input
pub const PROMPT_SYMBOL: &str = cstr!("<cyan>❯</>");

// ============================================================================
// Formatted Message Type
// ============================================================================

use std::fmt;

/// A message that has already been formatted with emoji and styling.
///
/// This type provides compile-time prevention of double-formatting. Message
/// functions like `error_message()` take `impl AsRef<str>` and return
/// `FormattedMessage`. Since `FormattedMessage` does NOT implement `AsRef<str>`,
/// passing it to a message function is a compile error.
///
/// # Type Safety
///
/// ```compile_fail
/// use worktrunk::styling::{error_message, FormattedMessage};
///
/// let msg = error_message("first error");
/// // This won't compile - FormattedMessage doesn't implement AsRef<str>
/// let double = error_message(msg);
/// ```
///
/// # Usage
///
/// ```
/// use worktrunk::styling::error_message;
///
/// let msg = error_message("Something went wrong");
/// println!("{}", msg);  // Uses Display
/// ```
#[derive(Debug, Clone)]
pub struct FormattedMessage(String);

impl FormattedMessage {
    /// Create a formatted message from a pre-formatted string.
    ///
    /// Use this when implementing `Into<FormattedMessage>` for error types
    /// that format themselves (like `GitError`).
    pub fn new(content: String) -> Self {
        Self(content)
    }

    /// Get the inner string for output.
    pub fn into_inner(self) -> String {
        self.0
    }

    /// Append raw content after the formatted message.
    ///
    /// Used when content must appear outside the message's color span,
    /// e.g., integration symbols that need their native dim styling.
    pub fn append(mut self, suffix: &str) -> Self {
        self.0.push_str(suffix);
        self
    }

    /// Borrow the inner string for inspection (e.g., in tests).
    ///
    /// Note: This does NOT implement `AsRef<str>` to prevent accidentally
    /// passing a `FormattedMessage` to message functions like `error_message()`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FormattedMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<FormattedMessage> for String {
    fn from(msg: FormattedMessage) -> String {
        msg.0
    }
}

// ============================================================================
// Message Formatting Functions
// ============================================================================
//
// These functions provide the canonical formatting for each message type.
// Used by both the output system (output::error, etc.) and Display impls
// (GitError, WorktrunkError) to ensure consistent styling.
//
// All functions take `impl AsRef<str>` (which FormattedMessage does NOT
// implement) and return `FormattedMessage`, preventing double-formatting.

use color_print::cformat;

/// Format an error message with symbol and red styling
///
/// Content can include inner styling like `<bold>`:
/// ```
/// use color_print::cformat;
/// use worktrunk::styling::error_message;
///
/// let name = "feature";
/// println!("{}", error_message(cformat!("Branch <bold>{name}</> not found")));
/// ```
pub fn error_message(content: impl AsRef<str>) -> FormattedMessage {
    FormattedMessage(cformat!("{ERROR_SYMBOL} <red>{}</>", content.as_ref()))
}

/// Format a hint message with symbol and dim styling
pub fn hint_message(content: impl AsRef<str>) -> FormattedMessage {
    FormattedMessage(cformat!("{HINT_SYMBOL} <dim>{}</>", content.as_ref()))
}

/// Format a warning message with symbol and yellow styling
pub fn warning_message(content: impl AsRef<str>) -> FormattedMessage {
    FormattedMessage(cformat!("{WARNING_SYMBOL} <yellow>{}</>", content.as_ref()))
}

/// Format a success message with symbol and green styling
pub fn success_message(content: impl AsRef<str>) -> FormattedMessage {
    FormattedMessage(cformat!("{SUCCESS_SYMBOL} <green>{}</>", content.as_ref()))
}

/// Format a progress message with symbol and cyan styling
pub fn progress_message(content: impl AsRef<str>) -> FormattedMessage {
    FormattedMessage(cformat!("{PROGRESS_SYMBOL} <cyan>{}</>", content.as_ref()))
}

/// Format an info message with symbol (no color on text - neutral status)
pub fn info_message(content: impl AsRef<str>) -> FormattedMessage {
    FormattedMessage(format!("{INFO_SYMBOL} {}", content.as_ref()))
}

/// Format a prompt message with symbol and cyan styling
pub fn prompt_message(content: impl AsRef<str>) -> FormattedMessage {
    FormattedMessage(cformat!("{PROMPT_SYMBOL} <cyan>{}</>", content.as_ref()))
}

/// Format a section heading (cyan uppercase text, no emoji)
///
/// Used for organizing output into distinct sections. Headings can have
/// optional suffix info (e.g., path, location).
///
/// ```
/// use worktrunk::styling::format_heading;
///
/// // Plain heading
/// let h = format_heading("BINARIES", None);
/// // => "BINARIES"
///
/// // Heading with suffix
/// let h = format_heading("USER CONFIG", Some("@ ~/.config/wt.toml"));
/// // => "USER CONFIG @ ~/.config/wt.toml"
/// ```
pub fn format_heading(title: &str, suffix: Option<&str>) -> String {
    match suffix {
        Some(s) => cformat!("<cyan>{}</> {}", title, s),
        None => cformat!("<cyan>{}</>", title),
    }
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;

    use super::*;

    #[test]
    fn test_symbol_constants() {
        assert_snapshot!(PROGRESS_SYMBOL, @"[36m◎[39m");
        assert_snapshot!(SUCCESS_SYMBOL, @"[32m✓[39m");
        assert_snapshot!(ERROR_SYMBOL, @"[31m✗[39m");
        assert_snapshot!(WARNING_SYMBOL, @"[33m▲[39m");
        assert_snapshot!(HINT_SYMBOL, @"[2m↳[22m");
        assert_snapshot!(INFO_SYMBOL, @"[2m○[22m");
        assert_snapshot!(PROMPT_SYMBOL, @"[36m❯[39m");
    }

    #[test]
    fn test_message_formatting() {
        assert_snapshot!(error_message("Something went wrong").as_str(), @"[31m✗[39m [31mSomething went wrong[39m");
        assert_snapshot!(hint_message("Try running --help").as_str(), @"[2m↳[22m [2mTry running --help[22m");
        assert_snapshot!(warning_message("Deprecated option").as_str(), @"[33m▲[39m [33mDeprecated option[39m");
        assert_snapshot!(success_message("Operation completed").as_str(), @"[32m✓[39m [32mOperation completed[39m");
        assert_snapshot!(progress_message("Loading data...").as_str(), @"[36m◎[39m [36mLoading data...[39m");
        assert_snapshot!(info_message("5 items found").as_str(), @"[2m○[22m 5 items found");
        assert_snapshot!(prompt_message("Continue?").as_str(), @"[36m❯[39m [36mContinue?[39m");
    }

    #[test]
    fn test_error_message_with_inner_styling() {
        let name = "feature";
        let msg = error_message(cformat!("Branch <bold>{name}</> not found"));
        assert_snapshot!(msg.as_str(), @"[31m✗[39m [31mBranch [1mfeature[22m not found[39m");
    }

    #[test]
    fn test_format_heading() {
        assert_snapshot!(format_heading("BINARIES", None), @"[36mBINARIES[39m");
        assert_snapshot!(format_heading("USER CONFIG", Some("~/.config/wt.toml")), @"[36mUSER CONFIG[39m ~/.config/wt.toml");
        assert_snapshot!(format_heading("", None), @"[36m[39m");
    }

    #[test]
    fn test_formatted_message_into_inner() {
        let msg = success_message("Done");
        assert_snapshot!(msg.into_inner(), @"[32m✓[39m [32mDone[39m");
    }
}
