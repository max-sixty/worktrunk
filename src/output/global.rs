//! Global output context with thread-safe mode propagation
//!
//! This provides a logging-like API where you configure output mode once
//! at program start, then use it anywhere without passing parameters.
//!
//! # Implementation
//!
//! Uses a simple global approach:
//! - `OnceLock<OutputMode>` stores the mode globally (set once at startup)
//! - `OnceLock<Mutex<OutputState>>` stores stateful data (target_dir, exec_command)
//! - All output functions check the mode and behave accordingly
//!
//! # Output Modes
//!
//! - **Interactive**: User messages to stderr, data (JSON) to stdout for piping
//! - **Directive**: All output to stderr, stdout reserved for shell script at end
//!
//! # Trade-offs
//!
//! - Zero parameter threading - call from anywhere
//! - Single initialization point - set once in main()
//! - Spawned threads automatically use correct mode
//! - Simple implementation - no traits, no handler structs

#[cfg(not(unix))]
use super::handlers::execute_streaming;
use crate::cli::DirectiveShell;
use std::io::{self, Write};
use std::path::Path;
use std::path::PathBuf;
#[cfg(unix)]
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
#[cfg(unix)]
use worktrunk::shell_exec::ShellConfig;
use worktrunk::styling::{eprintln, hint_message, println, stderr};

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

/// Display a shell integration hint
///
/// Shell integration hints like "Run `wt config shell install` to enable automatic cd" are only
/// shown in interactive mode. In directive mode, users already have shell integration.
/// This is the canonical check - call sites don't need to guard.
pub fn shell_integration_hint(message: impl Into<String>) -> io::Result<()> {
    if matches!(get_mode(), OutputMode::Directive(_)) {
        return Ok(());
    }
    eprintln!("{}", hint_message(message.into()));
    stderr().flush()
}

/// Print a message to stderr (written as-is)
///
/// Use with message formatting functions for semantic output:
/// ```ignore
/// use worktrunk::styling::{error_message, success_message, hint_message};
/// output::print(error_message("Failed to create branch"))?;
/// output::print(success_message("Branch created"))?;
/// output::print(hint_message("Use --force to override"))?;
/// ```
pub fn print(message: impl Into<String>) -> io::Result<()> {
    eprintln!("{}", message.into());
    stderr().flush()
}

/// Emit gutter-formatted content to stderr
///
/// Gutter content has its own visual structure (column 0 gutter + content),
/// so no additional emoji is added. Use with `format_with_gutter()` or `format_bash_with_gutter()`.
///
/// Note: Gutter content is pre-formatted with its own newlines, so we use write! not writeln!.
pub fn gutter(content: impl Into<String>) -> io::Result<()> {
    write!(stderr(), "{}", content.into())?;
    stderr().flush()
}

/// Emit a blank line for visual separation
pub fn blank() -> io::Result<()> {
    eprintln!();
    stderr().flush()
}

/// Emit structured data output without emoji decoration
///
/// Used for JSON and other pipeable data.
/// - **Interactive**: writes to stdout (for piping to `jq`, etc.)
/// - **Directive**: writes to stderr (stdout reserved for shell script)
///
/// Example:
/// ```rust,ignore
/// output::data(json_string)?;
/// ```
pub fn data(content: impl Into<String>) -> io::Result<()> {
    let content = content.into();
    match get_mode() {
        OutputMode::Interactive => {
            // Structured data goes to stdout for piping
            println!("{content}");
            io::stdout().flush()
        }
        OutputMode::Directive(_) => {
            // stdout reserved for shell script, data goes to stderr
            eprintln!("{content}");
            stderr().flush()
        }
    }
}

/// Emit raw data output, bypassing anstream color detection
///
/// Like `data()` but writes directly to stdout/stderr without anstream processing.
/// Use when ANSI codes must be preserved regardless of TTY detection (e.g., statusline
/// output for Claude Code which always expects ANSI).
///
/// - **Interactive**: writes to stdout
/// - **Directive**: writes to stderr (stdout reserved for shell script)
pub fn data_raw(content: impl Into<String>) -> io::Result<()> {
    let content = content.into();
    match get_mode() {
        OutputMode::Interactive => {
            writeln!(io::stdout(), "{content}")?;
            io::stdout().flush()
        }
        OutputMode::Directive(_) => {
            writeln!(io::stderr(), "{content}")?;
            io::stderr().flush()
        }
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
    eprintln!("{}", content.into());
    stderr().flush()
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

            execute_command(command, target_dir.as_deref())
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

/// Execute a command in the given directory (Unix: exec, non-Unix: spawn)
#[cfg(unix)]
fn execute_command(command: String, target_dir: Option<&Path>) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;

    let exec_dir = target_dir.unwrap_or_else(|| Path::new("."));
    let shell = ShellConfig::get();

    // Use exec() to replace wt process with the command.
    // This gives the command full TTY access (stdin, stdout, stderr all inherited),
    // enabling interactive programs like `claude` to work properly.
    let mut cmd = shell.command(&command);
    let err = cmd
        .current_dir(exec_dir)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .exec();

    // exec() only returns on error
    Err(anyhow::anyhow!(
        "Failed to exec '{}' with {}: {}",
        command,
        shell.name,
        err
    ))
}

/// Execute a command in the given directory (non-Unix: spawn and wait)
#[cfg(not(unix))]
fn execute_command(command: String, target_dir: Option<&Path>) -> anyhow::Result<()> {
    use worktrunk::git::WorktrunkError;

    // On non-Unix platforms, fall back to spawn-and-wait.
    // This uses the shell abstraction (Git Bash if available).
    let exec_dir = target_dir.unwrap_or_else(|| Path::new("."));
    if let Err(err) = execute_streaming(&command, exec_dir, false, None, true) {
        // If the command failed with an exit code, just exit with that code.
        // This matches Unix behavior where exec() replaces the process and
        // the shell's exit code becomes the process exit code (no error message).
        if let Some(WorktrunkError::ChildProcessExited { code, .. }) =
            err.downcast_ref::<WorktrunkError>()
        {
            std::process::exit(*code);
        }
        return Err(err);
    }
    Ok(())
}

/// Flush any buffered output
pub fn flush() -> io::Result<()> {
    io::stdout().flush()?;
    io::stderr().flush()
}

/// Flush streams before showing stderr prompt
///
/// This prevents stream interleaving. Interactive prompts write to stderr, so we must
/// ensure all previous output is flushed first.
pub fn flush_for_stderr_prompt() -> io::Result<()> {
    io::stdout().flush()?;
    io::stderr().flush()
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
                            // Always single-quote for consistent cross-platform behavior
                            // (shell_escape behaves differently on Windows vs Unix)
                            let escaped = path_str.replace('\'', "'\\''");
                            writeln!(stdout, "cd '{}'", escaped)?;
                        }
                        DirectiveShell::Powershell => {
                            // PowerShell: double single quotes for escaping
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
    use std::path::PathBuf;

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

    // Shell escaping tests (moved from directive.rs)

    #[test]
    fn test_shell_script_format() {
        // Test that POSIX quoting produces correct output
        let path = PathBuf::from("/test/path");
        let path_str = path.to_string_lossy();
        let escaped = path_str.replace('\'', "'\\''");
        let cd_cmd = format!("cd '{}'", escaped);
        assert_eq!(cd_cmd, "cd '/test/path'");
    }

    #[test]
    fn test_path_with_single_quotes() {
        // Paths with single quotes need escaping: ' -> '\''
        let path = PathBuf::from("/test/it's/path");
        let path_str = path.to_string_lossy();
        let escaped = path_str.replace('\'', "'\\''");
        let cd_cmd = format!("cd '{}'", escaped);
        assert_eq!(cd_cmd, "cd '/test/it'\\''s/path'");
    }

    #[test]
    fn test_path_with_spaces() {
        // Paths with spaces are safely quoted
        let path = PathBuf::from("/test/my path/here");
        let path_str = path.to_string_lossy();
        let escaped = path_str.replace('\'', "'\\''");
        let cd_cmd = format!("cd '{}'", escaped);
        assert_eq!(cd_cmd, "cd '/test/my path/here'");
    }

    #[test]
    fn test_powershell_path_format() {
        // PowerShell uses Set-Location and doubles single quotes for escaping
        let path = PathBuf::from("C:\\Users\\test\\path");
        let path_str = path.to_string_lossy();
        let escaped = path_str.replace('\'', "''");
        let ps_cmd = format!("Set-Location '{}'", escaped);
        assert_eq!(ps_cmd, "Set-Location 'C:\\Users\\test\\path'");
    }

    #[test]
    fn test_powershell_path_with_single_quotes() {
        // PowerShell escapes single quotes by doubling them
        let path = PathBuf::from("C:\\Users\\it's a test\\path");
        let path_str = path.to_string_lossy();
        let escaped = path_str.replace('\'', "''");
        let ps_cmd = format!("Set-Location '{}'", escaped);
        assert_eq!(ps_cmd, "Set-Location 'C:\\Users\\it''s a test\\path'");
    }

    /// Test that anstyle formatting is preserved
    #[test]
    fn test_success_preserves_anstyle() {
        use anstyle::{AnsiColor, Color, Style};

        let bold = Style::new().bold();
        let cyan = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan)));

        // Create a styled message
        let styled = format!("{cyan}Styled{cyan:#} {bold}message{bold:#}");

        // The styled message should contain ANSI escape codes
        assert!(
            styled.contains('\x1b'),
            "Styled message should contain ANSI escape codes"
        );
    }

    #[test]
    fn test_color_reset_on_empty_style() {
        // BUG HYPOTHESIS from CLAUDE.md (lines 154-177):
        // Using {:#} on Style::new() produces empty string, not reset code
        use anstyle::Style;

        let empty_style = Style::new();
        let output = format!("{:#}", empty_style);

        // This is the bug: {:#} on empty style produces empty string!
        assert_eq!(
            output, "",
            "BUG: Empty style reset produces empty string, not \\x1b[0m"
        );

        // This means colors can leak: "text in color{:#}" where # is on empty Style
        // doesn't actually reset, it just removes the style prefix!
    }

    #[test]
    fn test_proper_reset_with_anstyle_reset() {
        // The correct way to reset ALL styles is anstyle::Reset
        use anstyle::Reset;

        let output = format!("{}", Reset);

        // This should produce the actual reset escape code
        assert!(
            output.contains("\x1b[0m") || output == "\x1b[0m",
            "Reset should produce actual ANSI reset code"
        );
    }

    #[test]
    fn test_nested_style_resets_leak_color() {
        // BUG HYPOTHESIS from CLAUDE.md:
        // Nested style resets can leak colors
        use anstyle::{AnsiColor, Color, Style};

        let warning = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow)));
        let bold = Style::new().bold();

        // BAD pattern: nested reset
        let bad_output = format!("{warning}Text with {bold}nested{bold:#} styles{warning:#}");

        // When {bold:#} resets, it might also reset the warning color!
        // We can't easily test the actual ANSI codes here, but document the issue
        std::println!(
            "Nested reset output: {}",
            bad_output.replace('\x1b', "\\x1b")
        );

        // GOOD pattern: compose styles
        let warning_bold = warning.bold();
        let good_output =
            format!("{warning}Text with {warning_bold}composed{warning_bold:#} styles{warning:#}");
        std::println!("Composed output: {}", good_output.replace('\x1b', "\\x1b"));

        // The good pattern maintains color through the bold section
    }
}
