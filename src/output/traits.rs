//! Shared output handler trait with default implementations
//!
//! This trait extracts common patterns between Interactive and Directive output modes.
//! The fundamental operation is `write_message_line` - implementations control where
//! messages go (stdout for interactive, stderr for directive).
//!
//! # Message Formatting
//!
//! For semantic messages (errors, hints, warnings, etc.), use the message formatting
//! functions from `worktrunk::styling` with `output::print()`:
//!
//! ```ignore
//! use worktrunk::styling::{error_message, success_message, hint_message};
//! output::print(error_message("Operation failed"))?;
//! output::print(success_message("Operation complete"))?;
//! output::print(hint_message("Try --force"))?;
//! ```
//!
//! This decouples message formatting from the output system, allowing the same
//! formatting functions to be used both for output and in Display impls.

use std::io::{self, Write};
use std::path::Path;

/// Core output handler trait
///
/// Implementations provide their message stream via `write_message_line`
/// and override only the methods that differ between modes.
///
/// # Message Formatting
///
/// For semantic messages (errors, hints, etc.), use the message formatting functions
/// from `worktrunk::styling` combined with `print()`:
///
/// ```ignore
/// use worktrunk::styling::{error_message, hint_message};
/// output::print(error_message("Something went wrong"))?;
/// output::print(hint_message("Try --force"))?;
/// ```
pub trait OutputHandler {
    /// Write a single logical message line to the primary user stream
    fn write_message_line(&mut self, line: &str) -> io::Result<()>;

    /// Print a message (written as-is)
    ///
    /// Use with message formatting functions for semantic output:
    /// ```ignore
    /// output::print(error_message("Failed"))?;
    /// output::print(success_message("Done"))?;
    /// ```
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
