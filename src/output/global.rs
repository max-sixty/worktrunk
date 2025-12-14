//! Global output context with thread-safe mode propagation
//!
//! This provides a logging-like API where you configure output mode once
//! at program start, then use it anywhere without passing parameters.
//!
//! # Implementation
//!
//! Uses a simple global approach:
//! - `OnceLock<OutputMode>` stores the mode globally (set once at startup)
//! - Handlers are created on-demand for each operation
//! - Directive state (target_dir, exec_command) is stored globally for main thread
//!
//! # Trade-offs
//!
//! - ✅ Zero parameter threading - call from anywhere
//! - ✅ Single initialization point - set once in main()
//! - ✅ Spawned threads automatically use correct mode
//! - ✅ Simple mental model - one global mode, handlers created on-demand
//! - ✅ No thread-local complexity

use super::directive::DirectiveOutput;
use super::interactive::InteractiveOutput;
use super::traits::OutputHandler;
use crate::cli::DirectiveShell;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Output mode selection
#[derive(Debug, Clone, Copy)]
pub enum OutputMode {
    Interactive,
    /// Directive mode with shell type for output formatting
    Directive(DirectiveShell),
}

/// Global output mode, set once at initialization.
static GLOBAL_MODE: OnceLock<OutputMode> = OnceLock::new();

/// Accumulated state for change_directory/execute/terminate_output.
/// Only used by main thread - these operations are never called from spawned threads.
///
/// Uses `OnceLock<Mutex<T>>` pattern:
/// - `OnceLock` provides one-time initialization (set in `initialize()`)
/// - `Mutex` allows mutation after initialization
/// - No unsafe code required
///
/// Lock poisoning (from `.expect()`) is theoretically possible but practically
/// unreachable - the lock is only held for trivial Option assignments that cannot panic.
static OUTPUT_STATE: OnceLock<Mutex<OutputState>> = OnceLock::new();

#[derive(Default)]
struct OutputState {
    target_dir: Option<PathBuf>,
    exec_command: Option<String>,
}

/// Get the current output mode, defaulting to Interactive if not initialized.
fn get_mode() -> OutputMode {
    GLOBAL_MODE
        .get()
        .copied()
        .unwrap_or(OutputMode::Interactive)
}

/// Initialize the global output context
///
/// Call this once at program startup to set the output mode.
/// All threads will automatically use the same mode.
pub fn initialize(mode: OutputMode) {
    let _ = GLOBAL_MODE.set(mode);
    let _ = OUTPUT_STATE.set(Mutex::new(OutputState::default()));
}

/// Display a shell integration hint (suppressed in directive mode)
///
/// Shell integration hints like "Run `wt config shell install` to enable automatic cd" are only
/// shown in interactive mode since directive mode users already have shell integration.
///
/// This is kept as a separate method because it has mode-specific behavior.
pub fn shell_integration_hint(message: impl Into<String>) -> io::Result<()> {
    match get_mode() {
        OutputMode::Interactive => InteractiveOutput::new().shell_integration_hint(message.into()),
        OutputMode::Directive(_) => Ok(()), // Suppressed
    }
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
    match get_mode() {
        OutputMode::Interactive => InteractiveOutput::new().print(message.into()),
        OutputMode::Directive(_) => DirectiveOutput::new().print(message.into()),
    }
}

/// Emit gutter-formatted content
///
/// Gutter content has its own visual structure (column 0 gutter + content),
/// so no additional emoji is added. Use with `format_with_gutter()` or `format_bash_with_gutter()`.
pub fn gutter(content: impl Into<String>) -> io::Result<()> {
    match get_mode() {
        OutputMode::Interactive => InteractiveOutput::new().gutter(content.into()),
        OutputMode::Directive(_) => DirectiveOutput::new().gutter(content.into()),
    }
}

/// Emit a blank line for visual separation
///
/// Used to separate logical sections of output.
pub fn blank() -> io::Result<()> {
    match get_mode() {
        OutputMode::Interactive => InteractiveOutput::new().blank(),
        OutputMode::Directive(_) => DirectiveOutput::new().blank(),
    }
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
    match get_mode() {
        OutputMode::Interactive => InteractiveOutput::new().data(content.into()),
        OutputMode::Directive(_) => DirectiveOutput::new().data(content.into()),
    }
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
    match get_mode() {
        OutputMode::Interactive => InteractiveOutput::new().table(content.into()),
        OutputMode::Directive(_) => DirectiveOutput::new().table(content.into()),
    }
}

/// Request directory change (for shell integration)
///
/// In directive mode, buffers the path for the final shell script.
/// In interactive mode, stores path for execute() to use as working directory.
///
/// No-op if called before initialize() - this is safe since main thread
/// operations only happen after initialization.
pub fn change_directory(path: impl AsRef<Path>) -> io::Result<()> {
    if let Some(state) = OUTPUT_STATE.get() {
        state.lock().expect("OUTPUT_STATE lock poisoned").target_dir =
            Some(path.as_ref().to_path_buf());
    }
    Ok(())
}

/// Request command execution
///
/// In interactive mode, executes the command directly (replacing process on Unix).
/// In directive mode, buffers the command for the final shell script.
pub fn execute(command: impl Into<String>) -> anyhow::Result<()> {
    let command = command.into();
    match get_mode() {
        OutputMode::Interactive => {
            // Get target directory (lock released before execute to avoid holding across I/O)
            let target_dir = OUTPUT_STATE.get().and_then(|s| {
                let guard = s.lock().expect("OUTPUT_STATE lock poisoned");
                guard.target_dir.clone()
            });

            let mut handler = InteractiveOutput::new();
            if let Some(path) = target_dir {
                handler.change_directory(&path)?;
            }
            handler.execute(command)
        }
        OutputMode::Directive(_) => {
            if let Some(state) = OUTPUT_STATE.get() {
                state
                    .lock()
                    .expect("OUTPUT_STATE lock poisoned")
                    .exec_command = Some(command);
            }
            Ok(())
        }
    }
}

/// Flush any buffered output
pub fn flush() -> io::Result<()> {
    match get_mode() {
        OutputMode::Interactive => InteractiveOutput::new().flush(),
        OutputMode::Directive(_) => DirectiveOutput::new().flush(),
    }
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
    match get_mode() {
        OutputMode::Interactive => InteractiveOutput::new().flush_for_stderr_prompt(),
        OutputMode::Directive(_) => DirectiveOutput::new().flush_for_stderr_prompt(),
    }
}

/// Terminate command output
///
/// In directive mode, emits the buffered shell script (cd and exec commands) to stdout.
/// In interactive mode, this is a no-op.
pub fn terminate_output() -> io::Result<()> {
    match get_mode() {
        OutputMode::Interactive => Ok(()),
        OutputMode::Directive(shell) => {
            let mut stderr = io::stderr();

            // Reset ANSI state before returning to shell
            write!(stderr, "{}", anstyle::Reset)?;
            stderr.flush()?;

            // Emit shell script to stdout with buffered directives
            let mut stdout = io::stdout();

            if let Some(state) = OUTPUT_STATE.get() {
                let guard = state.lock().expect("OUTPUT_STATE lock poisoned");

                // cd command
                if let Some(ref path) = guard.target_dir {
                    let path_str = path.to_string_lossy();
                    match shell {
                        DirectiveShell::Posix => {
                            // shell_escape handles quoting (adds quotes if needed)
                            let escaped = shell_escape::escape(path_str.as_ref().into());
                            writeln!(stdout, "cd {}", escaped)?;
                        }
                        DirectiveShell::Powershell => {
                            // PowerShell: double single quotes for escaping
                            // (no crate support for PowerShell escaping)
                            let escaped = path_str.replace('\'', "''");
                            writeln!(stdout, "Set-Location '{}'", escaped)?;
                        }
                    }
                }

                // exec command
                if let Some(ref cmd) = guard.exec_command {
                    writeln!(stdout, "{}", cmd)?;
                }
            }

            stdout.flush()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initialize_does_not_panic() {
        use crate::cli::DirectiveShell;

        // Verify initialize() doesn't panic when called (possibly multiple times in tests).
        // Note: GLOBAL_MODE can only be set once per process.
        // In production, initialize() is called exactly once.
        initialize(OutputMode::Interactive);
        initialize(OutputMode::Directive(DirectiveShell::Posix));
        initialize(OutputMode::Directive(DirectiveShell::Powershell));
    }

    #[test]
    fn test_spawned_thread_uses_correct_mode() {
        use crate::cli::DirectiveShell;
        use std::sync::mpsc;

        // Initialize mode (may already be set by another test, which is fine)
        initialize(OutputMode::Directive(DirectiveShell::Posix));

        // Spawn a thread and verify it can access output without panicking.
        // The thread reads the same GLOBAL_MODE.
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            // Access output system in spawned thread
            let _ = flush();
            tx.send(()).unwrap();
        })
        .join()
        .unwrap();

        rx.recv().unwrap();
    }
}
