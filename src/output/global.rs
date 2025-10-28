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
//! - Enum dispatch avoids trait object overhead (static dispatch)
//!
//! # Trade-offs
//!
//! - ✅ Zero parameter threading - call from anywhere
//! - ✅ Single initialization point - set once in main()
//! - ✅ Fast access - thread-local is just a pointer lookup
//! - ⚠️ Per-thread state - not an issue for single-threaded CLI
//! - ⚠️ Runtime borrow checks - acceptable for this access pattern

use super::directive::DirectiveOutput;
use super::interactive::InteractiveOutput;
use std::cell::RefCell;
use std::io;
use std::path::Path;

/// Output mode selection
#[derive(Debug, Clone, Copy)]
pub enum OutputMode {
    Interactive,
    Directive,
}

/// Output handler - enum dispatch instead of trait object
enum OutputHandler {
    Interactive(InteractiveOutput),
    Directive(DirectiveOutput),
}

thread_local! {
    static OUTPUT_CONTEXT: RefCell<OutputHandler> = RefCell::new(
        OutputHandler::Interactive(InteractiveOutput::new())
    );
}

/// Initialize the global output context
///
/// Call this once at program startup to set the output mode.
pub fn initialize(mode: OutputMode) {
    let handler = match mode {
        OutputMode::Interactive => OutputHandler::Interactive(InteractiveOutput::new()),
        OutputMode::Directive => OutputHandler::Directive(DirectiveOutput::new()),
    };

    OUTPUT_CONTEXT.with(|ctx| {
        *ctx.borrow_mut() = handler;
    });
}

/// Emit a success message
pub fn success(message: impl Into<String>) -> io::Result<()> {
    OUTPUT_CONTEXT.with(|ctx| {
        let msg = message.into();
        match &mut *ctx.borrow_mut() {
            OutputHandler::Interactive(i) => i.success(msg),
            OutputHandler::Directive(d) => d.success(msg),
        }
    })
}

/// Emit a progress message (only shown in interactive mode)
///
/// Progress messages are intermediate status updates like "🔄 Cleaning up worktree..."
/// They are shown to human users but suppressed in directive mode (shell integration).
pub fn progress(message: impl Into<String>) -> io::Result<()> {
    OUTPUT_CONTEXT.with(|ctx| {
        let msg = message.into();
        match &mut *ctx.borrow_mut() {
            OutputHandler::Interactive(i) => i.progress(msg),
            OutputHandler::Directive(d) => d.progress(msg),
        }
    })
}

/// Request directory change (for shell integration)
pub fn change_directory(path: impl AsRef<Path>) -> io::Result<()> {
    OUTPUT_CONTEXT.with(|ctx| {
        let p = path.as_ref();
        match &mut *ctx.borrow_mut() {
            OutputHandler::Interactive(i) => i.change_directory(p),
            OutputHandler::Directive(d) => d.change_directory(p),
        }
    })
}

/// Request command execution
pub fn execute(command: impl Into<String>) -> io::Result<()> {
    OUTPUT_CONTEXT.with(|ctx| {
        let cmd = command.into();
        match &mut *ctx.borrow_mut() {
            OutputHandler::Interactive(i) => i.execute(cmd),
            OutputHandler::Directive(d) => d.execute(cmd),
        }
    })
}

/// Flush any buffered output
pub fn flush() -> io::Result<()> {
    OUTPUT_CONTEXT.with(|ctx| match &mut *ctx.borrow_mut() {
        OutputHandler::Interactive(i) => i.flush(),
        OutputHandler::Directive(d) => d.flush(),
    })
}

/// Check if interactive mode
pub fn is_interactive() -> bool {
    OUTPUT_CONTEXT.with(|ctx| match &*ctx.borrow() {
        OutputHandler::Interactive(i) => i.is_interactive(),
        OutputHandler::Directive(d) => d.is_interactive(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mode_switching() {
        // Default is interactive
        initialize(OutputMode::Interactive);
        assert!(is_interactive());

        // Switch to directive
        initialize(OutputMode::Directive);
        assert!(!is_interactive());
    }
}
