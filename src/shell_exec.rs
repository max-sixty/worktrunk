//! Cross-platform shell execution
//!
//! Provides a unified interface for executing shell commands across platforms:
//! - Unix: Uses `sh -c` (resolved via PATH)
//! - Windows: Uses Git Bash (requires Git for Windows)
//!
//! This enables hooks and commands to use the same bash syntax on all platforms.
//! On Windows, Git for Windows must be installed — this is nearly universal among
//! Windows developers since git itself is required.

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;
use std::time::Instant;

use crate::sync::Semaphore;

/// Semaphore to limit concurrent command execution.
/// Prevents resource exhaustion when spawning many parallel git commands.
static CMD_SEMAPHORE: OnceLock<Semaphore> = OnceLock::new();

/// Monotonic epoch for trace timestamps.
///
/// Using `Instant` instead of `SystemTime` ensures monotonic timestamps even if
/// the system clock steps backward. All trace timestamps are relative to this epoch.
static TRACE_EPOCH: OnceLock<Instant> = OnceLock::new();

fn trace_epoch() -> &'static Instant {
    TRACE_EPOCH.get_or_init(Instant::now)
}

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
    /// On Unix, returns sh. On Windows, returns Git Bash (panics if not installed).
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

/// Detect Git Bash on Windows
///
/// Panics if Git for Windows is not installed, since hooks require bash syntax.
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

    panic!(
        "Git for Windows is required but not found.\n\
         Install from https://git-scm.com/download/win"
    );
}

/// Find Git Bash executable on Windows
///
/// Finds `git.exe` in PATH and derives the bash.exe location from the Git installation.
/// We avoid `which bash` because on systems with WSL, `C:\Windows\System32\bash.exe`
/// (WSL launcher) often comes before Git Bash in PATH.
#[cfg(windows)]
fn find_git_bash() -> Option<PathBuf> {
    // Primary: find git in PATH and derive bash location
    if let Ok(git_path) = which::which("git") {
        // git.exe is typically at Git/cmd/git.exe or Git/bin/git.exe
        // bash.exe is at Git/bin/bash.exe or Git/usr/bin/bash.exe
        if let Some(git_dir) = git_path.parent().and_then(|p| p.parent()) {
            let bash_path = git_dir.join("bin").join("bash.exe");
            if bash_path.exists() {
                return Some(bash_path);
            }
            let bash_path = git_dir.join("usr").join("bin").join("bash.exe");
            if bash_path.exists() {
                return Some(bash_path);
            }
        }
    }

    // Fallback: standard Git for Windows path (needed on some CI environments
    // where `which` doesn't find git even though it's installed)
    let bash_path = PathBuf::from(r"C:\Program Files\Git\bin\bash.exe");
    if bash_path.exists() {
        return Some(bash_path);
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

/// Emit an instant trace event (a milestone marker with no duration).
///
/// Instant events appear as vertical lines in Chrome Trace Format visualization tools
/// (chrome://tracing, Perfetto). Use them to mark significant moments in execution:
///
/// ```text
/// [wt-trace] ts=1234567890 tid=3 event="Showed skeleton"
/// ```
///
/// # Example
///
/// ```ignore
/// use worktrunk::shell_exec::trace_instant;
///
/// // Mark when the skeleton UI was displayed
/// trace_instant("Showed skeleton");
///
/// // Or with more context
/// trace_instant("Progressive render: headers complete");
/// ```
pub fn trace_instant(event: &str) {
    let ts = Instant::now().duration_since(*trace_epoch()).as_micros() as u64;
    let tid = thread_id_number();

    log::debug!("[wt-trace] ts={} tid={} event=\"{}\"", ts, tid, event);
}

/// Extract numeric thread ID from ThreadId's debug format.
/// ThreadId debug format is "ThreadId(N)" where N is the numeric ID.
fn thread_id_number() -> u64 {
    let thread_id = std::thread::current().id();
    let debug_str = format!("{:?}", thread_id);
    debug_str
        .strip_prefix("ThreadId(")
        .and_then(|s| s.strip_suffix(")"))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
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
// Builder-style command execution
// ============================================================================

/// Builder for executing commands with logging, tracing, and optional stdin.
///
/// Provides the same benefits as `run()` (logging, semaphore, tracing) with a
/// cleaner interface that supports stdin piping.
///
/// # Examples
///
/// Basic usage:
/// ```ignore
/// let output = Cmd::new("git")
///     .args(["status", "--porcelain"])
///     .current_dir(&repo_path)
///     .context("my-worktree")
///     .run()?;
/// ```
///
/// With stdin:
/// ```ignore
/// let output = Cmd::new("git")
///     .args(["diff-tree", "--stdin", "--numstat"])
///     .stdin(hashes.join("\n"))
///     .run()?;
/// ```
pub struct Cmd {
    program: String,
    args: Vec<String>,
    current_dir: Option<std::path::PathBuf>,
    context: Option<String>,
    stdin_data: Option<Vec<u8>>,
    timeout: Option<std::time::Duration>,
    envs: Vec<(String, String)>,
    env_removes: Vec<String>,
}

impl Cmd {
    /// Create a new command builder for the given program.
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            current_dir: None,
            context: None,
            stdin_data: None,
            timeout: None,
            envs: Vec::new(),
            env_removes: Vec::new(),
        }
    }

    /// Add a single argument.
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Add multiple arguments.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Set the working directory for the command.
    pub fn current_dir(mut self, dir: impl Into<std::path::PathBuf>) -> Self {
        self.current_dir = Some(dir.into());
        self
    }

    /// Set the logging context (typically worktree name for git commands).
    pub fn context(mut self, ctx: impl Into<String>) -> Self {
        self.context = Some(ctx.into());
        self
    }

    /// Set data to write to the command's stdin.
    pub fn stdin(mut self, data: impl Into<Vec<u8>>) -> Self {
        self.stdin_data = Some(data.into());
        self
    }

    /// Set a timeout for command execution.
    pub fn timeout(mut self, duration: std::time::Duration) -> Self {
        self.timeout = Some(duration);
        self
    }

    /// Set an environment variable.
    pub fn env(mut self, key: impl Into<String>, val: impl Into<String>) -> Self {
        self.envs.push((key.into(), val.into()));
        self
    }

    /// Remove an environment variable.
    pub fn env_remove(mut self, key: impl Into<String>) -> Self {
        self.env_removes.push(key.into());
        self
    }

    /// Execute the command and return its output.
    ///
    /// Provides logging, semaphore limiting, and tracing like `run()`.
    pub fn run(self) -> std::io::Result<std::process::Output> {
        use std::io::Write;
        use std::process::Stdio;

        // Build command string for logging
        let cmd_str = if self.args.is_empty() {
            self.program.clone()
        } else {
            format!("{} {}", self.program, self.args.join(" "))
        };

        // Log command with optional context
        match &self.context {
            Some(ctx) => log::debug!("$ {} [{}]", cmd_str, ctx),
            None => log::debug!("$ {}", cmd_str),
        }

        // Acquire semaphore to limit concurrent commands
        let _guard = get_semaphore().acquire();

        // Capture timing for tracing
        let t0 = Instant::now();
        let ts = t0.duration_since(*trace_epoch()).as_micros() as u64;
        let tid = thread_id_number();

        // Build the Command
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args);
        cmd.env_remove(DIRECTIVE_FILE_ENV_VAR);

        if let Some(ref dir) = self.current_dir {
            cmd.current_dir(dir);
        }

        for (key, val) in &self.envs {
            cmd.env(key, val);
        }
        for key in &self.env_removes {
            cmd.env_remove(key);
        }

        // Determine effective timeout: explicit > thread-local > none
        let effective_timeout = self.timeout.or_else(|| COMMAND_TIMEOUT.with(|t| t.get()));

        // Execute with or without stdin
        let result = if let Some(stdin_data) = self.stdin_data {
            // Stdin piping requires spawn/write/wait
            // Note: stdin path doesn't support timeout (would need async I/O)
            cmd.stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());

            let mut child = cmd.spawn()?;

            // Write stdin data (ignore BrokenPipe - some commands exit early)
            if let Some(mut stdin) = child.stdin.take()
                && let Err(e) = stdin.write_all(&stdin_data)
                && e.kind() != std::io::ErrorKind::BrokenPipe
            {
                return Err(e);
            }

            child.wait_with_output()
        } else if let Some(timeout_duration) = effective_timeout {
            // Timeout handling uses the existing impl
            run_with_timeout_impl(&mut cmd, timeout_duration)
        } else {
            // Simple case: just run and capture output
            cmd.output()
        };

        // Log trace
        let dur_us = t0.elapsed().as_micros() as u64;
        match (&result, &self.context) {
            (Ok(output), Some(ctx)) => {
                log::debug!(
                    "[wt-trace] ts={} tid={} context={} cmd=\"{}\" dur_us={} ok={}",
                    ts,
                    tid,
                    ctx,
                    cmd_str,
                    dur_us,
                    output.status.success()
                );
            }
            (Ok(output), None) => {
                log::debug!(
                    "[wt-trace] ts={} tid={} cmd=\"{}\" dur_us={} ok={}",
                    ts,
                    tid,
                    cmd_str,
                    dur_us,
                    output.status.success()
                );
            }
            (Err(e), Some(ctx)) => {
                log::debug!(
                    "[wt-trace] ts={} tid={} context={} cmd=\"{}\" dur_us={} err=\"{}\"",
                    ts,
                    tid,
                    ctx,
                    cmd_str,
                    dur_us,
                    e
                );
            }
            (Err(e), None) => {
                log::debug!(
                    "[wt-trace] ts={} tid={} cmd=\"{}\" dur_us={} err=\"{}\"",
                    ts,
                    tid,
                    cmd_str,
                    dur_us,
                    e
                );
            }
        }

        result
    }
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
/// - Windows: Git Bash (requires Git for Windows)
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
    fn test_windows_uses_git_bash() {
        let config = ShellConfig::get();
        assert_eq!(config.name, "Git Bash");
        assert!(config.is_posix, "Git Bash should support POSIX syntax");
        assert!(
            config.args.contains(&"-c".to_string()),
            "Git Bash should use -c flag"
        );
    }

    #[test]
    #[cfg(windows)]
    fn test_windows_echo_command() {
        let config = ShellConfig::get();
        let output = config
            .command("echo test_output")
            .output()
            .expect("Failed to execute echo");

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success());
        assert!(
            stdout.contains("test_output"),
            "stdout should contain 'test_output', got: '{}'",
            stdout.trim()
        );
    }

    #[test]
    #[cfg(windows)]
    fn test_windows_posix_redirection() {
        let config = ShellConfig::get();
        // Test POSIX-style redirection: stdout redirected to stderr
        let output = config
            .command("echo redirected 1>&2")
            .output()
            .expect("Failed to execute redirection test");

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success());
        assert!(
            stderr.contains("redirected"),
            "stderr should contain 'redirected' (stdout redirected to stderr), got: '{}'",
            stderr.trim()
        );
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

    // ========================================================================
    // Cmd and timeout tests
    // ========================================================================

    #[test]
    fn test_cmd_completes_fast_command() {
        let result = Cmd::new("echo")
            .arg("hello")
            .timeout(Duration::from_secs(5))
            .run();
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("hello"));
    }

    #[test]
    #[cfg(unix)]
    fn test_cmd_timeout_kills_slow_command() {
        let result = Cmd::new("sleep")
            .arg("10")
            .timeout(Duration::from_millis(50))
            .run();
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
    }

    #[test]
    fn test_cmd_without_timeout_completes() {
        let result = Cmd::new("echo").arg("no timeout").run();
        assert!(result.is_ok());
    }

    #[test]
    fn test_cmd_with_context() {
        let result = Cmd::new("echo")
            .arg("with context")
            .context("test-context")
            .run();
        assert!(result.is_ok());
    }

    #[test]
    fn test_cmd_with_stdin() {
        let result = Cmd::new("cat").stdin("hello from stdin").run();
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("hello from stdin"));
    }

    #[test]
    fn test_thread_local_timeout_setting() {
        // Initially no timeout (or whatever was set by previous test)
        let initial = COMMAND_TIMEOUT.with(|t| t.get());

        // Set a timeout
        set_command_timeout(Some(Duration::from_millis(100)));
        let after_set = COMMAND_TIMEOUT.with(|t| t.get());
        assert_eq!(after_set, Some(Duration::from_millis(100)));

        // Clear the timeout
        set_command_timeout(initial);
        let after_clear = COMMAND_TIMEOUT.with(|t| t.get());
        assert_eq!(after_clear, initial);
    }

    #[test]
    fn test_cmd_uses_thread_local_timeout() {
        // Set no timeout (ensure fast completion)
        set_command_timeout(None);

        let result = Cmd::new("echo").arg("thread local test").run();
        assert!(result.is_ok());

        // Clean up
        set_command_timeout(None);
    }

    #[test]
    #[cfg(unix)]
    fn test_cmd_thread_local_timeout_kills_slow_command() {
        // Set a short thread-local timeout
        set_command_timeout(Some(Duration::from_millis(50)));

        // Command that would take too long
        let result = Cmd::new("sleep").arg("10").run();

        // Should be killed by the thread-local timeout
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::TimedOut);

        // Clean up
        set_command_timeout(None);
    }
}
