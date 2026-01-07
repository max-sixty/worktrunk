//! Global output context with file-based directive passing
//!
//! This provides a logging-like API where you configure output once
//! at program start, then use output functions anywhere without passing parameters.
//!
//! # Implementation
//!
//! Uses a simple global approach:
//! - `OnceLock<Mutex<OutputState>>` stores the directive file path and accumulated state
//! - If `WORKTRUNK_DIRECTIVE_FILE` env var is set, directives are written to that file
//! - Otherwise, commands execute directly
//!
//! # Shell Integration
//!
//! When `WORKTRUNK_DIRECTIVE_FILE` is set (by the shell wrapper), wt writes shell commands
//! (like `cd '/path'`) to that file. The shell wrapper sources the file after wt exits.
//! This allows the parent shell to change directory.
//!
//! # Trade-offs
//!
//! - Zero parameter threading - call from anywhere
//! - Lazy initialization - state initialized on first use
//! - Spawned threads automatically use correct context
//! - Simple implementation - no traits, no handler structs
//! - stdout always available for data output (JSON, etc.)

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;
use std::path::PathBuf;
#[cfg(unix)]
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use worktrunk::shell_exec::DIRECTIVE_FILE_ENV_VAR;
#[cfg(unix)]
use worktrunk::shell_exec::ShellConfig;
#[cfg(not(unix))]
use worktrunk::shell_exec::execute_streaming;
use worktrunk::styling::{eprintln, stderr};

/// Global output state, lazily initialized on first access.
///
/// Uses `OnceLock<Mutex<T>>` pattern:
/// - `OnceLock` provides one-time lazy initialization (via `get_or_init()`)
/// - `Mutex` allows mutation after initialization
/// - No unsafe code required
///
/// Lock poisoning (from `.expect()`) is theoretically possible but practically
/// unreachable - the lock is only held for trivial Option assignments that cannot panic.
static OUTPUT_STATE: OnceLock<Mutex<OutputState>> = OnceLock::new();

#[derive(Default)]
struct OutputState {
    /// Path to the directive file (from WORKTRUNK_DIRECTIVE_FILE env var)
    /// If None, we're in interactive mode (no shell wrapper)
    directive_file: Option<PathBuf>,
    /// Buffered target directory for execute() in interactive mode
    target_dir: Option<PathBuf>,
}

/// Get or lazily initialize the global output state.
///
/// Reads `WORKTRUNK_DIRECTIVE_FILE` from environment on first access.
/// Empty or whitespace-only strings are treated as "not set" to handle edge cases.
fn get_state() -> &'static Mutex<OutputState> {
    OUTPUT_STATE.get_or_init(|| {
        let directive_file = std::env::var(DIRECTIVE_FILE_ENV_VAR)
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from);

        Mutex::new(OutputState {
            directive_file,
            target_dir: None,
        })
    })
}

/// Check if shell integration is active (directive file is set)
fn has_directive_file() -> bool {
    get_state()
        .lock()
        .expect("OUTPUT_STATE lock poisoned")
        .directive_file
        .is_some()
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

/// Emit a blank line for visual separation
pub fn blank() -> io::Result<()> {
    eprintln!();
    stderr().flush()
}

/// Write to stdout (pipeable output)
///
/// Used for primary command output: table rows, JSON, prompts, statuslines.
/// This is pipeable — `wt list | grep feature` works because stdout data
/// goes to stdout while progress/warnings go to stderr.
///
/// Example:
/// ```rust,ignore
/// output::stdout(json_string)?;
/// output::stdout(layout.format_header_line())?;
/// ```
pub fn stdout(content: impl Into<String>) -> io::Result<()> {
    println!("{}", content.into());
    io::stdout().flush()
}

/// Write a directive to the directive file (if set)
fn write_directive(directive: &str) -> io::Result<()> {
    // Copy path out of lock to avoid holding mutex during I/O
    let path = {
        let guard = get_state().lock().expect("OUTPUT_STATE lock poisoned");
        guard.directive_file.clone()
    };

    let Some(path) = path else {
        return Ok(());
    };

    let mut file = OpenOptions::new().append(true).open(&path)?;
    writeln!(file, "{}", directive)?;
    file.flush()
}

/// Request directory change (for shell integration)
///
/// If shell integration is active (WORKTRUNK_DIRECTIVE_FILE set), writes `cd` command to the file.
/// Also stores path for execute() to use as working directory.
pub fn change_directory(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref();
    let mut guard = get_state().lock().expect("OUTPUT_STATE lock poisoned");

    // Store for execute() to use
    guard.target_dir = Some(path.to_path_buf());

    // Write to directive file if set
    if guard.directive_file.is_some() {
        drop(guard); // Release lock before I/O

        let path_str = path.to_string_lossy();
        // Escape based on shell type. Both shell families use single-quoted strings
        // where contents are literal, but they escape embedded quotes differently:
        // - PowerShell: double the quote ('it''s')
        // - POSIX (bash/zsh/fish): end quote, escaped quote, start quote ('it'\''s')
        let is_powershell = std::env::var("WORKTRUNK_SHELL")
            .map(|v| v.eq_ignore_ascii_case("powershell"))
            .unwrap_or(false);
        let escaped = if is_powershell {
            path_str.replace('\'', "''")
        } else {
            path_str.replace('\'', "'\\''")
        };
        write_directive(&format!("cd '{}'", escaped))?;
    }

    Ok(())
}

/// Request command execution
///
/// In interactive mode (no directive file), executes the command directly (replacing process on Unix).
/// In shell integration mode, writes the command to the directive file.
pub fn execute(command: impl Into<String>) -> anyhow::Result<()> {
    let command = command.into();

    let (has_directive, target_dir) = {
        let guard = get_state().lock().expect("OUTPUT_STATE lock poisoned");
        (guard.directive_file.is_some(), guard.target_dir.clone())
    };

    if has_directive {
        // Write to directive file
        write_directive(&command)?;
        Ok(())
    } else {
        // Execute directly
        execute_command(command, target_dir.as_deref())
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
    if let Err(err) = execute_streaming(&command, exec_dir, false, None, true, false) {
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

/// Flush any buffered output (both stdout and stderr)
///
/// Call before interactive prompts to prevent stream interleaving.
pub fn flush() -> io::Result<()> {
    io::stdout().flush()?;
    io::stderr().flush()
}

/// Terminate command output
///
/// Resets ANSI state on stderr when shell integration is active.
/// In interactive mode (no shell wrapper), message formatting functions
/// already reset their own styles, so no global reset is needed.
pub fn terminate_output() -> io::Result<()> {
    if !has_directive_file() {
        return Ok(());
    }

    let mut stderr = io::stderr();

    // Reset ANSI state before returning to shell
    write!(stderr, "{}", anstyle::Reset)?;
    stderr.flush()
}

/// Check if we're in shell integration mode (directive file is set)
///
/// This is useful for handlers that need to know whether shell integration is active.
pub fn is_shell_integration_active() -> bool {
    has_directive_file()
}

/// Compute whether to show "@ path" in hook announcements.
///
/// Returns `Some(hooks_run_at)` when the user's shell is (or will be) somewhere
/// else, so the path annotation helps them understand where hooks executed.
/// Returns `None` when no annotation is needed because the user is (or will be)
/// at the hook location.
///
/// # Arguments
///
/// * `hooks_run_at` - The directory where hooks will execute
/// * `user_location` - Where the user's shell is (or will be) when they see the output
///
/// # Higher-level helpers
///
/// For most cases, use the convenience functions instead of computing `user_location` manually:
/// - [`pre_hook_display_path`] - for pre-hooks and manual `wt hook` invocations
/// - [`post_hook_display_path`] - for post-hooks (handles shell integration internally)
pub fn compute_hooks_display_path<'a>(
    hooks_run_at: &'a std::path::Path,
    user_location: &std::path::Path,
) -> Option<&'a std::path::Path> {
    if hooks_run_at == user_location {
        None
    } else {
        Some(hooks_run_at)
    }
}

/// Display path for pre-hooks and manual `wt hook` invocations.
///
/// Pre-hooks run while the user is at cwd, and no cd will happen after.
/// Manual `wt hook` commands also run at cwd with no cd.
///
/// Shows the path if hooks run somewhere other than cwd.
///
/// # Examples
///
/// ```ignore
/// // In pre-commit, pre-merge, pre-remove hooks:
/// run_hook_with_filter(..., pre_hook_display_path(ctx.worktree_path))?;
///
/// // In manual wt hook commands (even for post-* hook types):
/// run_hook_with_filter(..., pre_hook_display_path(ctx.worktree_path))?;
/// ```
pub fn pre_hook_display_path(hooks_run_at: &std::path::Path) -> Option<&std::path::Path> {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(_) => return None, // Can't determine cwd, don't show path
    };
    compute_hooks_display_path(hooks_run_at, &cwd)
}

/// Display path for post-hooks.
///
/// Post-hooks run after the operation completes. If shell integration is active,
/// the user will be cd'd to the destination, so no path needs to be shown.
/// Without shell integration, shows the path if user is elsewhere.
///
/// # Examples
///
/// ```ignore
/// // In post-create, post-start, post-switch hooks (creation path):
/// ctx.spawn_post_start_commands(&extra_vars, post_hook_display_path(&destination))?;
///
/// // After remove when switching to main:
/// ctx.spawn_post_switch_commands(&[], post_hook_display_path(main_path))?;
/// ```
pub fn post_hook_display_path(destination: &std::path::Path) -> Option<&std::path::Path> {
    if is_shell_integration_active() {
        None // Shell will cd user to destination
    } else {
        pre_hook_display_path(destination)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_compute_hooks_display_path_same_location() {
        let path = PathBuf::from("/repo/worktree");
        let result = compute_hooks_display_path(&path, &path);
        assert!(result.is_none(), "Should return None when paths match");
    }

    #[test]
    fn test_compute_hooks_display_path_different_location() {
        let hooks_run_at = PathBuf::from("/repo/feature");
        let user_location = PathBuf::from("/repo/main");
        let result = compute_hooks_display_path(&hooks_run_at, &user_location);
        assert_eq!(result, Some(hooks_run_at.as_path()));
    }

    #[test]
    fn test_pre_hook_display_path_at_cwd() {
        // When hooks run at cwd, no path annotation needed
        let cwd = std::env::current_dir().unwrap();
        let result = pre_hook_display_path(&cwd);
        assert!(result.is_none(), "Should return None when hooks run at cwd");
    }

    #[test]
    fn test_pre_hook_display_path_elsewhere() {
        // When hooks run elsewhere, show the path
        let elsewhere = PathBuf::from("/some/other/path");
        let result = pre_hook_display_path(&elsewhere);
        assert_eq!(
            result,
            Some(elsewhere.as_path()),
            "Should return path when hooks run elsewhere"
        );
    }

    #[test]
    fn test_post_hook_display_path_no_shell_integration() {
        // Without shell integration, post_hook_display_path behaves like pre_hook_display_path
        // (This test runs without WORKTRUNK_DIRECTIVE_FILE set)
        let elsewhere = PathBuf::from("/some/destination");
        let result = post_hook_display_path(&elsewhere);
        // If cwd != elsewhere, should return Some
        // If cwd == elsewhere (unlikely), should return None
        let cwd = std::env::current_dir().unwrap();
        if cwd == elsewhere {
            assert!(result.is_none());
        } else {
            assert_eq!(result, Some(elsewhere.as_path()));
        }
    }

    #[test]
    fn test_post_hook_display_path_at_cwd_no_shell_integration() {
        // Without shell integration, if destination == cwd, no path needed
        let cwd = std::env::current_dir().unwrap();
        let result = post_hook_display_path(&cwd);
        assert!(
            result.is_none(),
            "Should return None when destination is cwd (no shell integration)"
        );
    }

    #[test]
    fn test_lazy_init_does_not_panic() {
        // Verify lazy initialization doesn't panic.
        // State is lazily initialized on first access.
        let _ = has_directive_file();
    }

    #[test]
    fn test_spawned_thread_uses_correct_state() {
        use std::sync::mpsc;

        // Spawn a thread and verify it can access output without panicking.
        // State is lazily initialized and shared across threads.
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

    // Shell escaping tests

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
