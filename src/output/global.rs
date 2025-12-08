//! Global output context using thread-local storage
//!
//! This provides a logging-like API where you configure output mode once
//! at program start, then use it anywhere without passing parameters.
//!
//! # Implementation
//!
//! Uses `thread_local!` to store per-thread output state:
//! - Each thread gets its own `OUTPUT_CONTEXT`
//! - `RefCell<T>` enables interior mutability (runtime borrow checking)
//! - Trait object (`Box<dyn OutputHandler>`) for runtime polymorphism
//!
//! # Trade-offs
//!
//! - ✅ Zero parameter threading - call from anywhere
//! - ✅ Single initialization point - set once in main()
//! - ✅ Fast access - thread-local is just a pointer lookup
//! - ✅ Simple mental model - one trait, no enum wrapper
//! - ⚠️ Per-thread state - not an issue for single-threaded CLI
//! - ⚠️ Runtime borrow checks - acceptable for this access pattern

use super::directive::DirectiveOutput;
use super::interactive::InteractiveOutput;
use super::traits::OutputHandler;
use std::cell::RefCell;
use std::io;
use std::path::Path;

/// Output mode selection
#[derive(Debug, Clone, Copy)]
pub enum OutputMode {
    Interactive,
    Directive,
}

thread_local! {
    static OUTPUT_CONTEXT: RefCell<Box<dyn OutputHandler>> = RefCell::new(
        Box::new(InteractiveOutput::new())
    );
}

/// Helper to access the output handler
fn with_output<R>(f: impl FnOnce(&mut dyn OutputHandler) -> R) -> R {
    OUTPUT_CONTEXT.with(|ctx| {
        let mut handler = ctx.borrow_mut();
        f(handler.as_mut())
    })
}

/// Initialize the global output context
///
/// Call this once at program startup to set the output mode.
pub fn initialize(mode: OutputMode) {
    let handler: Box<dyn OutputHandler> = match mode {
        OutputMode::Interactive => Box::new(InteractiveOutput::new()),
        OutputMode::Directive => Box::new(DirectiveOutput::new()),
    };

    OUTPUT_CONTEXT.with(|ctx| {
        *ctx.borrow_mut() = handler;
    });
}

/// Display a shell integration hint (suppressed in directive mode)
///
/// Shell integration hints like "Run `wt config shell install` to enable automatic cd" are only
/// shown in interactive mode since directive mode users already have shell integration.
///
/// This is kept as a separate method because it has mode-specific behavior.
pub fn shell_integration_hint(message: impl Into<String>) -> io::Result<()> {
    with_output(|h| h.shell_integration_hint(message.into()))
}

/// Print a message (written as-is)
///
/// Use with message formatting functions for semantic output:
/// ```ignore
/// use worktrunk::styling::{error_message, success_message, hint_message};
/// output::print(error_message("Failed to create branch"))?;
/// output::print(success_message("Branch created"))?;
/// output::print(hint_message("Use --force to override"))?;
/// ```
pub fn print(message: impl Into<String>) -> io::Result<()> {
    with_output(|h| h.print(message.into()))
}

/// Emit gutter-formatted content
///
/// Gutter content has its own visual structure (column 0 gutter + content),
/// so no additional emoji is added. Use with `format_with_gutter()` or `format_bash_with_gutter()`.
pub fn gutter(content: impl Into<String>) -> io::Result<()> {
    with_output(|h| h.gutter(content.into()))
}

/// Emit a blank line for visual separation
///
/// Used to separate logical sections of output.
pub fn blank() -> io::Result<()> {
    with_output(|h| h.blank())
}

/// Emit structured data output without emoji decoration
///
/// Used for JSON and other pipeable data. In interactive mode, writes to stdout
/// for piping. In directive mode, writes to stderr (where user messages go).
///
/// Example:
/// ```rust,ignore
/// output::data(json_string)?;
/// ```
pub fn data(content: impl Into<String>) -> io::Result<()> {
    with_output(|h| h.data(content.into()))
}

/// Emit table/UI output to stderr
///
/// Used for table rows and progress indicators that should appear on the same
/// stream as progress bars. Both modes write to stderr.
///
/// Example:
/// ```rust,ignore
/// output::table(layout.format_header_line())?;
/// for item in items {
///     output::table(layout.format_item_line(item))?;
/// }
/// ```
pub fn table(content: impl Into<String>) -> io::Result<()> {
    with_output(|h| h.table(content.into()))
}

/// Request directory change (for shell integration)
pub fn change_directory(path: impl AsRef<Path>) -> io::Result<()> {
    with_output(|h| h.change_directory(path.as_ref()))
}

/// Request command execution
pub fn execute(command: impl Into<String>) -> anyhow::Result<()> {
    with_output(|h| h.execute(command.into()))
}

/// Flush any buffered output
pub fn flush() -> io::Result<()> {
    with_output(|h| h.flush())
}

/// Flush streams before showing stderr prompt
///
/// This prevents stream interleaving. Interactive prompts write to stderr, so we must
/// ensure all previous output is flushed first:
/// - In directive mode: Flushes stderr (messages stream there in real-time)
/// - In interactive mode: Flushes both stdout and stderr
///
/// Note: With stderr separation (messages on stderr in directive mode), prompts
/// naturally appear after messages without needing special synchronization.
pub fn flush_for_stderr_prompt() -> io::Result<()> {
    with_output(|h| h.flush_for_stderr_prompt())
}

/// Terminate command output
///
/// In directive mode, emits the buffered shell script (cd and exec commands) to stdout.
/// In interactive mode, this is a no-op.
pub fn terminate_output() -> io::Result<()> {
    with_output(|h| h.terminate_output())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mode_switching() {
        // Default is interactive
        initialize(OutputMode::Interactive);
        // Just verify initialize doesn't panic

        // Switch to directive
        initialize(OutputMode::Directive);
        // Just verify initialize doesn't panic
    }
}
