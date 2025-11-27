//! Shared output handler trait with default implementations
//!
//! This trait extracts common patterns between Interactive and Directive output modes.
//! The fundamental operation is `write_message_line` - implementations control where
//! messages go (stdout for interactive, stderr for directive).

use std::io::{self, Write};
use std::path::Path;
use worktrunk::styling::{HINT_EMOJI, INFO_EMOJI, PROGRESS_EMOJI, SUCCESS_EMOJI, WARNING_EMOJI};

/// Core output handler trait
///
/// Implementations provide their message stream via `write_message_line`
/// and override only the methods that differ between modes.
pub trait OutputHandler {
    /// Write a single logical message line to the primary user stream
    fn write_message_line(&mut self, line: &str) -> io::Result<()>;

    /// Emit a success message
    fn success(&mut self, message: String) -> io::Result<()> {
        self.write_message_line(&format!("{SUCCESS_EMOJI} {message}"))
    }

    /// Emit a progress message
    fn progress(&mut self, message: String) -> io::Result<()> {
        self.write_message_line(&format!("{PROGRESS_EMOJI} {message}"))
    }

    /// Emit a hint message
    fn hint(&mut self, message: String) -> io::Result<()> {
        self.write_message_line(&format!("{HINT_EMOJI} {message}"))
    }

    /// Emit an info message
    fn info(&mut self, message: String) -> io::Result<()> {
        self.write_message_line(&format!("{INFO_EMOJI} {message}"))
    }

    /// Emit a warning message
    fn warning(&mut self, message: String) -> io::Result<()> {
        self.write_message_line(&format!("{WARNING_EMOJI} {message}"))
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

    /// Emit raw output without emoji decoration
    fn raw(&mut self, content: String) -> io::Result<()> {
        self.write_message_line(&content)
    }

    /// Emit raw terminal output to stderr
    ///
    /// Both modes write to stderr for table output
    fn raw_terminal(&mut self, content: String) -> io::Result<()> {
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
