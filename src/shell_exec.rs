//! Cross-platform shell execution
//!
//! Provides a unified interface for executing shell commands across platforms:
//! - Unix: Uses `/bin/sh -c`
//! - Windows: Prefers Git Bash if available, falls back to PowerShell
//!
//! This enables hooks and commands to use the same bash syntax on all platforms,
//! as long as Git for Windows is installed (which is nearly universal among
//! Windows developers).
//!
//! ## Windows Limitations
//!
//! When Git Bash is not available, PowerShell is used as a fallback with limitations:
//! - Hooks using bash syntax won't work
//! - No support for POSIX redirections like `{ cmd; } 1>&2`
//! - Different string escaping rules for JSON piping

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use crate::sync::Semaphore;

/// Semaphore to limit concurrent command execution.
/// Prevents resource exhaustion when spawning many parallel git commands.
static CMD_SEMAPHORE: OnceLock<Semaphore> = OnceLock::new();

/// Default concurrent external commands. Tuned to avoid hitting OS limits
/// (file descriptors, process limits) while maintaining good parallelism.
const DEFAULT_CONCURRENT_COMMANDS: usize = 32;

fn max_concurrent_commands() -> usize {
    std::env::var("WORKTRUNK_MAX_CONCURRENT_COMMANDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_CONCURRENT_COMMANDS)
}

fn get_semaphore() -> &'static Semaphore {
    CMD_SEMAPHORE.get_or_init(|| Semaphore::new(max_concurrent_commands()))
}

/// Cached shell configuration for the current platform
static SHELL_CONFIG: OnceLock<ShellConfig> = OnceLock::new();

/// Shell configuration for command execution
#[derive(Debug, Clone)]
pub struct ShellConfig {
    /// Path to the shell executable
    pub executable: PathBuf,
    /// Arguments to pass before the command (e.g., ["-c"] for sh, ["/C"] for cmd)
    pub args: Vec<String>,
    /// Whether this is a POSIX-compatible shell (bash/sh)
    pub is_posix: bool,
    /// Human-readable name for error messages
    pub name: String,
}

impl ShellConfig {
    /// Get the shell configuration for the current platform
    ///
    /// On Unix, this always returns sh.
    /// On Windows, this prefers Git Bash if available, then falls back to PowerShell.
    pub fn get() -> &'static ShellConfig {
        SHELL_CONFIG.get_or_init(detect_shell)
    }

    /// Create a Command configured for shell execution
    ///
    /// The command string will be passed to the shell for interpretation.
    pub fn command(&self, shell_command: &str) -> Command {
        let mut cmd = Command::new(&self.executable);
        for arg in &self.args {
            cmd.arg(arg);
        }
        cmd.arg(shell_command);
        cmd
    }

    /// Check if this shell supports POSIX syntax (bash, sh, zsh, etc.)
    ///
    /// When true, commands can use POSIX features like:
    /// - `{ cmd; } 1>&2` for stdout redirection
    /// - `printf '%s' ... | cmd` for stdin piping
    /// - `nohup ... &` for background execution
    pub fn is_posix(&self) -> bool {
        self.is_posix
    }

    /// Check if running on Windows without Git Bash (using PowerShell fallback)
    ///
    /// Returns true when hooks using bash syntax won't work properly.
    /// Used to show warnings to users about limited functionality.
    #[cfg(windows)]
    pub fn is_windows_without_git_bash(&self) -> bool {
        !self.is_posix
    }

    #[cfg(not(windows))]
    pub fn is_windows_without_git_bash(&self) -> bool {
        false
    }
}

/// Detect the best available shell for the current platform
fn detect_shell() -> ShellConfig {
    #[cfg(unix)]
    {
        ShellConfig {
            executable: PathBuf::from("sh"),
            args: vec!["-c".to_string()],
            is_posix: true,
            name: "sh".to_string(),
        }
    }

    #[cfg(windows)]
    {
        detect_windows_shell()
    }
}

/// Detect the best available shell on Windows
///
/// Priority order:
/// 1. Git Bash (if Git for Windows is installed)
/// 2. PowerShell (fallback, with warnings about syntax differences)
#[cfg(windows)]
fn detect_windows_shell() -> ShellConfig {
    if let Some(bash_path) = find_git_bash() {
        return ShellConfig {
            executable: bash_path,
            args: vec!["-c".to_string()],
            is_posix: true,
            name: "Git Bash".to_string(),
        };
    }

    // Fall back to PowerShell
    ShellConfig {
        executable: PathBuf::from("powershell.exe"),
        args: vec!["-NoProfile".to_string(), "-Command".to_string()],
        is_posix: false,
        name: "PowerShell".to_string(),
    }
}

/// Find Git Bash executable on Windows
///
/// Detection order (designed to always return absolute paths and avoid WSL):
/// 1. `git.exe` in PATH - derive bash.exe location from Git installation
/// 2. Standard Git for Windows and MSYS2 installation paths
///
/// We explicitly avoid `which bash` because on systems with WSL installed,
/// `C:\Windows\System32\bash.exe` (WSL launcher) often comes before Git Bash
/// in PATH, even when MSYSTEM is set.
#[cfg(windows)]
fn find_git_bash() -> Option<PathBuf> {
    // Primary method: Find Git installation via `git.exe` in PATH
    // This is the most reliable method and always returns an absolute path.
    // Works on CI systems like GitHub Actions where Git might be in non-standard locations.
    if let Ok(git_path) = which::which("git") {
        // git.exe is typically at Git/cmd/git.exe or Git/bin/git.exe
        // bash.exe is at Git/bin/bash.exe or Git/usr/bin/bash.exe
        if let Some(git_dir) = git_path.parent().and_then(|p| p.parent()) {
            // Try bin/bash.exe first (most common)
            let bash_path = git_dir.join("bin").join("bash.exe");
            if bash_path.exists() {
                return Some(bash_path);
            }
            // Also try usr/bin/bash.exe (some Git for Windows layouts)
            let bash_path = git_dir.join("usr").join("bin").join("bash.exe");
            if bash_path.exists() {
                return Some(bash_path);
            }
        }
    }

    // Fallback: Check standard installation paths for bash.exe
    // (Git for Windows and MSYS2 both provide POSIX-compatible bash)
    let bash_paths = [
        // Git for Windows
        r"C:\Program Files\Git\bin\bash.exe",
        r"C:\Program Files\Git\usr\bin\bash.exe",
        r"C:\Program Files (x86)\Git\bin\bash.exe",
        r"C:\Git\bin\bash.exe",
        // MSYS2 standalone (popular alternative to Git Bash)
        r"C:\msys64\usr\bin\bash.exe",
        r"C:\msys32\usr\bin\bash.exe",
    ];

    for path in &bash_paths {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }

    None
}

/// Environment variable removed from spawned subprocesses for security.
/// Hooks and other child processes should not be able to write to the directive file.
pub const DIRECTIVE_FILE_ENV_VAR: &str = "WORKTRUNK_DIRECTIVE_FILE";

// ============================================================================
// Thread-Local Command Timeout
// ============================================================================

use std::cell::Cell;
use std::time::Duration;

thread_local! {
    /// Thread-local command timeout. When set, all commands executed via `run()` on this
    /// thread will be killed if they exceed this duration.
    ///
    /// This is used by `wt select` to make the TUI responsive faster on large repos.
    /// The timeout is set per-worker-thread in Rayon's thread pool.
    static COMMAND_TIMEOUT: Cell<Option<Duration>> = const { Cell::new(None) };
}

/// Set the command timeout for the current thread.
///
/// When set, all commands executed via `run()` on this thread will be killed if they
/// exceed the specified duration. Set to `None` to disable timeout.
///
/// This is typically called at the start of a Rayon worker task to apply timeout
/// to all git operations within that task.
pub fn set_command_timeout(timeout: Option<Duration>) {
    COMMAND_TIMEOUT.with(|t| t.set(timeout));
}

/// Execute a command with timing and debug logging.
///
/// This is the **only** way to run external commands in worktrunk. All command execution
/// must go through this function to ensure consistent logging and tracing.
///
/// If a thread-local timeout is set via `set_command_timeout()`, the command will be
/// killed if it exceeds that duration.
///
/// The `WORKTRUNK_DIRECTIVE_FILE` environment variable is automatically removed from spawned
/// processes to prevent hooks from discovering and writing to the directive file.
///
/// ```text
/// $ git status [worktree-name]           # with context
/// $ gh pr list                           # without context
/// [wt-trace] context=worktree cmd="..." dur=12.3ms ok=true
/// ```
///
/// The `context` parameter is typically the worktree name for git commands, or `None` for
/// standalone CLI tools like `gh` and `glab`.
pub fn run(cmd: &mut Command, context: Option<&str>) -> std::io::Result<std::process::Output> {
    let timeout = COMMAND_TIMEOUT.with(|t| t.get());
    run_with_timeout(cmd, context, timeout)
}

/// Execute a command with an optional timeout.
///
/// Like `run()`, but allows specifying a timeout. If the command doesn't complete within
/// the timeout, it is killed and an error is returned.
///
/// Returns `std::io::ErrorKind::TimedOut` if the command times out.
pub fn run_with_timeout(
    cmd: &mut Command,
    context: Option<&str>,
    timeout: Option<std::time::Duration>,
) -> std::io::Result<std::process::Output> {
    use std::time::Instant;

    // Remove WORKTRUNK_DIRECTIVE_FILE to prevent hooks from writing to it
    cmd.env_remove(DIRECTIVE_FILE_ENV_VAR);

    // Build command string for logging
    let program = cmd.get_program().to_string_lossy();
    let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy()).collect();
    let cmd_str = if args.is_empty() {
        program.to_string()
    } else {
        format!("{} {}", program, args.join(" "))
    };

    // Log command with optional context
    match context {
        Some(ctx) => log::debug!("$ {} [{}]", cmd_str, ctx),
        None => log::debug!("$ {}", cmd_str),
    }

    // Acquire semaphore to limit concurrent commands (prevents resource exhaustion)
    // RAII guard ensures release even on panic
    let _guard = get_semaphore().acquire();

    let t0 = Instant::now();

    // Execute with or without timeout
    let result = match timeout {
        None => cmd.output(),
        Some(timeout_duration) => run_with_timeout_impl(cmd, timeout_duration),
    };

    let duration_ms = t0.elapsed().as_secs_f64() * 1000.0;

    // Log trace with timing
    match (&result, context) {
        (Ok(output), Some(ctx)) => {
            log::debug!(
                "[wt-trace] context={} cmd=\"{}\" dur={:.1}ms ok={}",
                ctx,
                cmd_str,
                duration_ms,
                output.status.success()
            );
        }
        (Ok(output), None) => {
            log::debug!(
                "[wt-trace] cmd=\"{}\" dur={:.1}ms ok={}",
                cmd_str,
                duration_ms,
                output.status.success()
            );
        }
        (Err(e), Some(ctx)) => {
            log::debug!(
                "[wt-trace] context={} cmd=\"{}\" dur={:.1}ms err=\"{}\"",
                ctx,
                cmd_str,
                duration_ms,
                e
            );
        }
        (Err(e), None) => {
            log::debug!(
                "[wt-trace] cmd=\"{}\" dur={:.1}ms err=\"{}\"",
                cmd_str,
                duration_ms,
                e
            );
        }
    }

    result
}

/// Implementation of timeout-based command execution.
///
/// Spawns the process, captures stdout/stderr in background threads, and waits with timeout.
/// If the timeout is exceeded, kills the process and returns TimedOut error.
fn run_with_timeout_impl(
    cmd: &mut Command,
    timeout: std::time::Duration,
) -> std::io::Result<std::process::Output> {
    use std::io::{ErrorKind, Read};
    use std::process::Stdio;
    use std::time::Instant;

    // Spawn process with piped stdout/stderr
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    // Take ownership of stdout/stderr handles
    let mut stdout_handle = child.stdout.take();
    let mut stderr_handle = child.stderr.take();

    // Spawn threads to read stdout/stderr in parallel
    // This prevents deadlock when buffers fill up
    let stdout_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(ref mut handle) = stdout_handle {
            let _ = handle.read_to_end(&mut buf);
        }
        buf
    });

    let stderr_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(ref mut handle) = stderr_handle {
            let _ = handle.read_to_end(&mut buf);
        }
        buf
    });

    // Wait for process with timeout
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None => {
                if Instant::now() >= deadline {
                    // Timeout exceeded - kill the process (SIGKILL on Unix)
                    let _ = child.kill();
                    let _ = child.wait(); // Reap the process

                    // Wait for reader threads to complete (they'll see EOF after kill)
                    // This prevents thread leaks
                    let _ = stdout_thread.join();
                    let _ = stderr_thread.join();

                    return Err(std::io::Error::new(
                        ErrorKind::TimedOut,
                        "command timed out",
                    ));
                }
                // Sleep briefly before checking again
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    };

    // Collect output from threads
    let stdout = stdout_thread.join().unwrap_or_default();
    let stderr = stderr_thread.join().unwrap_or_default();

    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

// ============================================================================
// Streaming command execution with signal handling
// ============================================================================

#[cfg(unix)]
fn process_group_alive(pgid: i32) -> bool {
    match nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pgid), None) {
        Ok(_) => true,
        Err(nix::errno::Errno::ESRCH) => false,
        Err(_) => true,
    }
}

#[cfg(unix)]
fn wait_for_exit(pgid: i32, grace: std::time::Duration) -> bool {
    std::thread::sleep(grace);
    !process_group_alive(pgid)
}

#[cfg(unix)]
fn forward_signal_with_escalation(pgid: i32, sig: i32) {
    let pgid = nix::unistd::Pid::from_raw(pgid);
    let initial_signal = match sig {
        signal_hook::consts::SIGINT => nix::sys::signal::Signal::SIGINT,
        signal_hook::consts::SIGTERM => nix::sys::signal::Signal::SIGTERM,
        _ => return,
    };

    let _ = nix::sys::signal::killpg(pgid, initial_signal);

    let grace = std::time::Duration::from_millis(200);
    match sig {
        signal_hook::consts::SIGINT => {
            if !wait_for_exit(pgid.as_raw(), grace) {
                let _ = nix::sys::signal::killpg(pgid, nix::sys::signal::Signal::SIGTERM);
                if !wait_for_exit(pgid.as_raw(), grace) {
                    let _ = nix::sys::signal::killpg(pgid, nix::sys::signal::Signal::SIGKILL);
                }
            }
        }
        signal_hook::consts::SIGTERM => {
            if !wait_for_exit(pgid.as_raw(), grace) {
                let _ = nix::sys::signal::killpg(pgid, nix::sys::signal::Signal::SIGKILL);
            }
        }
        _ => {}
    }
}

/// Execute a command with streaming output
///
/// Uses Stdio::inherit for stderr to preserve TTY behavior - this ensures commands like cargo
/// detect they're connected to a terminal and don't buffer their output.
///
/// If `redirect_stdout_to_stderr` is true, redirects child stdout to our stderr at the OS level
/// (via `Stdio::from(io::stderr())`). This ensures deterministic output ordering (all child output
/// flows through stderr). Per CLAUDE.md: child process output goes to stderr, worktrunk output
/// goes to stdout.
///
/// If `stdin_content` is provided, it will be piped to the command's stdin (used for hook context JSON).
///
/// If `inherit_stdin` is true and `stdin_content` is None, stdin is inherited from the parent process,
/// enabling interactive programs (like `claude`, `vim`, or `python -i`) to read user input.
/// If false and `stdin_content` is None, stdin is set to null (appropriate for non-interactive hooks).
///
/// Returns error if command exits with non-zero status.
///
/// ## Cross-Platform Shell Execution
///
/// Uses the platform's preferred shell via `ShellConfig`:
/// - Unix: `/bin/sh -c`
/// - Windows: Git Bash if available, PowerShell fallback
///
/// ## Signal Handling (Unix)
///
/// When `forward_signals` is true, the child is spawned in its own process group and
/// SIGINT/SIGTERM received by the parent are forwarded to that group so we can abort
/// the entire command tree without shell-wrapping. If the process group does not exit
/// promptly, we escalate to SIGTERM/SIGKILL (SIGINT path) or SIGKILL (SIGTERM path).
/// We still return exit code 128 + signal number (e.g., 130 for SIGINT) to match Unix conventions.
pub fn execute_streaming(
    command: &str,
    working_dir: &std::path::Path,
    redirect_stdout_to_stderr: bool,
    stdin_content: Option<&str>,
    inherit_stdin: bool,
    forward_signals: bool,
) -> anyhow::Result<()> {
    use crate::git::{GitError, WorktrunkError};
    use std::io::Write;
    #[cfg(unix)]
    use {
        signal_hook::consts::{SIGINT, SIGTERM},
        signal_hook::iterator::Signals,
        std::os::unix::process::CommandExt,
    };

    let shell = ShellConfig::get();
    #[cfg(not(unix))]
    let _ = forward_signals;

    // Determine stdout handling based on redirect flag
    // When redirecting, use Stdio::from(stderr) to redirect child stdout to our stderr at OS level.
    // This keeps stdout reserved for data output while hook output goes to stderr.
    // Previously used shell-level `{ cmd } 1>&2` wrapping, but OS-level redirect is simpler
    // and may improve signal handling by removing an extra shell process layer.
    let stdout_mode = if redirect_stdout_to_stderr {
        std::process::Stdio::from(std::io::stderr())
    } else {
        std::process::Stdio::inherit()
    };

    let stdin_mode = if stdin_content.is_some() {
        std::process::Stdio::piped()
    } else if inherit_stdin {
        std::process::Stdio::inherit()
    } else {
        std::process::Stdio::null()
    };

    #[cfg(unix)]
    let mut signals = if forward_signals {
        Some(Signals::new([SIGINT, SIGTERM])?)
    } else {
        None
    };

    let mut cmd = shell.command(command);
    #[cfg(unix)]
    if forward_signals {
        // Isolate the child in its own process group so we can signal the whole tree.
        cmd.process_group(0);
    }
    let mut child = cmd
        .current_dir(working_dir)
        .stdin(stdin_mode)
        .stdout(stdout_mode)
        .stderr(std::process::Stdio::inherit()) // Preserve TTY for errors
        // Prevent vergen "overridden" warning in nested cargo builds when run via `cargo run`.
        // Add more VERGEN_* variables here if we expand build.rs and hit similar issues.
        .env_remove("VERGEN_GIT_DESCRIBE")
        // Prevent hooks from writing to the directive file
        .env_remove(DIRECTIVE_FILE_ENV_VAR)
        .spawn()
        .map_err(|e| {
            anyhow::Error::from(GitError::Other {
                message: format!("Failed to execute command with {}: {}", shell.name, e),
            })
        })?;

    // Write stdin content if provided (used for hook context JSON)
    // We ignore write errors here because:
    // 1. The child may have already exited (broken pipe)
    // 2. Hooks that don't read stdin will still work
    // 3. Hooks that need stdin will fail with their own error message
    if let Some(content) = stdin_content
        && let Some(mut stdin) = child.stdin.take()
    {
        // Write and close stdin immediately so the child doesn't block waiting for more input
        let _ = stdin.write_all(content.as_bytes());
        // stdin is dropped here, closing the pipe
    }

    #[cfg(unix)]
    let (status, seen_signal) = if forward_signals {
        let child_pgid = child.id() as i32;
        let mut seen_signal: Option<i32> = None;
        loop {
            if let Some(status) = child.try_wait().map_err(|e| {
                anyhow::Error::from(GitError::Other {
                    message: format!("Failed to wait for command: {}", e),
                })
            })? {
                break (status, seen_signal);
            }
            if let Some(signals) = signals.as_mut() {
                for sig in signals.pending() {
                    if seen_signal.is_none() {
                        seen_signal = Some(sig);
                        forward_signal_with_escalation(child_pgid, sig);
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    } else {
        let status = child.wait().map_err(|e| {
            anyhow::Error::from(GitError::Other {
                message: format!("Failed to wait for command: {}", e),
            })
        })?;
        (status, None)
    };

    #[cfg(not(unix))]
    let status = child.wait().map_err(|e| {
        anyhow::Error::from(GitError::Other {
            message: format!("Failed to wait for command: {}", e),
        })
    })?;

    #[cfg(unix)]
    if let Some(sig) = seen_signal {
        return Err(WorktrunkError::ChildProcessExited {
            code: 128 + sig,
            message: format!("terminated by signal {}", sig),
        }
        .into());
    }

    // Check if child was killed by a signal (Unix only)
    // This handles Ctrl-C: when SIGINT is sent, the child receives it and terminates,
    // and we propagate the signal exit code (128 + signal number, e.g., 130 for SIGINT)
    #[cfg(unix)]
    if let Some(sig) = std::os::unix::process::ExitStatusExt::signal(&status) {
        return Err(WorktrunkError::ChildProcessExited {
            code: 128 + sig,
            message: format!("terminated by signal {}", sig),
        }
        .into());
    }

    if !status.success() {
        // Get the exit code if available (None means terminated by signal on some platforms)
        let code = status.code().unwrap_or(1);
        return Err(WorktrunkError::ChildProcessExited {
            code,
            message: format!("exit status: {}", code),
        }
        .into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_config_is_available() {
        let config = ShellConfig::get();
        assert!(!config.name.is_empty());
        assert!(!config.args.is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn test_unix_shell_is_posix() {
        let config = ShellConfig::get();
        assert!(config.is_posix);
        assert_eq!(config.name, "sh");
    }

    #[test]
    fn test_command_creation() {
        let config = ShellConfig::get();
        let cmd = config.command("echo hello");
        // Just verify it doesn't panic
        let _ = format!("{:?}", cmd);
    }

    #[test]
    fn test_shell_command_execution() {
        let config = ShellConfig::get();
        let output = config
            .command("echo hello")
            .output()
            .expect("Failed to execute shell command");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "echo should succeed. Shell: {} ({:?}), exit: {:?}, stdout: '{}', stderr: '{}'",
            config.name,
            config.executable,
            output.status.code(),
            stdout.trim(),
            stderr.trim()
        );
        assert!(
            stdout.contains("hello"),
            "stdout should contain 'hello', got: '{}'",
            stdout.trim()
        );
    }

    #[test]
    #[cfg(windows)]
    fn test_windows_shell_detection() {
        let config = ShellConfig::get();
        // On Windows CI, Git is installed, so we should have Git Bash
        // If this fails on a system without Git, PowerShell fallback should work
        assert!(
            config.name == "Git Bash" || config.name == "PowerShell",
            "Expected 'Git Bash' or 'PowerShell', got '{}'",
            config.name
        );
    }

    #[test]
    #[cfg(windows)]
    fn test_windows_git_bash_has_posix_syntax() {
        let config = ShellConfig::get();
        if config.name == "Git Bash" {
            assert!(config.is_posix, "Git Bash should support POSIX syntax");
            assert!(
                config.args.contains(&"-c".to_string()),
                "Git Bash should use -c flag"
            );
        }
    }

    #[test]
    #[cfg(windows)]
    fn test_windows_powershell_fallback_not_posix() {
        let config = ShellConfig::get();
        if config.name == "PowerShell" {
            assert!(!config.is_posix, "PowerShell should not be marked as POSIX");
            assert!(
                config.args.contains(&"-Command".to_string()),
                "PowerShell should use -Command flag"
            );
        }
    }

    #[test]
    #[cfg(windows)]
    fn test_windows_echo_command() {
        // Test that echo works regardless of which shell we detected
        let config = ShellConfig::get();
        let output = config
            .command("echo test_output")
            .output()
            .expect("Failed to execute echo");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "echo should succeed. Shell: {} ({:?}), exit: {:?}, stdout: '{}', stderr: '{}'",
            config.name,
            config.executable,
            output.status.code(),
            stdout.trim(),
            stderr.trim()
        );
        assert!(
            stdout.contains("test_output"),
            "stdout should contain 'test_output', got: '{}'",
            stdout.trim()
        );
    }

    #[test]
    #[cfg(windows)]
    fn test_windows_posix_redirection_with_git_bash() {
        let config = ShellConfig::get();
        if config.is_posix() {
            // Test POSIX-style redirection: stdout redirected to stderr
            let output = config
                .command("echo redirected 1>&2")
                .output()
                .expect("Failed to execute redirection test");

            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                output.status.success(),
                "redirection command should succeed. Shell: {} ({:?}), exit: {:?}, stdout: '{}', stderr: '{}'",
                config.name,
                config.executable,
                output.status.code(),
                stdout.trim(),
                stderr.trim()
            );
            assert!(
                stderr.contains("redirected"),
                "stderr should contain 'redirected' (stdout redirected to stderr), got: '{}'",
                stderr.trim()
            );
        }
    }

    #[test]
    fn test_shell_config_debug() {
        let config = ShellConfig::get();
        let debug = format!("{:?}", config);
        assert!(debug.contains("ShellConfig"));
        assert!(debug.contains(&config.name));
    }

    #[test]
    fn test_shell_config_clone() {
        let config = ShellConfig::get();
        let cloned = config.clone();
        assert_eq!(config.name, cloned.name);
        assert_eq!(config.is_posix, cloned.is_posix);
        assert_eq!(config.args, cloned.args);
    }

    #[test]
    fn test_shell_is_posix_method() {
        let config = ShellConfig::get();
        // is_posix method should match the field
        assert_eq!(config.is_posix(), config.is_posix);
    }

    #[test]
    #[cfg(not(windows))]
    fn test_unix_is_not_windows_without_git_bash() {
        let config = ShellConfig::get();
        assert!(!config.is_windows_without_git_bash());
    }

    // ========================================================================
    // Timeout tests
    // ========================================================================

    #[test]
    fn test_run_with_timeout_completes_fast_command() {
        let mut cmd = Command::new("echo");
        cmd.arg("hello");
        let result = run_with_timeout(&mut cmd, None, Some(Duration::from_secs(5)));
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("hello"));
    }

    #[test]
    #[cfg(unix)]
    fn test_run_with_timeout_kills_slow_command() {
        let mut cmd = Command::new("sleep");
        cmd.arg("10");
        let result = run_with_timeout(&mut cmd, None, Some(Duration::from_millis(50)));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
    }

    #[test]
    fn test_run_with_no_timeout_completes() {
        let mut cmd = Command::new("echo");
        cmd.arg("no timeout");
        let result = run_with_timeout(&mut cmd, None, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_thread_local_timeout_setting() {
        // Initially no timeout
        let initial = COMMAND_TIMEOUT.with(|t| t.get());
        assert!(initial.is_none() || initial == Some(Duration::from_millis(500)));

        // Set a timeout
        set_command_timeout(Some(Duration::from_millis(100)));
        let after_set = COMMAND_TIMEOUT.with(|t| t.get());
        assert_eq!(after_set, Some(Duration::from_millis(100)));

        // Clear the timeout
        set_command_timeout(None);
        let after_clear = COMMAND_TIMEOUT.with(|t| t.get());
        assert!(after_clear.is_none());
    }

    #[test]
    fn test_run_uses_thread_local_timeout() {
        // Set no timeout (ensure fast completion)
        set_command_timeout(None);

        let mut cmd = Command::new("echo");
        cmd.arg("thread local test");
        let result = run(&mut cmd, None);
        assert!(result.is_ok());

        // Clean up
        set_command_timeout(None);
    }
}
