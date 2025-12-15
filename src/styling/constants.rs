//! Style constants and emojis for terminal output
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
//! // - Secondary: <bright-black>...</>
//! ```
//!
//! # anstyle constants
//!
//! A few `Style` constants remain for programmatic use with `StyledLine` and
//! table rendering where computed styles are needed at runtime.

use anstyle::{AnsiColor, Color, Style};

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

// ============================================================================
// Message Emojis
// ============================================================================

/// Progress emoji: `cformat!("{PROGRESS_EMOJI} <cyan>message</>")`
pub const PROGRESS_EMOJI: &str = "🔄";

/// Success emoji: `cformat!("{SUCCESS_EMOJI} <green>message</>")`
pub const SUCCESS_EMOJI: &str = "✅";

/// Error emoji: `cformat!("{ERROR_EMOJI} <red>message</>")`
pub const ERROR_EMOJI: &str = "❌";

/// Warning emoji: `cformat!("{WARNING_EMOJI} <yellow>message</>")`
pub const WARNING_EMOJI: &str = "🟡";

/// Hint emoji: `cformat!("{HINT_EMOJI} <dim>message</>")`
pub const HINT_EMOJI: &str = "💡";

/// Info emoji - use for neutral status (primary status NOT dimmed, metadata may be dimmed)
/// Primary status: `output::info("All commands already approved")?;`
/// Metadata: `cformat!("{INFO_EMOJI} <dim>Showing 5 worktrees...</>")`
pub const INFO_EMOJI: &str = "⚪";

/// Prompt emoji - use for questions requiring user input
/// `eprint!("{PROMPT_EMOJI} Proceed? [y/N] ")`
pub const PROMPT_EMOJI: &str = "❓";

// ============================================================================
// Message Formatting Functions
// ============================================================================
//
// These functions provide the canonical formatting for each message type.
// Used by both the output system (output::error, etc.) and Display impls
// (GitError, WorktrunkError) to ensure consistent styling.

use color_print::cformat;

/// Format an error message with emoji and red styling
///
/// Content can include inner styling like `<bold>`:
/// ```
/// use color_print::cformat;
/// use worktrunk::styling::error_message;
///
/// let name = "feature";
/// println!("{}", error_message(cformat!("Branch <bold>{name}</> not found")));
/// ```
pub fn error_message(content: impl AsRef<str>) -> String {
    cformat!("{ERROR_EMOJI} <red>{}</>", content.as_ref())
}

/// Format a hint message with emoji and dim styling
pub fn hint_message(content: impl AsRef<str>) -> String {
    cformat!("{HINT_EMOJI} <dim>{}</>", content.as_ref())
}

/// Format a warning message with emoji and yellow styling
pub fn warning_message(content: impl AsRef<str>) -> String {
    cformat!("{WARNING_EMOJI} <yellow>{}</>", content.as_ref())
}

/// Format a success message with emoji and green styling
pub fn success_message(content: impl AsRef<str>) -> String {
    cformat!("{SUCCESS_EMOJI} <green>{}</>", content.as_ref())
}

/// Format a progress message with emoji and cyan styling
pub fn progress_message(content: impl AsRef<str>) -> String {
    cformat!("{PROGRESS_EMOJI} <cyan>{}</>", content.as_ref())
}

/// Format an info message with emoji (no color - neutral status)
pub fn info_message(content: impl AsRef<str>) -> String {
    cformat!("{INFO_EMOJI} {}", content.as_ref())
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
/// let h = format_heading("USER CONFIG", Some("~/.config/wt.toml"));
/// // => "USER CONFIG  ~/.config/wt.toml"
/// ```
pub fn format_heading(title: &str, suffix: Option<&str>) -> String {
    match suffix {
        Some(s) => cformat!("<cyan>{}</>  {}", title, s),
        None => cformat!("<cyan>{}</>", title),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================================
    // Style Constants Tests
    // ============================================================================

    #[test]
    fn test_addition_style() {
        // ADDITION should be green foreground
        let rendered = ADDITION.render().to_string();
        // Green is ANSI 32
        assert!(rendered.contains("32"));
    }

    #[test]
    fn test_deletion_style() {
        // DELETION should be red foreground
        let rendered = DELETION.render().to_string();
        // Red is ANSI 31
        assert!(rendered.contains("31"));
    }

    #[test]
    fn test_gutter_style() {
        // GUTTER should have bright white background
        let rendered = GUTTER.render().to_string();
        // BrightWhite background is ANSI 107
        assert!(rendered.contains("107"));
    }

    // ============================================================================
    // Emoji Constants Tests
    // ============================================================================

    #[test]
    fn test_emoji_constants() {
        assert_eq!(PROGRESS_EMOJI, "🔄");
        assert_eq!(SUCCESS_EMOJI, "✅");
        assert_eq!(ERROR_EMOJI, "❌");
        assert_eq!(WARNING_EMOJI, "🟡");
        assert_eq!(HINT_EMOJI, "💡");
        assert_eq!(INFO_EMOJI, "⚪");
        assert_eq!(PROMPT_EMOJI, "❓");
    }

    // ============================================================================
    // Message Formatting Functions Tests
    // ============================================================================

    #[test]
    fn test_error_message() {
        let msg = error_message("Something went wrong");
        assert!(msg.contains("❌"));
        assert!(msg.contains("Something went wrong"));
    }

    #[test]
    fn test_error_message_with_inner_styling() {
        let name = "feature";
        let msg = error_message(cformat!("Branch <bold>{name}</> not found"));
        assert!(msg.contains("❌"));
        assert!(msg.contains("Branch"));
        assert!(msg.contains("feature"));
    }

    #[test]
    fn test_hint_message() {
        let msg = hint_message("Try running --help");
        assert!(msg.contains("💡"));
        assert!(msg.contains("Try running --help"));
    }

    #[test]
    fn test_warning_message() {
        let msg = warning_message("Deprecated option");
        assert!(msg.contains("🟡"));
        assert!(msg.contains("Deprecated option"));
    }

    #[test]
    fn test_success_message() {
        let msg = success_message("Operation completed");
        assert!(msg.contains("✅"));
        assert!(msg.contains("Operation completed"));
    }

    #[test]
    fn test_progress_message() {
        let msg = progress_message("Loading data...");
        assert!(msg.contains("🔄"));
        assert!(msg.contains("Loading data..."));
    }

    #[test]
    fn test_info_message() {
        let msg = info_message("5 items found");
        assert!(msg.contains("⚪"));
        assert!(msg.contains("5 items found"));
    }

    // ============================================================================
    // format_heading Tests
    // ============================================================================

    #[test]
    fn test_format_heading_without_suffix() {
        let heading = format_heading("BINARIES", None);
        assert!(heading.contains("BINARIES"));
        // Should NOT contain extra spacing for suffix
        assert!(!heading.ends_with("  "));
    }

    #[test]
    fn test_format_heading_with_suffix() {
        let heading = format_heading("USER CONFIG", Some("~/.config/wt.toml"));
        assert!(heading.contains("USER CONFIG"));
        assert!(heading.contains("~/.config/wt.toml"));
        // Should have double-space separator
        assert!(heading.contains("  "));
    }

    #[test]
    fn test_format_heading_empty_title() {
        let heading = format_heading("", None);
        // Empty string, still formatted
        assert!(heading.is_empty() || heading.contains('\u{1b}'));
    }
}
