//! Shared output handler trait with default implementations
//!
//! This trait extracts common patterns between Interactive and Directive output modes.
//! The fundamental operation is `write_message_line` - implementations control where
//! messages go (stdout for interactive, stderr for directive).
//!
//! # Semantic Colors
//!
//! Output functions automatically wrap content in semantic colors:
//! - success → green
//! - progress → cyan
//! - hint → dimmed
//! - warning → yellow
//! - error → red
//! - info → no color (neutral status)
//!
//! Callers provide content with optional inner styling (like `<bold>`) using `cformat!`.
//! The trait adds the outer semantic color, so callers don't repeat `<green>...</>` etc.

use color_print::cformat;
use std::io::{self, Write};
use std::path::Path;
use worktrunk::styling::{
    ERROR_EMOJI, HINT_EMOJI, INFO_EMOJI, PROGRESS_EMOJI, SUCCESS_EMOJI, WARNING_EMOJI,
};

/// Core output handler trait
///
/// Implementations provide their message stream via `write_message_line`
/// and override only the methods that differ between modes.
pub trait OutputHandler {
    /// Write a single logical message line to the primary user stream
    fn write_message_line(&mut self, line: &str) -> io::Result<()>;

    /// Emit a success message (automatically wrapped in green)
    fn success(&mut self, message: String) -> io::Result<()> {
        self.write_message_line(&cformat!("{SUCCESS_EMOJI} <green>{message}</>"))
    }

    /// Emit a progress message (automatically wrapped in cyan)
    fn progress(&mut self, message: String) -> io::Result<()> {
        self.write_message_line(&cformat!("{PROGRESS_EMOJI} <cyan>{message}</>"))
    }

    /// Emit a hint message (automatically wrapped in dim styling)
    fn hint(&mut self, message: String) -> io::Result<()> {
        self.write_message_line(&cformat!("{HINT_EMOJI} <dim>{message}</>"))
    }

    /// Emit an info message (no color - neutral status)
    fn info(&mut self, message: String) -> io::Result<()> {
        self.write_message_line(&cformat!("{INFO_EMOJI} {message}"))
    }

    /// Emit a warning message (automatically wrapped in yellow)
    fn warning(&mut self, message: String) -> io::Result<()> {
        self.write_message_line(&cformat!("{WARNING_EMOJI} <yellow>{message}</>"))
    }

    /// Emit an error message (automatically wrapped in red)
    fn error(&mut self, message: String) -> io::Result<()> {
        self.write_message_line(&cformat!("{ERROR_EMOJI} <red>{message}</>"))
    }

    /// Print a message (written as-is)
    fn print(&mut self, message: String) -> io::Result<()> {
        self.write_message_line(&message)
    }

    /// Emit gutter-formatted content (no emoji)
    ///
    /// Gutter content is pre-formatted with its own newlines, so we write it raw
    /// without adding additional newlines.
    fn gutter(&mut self, content: String) -> io::Result<()>;

    /// Emit a blank line for visual separation
    fn blank(&mut self) -> io::Result<()> {
        self.write_message_line("")
    }

    /// Emit structured data output without emoji decoration
    ///
    /// Used for JSON and other pipeable data. In interactive mode, writes to stdout
    /// for piping. In directive mode, writes to stderr (where user messages go).
    fn data(&mut self, content: String) -> io::Result<()> {
        self.write_message_line(&content)
    }

    /// Emit table/UI output to stderr
    ///
    /// Used for table rows and progress indicators that should appear on the same
    /// stream as progress bars. Both modes write to stderr.
    fn table(&mut self, content: String) -> io::Result<()> {
        use worktrunk::styling::eprintln;
        eprintln!("{content}");
        io::stderr().flush()
    }

    /// Flush output buffers
    fn flush(&mut self) -> io::Result<()> {
        io::stdout().flush()?;
        io::stderr().flush()
    }

    /// Flush streams before showing stderr prompt
    fn flush_for_stderr_prompt(&mut self) -> io::Result<()> {
        io::stdout().flush()?;
        io::stderr().flush()
    }

    // Methods that must be implemented per-mode (no sensible default)

    /// Display a shell integration hint
    ///
    /// Interactive shows it, Directive suppresses it
    fn shell_integration_hint(&mut self, message: String) -> io::Result<()>;

    /// Request directory change
    ///
    /// Interactive stores path, Directive emits directive
    fn change_directory(&mut self, path: &Path) -> io::Result<()>;

    /// Request command execution
    ///
    /// Interactive runs command, Directive emits directive
    fn execute(&mut self, command: String) -> anyhow::Result<()>;

    /// Terminate output
    ///
    /// Interactive no-op, Directive writes NUL
    fn terminate_output(&mut self) -> io::Result<()>;
}
