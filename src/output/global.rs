//! Global output context with thread-safe mode propagation
//!
//! This provides a logging-like API where you configure output mode once
//! at program start, then use it anywhere without passing parameters.
//!
//! # Implementation
//!
//! Uses a hybrid approach:
//! - `OnceLock<OutputMode>` stores the mode globally (set once at startup)
//! - `thread_local!` stores per-thread handlers, initialized from the global mode
//! - `RefCell<T>` enables interior mutability (runtime borrow checking)
//! - Trait object (`Box<dyn OutputHandler>`) for runtime polymorphism
//!
//! # Trade-offs
//!
//! - ✅ Zero parameter threading - call from anywhere
//! - ✅ Single initialization point - set once in main()
//! - ✅ Fast access - thread-local is just a pointer lookup
//! - ✅ Spawned threads inherit the mode - handlers created with correct mode
//! - ✅ Simple mental model - one trait, no enum wrapper
//! - ⚠️ Runtime borrow checks - acceptable for this access pattern

use super::directive::DirectiveOutput;
use super::interactive::InteractiveOutput;
use super::traits::OutputHandler;
use crate::cli::DirectiveShell;
use std::cell::RefCell;
use std::io;
use std::path::Path;
use std::sync::OnceLock;

/// Output mode selection
#[derive(Debug, Clone, Copy)]
pub enum OutputMode {
    Interactive,
    /// Directive mode with shell type for output formatting
    Directive(DirectiveShell),
}

/// Global output mode, set once at initialization.
/// Spawned threads read this to initialize their thread-local handlers.
static GLOBAL_MODE: OnceLock<OutputMode> = OnceLock::new();

thread_local! {
    static OUTPUT_CONTEXT: RefCell<Box<dyn OutputHandler>> = RefCell::new({
        // Read mode from global (set by initialize()), default to Interactive if unset
        let mode = GLOBAL_MODE.get().copied().unwrap_or(OutputMode::Interactive);
        match mode {
            OutputMode::Interactive => Box::new(InteractiveOutput::new()),
            OutputMode::Directive(shell) => Box::new(DirectiveOutput::new(shell)),
        }
    });
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
/// Spawned threads will automatically use the same mode.
pub fn initialize(mode: OutputMode) {
    // Set global mode FIRST so spawned threads pick it up
    let _ = GLOBAL_MODE.set(mode);

    // Then initialize current thread's context
    let handler: Box<dyn OutputHandler> = match mode {
        OutputMode::Interactive => Box::new(InteractiveOutput::new()),
        OutputMode::Directive(shell) => Box::new(DirectiveOutput::new(shell)),
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
    fn test_initialize_does_not_panic() {
        use crate::cli::DirectiveShell;

        // Verify initialize() doesn't panic when called (possibly multiple times in tests).
        // Note: GLOBAL_MODE can only be set once per process, but the current thread's
        // handler is always updated. In production, initialize() is called exactly once.
        initialize(OutputMode::Interactive);
        initialize(OutputMode::Directive(DirectiveShell::Posix));
        initialize(OutputMode::Directive(DirectiveShell::Powershell));
    }

    #[test]
    fn test_spawned_thread_inherits_mode() {
        use crate::cli::DirectiveShell;
        use std::sync::mpsc;

        // Initialize mode (may already be set by another test, which is fine)
        initialize(OutputMode::Directive(DirectiveShell::Posix));

        // Spawn a thread and verify it can access output without panicking.
        // The thread will inherit the mode from GLOBAL_MODE.
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            // Access output system in spawned thread - this should use Directive mode
            // if GLOBAL_MODE was set, or Interactive as fallback.
            let _ = with_output(|h| h.flush());
            tx.send(()).unwrap();
        })
        .join()
        .unwrap();

        rx.recv().unwrap();
    }
}
