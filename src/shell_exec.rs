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

/// Log command output (stdout/stderr) for debugging.
fn log_output(output: &std::process::Output) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    for line in stdout.lines() {
        log::debug!("  {}", line);
    }
    for line in stderr.lines() {
        log::debug!("  ! {}", line);
    }
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

/// Builder for executing commands with two modes of operation.
///
/// - `.run()` — captures output, provides logging/semaphore/tracing
/// - `.stream()` — inherits stdout/stderr for TTY preservation (hooks, interactive);
///   stdin defaults to null unless configured with `.stdin(Stdio)` or `.stdin_bytes()`
///
/// # Examples
///
/// Capture output:
/// ```ignore
/// let output = Cmd::new("git")
///     .args(["status", "--porcelain"])
///     .current_dir(&repo_path)
///     .context("my-worktree")
///     .run()?;
/// ```
///
/// Stream output (hooks, interactive):
/// ```ignore
/// use std::process::Stdio;
///
/// Cmd::shell("npm run build")
///     .current_dir(&repo_path)
///     .stdout(Stdio::from(std::io::stderr()))
///     .forward_signals()
///     .stream()?;
/// ```
pub struct Cmd {
    /// Program name or shell command string (if shell_wrap is true)
    program: String,
    args: Vec<String>,
    current_dir: Option<std::path::PathBuf>,
    context: Option<String>,
    stdin_data: Option<Vec<u8>>,
    timeout: Option<std::time::Duration>,
    envs: Vec<(String, String)>,
    env_removes: Vec<String>,
    /// If true, wrap command through ShellConfig (for stream())
    shell_wrap: bool,
    /// Stdout configuration for stream() (defaults to inherit)
    stdout_cfg: Option<std::process::Stdio>,
    /// Stdin configuration for stream() (defaults to null, or piped if stdin_data is set)
    stdin_cfg: Option<std::process::Stdio>,
    /// If true, forward signals to child process group (for stream(), Unix only)
    forward_signals: bool,
}

impl Cmd {
    /// Create a new command builder for the given program.
    ///
    /// The program is executed directly without shell interpretation.
    /// For shell commands (with pipes, redirects, etc.), use [`Cmd::shell()`].
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
            shell_wrap: false,
            stdout_cfg: None,
            stdin_cfg: None,
            forward_signals: false,
        }
    }

    /// Create a command builder for a shell command string.
    ///
    /// The command is executed through the platform's shell (`sh -c` on Unix,
    /// Git Bash on Windows), enabling shell features like pipes and redirects.
    ///
    /// Only valid with `.stream()` — shell commands cannot use `.run()`.
    pub fn shell(command: impl Into<String>) -> Self {
        Self {
            program: command.into(),
            args: Vec::new(),
            current_dir: None,
            context: None,
            stdin_data: None,
            timeout: None,
            envs: Vec::new(),
            env_removes: Vec::new(),
            shell_wrap: true,
            stdout_cfg: None,
            stdin_cfg: None,
            forward_signals: false,
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

    /// Set data to pipe to the command's stdin.
    ///
    /// For `.run()`, the data is written to a piped stdin.
    /// For `.stream()`, this takes precedence over `.stdin(Stdio)`.
    pub fn stdin_bytes(mut self, data: impl Into<Vec<u8>>) -> Self {
        self.stdin_data = Some(data.into());
        self
    }

    /// Set a timeout for command execution (only applies to `.run()`).
    ///
    /// Note: Timeout is not supported by `.stream()` since streaming commands
    /// are interactive and should not be time-limited.
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

    /// Set stdout configuration for `.stream()`.
    ///
    /// Defaults to `Stdio::inherit()`. Use `Stdio::from(io::stderr())` to redirect
    /// stdout to stderr for deterministic output ordering.
    ///
    /// Only affects `.stream()`. For `.run()`, output is always captured separately.
    pub fn stdout(mut self, cfg: std::process::Stdio) -> Self {
        self.stdout_cfg = Some(cfg);
        self
    }

    /// Set stdin configuration for `.stream()`.
    ///
    /// Defaults to `Stdio::null()`. Use `Stdio::inherit()` for interactive commands
    /// that need to read user input.
    ///
    /// Only affects `.stream()`. For `.run()`, stdin defaults to null unless
    /// data is provided via `.stdin_bytes()`.
    pub fn stdin(mut self, cfg: std::process::Stdio) -> Self {
        self.stdin_cfg = Some(cfg);
        self
    }

    /// Forward signals (SIGINT, SIGTERM) to child process group.
    ///
    /// On Unix, spawns the child in its own process group and forwards signals
    /// with escalation (SIGINT → SIGTERM → SIGKILL). This enables clean shutdown
    /// of the entire process tree on Ctrl-C.
    ///
    /// Only affects `.stream()` on Unix. No-op on Windows.
    pub fn forward_signals(mut self) -> Self {
        self.forward_signals = true;
        self
    }

    /// Execute the command and return its output.
    ///
    /// Captures stdout/stderr and returns them in `Output`. For interactive
    /// commands or hooks where output should stream to the terminal, use
    /// `.stream()` instead.
    ///
    /// # Panics
    ///
    /// Panics if called on a shell-wrapped command (created via `Cmd::shell()`).
    /// Shell commands must use `.stream()` because they need TTY preservation.
    pub fn run(self) -> std::io::Result<std::process::Output> {
        use std::io::Write;
        use std::process::Stdio;

        assert!(
            !self.shell_wrap,
            "Cmd::shell() commands must use .stream(), not .run()"
        );

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

        if let Some(ref dir) = self.current_dir {
            cmd.current_dir(dir);
        }

        for (key, val) in &self.envs {
            cmd.env(key, val);
        }
        for key in &self.env_removes {
            cmd.env_remove(key);
        }

        // Prevent subprocesses from writing shell directives (security).
        // Applied last to ensure it can't be re-added by user-provided envs.
        cmd.env_remove(DIRECTIVE_FILE_ENV_VAR);

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
                log_output(output);
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
                log_output(output);
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

    /// Execute the command with streaming output (inherits stdio).
    ///
    /// Unlike `.run()`, this method:
    /// - Inherits stderr to preserve TTY behavior (colors, progress bars)
    /// - Optionally redirects stdout to stderr (via `.stdout(Stdio::from(io::stderr()))`)
    /// - Optionally inherits stdin for interactive commands (via `.stdin(Stdio::inherit())`)
    /// - Optionally forwards signals to child process group (via `.forward_signals()`)
    /// - Does not use concurrency limiting (streaming commands run sequentially by nature)
    /// - Does not support timeout (interactive commands should not be time-limited)
    ///
    /// Shell commands created via `Cmd::shell()` are executed through the platform's
    /// shell (`sh -c` on Unix, Git Bash on Windows).
    ///
    /// Returns error if command exits with non-zero status.
    pub fn stream(self) -> anyhow::Result<()> {
        use crate::git::{GitError, WorktrunkError};
        use std::io::Write;
        #[cfg(unix)]
        use {
            signal_hook::consts::{SIGINT, SIGTERM},
            signal_hook::iterator::Signals,
            std::os::unix::process::CommandExt,
        };

        // Shell-wrapped commands don't use args (the command string is the full command)
        assert!(
            !self.shell_wrap || self.args.is_empty(),
            "Cmd::shell() cannot use .arg() - include arguments in the shell command string"
        );

        let working_dir = self
            .current_dir
            .as_deref()
            .unwrap_or_else(|| std::path::Path::new("."));

        // Build the command - either shell-wrapped or direct
        let (mut cmd, exec_mode) = if self.shell_wrap {
            let shell = ShellConfig::get();
            let mode = format!("shell: {}", shell.name);
            (shell.command(&self.program), mode)
        } else {
            let mut cmd = Command::new(&self.program);
            cmd.args(&self.args);
            (cmd, "direct".to_string())
        };

        // Build command string for logging (shell commands have full command in program,
        // non-shell commands may have args)
        let cmd_str = if self.shell_wrap || self.args.is_empty() {
            self.program.clone()
        } else {
            format!("{} {}", self.program, self.args.join(" "))
        };

        // Log command for debugging (output goes to logger, not stdout/stderr)
        match &self.context {
            Some(ctx) => log::debug!("$ {} [{}] (streaming, {})", cmd_str, ctx, exec_mode),
            None => log::debug!("$ {} (streaming, {})", cmd_str, exec_mode),
        }

        #[cfg(not(unix))]
        let _ = self.forward_signals;

        // Determine stdout handling (default: inherit)
        let stdout_mode = self.stdout_cfg.unwrap_or_else(std::process::Stdio::inherit);

        // Determine stdin handling (stdin_bytes takes precedence, then stdin cfg, then null)
        let stdin_mode = if self.stdin_data.is_some() {
            std::process::Stdio::piped()
        } else {
            self.stdin_cfg.unwrap_or_else(std::process::Stdio::null)
        };

        #[cfg(unix)]
        let mut signals = if self.forward_signals {
            Some(Signals::new([SIGINT, SIGTERM])?)
        } else {
            None
        };

        #[cfg(unix)]
        if self.forward_signals {
            // Isolate the child in its own process group so we can signal the whole tree.
            cmd.process_group(0);
        }

        // Apply environment and spawn
        cmd.current_dir(working_dir)
            .stdin(stdin_mode)
            .stdout(stdout_mode)
            .stderr(std::process::Stdio::inherit()) // Preserve TTY for errors
            // Prevent vergen "overridden" warning in nested cargo builds
            .env_remove("VERGEN_GIT_DESCRIBE");

        for (key, val) in &self.envs {
            cmd.env(key, val);
        }
        for key in &self.env_removes {
            cmd.env_remove(key);
        }

        // Prevent hooks from writing shell directives (security).
        // Applied last to ensure it can't be re-added by user-provided envs.
        cmd.env_remove(DIRECTIVE_FILE_ENV_VAR);

        let mut child = cmd.spawn().map_err(|e| {
            anyhow::Error::from(GitError::Other {
                message: format!("Failed to execute command ({}): {}", exec_mode, e),
            })
        })?;

        // Write stdin content if provided (ignore BrokenPipe - child may exit early)
        if let Some(ref content) = self.stdin_data
            && let Some(mut stdin) = child.stdin.take()
            && let Err(e) = stdin.write_all(content)
            && e.kind() != std::io::ErrorKind::BrokenPipe
        {
            return Err(e.into());
        }
        // stdin handle is dropped here, closing the pipe

        // Wait for child with optional signal forwarding
        #[cfg(unix)]
        let (status, seen_signal) = if self.forward_signals {
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

        // Handle signals (Unix only)
        #[cfg(unix)]
        if let Some(sig) = seen_signal {
            return Err(WorktrunkError::ChildProcessExited {
                code: 128 + sig,
                message: format!("terminated by signal {}", sig),
            }
            .into());
        }

        #[cfg(unix)]
        if let Some(sig) = std::os::unix::process::ExitStatusExt::signal(&status) {
            return Err(WorktrunkError::ChildProcessExited {
                code: 128 + sig,
                message: format!("terminated by signal {}", sig),
            }
            .into());
        }

        if !status.success() {
            let code = status.code().unwrap_or(1);
            return Err(WorktrunkError::ChildProcessExited {
                code,
                message: format!("exit status: {}", code),
            }
            .into());
        }

        Ok(())
    }
}

// ============================================================================
// Signal forwarding helpers (Unix only)
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
    // Escalate if process doesn't exit gracefully
    if sig == signal_hook::consts::SIGINT {
        if !wait_for_exit(pgid.as_raw(), grace) {
            let _ = nix::sys::signal::killpg(pgid, nix::sys::signal::Signal::SIGTERM);
            if !wait_for_exit(pgid.as_raw(), grace) {
                let _ = nix::sys::signal::killpg(pgid, nix::sys::signal::Signal::SIGKILL);
            }
        }
    } else {
        // SIGTERM - escalate directly to SIGKILL
        if !wait_for_exit(pgid.as_raw(), grace) {
            let _ = nix::sys::signal::killpg(pgid, nix::sys::signal::Signal::SIGKILL);
        }
    }
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
        let result = Cmd::new("cat").stdin_bytes("hello from stdin").run();
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

    // ========================================================================
    // Cmd::stream() tests
    // ========================================================================

    #[test]
    fn test_cmd_shell_stream_succeeds() {
        let result = Cmd::shell("echo hello").stream();
        assert!(result.is_ok());
    }

    #[test]
    fn test_cmd_shell_stream_fails_on_nonzero_exit() {
        use crate::git::WorktrunkError;

        let result = Cmd::shell("exit 42").stream();
        assert!(result.is_err());

        let err = result.unwrap_err();
        let wt_err = err.downcast_ref::<WorktrunkError>().unwrap();
        match wt_err {
            WorktrunkError::ChildProcessExited { code, .. } => {
                assert_eq!(*code, 42);
            }
            _ => panic!("Expected ChildProcessExited error"),
        }
    }

    #[test]
    #[cfg(unix)]
    fn test_cmd_shell_stream_with_stdin() {
        // cat should echo stdin content (output goes to inherited stdout, we can't capture it,
        // but we can verify no error)
        let result = Cmd::shell("cat").stdin_bytes("test content").stream();
        assert!(result.is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn test_cmd_new_stream_succeeds() {
        // Non-shell command via stream() (uses direct execution, not shell wrapping)
        let result = Cmd::new("echo").arg("hello").stream();
        assert!(result.is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn test_cmd_shell_stream_with_stdout_redirect() {
        use std::process::Stdio;
        // Redirect stdout to stderr (common pattern for hooks)
        let result = Cmd::shell("echo redirected")
            .stdout(Stdio::from(std::io::stderr()))
            .stream();
        assert!(result.is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn test_cmd_shell_stream_with_stdin_inherit() {
        use std::process::Stdio;
        // Test stdin configuration (true immediately exits, doesn't actually read stdin)
        let result = Cmd::shell("true").stdin(Stdio::inherit()).stream();
        assert!(result.is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn test_cmd_shell_stream_with_env() {
        // Test .env() and .env_remove() with stream()
        let result = Cmd::shell("printenv TEST_VAR")
            .env("TEST_VAR", "test_value")
            .env_remove("SOME_NONEXISTENT_VAR")
            .stream();
        assert!(result.is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn test_process_group_alive_with_current_process() {
        // Current process group should be alive
        let pgid = nix::unistd::getpgrp().as_raw();
        assert!(super::process_group_alive(pgid));
    }

    #[test]
    #[cfg(unix)]
    fn test_process_group_alive_with_nonexistent_pgid() {
        // Very high PGID unlikely to exist
        assert!(!super::process_group_alive(999_999_999));
    }

    #[test]
    #[cfg(unix)]
    fn test_forward_signal_with_escalation_unknown_signal() {
        // Unknown signal should return early without doing anything
        // Use a signal number that's not SIGINT or SIGTERM
        super::forward_signal_with_escalation(1, 999);
        // No panic = success (function returns early for unknown signals)
    }
}
