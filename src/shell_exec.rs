//! Cross-platform shell execution
//!
//! Provides a unified interface for executing shell commands across platforms:
//! - Unix: Uses `sh -c` (resolved via PATH)
//! - Windows: Uses Git Bash (requires Git for Windows)
//!
//! This enables hooks and commands to use the same bash syntax on all platforms.
//! On Windows, Git for Windows must be installed — this is nearly universal among
//! Windows developers since git itself is required.

use std::ffi::{OsStr, OsString};
use std::io::{ErrorKind, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Instant;

use wait_timeout::ChildExt;

use crate::git::{GitError, WorktrunkError};
use crate::sync::Semaphore;

/// Semaphore to limit concurrent command execution.
/// Prevents resource exhaustion when spawning many parallel git commands.
static CMD_SEMAPHORE: OnceLock<Semaphore> = OnceLock::new();

/// The working directory at `wt` startup. Captured once so relative `GIT_*`
/// path variables inherited from a parent `git` process can be resolved to
/// absolute paths regardless of each subsequent child command's `current_dir`.
static STARTUP_CWD: OnceLock<Option<PathBuf>> = OnceLock::new();

/// `GIT_*` environment variables that name paths used by git for repository
/// discovery and I/O. When git invokes shell aliases (`alias.x = "!cmd"`) it
/// may set some of these to *relative* paths (e.g. `GIT_DIR=.git`), which
/// then resolve against whatever `current_dir` a child process happens to
/// run in — not the directory where `wt` was invoked. Normalizing them to
/// absolute paths keeps git's alias context without breaking discovery.
const INHERITED_GIT_PATH_VARS: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
];

/// Record the current working directory at `wt` startup so relative `GIT_*`
/// path variables inherited from a parent process can later be resolved to
/// absolute paths by [`Cmd`]'s env setup.
///
/// Call once during `wt` startup, before any code changes the process's
/// working directory. Subsequent calls are no-ops.
pub fn init_startup_cwd() {
    STARTUP_CWD.get_or_init(|| std::env::current_dir().ok());
}

fn startup_cwd() -> Option<&'static PathBuf> {
    STARTUP_CWD
        .get_or_init(|| std::env::current_dir().ok())
        .as_ref()
}

/// Pure helper: given a base directory and a lookup function for environment
/// variables, compute the `(var, absolute_value)` overrides that should be
/// applied to a child process's environment to shadow any inherited relative
/// `GIT_*` path variables. Absolute values and unset variables are skipped.
///
/// Factored out from [`inherited_git_env_overrides`] so it can be unit-tested
/// without touching process-wide state.
fn compute_git_env_overrides<F>(base: &std::path::Path, lookup: F) -> Vec<(&'static str, OsString)>
where
    F: Fn(&str) -> Option<OsString>,
{
    let mut overrides = Vec::new();
    for var in INHERITED_GIT_PATH_VARS {
        let Some(value) = lookup(var) else {
            continue;
        };
        let path = std::path::Path::new(&value);
        if path.is_absolute() {
            continue;
        }
        overrides.push((*var, base.join(path).into_os_string()));
    }
    overrides
}

/// Cached absolute forms of any inherited relative `GIT_*` path variables.
/// Computed once from the startup cwd and process environment, since neither
/// changes during the process lifetime.
static GIT_ENV_OVERRIDES: OnceLock<Vec<(&'static str, OsString)>> = OnceLock::new();

/// For each inherited `GIT_*` path variable that is set to a *relative* path,
/// produce an absolute form resolved against the startup cwd. Returns the
/// `(var, absolute_value)` pairs that should be applied to a child process's
/// environment to shadow the inherited relative values.
fn inherited_git_env_overrides() -> &'static [(&'static str, OsString)] {
    GIT_ENV_OVERRIDES.get_or_init(|| {
        let Some(cwd) = startup_cwd() else {
            return Vec::new();
        };
        compute_git_env_overrides(cwd, |var| std::env::var_os(var))
    })
}

/// Default concurrent external commands. Tuned to avoid hitting OS limits
/// (file descriptors, process limits) while maintaining good parallelism.
const DEFAULT_CONCURRENT_COMMANDS: usize = 32;

/// Parse the concurrency limit from a string value.
/// Returns None if invalid (not a number), otherwise applies the 0 = unlimited rule.
fn parse_concurrent_limit(value: &str) -> Option<usize> {
    value
        .parse::<usize>()
        .ok()
        // 0 = no limit (use usize::MAX as effectively unlimited)
        .map(|n| if n == 0 { usize::MAX } else { n })
}

fn max_concurrent_commands() -> usize {
    std::env::var("WORKTRUNK_MAX_CONCURRENT_COMMANDS")
        .ok()
        .and_then(|s| parse_concurrent_limit(&s))
        .unwrap_or(DEFAULT_CONCURRENT_COMMANDS)
}

fn semaphore() -> &'static Semaphore {
    CMD_SEMAPHORE.get_or_init(|| Semaphore::new(max_concurrent_commands()))
}

/// Cached shell configuration for the current platform
static SHELL_CONFIG: OnceLock<Result<ShellConfig, String>> = OnceLock::new();

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
    /// On Unix, returns sh. On Windows, returns Git Bash or an error if not installed.
    pub fn get() -> anyhow::Result<&'static ShellConfig> {
        SHELL_CONFIG
            .get_or_init(detect_shell)
            .as_ref()
            .map_err(|e| anyhow::anyhow!("{e}"))
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
fn detect_shell() -> Result<ShellConfig, String> {
    #[cfg(unix)]
    {
        Ok(ShellConfig {
            executable: PathBuf::from("sh"),
            args: vec!["-c".to_string()],
            is_posix: true,
            name: "sh".to_string(),
        })
    }

    #[cfg(windows)]
    {
        detect_windows_shell()
    }
}

/// Detect Git Bash on Windows
///
/// Returns an error if Git for Windows is not installed, since hooks require
/// bash syntax.
#[cfg(windows)]
fn detect_windows_shell() -> Result<ShellConfig, String> {
    if let Some(bash_path) = find_git_bash() {
        return Ok(ShellConfig {
            executable: bash_path,
            args: vec!["-c".to_string()],
            is_posix: true,
            name: "Git Bash".to_string(),
        });
    }

    Err("Git for Windows is required but not found.\n\
         Install from https://git-scm.com/download/win"
        .to_string())
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

    // Fallback: standard Git for Windows paths (needed on some CI environments
    // where `which` doesn't find git even though it's installed)
    let bash_path = PathBuf::from(r"C:\Program Files\Git\bin\bash.exe");
    if bash_path.exists() {
        return Some(bash_path);
    }

    // Per-user Git for Windows installation (default path when installed without admin rights)
    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        let bash_path = PathBuf::from(local_app_data)
            .join("Programs")
            .join("Git")
            .join("bin")
            .join("bash.exe");
        if bash_path.exists() {
            return Some(bash_path);
        }
    }

    None
}

/// Environment variable naming the directive file for `cd` path changes.
///
/// Shell wrappers set this to a temp file; wt writes a raw absolute path to
/// it (one line, no shell escaping). The wrapper `cd`s to that path after wt
/// exits. Because the file contents are a literal path, there is no shell
/// injection surface — this is safe to pass through to alias/hook shell bodies.
pub const DIRECTIVE_CD_FILE_ENV_VAR: &str = "WORKTRUNK_DIRECTIVE_CD_FILE";

/// Environment variable naming the directive file for arbitrary exec commands.
///
/// Shell wrappers set this to a temp file; wt writes shell commands (e.g. the
/// body of `wt switch --execute`) to it and the wrapper sources the file after
/// wt exits, so the command runs in the user's interactive shell. Because the
/// file contents are arbitrary shell, wt scrubs this from alias/hook child
/// environments — a hook body writing to this file could inject shell into
/// the parent shell. Only trusted wt-internal callers write to it.
pub const DIRECTIVE_EXEC_FILE_ENV_VAR: &str = "WORKTRUNK_DIRECTIVE_EXEC_FILE";

/// Legacy pre-split directive file env var. Honored for one release so users
/// who upgraded `wt` without restarting their shell still get shell integration
/// from their current session's old wrapper. When only this is set (no new
/// vars), wt writes shell-format directives to it. Remove in the next breaking
/// release.
pub const DIRECTIVE_FILE_ENV_VAR: &str = "WORKTRUNK_DIRECTIVE_FILE";

/// Scrub all directive file env vars from a `std::process::Command`.
///
/// Prevents child processes from writing to the parent shell's directive
/// files. Called by every code path that spawns external commands (Cmd,
/// help pager, picker pager, background hooks, git credential helpers).
pub fn scrub_directive_env_vars(cmd: &mut std::process::Command) {
    cmd.env_remove(DIRECTIVE_CD_FILE_ENV_VAR);
    cmd.env_remove(DIRECTIVE_EXEC_FILE_ENV_VAR);
    cmd.env_remove(DIRECTIVE_FILE_ENV_VAR);
}

// ============================================================================
// Thread-Local Command Timeout
// ============================================================================

use std::cell::Cell;
use std::time::Duration;

thread_local! {
    /// Thread-local command timeout. When set, all commands executed via `run()` on this
    /// thread will be killed if they exceed this duration.
    ///
    /// This is used by `wt switch` interactive picker to make the TUI responsive faster on large repos.
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
/// Re-exported from [`crate::trace::emit::instant`] for convenience at the
/// call sites that already import from `shell_exec`. Instant events appear as
/// vertical lines in Chrome Trace Format visualization tools
/// (chrome://tracing, Perfetto).
pub fn trace_instant(event: &str) {
    crate::trace::emit::instant(event);
}

/// Maximum lines of captured stdout/stderr emitted per stream at `log::debug!`.
/// Exceeded content is elided with a `… (N more lines, M bytes elided)` marker.
/// Raise the level to `log::trace!` (via `-vvv` or `RUST_LOG=trace`) for full output.
const LOG_OUTPUT_MAX_LINES: usize = 200;

/// Maximum bytes of captured stdout/stderr emitted per stream at `log::debug!`.
/// Applied in addition to [`LOG_OUTPUT_MAX_LINES`].
const LOG_OUTPUT_MAX_BYTES: usize = 64 * 1024;

/// Log captured stdout/stderr of a finished command.
///
/// At `log::trace!` (enabled by `-vvv`) the full output is emitted, one line
/// per record with indent prefix (`  ` for stdout, `  ! ` for stderr). At
/// `log::debug!` (`-vv`) the output is capped at [`LOG_OUTPUT_MAX_LINES`] and
/// [`LOG_OUTPUT_MAX_BYTES`] per stream with an elision marker, so large
/// subprocess bodies (diffs, prompts) don't flood the verbose stream.
fn log_output(output: &std::process::Output) {
    if log::log_enabled!(log::Level::Trace) {
        for line in format_stream_full(&output.stdout, "  ") {
            log::trace!("{}", line);
        }
        for line in format_stream_full(&output.stderr, "  ! ") {
            log::trace!("{}", line);
        }
    } else if log::log_enabled!(log::Level::Debug) {
        for line in format_stream_bounded(&output.stdout, "  ") {
            log::debug!("{}", line);
        }
        for line in format_stream_bounded(&output.stderr, "  ! ") {
            log::debug!("{}", line);
        }
    }
}

/// Split captured bytes into prefixed lines — full output, no cap.
fn format_stream_full(bytes: &[u8], prefix: &str) -> Vec<String> {
    if bytes.is_empty() {
        return Vec::new();
    }
    String::from_utf8_lossy(bytes)
        .lines()
        .map(|line| format!("{}{}", prefix, line))
        .collect()
}

/// Split captured bytes into prefixed lines with at most [`LOG_OUTPUT_MAX_LINES`]
/// and [`LOG_OUTPUT_MAX_BYTES`] emitted; remainder replaced by a single
/// `… (N more lines, M bytes elided — use -vvv for full output)` marker.
fn format_stream_bounded(bytes: &[u8], prefix: &str) -> Vec<String> {
    if bytes.is_empty() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(bytes);
    let total_bytes = bytes.len();

    let mut out = Vec::new();
    let mut bytes_emitted = 0;
    let mut lines = text.lines().enumerate();
    for (lines_emitted, line) in &mut lines {
        if lines_emitted >= LOG_OUTPUT_MAX_LINES || bytes_emitted >= LOG_OUTPUT_MAX_BYTES {
            let remaining_lines = 1 + lines.count();
            let remaining_bytes = total_bytes.saturating_sub(bytes_emitted);
            out.push(format!(
                "{}… ({} more lines, {} bytes elided — use -vvv for full output)",
                prefix, remaining_lines, remaining_bytes
            ));
            return out;
        }
        out.push(format!("{}{}", prefix, line));
        bytes_emitted += line.len() + 1;
    }
    out
}

/// Emit a `[wt-trace]` line plus stdout/stderr for a finished command.
fn log_command_result(
    context: Option<&str>,
    cmd_str: &str,
    ts: u64,
    tid: u64,
    dur_us: u64,
    result: &std::io::Result<std::process::Output>,
) {
    match result {
        Ok(output) => {
            crate::trace::emit::command_completed(
                context,
                cmd_str,
                ts,
                tid,
                dur_us,
                output.status.success(),
            );
            log_output(output);
        }
        Err(e) => {
            crate::trace::emit::command_errored(context, cmd_str, ts, tid, dur_us, e);
        }
    }
}

/// Implementation of timeout-based command execution.
///
/// Spawns reader threads to drain stdout/stderr concurrently (preventing deadlock when
/// output exceeds the OS pipe buffer), then waits with timeout. On timeout, kills the
/// child; scoped threads see EOF and join automatically before the function returns.
fn run_with_timeout_impl(
    cmd: &mut Command,
    timeout: std::time::Duration,
) -> std::io::Result<std::process::Output> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut child_stdout = child.stdout.take();
    let mut child_stderr = child.stderr.take();

    std::thread::scope(|s| {
        let stdout_thread = s.spawn(|| {
            let mut buf = Vec::new();
            child_stdout
                .as_mut()
                .map(|h| h.read_to_end(&mut buf))
                .transpose()?;
            Ok::<_, std::io::Error>(buf)
        });
        let stderr_thread = s.spawn(|| {
            let mut buf = Vec::new();
            child_stderr
                .as_mut()
                .map(|h| h.read_to_end(&mut buf))
                .transpose()?;
            Ok::<_, std::io::Error>(buf)
        });

        match child.wait_timeout(timeout)? {
            Some(status) => {
                let stdout = stdout_thread.join().unwrap()?;
                let stderr = stderr_thread.join().unwrap()?;
                Ok(std::process::Output {
                    status,
                    stdout,
                    stderr,
                })
            }
            None => {
                let _ = child.kill();
                let _ = child.wait();
                Err(std::io::Error::new(
                    ErrorKind::TimedOut,
                    "command timed out",
                ))
            }
        }
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
    envs: Vec<(OsString, OsString)>,
    env_removes: Vec<OsString>,
    /// If true, wrap command through ShellConfig (for stream())
    shell_wrap: bool,
    /// Stdout configuration for stream() (defaults to inherit)
    stdout_cfg: Option<std::process::Stdio>,
    /// Stdin configuration for stream() (defaults to null, or piped if stdin_data is set)
    stdin_cfg: Option<std::process::Stdio>,
    /// If true, forward signals to child process group (for stream(), Unix only)
    forward_signals: bool,
    /// When set, log this command to the command log after execution.
    /// The label identifies what triggered the command (e.g., "pre-merge user:lint").
    external_label: Option<String>,
    /// When set, re-adds `WORKTRUNK_DIRECTIVE_CD_FILE` after the security scrub
    /// in `apply_common_settings`. Used by aliases and foreground hooks — their
    /// shell bodies may emit cd directives (the file holds a raw path, no shell
    /// injection surface). `WORKTRUNK_DIRECTIVE_EXEC_FILE` is NEVER re-added,
    /// so alias/hook bodies cannot inject arbitrary shell into the parent.
    directive_cd_file: Option<std::path::PathBuf>,
    /// When set, re-adds the legacy `WORKTRUNK_DIRECTIVE_FILE` env var. Used in
    /// legacy-wrapper compat mode to preserve pre-split behavior for alias/hook
    /// bodies running under an old shell wrapper.
    directive_legacy_file: Option<std::path::PathBuf>,
}

struct ExternalCommandLog {
    label: Option<String>,
    cmd_str: String,
    started_at: Option<Instant>,
}

impl ExternalCommandLog {
    fn new(label: Option<String>, cmd_str: String) -> Self {
        let started_at = label.as_ref().map(|_| Instant::now());
        Self {
            label,
            cmd_str,
            started_at,
        }
    }

    fn record(&self, exit_code: Option<i32>) {
        if let Some(label) = &self.label {
            let duration = self.started_at.as_ref().map(Instant::elapsed);
            crate::command_log::log_command(label, &self.cmd_str, exit_code, duration);
        }
    }
}

impl Cmd {
    fn builder(program: impl Into<String>, shell_wrap: bool) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            current_dir: None,
            context: None,
            stdin_data: None,
            timeout: None,
            envs: Vec::new(),
            env_removes: Vec::new(),
            shell_wrap,
            stdout_cfg: None,
            stdin_cfg: None,
            forward_signals: false,
            external_label: None,
            directive_cd_file: None,
            directive_legacy_file: None,
        }
    }

    /// Create a new command builder for the given program.
    ///
    /// The program is executed directly without shell interpretation.
    /// For shell commands (with pipes, redirects, etc.), use [`Cmd::shell()`].
    pub fn new(program: impl Into<String>) -> Self {
        Self::builder(program, false)
    }

    /// Create a command builder for a shell command string.
    ///
    /// The command is executed through the platform's shell (`sh -c` on Unix,
    /// Git Bash on Windows), enabling shell features like pipes and redirects.
    ///
    /// Only valid with `.stream()` — shell commands cannot use `.run()`.
    pub fn shell(command: impl Into<String>) -> Self {
        Self::builder(command, true)
    }

    fn command_string(&self) -> String {
        if self.shell_wrap || self.args.is_empty() {
            self.program.clone()
        } else {
            format!("{} {}", self.program, self.args.join(" "))
        }
    }

    fn direct_command(&self) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args);
        cmd
    }

    fn apply_common_settings(&self, cmd: &mut Command) {
        if let Some(dir) = &self.current_dir {
            cmd.current_dir(dir);
        }

        // Normalize inherited relative `GIT_*` path variables (e.g. the
        // `GIT_DIR=.git` git sets for shell aliases) to absolute paths
        // resolved against the startup cwd, so they don't re-resolve against
        // the child's `current_dir`. See issue #1914.
        for (key, val) in inherited_git_env_overrides() {
            cmd.env(key, val);
        }

        for (key, val) in &self.envs {
            cmd.env(key, val);
        }
        for key in &self.env_removes {
            cmd.env_remove(key);
        }

        // Prevent subprocesses from writing shell directives (security).
        // Applied last so it can't be re-added by user-provided envs.
        // `stream()` selectively re-adds `WORKTRUNK_DIRECTIVE_CD_FILE` (and
        // the legacy var, in compat mode) for trusted contexts — but never
        // `WORKTRUNK_DIRECTIVE_EXEC_FILE`, which carries arbitrary shell.
        scrub_directive_env_vars(cmd);
    }

    fn log_run_start(&self, cmd_str: &str) {
        match &self.context {
            Some(ctx) => log::debug!("$ {} [{}]", cmd_str, ctx),
            None => log::debug!("$ {}", cmd_str),
        }
    }

    fn log_stream_start(&self, cmd_str: &str, exec_mode: &str) {
        match &self.context {
            Some(ctx) => log::debug!("$ {} [{}] (streaming, {})", cmd_str, ctx, exec_mode),
            None => log::debug!("$ {} (streaming, {})", cmd_str, exec_mode),
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
    ///
    /// Accepts the same types as [`Command::env`]: string literals, `String`,
    /// `&Path`, `PathBuf`, `OsString`, etc.
    pub fn env(mut self, key: impl AsRef<OsStr>, val: impl AsRef<OsStr>) -> Self {
        self.envs
            .push((key.as_ref().to_os_string(), val.as_ref().to_os_string()));
        self
    }

    /// Remove an environment variable.
    pub fn env_remove(mut self, key: impl AsRef<OsStr>) -> Self {
        self.env_removes.push(key.as_ref().to_os_string());
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

    /// Mark this command as an external (user-configured) command for logging.
    ///
    /// When set, the command execution is logged to `.git/wt/logs/commands.jsonl`
    /// with the given label (e.g., "pre-merge user:lint", "commit.generation").
    pub fn external(mut self, label: impl Into<String>) -> Self {
        self.external_label = Some(label.into());
        self
    }

    /// Pass the CD directive file through to the child process.
    ///
    /// By default, `Cmd` scrubs all directive file env vars from child
    /// processes. This re-adds `WORKTRUNK_DIRECTIVE_CD_FILE` for trusted
    /// contexts (aliases, foreground hooks) where the child should be able
    /// to request a directory change. It is always safe to pass through: the
    /// file holds a raw path, not shell, so there is no injection surface.
    ///
    /// `WORKTRUNK_DIRECTIVE_EXEC_FILE` is intentionally *not* exposed by any
    /// `Cmd` method — only wt-internal Rust code writes arbitrary shell
    /// directives, so no child process ever needs the env var.
    pub fn directive_cd_file(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.directive_cd_file = Some(path.into());
        self
    }

    /// Pass the legacy (pre-split) directive file through to the child process.
    ///
    /// Used only in legacy-wrapper compat mode. Preserves pre-split behavior
    /// for alias/hook bodies running under an old shell wrapper that still
    /// sets `WORKTRUNK_DIRECTIVE_FILE`. Remove together with the legacy env
    /// var in the next breaking release.
    pub fn directive_legacy_file(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.directive_legacy_file = Some(path.into());
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
        assert!(
            !self.shell_wrap,
            "Cmd::shell() commands must use .stream(), not .run()"
        );
        debug_assert!(
            self.directive_cd_file.is_none() && self.directive_legacy_file.is_none(),
            "directive_*_file is only applied by .stream(), not .run()"
        );

        let cmd_str = self.command_string();
        let external_log = ExternalCommandLog::new(self.external_label.clone(), cmd_str.clone());
        self.log_run_start(&cmd_str);

        // Acquire semaphore to limit concurrent commands
        let _guard = semaphore().acquire();

        // Capture timing for tracing
        let t0 = Instant::now();
        let ts = t0
            .duration_since(crate::trace::emit::trace_epoch())
            .as_micros() as u64;
        let tid = crate::trace::emit::thread_id();

        let mut cmd = self.direct_command();
        self.apply_common_settings(&mut cmd);

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
        log_command_result(self.context.as_deref(), &cmd_str, ts, tid, dur_us, &result);

        let exit_code = result.as_ref().ok().and_then(|output| output.status.code());
        external_log.record(exit_code);

        result
    }

    /// Run `self` with its stdout piped directly into `next`'s stdin, and
    /// return both children's captured output.
    ///
    /// The intermediate data (`self`'s stdout) flows between the two child
    /// processes via an OS pipe — it never lands in our process memory or
    /// debug logs. This keeps large intermediate outputs (for example
    /// `git diff-tree -p | git patch-id`) out of the `-vv` trace stream, where
    /// `log_output` would otherwise dump every line of the raw diff.
    ///
    /// Each command is logged and traced individually (same format as
    /// `.run()`), so `-vv` still shows both commands and their exit status.
    /// The returned tuple is `(source_output, sink_output)` — callers can
    /// inspect either child's exit status and stderr. `source_output.stdout`
    /// is empty (it was routed to the sink via OS pipe).
    ///
    /// Timeouts, `stdin_bytes`, and `external()` logging are not supported on
    /// either side. The pipeline consumes one semaphore permit even though it
    /// runs two processes concurrently — acquiring two would deadlock under
    /// `concurrency = 1`.
    pub fn pipe_into(
        self,
        next: Cmd,
    ) -> std::io::Result<(std::process::Output, std::process::Output)> {
        assert!(
            !self.shell_wrap && !next.shell_wrap,
            "Cmd::shell() commands cannot be used with pipe_into"
        );
        assert!(
            self.stdin_data.is_none(),
            "pipe_into source cannot also use stdin_bytes"
        );
        assert!(
            next.stdin_data.is_none(),
            "pipe_into sink cannot use stdin_bytes (stdin comes from source)"
        );
        assert!(
            self.timeout.is_none() && next.timeout.is_none(),
            "pipe_into does not support timeouts"
        );
        assert!(
            self.external_label.is_none() && next.external_label.is_none(),
            "pipe_into does not support external() logging"
        );
        debug_assert!(
            self.directive_cd_file.is_none()
                && self.directive_legacy_file.is_none()
                && next.directive_cd_file.is_none()
                && next.directive_legacy_file.is_none(),
            "directive_*_file is only applied by .stream(), not pipe_into"
        );

        let first_cmd_str = self.command_string();
        let second_cmd_str = next.command_string();
        self.log_run_start(&first_cmd_str);
        next.log_run_start(&second_cmd_str);

        let _guard = semaphore().acquire();

        let t0 = Instant::now();
        let ts = t0
            .duration_since(crate::trace::emit::trace_epoch())
            .as_micros() as u64;
        let tid = crate::trace::emit::thread_id();

        let mut first = self.direct_command();
        self.apply_common_settings(&mut first);
        first
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut first_child = first.spawn()?;
        let first_stdout = first_child
            .stdout
            .take()
            .expect("stdout was configured as piped");

        let mut second = next.direct_command();
        next.apply_common_settings(&mut second);
        second
            .stdin(Stdio::from(first_stdout))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Spawn `next` before waiting on either child so `self`'s stdout keeps
        // flowing through the pipe (otherwise a full pipe buffer would wedge
        // `self`). If the spawn itself fails, clean up `self` before returning.
        let second_child = match second.spawn() {
            Ok(child) => child,
            Err(e) => {
                let _ = first_child.kill();
                let _ = first_child.wait();
                return Err(e);
            }
        };

        // `first`'s stderr must be drained concurrently with `second`'s
        // execution; otherwise pathological stderr volume (~64 KiB pipe
        // buffer) could block `first` on write, which then never closes its
        // stdout, which wedges `second`. Scoped thread drains in parallel.
        let mut first_stderr_pipe = first_child
            .stderr
            .take()
            .expect("stderr was configured as piped");

        let (first_result, second_result, first_dur_us, second_dur_us) = std::thread::scope(|s| {
            let stderr_thread = s.spawn(move || {
                let mut buf = Vec::new();
                first_stderr_pipe.read_to_end(&mut buf).map(|_| buf)
            });

            // Drain `next` first (its `wait_with_output` reads its own
            // stdout/stderr), so `first`'s writes can complete.
            let second_result = second_child.wait_with_output();
            let second_dur_us = t0.elapsed().as_micros() as u64;

            // Reap `first`. Its stderr is already being drained; combine
            // the captured stderr with the exit status into an Output.
            let first_status = first_child.wait();
            let first_stderr = stderr_thread.join().unwrap();
            let first_dur_us = t0.elapsed().as_micros() as u64;

            let first_result = first_status.and_then(|status| {
                first_stderr.map(|stderr| std::process::Output {
                    status,
                    stdout: Vec::new(),
                    stderr,
                })
            });

            (first_result, second_result, first_dur_us, second_dur_us)
        });

        log_command_result(
            self.context.as_deref(),
            &first_cmd_str,
            ts,
            tid,
            first_dur_us,
            &first_result,
        );
        log_command_result(
            next.context.as_deref(),
            &second_cmd_str,
            ts,
            tid,
            second_dur_us,
            &second_result,
        );

        Ok((first_result?, second_result?))
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
    pub fn stream(mut self) -> anyhow::Result<()> {
        #[cfg(unix)]
        use {
            signal_hook::consts::{SIGINT, SIGPIPE, SIGTERM},
            signal_hook::iterator::Signals,
            std::os::unix::process::CommandExt,
        };

        // Shell-wrapped commands don't use args (the command string is the full command)
        assert!(
            !self.shell_wrap || self.args.is_empty(),
            "Cmd::shell() cannot use .arg() - include arguments in the shell command string"
        );

        // Build the command - either shell-wrapped or direct
        let (mut cmd, exec_mode) = if self.shell_wrap {
            let shell = ShellConfig::get()?;
            let mode = format!("shell: {}", shell.name);
            (shell.command(&self.program), mode)
        } else {
            (self.direct_command(), "direct".to_string())
        };

        let cmd_str = self.command_string();
        let external_log = ExternalCommandLog::new(self.external_label.take(), cmd_str.clone());
        self.log_stream_start(&cmd_str, &exec_mode);
        self.apply_common_settings(&mut cmd);

        // Re-add directive files after security scrub for trusted contexts.
        // CD file is always safe to pass through (raw path, no shell). EXEC
        // file is never re-added — alias/hook bodies must not inject shell.
        if let Some(ref path) = self.directive_cd_file {
            cmd.env(DIRECTIVE_CD_FILE_ENV_VAR, path);
        }
        if let Some(ref path) = self.directive_legacy_file {
            cmd.env(DIRECTIVE_FILE_ENV_VAR, path);
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
        cmd.stdin(stdin_mode)
            .stdout(stdout_mode)
            .stderr(std::process::Stdio::inherit()) // Preserve TTY for errors
            // Prevent vergen "overridden" warning in nested cargo builds
            .env_remove("VERGEN_GIT_DESCRIBE");

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
            external_log.record(Some(128 + sig));
            return Err(WorktrunkError::ChildProcessExited {
                code: 128 + sig,
                message: format!("terminated by signal {}", sig),
                signal: Some(sig),
            }
            .into());
        }

        #[cfg(unix)]
        if let Some(sig) = std::os::unix::process::ExitStatusExt::signal(&status) {
            // SIGPIPE (13) is expected when a pager (less, bat) exits before the
            // child finishes writing — not an error from the user's perspective.
            if sig == SIGPIPE {
                external_log.record(Some(0));
                return Ok(());
            }
            external_log.record(Some(128 + sig));
            return Err(WorktrunkError::ChildProcessExited {
                code: 128 + sig,
                message: format!("terminated by signal {}", sig),
                signal: Some(sig),
            }
            .into());
        }

        if !status.success() {
            let code = status.code().unwrap_or(1);
            external_log.record(status.code());
            return Err(WorktrunkError::ChildProcessExited {
                code,
                message: format!("exit status: {}", code),
                signal: None,
            }
            .into());
        }

        external_log.record(Some(0));

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
pub fn forward_signal_with_escalation(pgid: i32, sig: i32) {
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
    fn test_compute_git_env_overrides() {
        // Use a platform-appropriate absolute base path so `Path::is_absolute`
        // behaves the same on Windows and Unix (Unix-style `/abs/...` paths
        // are not absolute on Windows).
        let base_buf = std::env::temp_dir().join("wt-test-startup-cwd");
        let base = base_buf.as_path();
        let abs_work = std::env::temp_dir().join("wt-test-abs-work");
        let env: std::collections::HashMap<&str, OsString> = [
            // relative — should be resolved against base
            ("GIT_DIR", OsString::from(".git")),
            // absolute — should be skipped
            ("GIT_WORK_TREE", abs_work.clone().into_os_string()),
            // relative with parent traversal
            ("GIT_INDEX_FILE", OsString::from("../index")),
            // unrelated var — should not appear
            ("GIT_AUTHOR_NAME", OsString::from("Test User")),
        ]
        .into_iter()
        .collect();

        let overrides = compute_git_env_overrides(base, |var| env.get(var).cloned());

        // Unset GIT_COMMON_DIR / GIT_OBJECT_DIRECTORY are skipped, absolute
        // GIT_WORK_TREE is skipped, unrelated vars are never consulted.
        assert_eq!(overrides.len(), 2);
        let as_map: std::collections::HashMap<_, _> = overrides.into_iter().collect();
        assert_eq!(
            as_map.get("GIT_DIR"),
            Some(&base.join(".git").into_os_string())
        );
        assert_eq!(
            as_map.get("GIT_INDEX_FILE"),
            Some(&base.join("../index").into_os_string())
        );
    }

    #[test]
    fn test_compute_git_env_overrides_all_absolute() {
        let base_buf = std::env::temp_dir().join("wt-test-startup-cwd");
        let abs_git = std::env::temp_dir().join("wt-test-abs.git");
        let env: std::collections::HashMap<&str, OsString> =
            [("GIT_DIR", abs_git.into_os_string())]
                .into_iter()
                .collect();

        let overrides = compute_git_env_overrides(base_buf.as_path(), |var| env.get(var).cloned());
        assert!(overrides.is_empty());
    }

    #[test]
    fn test_compute_git_env_overrides_all_unset() {
        let base_buf = std::env::temp_dir().join("wt-test-startup-cwd");
        let overrides = compute_git_env_overrides(base_buf.as_path(), |_| None);
        assert!(overrides.is_empty());
    }

    #[test]
    fn test_max_concurrent_commands_defaults() {
        // When no env var is set, default should be used
        assert!(max_concurrent_commands() >= 1, "Default should be >= 1");
        assert_eq!(
            max_concurrent_commands(),
            DEFAULT_CONCURRENT_COMMANDS,
            "Without env var, should use default"
        );
    }

    #[test]
    fn test_parse_concurrent_limit() {
        // Normal values pass through unchanged
        assert_eq!(parse_concurrent_limit("1"), Some(1));
        assert_eq!(parse_concurrent_limit("32"), Some(32));
        assert_eq!(parse_concurrent_limit("100"), Some(100));

        // 0 means unlimited (maps to usize::MAX)
        assert_eq!(parse_concurrent_limit("0"), Some(usize::MAX));

        // Invalid values return None
        assert_eq!(parse_concurrent_limit(""), None);
        assert_eq!(parse_concurrent_limit("abc"), None);
        assert_eq!(parse_concurrent_limit("-1"), None);
        assert_eq!(parse_concurrent_limit("1.5"), None);
    }

    #[test]
    fn test_shell_config_is_available() {
        let config = ShellConfig::get().unwrap();
        assert!(!config.name.is_empty());
        assert!(!config.args.is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn test_unix_shell_is_posix() {
        let config = ShellConfig::get().unwrap();
        assert!(config.is_posix);
        assert_eq!(config.name, "sh");
    }

    #[test]
    fn test_command_creation() {
        let config = ShellConfig::get().unwrap();
        let cmd = config.command("echo hello");
        // Just verify it doesn't panic
        let _ = format!("{:?}", cmd);
    }

    #[test]
    fn test_shell_command_execution() {
        let config = ShellConfig::get().unwrap();
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
        let config = ShellConfig::get().unwrap();
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
        let config = ShellConfig::get().unwrap();
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
        let config = ShellConfig::get().unwrap();
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
    fn test_shell_config_clone() {
        let config = ShellConfig::get().unwrap();
        let cloned = config.clone();
        assert_eq!(config.name, cloned.name);
        assert_eq!(config.is_posix, cloned.is_posix);
        assert_eq!(config.args, cloned.args);
    }

    #[test]
    fn test_shell_is_posix_method() {
        let config = ShellConfig::get().unwrap();
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
    fn test_cmd_stream_sigpipe_is_not_an_error() {
        // Simulates pager quit: the child is killed by SIGPIPE, same as when
        // `git diff` writes to a pager and the user presses `q`.
        // `sh -c 'kill -PIPE $$'` sends SIGPIPE to itself, terminating with signal 13.
        let result = Cmd::new("sh").args(["-c", "kill -PIPE $$"]).stream();
        assert!(
            result.is_ok(),
            "SIGPIPE should not be treated as an error: {result:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_cmd_stream_other_signals_are_errors() {
        use crate::git::WorktrunkError;

        // Non-SIGPIPE signals (like SIGTERM) should still be treated as errors.
        let result = Cmd::new("sh").args(["-c", "kill -TERM $$"]).stream();
        assert!(result.is_err());

        let err = result.unwrap_err();
        let wt_err = err.downcast_ref::<WorktrunkError>().unwrap();
        match wt_err {
            WorktrunkError::ChildProcessExited { code, .. } => {
                assert_eq!(*code, 128 + 15); // SIGTERM = 15
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

    #[test]
    fn test_format_stream_full_empty() {
        assert!(format_stream_full(b"", "  ").is_empty());
    }

    #[test]
    fn test_format_stream_full_prefixes_each_line() {
        let lines = format_stream_full(b"alpha\nbeta\ngamma\n", "  ");
        assert_eq!(lines, vec!["  alpha", "  beta", "  gamma"]);
    }

    #[test]
    fn test_format_stream_full_stderr_prefix() {
        let lines = format_stream_full(b"err1\nerr2\n", "  ! ");
        assert_eq!(lines, vec!["  ! err1", "  ! err2"]);
    }

    #[test]
    fn test_format_stream_bounded_empty() {
        assert!(format_stream_bounded(b"", "  ").is_empty());
    }

    #[test]
    fn test_format_stream_bounded_below_caps_emits_all() {
        let lines = format_stream_bounded(b"one\ntwo\nthree\n", "  ");
        assert_eq!(lines, vec!["  one", "  two", "  three"]);
    }

    #[test]
    fn test_format_stream_bounded_line_cap_triggers_elision() {
        // Build LOG_OUTPUT_MAX_LINES + 5 short lines so the line cap trips first.
        let input: String = (0..LOG_OUTPUT_MAX_LINES + 5)
            .map(|i| format!("line{i}\n"))
            .collect();
        let lines = format_stream_bounded(input.as_bytes(), "  ");

        assert_eq!(lines.len(), LOG_OUTPUT_MAX_LINES + 1, "cap + 1 marker");
        let marker = lines.last().unwrap();
        assert!(
            marker.starts_with("  … (5 more lines, "),
            "marker should count the 5 lines past the cap: {marker}"
        );
        assert!(marker.contains("use -vvv for full output"));
    }

    #[test]
    fn test_format_stream_bounded_byte_cap_triggers_elision() {
        // One long line past the byte cap, then extra lines.
        let long = "x".repeat(LOG_OUTPUT_MAX_BYTES + 100);
        let input = format!("{long}\nafter1\nafter2\n");
        let lines = format_stream_bounded(input.as_bytes(), "  ");

        // The long first line gets emitted (bytes_emitted==0 at entry); the
        // byte cap trips on the next iteration and the remaining 2 lines are elided.
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), 2 + long.len());
        let marker = &lines[1];
        assert!(
            marker.starts_with("  … (2 more lines, "),
            "marker should count after1 + after2: {marker}"
        );
    }
}
