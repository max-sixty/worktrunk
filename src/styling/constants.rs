//! Style constants and emojis for terminal output
//!
//! Provides semantic constants for consistent styling across the codebase.

use anstyle::{AnsiColor, Color, Style};

// ============================================================================
// Semantic Style Constants
// ============================================================================

/// Error style (red) - use as `{ERROR}text{ERROR:#}`
pub const ERROR: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Red)));

/// Error bold style (red + bold) - use as `{ERROR_BOLD}text{ERROR_BOLD:#}`
pub const ERROR_BOLD: Style = Style::new()
    .fg_color(Some(Color::Ansi(AnsiColor::Red)))
    .bold();

/// Warning style (yellow) - use as `{WARNING}text{WARNING:#}`
pub const WARNING: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow)));

/// Warning bold style (yellow + bold) - use as `{WARNING_BOLD}text{WARNING_BOLD:#}`
pub const WARNING_BOLD: Style = Style::new()
    .fg_color(Some(Color::Ansi(AnsiColor::Yellow)))
    .bold();

/// Hint style (dimmed) - use as `{HINT}text{HINT:#}`
pub const HINT: Style = Style::new().dimmed();

/// Hint bold style (dimmed + bold) - use as `{HINT_BOLD}text{HINT_BOLD:#}`
pub const HINT_BOLD: Style = Style::new().dimmed().bold();

/// Current worktree style (magenta + bold)
pub const CURRENT: Style = Style::new()
    .bold()
    .fg_color(Some(Color::Ansi(AnsiColor::Magenta)));

/// Addition style for diffs (green)
pub const ADDITION: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Green)));

/// Deletion style for diffs (red)
pub const DELETION: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Red)));

/// Cyan style - use as `{CYAN}text{CYAN:#}`
pub const CYAN: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan)));

/// Cyan bold style - use as `{CYAN_BOLD}text{CYAN_BOLD:#}`
pub const CYAN_BOLD: Style = Style::new()
    .fg_color(Some(Color::Ansi(AnsiColor::Cyan)))
    .bold();

/// Green style - use as `{GREEN}text{GREEN:#}`
pub const GREEN: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Green)));

/// Green bold style - use as `{GREEN_BOLD}text{GREEN_BOLD:#}`
pub const GREEN_BOLD: Style = Style::new()
    .fg_color(Some(Color::Ansi(AnsiColor::Green)))
    .bold();

/// Gray style for status arrows (divergence indicators) - use as `{GRAY}text{GRAY:#}`
pub const GRAY: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::BrightBlack)));

// ============================================================================
// Message Emojis
// ============================================================================

/// Progress emoji - use with cyan style: `println!("{PROGRESS_EMOJI} {cyan}message{cyan:#}");`
pub const PROGRESS_EMOJI: &str = "🔄";

/// Success emoji - use with GREEN style: `println!("{SUCCESS_EMOJI} {GREEN}message{GREEN:#}");`
pub const SUCCESS_EMOJI: &str = "✅";

/// Error emoji - use with ERROR style: `println!("{ERROR_EMOJI} {ERROR}message{ERROR:#}");`
pub const ERROR_EMOJI: &str = "❌";

/// Warning emoji - use with WARNING style: `println!("{WARNING_EMOJI} {WARNING}message{WARNING:#}");`
pub const WARNING_EMOJI: &str = "🟡";

/// Hint emoji - use with HINT style: `println!("{HINT_EMOJI} {HINT}message{HINT:#}");`
pub const HINT_EMOJI: &str = "💡";

/// Info emoji - use for neutral status (primary status NOT dimmed, metadata may be dimmed)
/// Primary status: `output::info("All commands already approved")?;`
/// Metadata: `println!("{INFO_EMOJI} {dim}Showing 5 worktrees...{dim:#}");`
pub const INFO_EMOJI: &str = "⚪";
