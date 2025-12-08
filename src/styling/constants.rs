//! Style constants and emojis for terminal output
//!
//! # Styling with color-print
//!
//! Use `cformat!` with HTML-like tags for all user-facing messages:
//!
//! ```rust,ignore
//! use color_print::cformat;
//!
//! // Simple styling
//! cformat!("<green>Success message</>")
//!
//! // Nested styles - bold inherits green
//! cformat!("<green>Removed branch <bold>{branch}</> successfully</>")
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
/// ```ignore
/// error_message(cformat!("Branch <bold>{name}</> not found"))
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
