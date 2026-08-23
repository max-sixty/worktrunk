//! Cross-platform shell execution
//!
//! Provides a unified interface for executing shell commands across platforms:
//! - Unix: Uses `sh -c` (resolved via PATH)
//! - Windows: Uses Git Bash (requires Git for Windows)
//!
//! This enables hooks and commands to use the same bash syntax on all platforms.
//! On Windows, Git for Windows must be installed — this is nearly universal among
//! Windows developers since git itself is required.
//!
//! ## Process groups and signal handling (Unix, `Cmd::stream`)
//!
//! Foreground children spawned through `Cmd::stream()` fall into one of two
//! shapes, selected by the `forward_signals` / `share_parent_pgroup` pair:
//!
//! - **Isolated** (`forward_signals()` alone): the child gets its own process
//!   group via `process_group(0)`. A signal_hook listener catches SIGINT/
//!   SIGTERM in wt and `killpg`s the child group with SIGINT→SIGTERM→SIGKILL
//!   escalation. Used for non-interactive children that may fork further
//!   subprocesses (hook pipelines, alias steps that read from stdin) — `killpg`
//!   reaches the whole subtree, which a shared-pgroup approach cannot.
//!
//! - **Shared-tty** (`forward_signals().inherit_stdin()`): the child stays in
//!   wt's process group so it can drive `/dev/tty` (raw mode, `tcsetattr`)
//!   without the kernel raising SIGTTOU. Tty-initiated signals (Ctrl-C, hangup)
//!   reach the child via the kernel's foreground-pgroup broadcast; the listener
//!   additionally delivers externally-targeted signals (e.g. `kill -TERM
//!   <wt-pid>`) to the child by PID, single-shot. Used for interactive TUIs
//!   (skim picker, pagers, `$EDITOR`).
//!
//! In both cases the listener still records `seen_signal`, so a signal-derived
//! exit surfaces as `WorktrunkError::ChildProcessExited { signal: Some(_) }` —
//! the structured channel that loop callers (`for-each`, hook/alias pipelines)
//! use to abort their loops on Ctrl-C rather than continuing to the next
//! iteration. See `git/interrupt_signal` for the consumer side.
//!
//! Concurrent foreground children (`output/concurrent.rs`) and detached
//! background children (`commands/process.rs::spawn_detached_*`) have separate
//! spawn paths; both isolate-by-default for the same `killpg` reason. They
//! never share wt's pgroup because they don't drive the tty (concurrent uses
//! piped stdio, detached escapes the PTY entirely).
//!
//! ## Cancelling background children
//!
//! `wt` exiting ends its own threads but not the children they spawned, which
//! keep running as orphans. [`cancel_background_commands`] lets the foreground
//! thread stop that work — both what is running and what has yet to start —
//! once nobody is left to read its results.
//!
//! ## Timed waits
//!
//! Unix has no "wait for this child, but give up after N milliseconds" syscall,
//! so every deadline here (`Cmd::timeout`, `Cmd::delayed_stream`, the picker's
//! pager) goes through [`shared_child::SharedChild`]: `waitid(WNOWAIT)` for the
//! blocking wait, and for the timed one a `SIGCHLD` self-pipe it polls against
//! the deadline. Windows needs none of that — it waits the process handle.
//!
//! **Why not `wait-timeout`.** wt used it until #3856. Its `SIGCHLD` handler
//! pokes an `AF_UNIX` socketpair with `send()` and `panic!`s on any errno but
//! `WouldBlock`; the handler is `extern "C"`, so that panic cannot unwind and
//! goes straight to `abort()`. Under a sandbox that denies the send (the Codex
//! CLI's `workspace-write` mode) every timed wait in wt became an uncatchable
//! `SIGABRT` with no diagnostic. `shared_child` reaches the same signal through
//! `signal_hook`, whose wake deliberately discards write errors — a missed
//! wakeup costs at worst a wait that runs to its deadline, which is what a
//! deadline is for. It also probes the wake fd and falls back to `write()` on a
//! pipe, so the syscall that sandbox denies is not even on the path.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs::Metadata;
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Context;
use shared_child::SharedChild;

use crate::git::{GitError, WorktrunkError};
use crate::sync::Semaphore;
use crate::trace::CommandTrace;

/// Semaphore to limit concurrent command execution.
/// Prevents resource exhaustion when spawning many parallel git commands.
///
/// Only background threads consume permits. The foreground thread runs
/// commands one at a time, so it can't contribute to fan-out — and it is what
/// the user is waiting on: were it to queue here, background work that
/// saturates the permits (per-row preview diffs in the picker, each holding a
/// permit for seconds on a large repo) would stall the accept path of
/// `wt switch` until the pool drained. The cap is deliberately approximate:
/// a work-stealing pool (rayon) can run a capped closure on the foreground
/// thread, briefly exceeding the limit.
static CMD_SEMAPHORE: OnceLock<Semaphore> = OnceLock::new();

/// The thread that entered `main()`, captured by [`init_startup`]. Commands
/// on this thread bypass [`CMD_SEMAPHORE`] (see there). Unset when `wt` runs
/// as a library (unit tests), where every thread is capped.
static FOREGROUND_THREAD: OnceLock<std::thread::ThreadId> = OnceLock::new();

fn is_foreground_thread() -> bool {
    FOREGROUND_THREAD.get() == Some(&std::thread::current().id())
}

/// PIDs of capture-mode commands currently running on background threads, so
/// the foreground thread can cancel them once nobody will read their results.
///
/// A background thread dies with the process, but the `git` child it spawned
/// does not — it keeps running, orphaned, against a repo `wt` has already
/// left. The picker is where this bites: accepting a row abandons one preview
/// diff per worktree, each able to churn disk for seconds on a large repo,
/// filling an in-memory cache that no longer exists.
///
/// Only cancellable threads register ([`is_cancellable_thread`]): the
/// foreground thread is the one that cancels, and is never itself inside a
/// tracked command while doing so, so the work the user is actually waiting
/// on is never a target; an [`uninterruptible`] thread finishes what it
/// started.
static BACKGROUND_PIDS: Mutex<BTreeSet<u32>> = Mutex::new(BTreeSet::new());

/// Deregisters a background command's PID however the command finishes.
///
/// The child is reaped inside the `wait` this guard outlives, so for the few
/// instructions between that reap and this `drop` a freed PID is still listed,
/// and a sweep landing in that window would signal whatever the kernel handed
/// the number to next. Signalling by PID can't close this — the reap and the
/// deregistration are not one operation — and deregistering before the wait
/// instead is not a fix but a removal: the wait *is* the command's lifetime,
/// so nothing would ever be cancellable. Accepted rather than mitigated:
/// PID allocation is incremental up to `pid_max`, which makes reuse inside a
/// microsecond window require wrapping the entire PID space first.
struct BackgroundPid(u32);

impl Drop for BackgroundPid {
    fn drop(&mut self) {
        BACKGROUND_PIDS.lock().unwrap().remove(&self.0);
    }
}

std::thread_local! {
    /// Whether this thread is running work cancellation must not touch. See
    /// [`uninterruptible`].
    static UNINTERRUPTIBLE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `f` with this thread's commands exempt from cancellation, for work the
/// user has already asked for rather than work done on spec.
///
/// Cancelling is safe for a preview because its only effect is a cache entry
/// nobody will read. It is not safe for a mutation: the picker runs an `alt-x`
/// worktree removal on a background thread so the UI stays live, and a SIGTERM
/// landing between `git worktree remove` and the branch delete would leave the
/// user half-removed. Such a thread finishes what it started; only its result
/// is discardable, not its effects.
pub fn uninterruptible<T>(f: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            UNINTERRUPTIBLE.with(|flag| flag.set(self.0));
        }
    }

    let _restore = Restore(UNINTERRUPTIBLE.with(|flag| flag.replace(true)));
    f()
}

/// Whether this thread's commands are subject to cancellation: background (the
/// foreground thread is the one doing the cancelling, and goes on to run the
/// switch itself) and not marked [`uninterruptible`].
fn is_cancellable_thread() -> bool {
    !is_foreground_thread() && !UNINTERRUPTIBLE.with(std::cell::Cell::get)
}

/// Register a freshly spawned child as cancellable for as long as the returned
/// guard lives. Returns `None` when this thread's commands aren't subject to
/// cancellation (see [`is_cancellable_thread`]).
fn track_if_cancellable(pid: u32) -> Option<BackgroundPid> {
    is_cancellable_thread().then(|| {
        BACKGROUND_PIDS.lock().unwrap().insert(pid);
        let guard = BackgroundPid(pid);
        // Re-read after publishing the PID, closing the window between this
        // command's pre-spawn check and its registration. Either the sweep
        // takes the set lock after the insert and signals this PID itself, or
        // it ran first — in which case the store it followed is visible here
        // and the signal it couldn't deliver is delivered now. A child spawned
        // into that window is exactly the orphan the sweep exists to prevent.
        if BACKGROUND_CANCELLED.load(Ordering::SeqCst) {
            signal_background_pid(pid);
        }
        guard
    })
}

/// Set once the foreground thread has cancelled background work, so commands
/// that haven't spawned yet never do.
///
/// Cancellation has to be a state, not a one-shot sweep over
/// [`BACKGROUND_PIDS`]. A task that already cleared its caller's own
/// supersede check and then parked on [`CMD_SEMAPHORE`] holds no PID for a
/// sweep to find, and would spawn the moment a permit frees — precisely the
/// permits the sweep just freed by signalling everything holding one.
static BACKGROUND_CANCELLED: AtomicBool = AtomicBool::new(false);

/// Whether the calling thread's commands have been cancelled.
fn background_cancelled() -> bool {
    is_cancellable_thread() && BACKGROUND_CANCELLED.load(Ordering::SeqCst)
}

fn cancelled_error() -> std::io::Error {
    std::io::Error::new(ErrorKind::Interrupted, "background command cancelled")
}

/// Abandon background work: nothing further spawns, and whatever is already
/// running is signalled rather than left to finish as an orphan.
///
/// Callers see either as an ordinary command failure, which every background
/// caller already treats as "no result".
pub fn cancel_background_commands() {
    BACKGROUND_CANCELLED.store(true, Ordering::SeqCst);
    for &pid in BACKGROUND_PIDS.lock().unwrap().iter() {
        signal_background_pid(pid);
    }
}

/// SIGTERM rather than SIGKILL: git's lockfile handlers run on the former, so
/// a diff interrupted mid-index-refresh cleans up after itself instead of
/// stranding an `index.lock` in a worktree the user is about to work in.
#[cfg(unix)]
fn signal_background_pid(pid: u32) {
    forward_signal_to_pid(pid as i32, signal_hook::consts::SIGTERM);
}

/// Windows has no signal to deliver to an unrelated PID, so a command already
/// running there runs to completion; the latch still stops everything that
/// hasn't spawned, which is the bulk of a large fan-out.
#[cfg(windows)]
fn signal_background_pid(_pid: u32) {}

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
///
/// Also consumed by [`crate::testing::scrub_git_path_vars`] so test/bench
/// helpers strip the same list before spawning a `git` subprocess that
/// targets an explicit path.
pub const INHERITED_GIT_PATH_VARS: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
];

/// Record process-startup facts: the current working directory (so relative
/// `GIT_*` path variables inherited from a parent process can later be
/// resolved to absolute paths by [`Cmd`]'s env setup) and the calling thread
/// (the foreground thread, exempt from the command semaphore —
/// see `CMD_SEMAPHORE`).
///
/// Call once from `main()`, before any code changes the process's working
/// directory and before spawning threads. Subsequent calls are no-ops.
pub fn init_startup() {
    STARTUP_CWD.get_or_init(|| std::env::current_dir().ok());
    FOREGROUND_THREAD.get_or_init(|| std::thread::current().id());
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

fn ensure_executable_path(path: &Path) -> std::io::Result<()> {
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(std::io::Error::new(
            ErrorKind::PermissionDenied,
            format!("{} is not an executable file", path.display()),
        ));
    }
    if !is_executable(&metadata) {
        return Err(std::io::Error::new(
            ErrorKind::PermissionDenied,
            format!("{} is not executable", path.display()),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn is_executable(metadata: &Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;

    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &Metadata) -> bool {
    true
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
    /// Whether this shell supports POSIX syntax (bash, sh, zsh, etc.).
    ///
    /// When true, commands can use POSIX features like:
    /// - `{ cmd; } 1>&2` for stdout redirection
    /// - `printf '%s' ... | cmd` for stdin piping
    /// - `nohup ... &` for background execution
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
/// file contents are arbitrary shell, wt scrubs this from project-alias and
/// hook child environments — a body authored in shared config could inject
/// shell into the parent shell. User-source aliases re-add it (issue #2101)
/// since the body is user-authored just like a top-level `wt --execute`.
pub const DIRECTIVE_EXEC_FILE_ENV_VAR: &str = "WORKTRUNK_DIRECTIVE_EXEC_FILE";

/// Retired pre-split directive file env var.
///
/// wt never writes to or passes this file through, but still removes the
/// variable from child environments so an old wrapper cannot expose a file
/// that its parent shell will source.
pub const RETIRED_DIRECTIVE_FILE_ENV_VAR: &str = "WORKTRUNK_DIRECTIVE_FILE";

/// Environment variable carrying the directory the user's shell is in.
///
/// A top-level `wt` reads that directory from its own process cwd, which is
/// what subdirectory preservation resolves the user's position against (from
/// `monorepo.feature/subproject/`, `wt switch main` lands in
/// `monorepo/subproject/`). A `wt` nested inside an alias or hook body cannot:
/// those bodies run with a wt-chosen working directory (the worktree root), so
/// the nested process would resolve the user to that root and drop them there.
///
/// [`apply_cd_directive_env`] therefore sets this alongside the CD directive
/// file — the same children that are allowed to move the user's shell are the
/// ones that need to know where it currently is. Nesting composes because
/// [`shell_cwd`] prefers the inherited value over the process cwd, so each
/// layer forwards the shell's directory rather than its own. See issue #3723.
///
/// Private, unlike the directive-file vars above: the setter, the scrubber and
/// the reader all live in this module, and a name spells the coupling between
/// them that a repeated literal would leave to a spelling match.
const SHELL_CWD_ENV_VAR: &str = "WORKTRUNK_SHELL_CWD";

/// The directory the user's shell is in: the inherited `WORKTRUNK_SHELL_CWD`
/// when a parent `wt` set it, otherwise this process's own startup cwd.
///
/// Callers that ask "where is the user physically standing?" use this rather
/// than [`std::env::current_dir`]. `-C` is deliberately not consulted: it sets
/// git's discovery path without moving the shell.
pub fn shell_cwd() -> Option<PathBuf> {
    shell_cwd_from(std::env::var_os(SHELL_CWD_ENV_VAR), startup_cwd())
}

/// Pure form of [`shell_cwd`], split out so the relative-value fall-through is
/// unit-testable without mutating process-wide state — the same split
/// [`compute_git_env_overrides`] makes for [`inherited_git_env_overrides`].
fn shell_cwd_from(inherited: Option<OsString>, fallback: Option<&PathBuf>) -> Option<PathBuf> {
    if let Some(value) = inherited {
        let path = PathBuf::from(value);
        // A relative value can't name the shell's position from a child that
        // runs elsewhere, so fall through to the process cwd. A stale absolute
        // one needs no guard here: the subdirectory it resolves to has to exist
        // in the destination before anything `cd`s there.
        if path.is_absolute() {
            return Some(path);
        }
    }
    fallback.cloned()
}

/// Scrub all directive file env vars from a `std::process::Command`.
///
/// Prevents child processes from writing to the parent shell's directive
/// files. Called by every code path that spawns external commands (Cmd,
/// help pager, picker pager, background hooks, git credential helpers).
///
/// `WORKTRUNK_SHELL_CWD` goes with them: it is meaningful only to a child that
/// can act on the shell's position, and those children get it back from
/// [`apply_cd_directive_env`].
pub fn scrub_directive_env_vars(cmd: &mut std::process::Command) {
    cmd.env_remove(DIRECTIVE_CD_FILE_ENV_VAR);
    cmd.env_remove(DIRECTIVE_EXEC_FILE_ENV_VAR);
    cmd.env_remove(RETIRED_DIRECTIVE_FILE_ENV_VAR);
    cmd.env_remove(SHELL_CWD_ENV_VAR);
}

/// Re-add the CD directive file to a trusted child's environment, together
/// with the shell cwd that makes its `cd` land where the user actually is.
///
/// The two travel together by construction: a child that can redirect the
/// parent shell resolves the user's subdirectory against [`shell_cwd`], and
/// without it every nested `wt switch` / `wt remove` inside an alias or hook
/// body would `cd` to a worktree root (issue #3723).
pub fn apply_cd_directive_env(cmd: &mut std::process::Command, cd_file: &std::path::Path) {
    cmd.env(DIRECTIVE_CD_FILE_ENV_VAR, cd_file);
    if let Some(cwd) = shell_cwd() {
        cmd.env(SHELL_CWD_ENV_VAR, cwd);
    }
}

/// Scrub the git-discovery path vars ([`INHERITED_GIT_PATH_VARS`]) from a child
/// `Command`, so the spawned process discovers its repository from its working
/// directory rather than a `GIT_DIR`/`GIT_WORK_TREE` that `wt` inherited.
///
/// Git resolves these vars **before** walking up from the cwd, so an inherited
/// value silently overrides whatever working directory the child was given.
/// Whether a child keeps them is decided by who chose its cwd:
///
/// - **`wt` relocated a user command into a worktree it selected** — hooks
///   (run in the operation's worktree), `wt step for-each` (run in each
///   worktree in turn), and the `--execute` no-integration fallback (run in
///   the switch target when `wt` itself executes the payload) — the cwd
///   carries `wt`'s intent, so the inherited context is scrubbed and the
///   command's `git` calls discover the worktree from the cwd. The inherited
///   context is common, not exotic: `git` exports an absolute `GIT_DIR`
///   pinned to the invoking worktree's private gitdir when `wt` runs as a
///   `!wt` alias from a **linked worktree**, and git itself exports discovery
///   vars (e.g. `GIT_INDEX_FILE`) to the hooks it spawns. Forwarding those
///   into a relocated command misdirects every `git` call in it; with both
///   `GIT_DIR` and `GIT_WORK_TREE` present, a `git init` even writes
///   `core.worktree` into the *inherited* repo's config, silently redirecting
///   every later plain git command there. See issue #3373.
///
/// - **The child runs where the user already was** — aliases (the user's own
///   top-level command, run from the invoking worktree) and `commit.generation`
///   commands (spawned with no `current_dir`) — the inherited context *is* the
///   user's context, so it is forwarded untouched. Under shell integration the
///   `--execute` payload also lands here: the wrapper shell evaluates it, so
///   it sees that shell's own environment (which never contains the vars a
///   `!wt` git alias exports — git's exports die with `wt`'s process tree).
///   Scrubbing the fallback therefore *converges* the two `--execute` paths
///   for the alias case; only a `GIT_DIR` the user globally exported in their
///   interactive shell still differs, and such an env misdirects every plain
///   `git` command they run anyway.
///
/// - **`wt`'s own git plumbing** ([`Cmd`] via `Repository::run_command`) keeps
///   the inherited context on purpose (relative values absolutized, see issue
///   #1914): `wt` honoring the context it was handed is the point of running
///   `wt` under `git`.
///
/// Any new spawn site that relocates a user command into a `wt`-chosen
/// worktree must apply this scrub, via this helper or
/// [`Cmd::scrub_git_discovery_env`].
pub fn scrub_git_discovery_env_vars(cmd: &mut std::process::Command) {
    for var in INHERITED_GIT_PATH_VARS {
        cmd.env_remove(var);
    }
}

/// The hermetic git-config floor for test processes: the deny pair points
/// global and system config at a path that does not exist (git reads a
/// missing config file as empty), and `GIT_CONFIG_COUNT` with its numbered
/// keys and values — git's environment spelling of `-c` — supplies the two
/// settings the suite needs in the denied config's place. What each entry is
/// for, and why `-c` precedence keeps this list short: `tests/CLAUDE.md` →
/// Git Config Isolation.
pub const HERMETIC_TEST_GIT_ENV: [(&str, &str); 7] = [
    ("GIT_CONFIG_GLOBAL", "/nonexistent/wt/gitconfig"),
    ("GIT_CONFIG_SYSTEM", "/nonexistent/wt/gitconfig"),
    ("GIT_CONFIG_COUNT", "2"),
    ("GIT_CONFIG_KEY_0", "user.useConfigOnly"),
    ("GIT_CONFIG_VALUE_0", "true"),
    ("GIT_CONFIG_KEY_1", "rerere.enabled"),
    ("GIT_CONFIG_VALUE_1", "false"),
];

/// When latched, every child spawned through [`Cmd`] gets
/// [`HERMETIC_TEST_GIT_ENV`] — including the git that *production* code
/// spawns while a test drives it in-process, which no per-command harness
/// hook can reach. The test harness latches it before the first fixture;
/// production code never does. An in-process test cannot set its own
/// environment instead — `std::env::set_var` races the other test threads —
/// but an atomic latch is sound from any thread.
///
/// TODO(hermetic-env): a test-serving switch in production code, accepted as
/// the pragmatic middle over its alternatives — a pre-`main` constructor
/// crate every test target must link, or threading an explicit env value
/// through `Repository`, which is the structural fix.
static HERMETIC_TEST_ENV_LATCHED: AtomicBool = AtomicBool::new(false);

/// Latch [`HERMETIC_TEST_GIT_ENV`] onto every future [`Cmd`] child in this
/// process. Called by the `worktrunk::testing` harness; idempotent.
pub fn enable_hermetic_test_env() {
    HERMETIC_TEST_ENV_LATCHED.store(true, Ordering::Relaxed);
}

/// Apply [`HERMETIC_TEST_GIT_ENV`] to `cmd` if the latch is set. The unlatched
/// path is production's: one relaxed load, no env writes.
pub fn apply_hermetic_test_env(cmd: &mut std::process::Command) {
    if HERMETIC_TEST_ENV_LATCHED.load(Ordering::Relaxed) {
        for (key, val) in HERMETIC_TEST_GIT_ENV {
            cmd.env(key, val);
        }
    }
}

// ============================================================================
// Directive-Payload Shell Escaping
// ============================================================================

/// Environment variable a shell wrapper sets to identify itself.
///
/// The PowerShell wrapper sets `WORKTRUNK_SHELL=powershell` and the fish
/// wrapper sets `WORKTRUNK_SHELL=fish`; bash, zsh, and nushell leave it unset.
/// Absence therefore means "POSIX", which is correct for those three wrappers
/// *and* for the non-integration path where wt runs the `--execute` payload
/// itself through `sh -c`.
pub const WORKTRUNK_SHELL_ENV_VAR: &str = "WORKTRUNK_SHELL";

/// How to single-quote a value spliced into a directive payload.
///
/// `wt switch --execute` builds one command string and hands it to the active
/// shell — directly (`sh -c`) or, under shell integration, by writing it to the
/// EXEC directive file the wrapper evaluates. POSIX shells, PowerShell, and
/// fish all single-quote, but escape the string body differently — POSIX takes
/// `\` literally inside `'…'` while fish treats it as an escape — so the
/// escaper must be keyed on which shell will parse the payload.
///
/// `Literal` is the no-escaping mode for filesystem-path templates that are
/// never spliced into a shell command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellEscapeMode {
    /// Substitute values verbatim — used for filesystem paths.
    Literal,
    /// POSIX single-quoting (`'it'\''s'`). The wrapper-independent default for
    /// bash, zsh, and nushell, plus `Cmd::shell` (hooks, aliases).
    Posix,
    /// PowerShell single-quoting (`'it''s'`), for the PowerShell wrapper's
    /// `Invoke-Expression` of the EXEC directive file.
    PowerShell,
    /// fish single-quoting (`'it\'s'`), for the fish wrapper's `eval` of the
    /// EXEC directive file. fish — unlike POSIX — treats `\` as an escape
    /// inside `'…'`, so the POSIX escaper corrupts backslashes there.
    Fish,
}

/// Escape mode for a payload the *active directive shell* will parse.
///
/// Reads [`WORKTRUNK_SHELL_ENV_VAR`] — the single source of truth for the
/// per-shell escaping decision shared by the `--execute` command template and
/// its trailing args. `powershell` ⇒
/// [`ShellEscapeMode::PowerShell`], `fish` ⇒ [`ShellEscapeMode::Fish`], any
/// other value or absent ⇒ [`ShellEscapeMode::Posix`].
pub fn directive_shell_escape_mode() -> ShellEscapeMode {
    match std::env::var(WORKTRUNK_SHELL_ENV_VAR) {
        Ok(v) if v.eq_ignore_ascii_case("powershell") => ShellEscapeMode::PowerShell,
        Ok(v) if v.eq_ignore_ascii_case("fish") => ShellEscapeMode::Fish,
        _ => ShellEscapeMode::Posix,
    }
}

/// Single-quote `s` for PowerShell: wrap in `'…'`, doubling every embedded `'`.
///
/// PowerShell's literal string is `'…'` with `''` as the escape for one quote
/// (`'can''t'` is `can't`). The POSIX `'\''` idiom is invalid here — that is
/// the bug B1 fixes for the PowerShell wrapper's `Invoke-Expression` path.
/// Empty input yields `''`.
pub fn powershell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Single-quote `s` for fish: wrap in `'…'`, backslash-escaping `\` and `'`.
///
/// fish's single-quoted string — unlike POSIX — treats `\` as an escape
/// character, with only `\\` and `\'` recognized inside `'…'`. So `\` must be
/// doubled and `'` backslash-escaped; backslash *before* quote, since escaping
/// the quote introduces a backslash that must not be doubled again. The POSIX
/// `'\''` idiom would corrupt backslashes (and leave a trailing-`\` argument
/// as an unterminated string) under the fish wrapper's `eval`. Empty input
/// yields `''`.
pub fn fish_escape(s: &str) -> String {
    format!("'{}'", s.replace('\\', r"\\").replace('\'', r"\'"))
}

/// Shell-escape `s` for the given [`ShellEscapeMode`].
///
/// The single dispatcher every directive-payload escaper routes through.
pub fn shell_escape_for(mode: ShellEscapeMode, s: &str) -> String {
    match mode {
        ShellEscapeMode::Literal => s.to_string(),
        ShellEscapeMode::Posix => {
            shell_escape::unix::escape(std::borrow::Cow::Borrowed(s)).into_owned()
        }
        ShellEscapeMode::PowerShell => powershell_escape(s),
        ShellEscapeMode::Fish => fish_escape(s),
    }
}

/// Maximum lines of the bounded subprocess preview per stream. Exceeded
/// content is elided with a `… (N more lines, M bytes elided)` marker; the
/// full output is still written to `subprocess.log` via
/// [`SUBPROCESS_FULL_TARGET`].
const LOG_OUTPUT_MAX_LINES: usize = 200;

/// Maximum bytes of the bounded subprocess preview per stream. Applied in
/// addition to [`LOG_OUTPUT_MAX_LINES`].
const LOG_OUTPUT_MAX_BYTES: usize = 64 * 1024;

/// Log target used for *full* subprocess stdin/stdout/stderr (and the per-command
/// `$ cmd … seq=N` header `log_output` prepends to each block). The
/// tracing-subscriber `subprocess.log` layer filters on this target, so raw
/// subprocess bodies (diffs, `git log -p`, patch-id pipelines) never reach
/// stderr or `trace.log`.
pub const SUBPROCESS_FULL_TARGET: &str = "worktrunk::subprocess_full";

/// Log target used for the *bounded* preview of subprocess output (capped
/// at `LOG_OUTPUT_MAX_LINES` / `LOG_OUTPUT_MAX_BYTES` with an elision
/// marker). Shares the routing of all non-full records: stderr at `-v`
/// (or `RUST_LOG=debug` without `-vv`), `trace.log` at `-vv`. The
/// uncapped version is captured via [`SUBPROCESS_FULL_TARGET`].
pub const SUBPROCESS_BOUNDED_TARGET: &str = "worktrunk::subprocess_bounded";

/// The `$ <cmd> [<context>]` command header. Shared by the stderr/`trace.log`
/// start line ([`Cmd::log_run_start`] and friends) and the per-command header
/// that [`log_output`] prepends to each block in `subprocess.log`, so the two
/// render the command identically.
fn command_header(cmd: &str, context: Option<&str>) -> String {
    match context {
        Some(ctx) => format!("$ {cmd} [{ctx}]"),
        None => format!("$ {cmd}"),
    }
}

/// Log a command's captured stdin, stdout, and stderr to the debug targets.
///
/// At `tracing::DEBUG` (`-vv`) each stream is emitted twice, and the
/// tracing-subscriber layers route the two targets:
///   - [`SUBPROCESS_FULL_TARGET`] → `subprocess.log`: uncapped, one record per
///     line. Each command with I/O opens with a `$ cmd … [seq=N tid=T]` header
///     over its stdin (`  < `), stdout (`  `), and stderr (`  ! `) lines, so the
///     otherwise-undelimited bytes segment into blocks and `seq` joins each
///     block to its `[wt-trace]` record.
///   - [`SUBPROCESS_BOUNDED_TARGET`] → `trace.log` at `-vv` (else stderr):
///     capped at [`LOG_OUTPUT_MAX_LINES`] / [`LOG_OUTPUT_MAX_BYTES`] with an
///     elision marker, under the `$ cmd` start line `trace.log` already carries.
///
/// One record per line keeps a command's own output contiguous but lets
/// concurrent commands interleave by line; `tid` groups a block and the atomic
/// header recovers the `seq` join.
///
/// `output` is `None` for a command that never produced any (spawn or wait
/// failure); its header and stdin still emit, so the input survives in the deep
/// log even when nothing ran.
///
/// Below Debug both targets are disabled and this is a no-op.
fn log_output(trace: &CommandTrace, stdin: Option<&[u8]>, output: Option<&std::process::Output>) {
    // `log::max_level` (held at the verbosity/`RUST_LOG` ceiling by the
    // `LogTracer` cap in `logging::init`) is the coarse "deep logging on at
    // all?" gate that skips building the full *and* bounded output strings
    // when nothing consumes them. It stays a `log::*` check on purpose: this
    // body feeds both `SUBPROCESS_FULL_TARGET` and `SUBPROCESS_BOUNDED_TARGET`,
    // so the gate must fire whenever *either* deep sink is live — exactly the
    // verbosity cap. A per-target `tracing::enabled!(SUBPROCESS_FULL_TARGET …)`
    // would wrongly skip the bounded preview in the rare case where `-vv`
    // opened `trace.log` but `subprocess.log` failed to open.
    if !log::log_enabled!(log::Level::Debug) {
        return;
    }
    let stdin = stdin.unwrap_or_default();
    // `None` when the command never produced output (spawn/wait failure); stdin
    // is still logged so the input survives in the deep log.
    let (stdout, stderr) = output
        .map(|o| (o.stdout.as_slice(), o.stderr.as_slice()))
        .unwrap_or_default();
    if !stdin.is_empty() || !stdout.is_empty() || !stderr.is_empty() {
        tracing::debug!(
            target: SUBPROCESS_FULL_TARGET,
            "{}  [seq={} tid={}]",
            command_header(trace.cmd(), trace.context()),
            trace.seq(),
            trace.tid(),
        );
    }
    for line in format_stream_full(stdin, "  < ") {
        tracing::debug!(target: SUBPROCESS_FULL_TARGET, "{}", line);
    }
    for line in format_stream_full(stdout, "  ") {
        tracing::debug!(target: SUBPROCESS_FULL_TARGET, "{}", line);
    }
    for line in format_stream_full(stderr, "  ! ") {
        tracing::debug!(target: SUBPROCESS_FULL_TARGET, "{}", line);
    }
    for line in format_stream_bounded(stdout, "  ") {
        tracing::debug!(target: SUBPROCESS_BOUNDED_TARGET, "{}", line);
    }
    for line in format_stream_bounded(stderr, "  ! ") {
        tracing::debug!(target: SUBPROCESS_BOUNDED_TARGET, "{}", line);
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
/// `… (N more lines, M bytes elided — <hint>)` marker. The hint text tracks
/// whether `subprocess.log` is collecting the full bodies, asked through
/// `tracing::enabled!` against [`SUBPROCESS_FULL_TARGET`] — true iff the
/// `subprocess.log` layer is registered and accepting that target (`-vv` opened
/// the file successfully).
///
/// Preview lines are emitted raw here; control bytes (most commonly NUL from
/// `-z`/`--null` git output) are escaped downstream at the single point that
/// feeds both human-facing sinks — `render_event_message` in the binary's
/// logging layer, which renders this target's records to stderr and `trace.log`.
/// The uncapped [`SUBPROCESS_FULL_TARGET`] copy bypasses that escape and keeps
/// the bytes verbatim in `subprocess.log`.
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
            let hint = if tracing::enabled!(target: SUBPROCESS_FULL_TARGET, tracing::Level::DEBUG) {
                "full output in subprocess.log"
            } else {
                "rerun with -vv for full output"
            };
            out.push(format!(
                "{}… ({} more lines, {} bytes elided — {})",
                prefix, remaining_lines, remaining_bytes, hint
            ));
            return out;
        }
        out.push(format!("{}{}", prefix, line));
        bytes_emitted += line.len() + 1;
    }
    out
}

/// Implementation of timeout-based command execution.
///
/// Spawns reader threads to drain stdout/stderr concurrently (preventing deadlock when
/// output exceeds the OS pipe buffer), then waits with timeout. On timeout, tears down
/// the child's whole process tree; scoped threads see EOF and join automatically before
/// the function returns.
///
/// **The teardown reaches the tree, not just the child, because otherwise the timeout
/// doesn't bound anything.** A grandchild inherits the child's stderr pipe, so a
/// surviving one holds the write end open and `read_to_end` blocks until it exits —
/// making this function return `TimedOut` only after the *grandchild's* full runtime.
/// The case that matters is the one this timeout exists for: `git ls-remote` against an
/// unreachable host spawns `git-remote-https`, which sits in `connect()` for ~127 s per
/// address on Linux and does not notice that git died. So the child is spawned into its
/// own process group and [`kill_timed_out_tree`] signals the group.
///
/// Isolating the group costs the kernel's tty broadcast: a Ctrl-C no longer reaches a
/// timed child directly, so the user waits out the remaining timeout instead of
/// interrupting it. That is bounded by the timeout the caller chose (seconds), whereas
/// the orphan the alternative leaves behind — kill the child, stop waiting on the
/// readers — holds a pipe for as long as its own operation takes, once per spawn.
fn run_with_timeout_impl(
    cmd: &mut Command,
    timeout: std::time::Duration,
) -> std::io::Result<std::process::Output> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let child = SharedChild::spawn(
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )?;
    let _tracked = track_if_cancellable(child.id());

    let mut child_stdout = child.take_stdout();
    let mut child_stderr = child.take_stderr();

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

        match child.wait_timeout(timeout) {
            Ok(Some(status)) => {
                let stdout = stdout_thread.join().unwrap()?;
                let stderr = stderr_thread.join().unwrap()?;
                Ok(std::process::Output {
                    status,
                    stdout,
                    stderr,
                })
            }
            // Timed out, or the wait itself failed. A failed wait has to tear the
            // tree down too: propagating it instead would leave the child running,
            // and the scope's join then blocks on `read_to_end` until the child
            // closes its pipes — the caller waits out the full runtime the timeout
            // exists to bound, and gets back an error that isn't `TimedOut`.
            outcome => {
                kill_timed_out_tree(child.id());
                let _ = child.kill();
                let _ = child.wait();
                Err(outcome.err().unwrap_or_else(|| {
                    std::io::Error::new(ErrorKind::TimedOut, "command timed out")
                }))
            }
        }
    })
}

/// Tear down the process tree of a child that outlived its timeout.
///
/// `run_with_timeout_impl` made the child its own process-group leader, so its
/// pid is the pgid and the TERM → KILL escalation reaches every member. SIGTERM
/// first for the same reason [`signal_background_pid`] uses it: git's lockfile
/// handlers run on TERM, so an interrupted git cleans up after itself.
///
/// Signalling by pid is safe here because the caller still holds an unreaped
/// [`shared_child::SharedChild`]: a timed wait uses `waitid(WNOWAIT)`, so a child
/// that exited on the deadline's other side is a zombie that keeps its pid
/// reserved until the caller's own `wait()`. The pid cannot name a different
/// process group by the time the signal lands.
#[cfg(unix)]
fn kill_timed_out_tree(pid: u32) {
    forward_signal_with_escalation(pid as i32, signal_hook::consts::SIGTERM);
}

/// `taskkill /T` walks the child tree Windows has no process group for; `/F`
/// forces. Best-effort — the pid may already be gone, or have left children
/// that detached from it.
#[cfg(windows)]
fn kill_timed_out_tree(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
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
/// ```no_run
/// use worktrunk::shell_exec::Cmd;
/// # fn example(repo_path: std::path::PathBuf) -> Result<(), Box<dyn std::error::Error>> {
/// let output = Cmd::new("git")
///     .args(["status", "--porcelain"])
///     .current_dir(&repo_path)
///     .context("my-worktree")
///     .run()?;
/// # let _ = output;
/// # Ok(())
/// # }
/// ```
///
/// Stream output (hooks, interactive):
/// ```no_run
/// use std::process::Stdio;
/// use worktrunk::shell_exec::Cmd;
/// # fn example(repo_path: std::path::PathBuf) -> Result<(), Box<dyn std::error::Error>> {
/// Cmd::shell("npm run build")
///     .current_dir(&repo_path)
///     .stdout(Stdio::from(std::io::stderr()))
///     .forward_signals()
///     .stream()?;
/// # Ok(())
/// # }
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
    /// If true, the child shares the parent's process group instead of being
    /// isolated in its own — required when the child reads the controlling
    /// terminal (interactive TUI pickers, pagers). Set via `.inherit_stdin()`.
    /// See the comment in `stream()` for the SIGTTOU rationale.
    share_parent_pgroup: bool,
    /// If true, forward signals to child process group (for stream(), Unix only)
    forward_signals: bool,
    /// When set, log this command to the command log after execution.
    /// The label identifies what triggered the command (e.g., "pre-merge user:lint").
    external_label: Option<String>,
    /// When set, re-adds `WORKTRUNK_DIRECTIVE_CD_FILE` after the security scrub
    /// in `apply_common_settings`. Used by aliases and foreground hooks — their
    /// shell bodies may emit cd directives (the file holds a raw path, no shell
    /// injection surface).
    directive_cd_file: Option<std::path::PathBuf>,
    /// When set, re-adds `WORKTRUNK_DIRECTIVE_EXEC_FILE` after the security
    /// scrub. Set only by user-source aliases (issue #2101): the body is the
    /// user's own config, so a nested `wt --execute` is no different from the
    /// user typing it at the top level. Project aliases and hooks must NEVER
    /// set this — they could inject arbitrary shell into the parent session.
    directive_exec_file: Option<std::path::PathBuf>,
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

/// Resolve a [`CommandTrace`] from a finished `run`/`pipe_into` invocation and
/// surface the command's captured stdin/stdout/stderr to the debug log.
///
/// The buffered counterpart to calling `complete`/`fail` directly: `run` and
/// `pipe_into` capture output, so they also feed it to [`log_output`] — stdin
/// regardless of outcome, stdout/stderr when the command produced any.
/// `stream`/`delayed_stream` inherit or stream stdio and resolve their
/// `CommandTrace` directly at each exit point.
fn record_captured(
    trace: &mut CommandTrace,
    stdin: Option<&[u8]>,
    result: &std::io::Result<std::process::Output>,
) {
    match result {
        Ok(output) => trace.complete(output.status.success()),
        Err(e) => trace.fail(e),
    }
    // stdin is logged either way; stdout/stderr only when the command produced
    // output — a command that failed to spawn still leaves its input behind.
    log_output(trace, stdin, result.as_ref().ok());
}

/// Structured error from [`Cmd::delayed_stream`].
///
/// Separates command output from command identity so callers can format each
/// part with appropriate styling (e.g., bold command, gray exit code). The
/// streaming counterpart to [`crate::git::CommandError`]: the delayed-stream
/// path interleaves stdout/stderr into a single buffer, so a string body is
/// the most it can recover. `Repository::extract_failed_command` downcasts
/// this to render git's failure to the user.
#[derive(Debug)]
pub struct StreamCommandError {
    /// Lines of output from the command (may be empty).
    pub output: String,
    /// The command string, e.g., "git worktree add /path -b fix main".
    pub command: String,
    /// Exit information, e.g., "exit code 255" or "killed by signal".
    pub exit_info: String,
}

impl std::fmt::Display for StreamCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Callers use Repository::extract_failed_command() to access fields
        // directly. This Display impl exists only to satisfy the Error trait
        // bound.
        write!(f, "{}", self.output)
    }
}

impl std::error::Error for StreamCommandError {}

/// Convert a finished child's exit status into `Ok(())` or a
/// [`StreamCommandError`] carrying the buffered output.
fn stream_exit_result(
    status: std::process::ExitStatus,
    buffer: &Arc<Mutex<Vec<String>>>,
    cmd_str: &str,
) -> anyhow::Result<()> {
    if status.success() {
        return Ok(());
    }
    let lines = buffer.lock().unwrap();
    let exit_info = status
        .code()
        .map(|c| format!("exit code {c}"))
        .unwrap_or_else(|| "killed by signal".to_string());
    Err(StreamCommandError {
        output: lines.join("\n"),
        command: cmd_str.to_string(),
        exit_info,
    }
    .into())
}

/// Spawn a reader thread for [`Cmd::delayed_stream`]: each line is written to
/// stderr live once `streaming` is set, or buffered (for the quiet-fast and
/// pre-threshold cases) until then. Both the child's stdout and stderr use
/// this; everything routes to stderr to keep our stdout clean.
fn spawn_delayed_reader<R: Read + Send + 'static>(
    stream: R,
    streaming: Arc<AtomicBool>,
    buffer: Arc<Mutex<Vec<String>>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let reader = BufReader::new(stream);
        for line in reader.lines().map_while(Result::ok) {
            if streaming.load(Ordering::Relaxed) {
                let _ = writeln!(std::io::stderr(), "{}", line);
                let _ = std::io::stderr().flush();
            } else {
                buffer.lock().unwrap().push(line);
            }
        }
    })
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
            share_parent_pgroup: false,
            forward_signals: false,
            external_label: None,
            directive_cd_file: None,
            directive_exec_file: None,
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

    fn check_spawn_preconditions(&self) -> std::io::Result<()> {
        if let Some(dir) = &self.current_dir {
            let metadata = std::fs::metadata(dir)?;
            if !metadata.is_dir() {
                return Err(std::io::Error::new(
                    ErrorKind::NotADirectory,
                    format!("{} is not a directory", dir.display()),
                ));
            }
        }

        let program = Path::new(&self.program);
        if !self.shell_wrap && program.is_absolute() {
            ensure_executable_path(program)?;
        }

        Ok(())
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

        // Before `self.envs`, so a per-command env can override the floor.
        apply_hermetic_test_env(cmd);

        for (key, val) in &self.envs {
            cmd.env(key, val);
        }
        for key in &self.env_removes {
            cmd.env_remove(key);
        }

        // Prevent subprocesses from writing shell directives (security).
        // Applied last so it can't be re-added by user-provided envs.
        // `stream()` selectively re-adds `WORKTRUNK_DIRECTIVE_CD_FILE` for
        // trusted contexts, and `WORKTRUNK_DIRECTIVE_EXEC_FILE` only for
        // trusted user-source aliases.
        scrub_directive_env_vars(cmd);
    }

    fn log_run_start(&self, cmd_str: &str) {
        tracing::debug!("{}", command_header(cmd_str, self.context.as_deref()));
    }

    fn log_stream_start(&self, cmd_str: &str, exec_mode: &str) {
        tracing::debug!(
            "{} (streaming, {})",
            command_header(cmd_str, self.context.as_deref()),
            exec_mode
        );
    }

    fn log_delayed_stream_start(&self, cmd_str: &str, delay_ms: i64) {
        tracing::debug!(
            "{} (delayed stream, {}ms)",
            command_header(cmd_str, self.context.as_deref()),
            delay_ms
        );
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
    ///
    /// A timed command runs in its own process group so expiry can tear down
    /// its whole tree, which also means Ctrl-C no longer reaches it — see
    /// `run_with_timeout_impl` for both halves of that.
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

    /// Scrub inherited git-discovery vars ([`INHERITED_GIT_PATH_VARS`]) from the
    /// child environment. Applied by spawn sites that relocate a user command
    /// into a `wt`-chosen worktree (hooks, `wt step for-each`) so the command's
    /// `git` calls discover the repository from the working directory `wt` sets,
    /// not a `GIT_DIR`/`GIT_WORK_TREE` `wt` inherited. See
    /// [`scrub_git_discovery_env_vars`] for the site classification (issue #3373).
    ///
    /// Applied after the inherited-`GIT_*` absolutization in
    /// `apply_common_settings` (env-removes run last), so it also overrides the
    /// relative-path absolutization that would otherwise re-add these vars.
    pub fn scrub_git_discovery_env(mut self) -> Self {
        for var in INHERITED_GIT_PATH_VARS {
            self.env_removes.push(OsString::from(*var));
        }
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
    /// Defaults to `Stdio::null()`. For interactive commands that need the
    /// parent's controlling terminal, use [`Cmd::inherit_stdin()`] instead —
    /// it also makes `.forward_signals()` keep the child in the parent's
    /// process group (required to avoid SIGTTOU stopping the child mid-render).
    ///
    /// Only affects `.stream()`. For `.run()`, stdin defaults to null unless
    /// data is provided via `.stdin_bytes()`.
    pub fn stdin(mut self, cfg: std::process::Stdio) -> Self {
        self.stdin_cfg = Some(cfg);
        self
    }

    /// Inherit the parent's stdin, including the controlling terminal.
    ///
    /// Required for children that read from a TTY (interactive TUI pickers,
    /// pagers, anything that calls `tcsetattr` on `/dev/tty`). When combined
    /// with [`Cmd::forward_signals()`], the child is *not* isolated in its
    /// own process group — it shares the parent's. A child reading the
    /// parent's tty must share the foreground pgroup, or the kernel sends
    /// SIGTTOU when the child manipulates terminal settings, suspending it.
    ///
    /// In the shared-pgroup case the kernel already delivers tty-initiated
    /// signals (SIGINT from Ctrl-C, SIGTSTP from Ctrl-Z, SIGHUP on hangup)
    /// to every process in the foreground pgroup. For externally-delivered
    /// signals that only reach wt (e.g. `kill -TERM <wt-pid>`), the listener
    /// re-delivers to the child by PID so the child also exits — without
    /// escalation, since the caller chose the signal and a premature SIGKILL
    /// would skip the child's tty restore.
    pub fn inherit_stdin(mut self) -> Self {
        self.stdin_cfg = Some(std::process::Stdio::inherit());
        self.share_parent_pgroup = true;
        self
    }

    /// Forward signals (SIGINT, SIGTERM) to child process group.
    ///
    /// On Unix, spawns the child in its own process group and forwards signals
    /// with escalation (SIGINT → SIGTERM → SIGKILL). This enables clean shutdown
    /// of the entire process tree on Ctrl-C.
    ///
    /// When combined with [`Cmd::inherit_stdin()`], the new-pgroup isolation
    /// is skipped (the child shares the parent's pgroup so it can drive the
    /// controlling terminal); the listener then forwards by PID single-shot
    /// rather than `killpg`-with-escalation, so externally-delivered signals
    /// (e.g. `kill -TERM <wt-pid>`) still reach the child. Either way,
    /// signal-derived child exits surface as
    /// `WorktrunkError::ChildProcessExited { signal: .. }`.
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
    pub fn directive_cd_file(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.directive_cd_file = Some(path.into());
        self
    }

    /// Pass the EXEC directive file through to the child process.
    ///
    /// Re-adds `WORKTRUNK_DIRECTIVE_EXEC_FILE`, allowing the child to request
    /// arbitrary shell execution in the parent shell (the wrapper sources the
    /// file after wt exits). This is only safe when the child's command body
    /// is itself trusted user-authored input — currently just user-source
    /// aliases, where the alias body lives in the user's own config and
    /// nesting `wt --execute` is no different from the user typing it. Project
    /// aliases and hooks must not call this — see issue #2101.
    pub fn directive_exec_file(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.directive_exec_file = Some(path.into());
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
            self.directive_cd_file.is_none() && self.directive_exec_file.is_none(),
            "directive_*_file is only applied by .stream(), not .run()"
        );

        let cmd_str = self.command_string();
        let external_log = ExternalCommandLog::new(self.external_label.clone(), cmd_str.clone());
        self.log_run_start(&cmd_str);

        // Limit concurrent commands (background threads only; see CMD_SEMAPHORE)
        let _guard = (!is_foreground_thread()).then(|| semaphore().acquire());

        let mut trace = CommandTrace::new(self.context.as_deref(), &cmd_str)
            .reads_stdin(self.stdin_data.is_some());

        // Checked after the permit, not before: a command can be cancelled
        // while parked on the semaphore, and that is the common case in a
        // large fan-out.
        if background_cancelled() {
            let e = cancelled_error();
            trace.fail(&e);
            external_log.record(None);
            return Err(e);
        }

        if let Err(e) = self.check_spawn_preconditions() {
            trace.fail(&e);
            external_log.record(None);
            return Err(e);
        }

        let mut cmd = self.direct_command();
        self.apply_common_settings(&mut cmd);

        // Execute with or without stdin. Every branch produces a single
        // `Result<Output>` so spawn/write failures resolve the trace through
        // `record_captured` rather than `?`-ing past it (which would leave the
        // command unattributed and trip CommandTrace's drop assertion).
        let result = if let Some(stdin_data) = self.stdin_data.as_deref() {
            // Stdin piping requires spawn/write/wait
            // Note: stdin path doesn't support timeout (would need async I/O)
            cmd.stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());

            match cmd.spawn() {
                Ok(mut child) => {
                    let _tracked = track_if_cancellable(child.id());
                    // Write stdin data in an inner scope so the handle DROPS
                    // (closing the pipe) before `wait_with_output` — otherwise a
                    // child that reads stdin to EOF (e.g. `git … --stdin`) blocks
                    // forever. (ignore BrokenPipe - some commands exit early)
                    let write_result = {
                        let mut stdin = child.stdin.take().expect("stdin was configured as piped");
                        stdin.write_all(stdin_data)
                    };
                    match write_result {
                        Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => Err(e),
                        _ => child.wait_with_output(),
                    }
                }
                Err(e) => Err(e),
            }
        } else if let Some(timeout_duration) = self.timeout {
            // Timeout handling uses the existing impl
            run_with_timeout_impl(&mut cmd, timeout_duration)
        } else {
            // Simple case: run and capture output. Spawned explicitly rather
            // than via `cmd.output()` — which matches these stdio defaults —
            // because `output()` hands back only the finished result, never
            // the running child, and a background command has to be
            // registered as cancellable while it runs (see BACKGROUND_PIDS).
            cmd.stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            match cmd.spawn() {
                Ok(child) => {
                    let _tracked = track_if_cancellable(child.id());
                    child.wait_with_output()
                }
                Err(e) => Err(e),
            }
        };

        record_captured(&mut trace, self.stdin_data.as_deref(), &result);

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
    /// The source's *own* stdin (`stdin_bytes`, e.g. the `git rev-list` commit
    /// list) is logged under `  < ` like any `.run()` command — only the
    /// intermediate stream above is suppressed.
    ///
    /// Each command is logged and traced individually (same format as
    /// `.run()`), so `-vv` still shows both commands and their exit status.
    /// The returned tuple is `(source_output, sink_output)` — callers can
    /// inspect either child's exit status and stderr. `source_output.stdout`
    /// is empty (it was routed to the sink via OS pipe).
    ///
    /// `stdin_bytes` on the source feeds the pipeline's input (the sink's
    /// stdin always comes from the source). Timeouts and `external()` logging
    /// are not supported on either side. On a background thread the pipeline
    /// consumes one semaphore permit even though it runs two processes
    /// concurrently — acquiring two would deadlock under `concurrency = 1`;
    /// the foreground thread is exempt (see `CMD_SEMAPHORE`).
    pub fn pipe_into(
        mut self,
        next: Cmd,
    ) -> std::io::Result<(std::process::Output, std::process::Output)> {
        assert!(
            !self.shell_wrap && !next.shell_wrap,
            "Cmd::shell() commands cannot be used with pipe_into"
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
                && self.directive_exec_file.is_none()
                && next.directive_cd_file.is_none()
                && next.directive_exec_file.is_none(),
            "directive_*_file is only applied by .stream(), not pipe_into"
        );

        let first_cmd_str = self.command_string();
        let second_cmd_str = next.command_string();
        self.log_run_start(&first_cmd_str);
        next.log_run_start(&second_cmd_str);

        let _guard = (!is_foreground_thread()).then(|| semaphore().acquire());

        // Cancelled, possibly while parked on the semaphore above (see
        // `background_cancelled`). Nothing has spawned, so neither half runs.
        if background_cancelled() {
            let e = cancelled_error();
            CommandTrace::record_failed(
                self.context.as_deref(),
                &first_cmd_str,
                self.stdin_data.is_some(),
                &e,
            );
            return Err(e);
        }

        // Validate both commands before spawning either. Nothing has spawned
        // yet, so a precondition failure emits a one-shot failed record rather
        // than holding a guard across an execution that never happens.
        if let Err(e) = self.check_spawn_preconditions() {
            // stdin_data is still owned here (taken below), so it reflects
            // whether the source would have read a buffer; the sink always reads
            // the upstream pipe.
            CommandTrace::record_failed(
                self.context.as_deref(),
                &first_cmd_str,
                self.stdin_data.is_some(),
                &e,
            );
            return Err(e);
        }
        if let Err(e) = next.check_spawn_preconditions() {
            CommandTrace::record_failed(next.context.as_deref(), &second_cmd_str, true, &e);
            return Err(e);
        }

        let source_stdin = self.stdin_data.take();

        let mut first = self.direct_command();
        self.apply_common_settings(&mut first);
        first
            .stdin(if source_stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Trace each command from just before its own spawn so the duration
        // brackets the real spawn → wait span. The source reads stdin only when
        // fed a buffer; the sink always reads it (the source's piped stdout).
        let mut first_trace = CommandTrace::new(self.context.as_deref(), &first_cmd_str)
            .reads_stdin(source_stdin.is_some());
        let mut first_child = match first.spawn() {
            Ok(child) => child,
            Err(e) => {
                first_trace.fail(&e);
                return Err(e);
            }
        };
        let _first_tracked = track_if_cancellable(first_child.id());
        let first_stdout = first_child
            .stdout
            .take()
            .expect("stdout was configured as piped");
        // Take the source's stdin pipe, keeping `source_stdin` owned so it can
        // also be logged below; a writer thread (in the scope) drains it
        // concurrently with the rest of the pipeline.
        let source_stdin_pipe = source_stdin.as_ref().map(|_| {
            first_child
                .stdin
                .take()
                .expect("stdin was configured as piped")
        });

        let mut second = next.direct_command();
        next.apply_common_settings(&mut second);
        second
            .stdin(Stdio::from(first_stdout))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Spawn `next` before waiting on either child so `self`'s stdout keeps
        // flowing through the pipe (otherwise a full pipe buffer would wedge
        // `self`). If the spawn itself fails, clean up `self` before returning.
        let mut second_trace =
            CommandTrace::new(next.context.as_deref(), &second_cmd_str).reads_stdin(true);
        let second_child = match second.spawn() {
            Ok(child) => child,
            Err(e) => {
                second_trace.fail(&e);
                let _ = first_child.kill();
                let _ = first_child.wait();
                // `first` spawned but is being torn down because the pipeline
                // can't run — record it as a non-success rather than leaving
                // the guard unresolved.
                first_trace.complete(false);
                return Err(e);
            }
        };
        let _second_tracked = track_if_cancellable(second_child.id());

        // `first`'s stderr must be drained concurrently with `second`'s
        // execution; otherwise pathological stderr volume (~64 KiB pipe
        // buffer) could block `first` on write, which then never closes its
        // stdout, which wedges `second`. Scoped thread drains in parallel.
        let mut first_stderr_pipe = first_child
            .stderr
            .take()
            .expect("stderr was configured as piped");

        let (first_result, second_result) = std::thread::scope(|s| {
            let stderr_thread = s.spawn(move || {
                let mut buf = Vec::new();
                first_stderr_pipe.read_to_end(&mut buf).map(|_| buf)
            });

            // Feed the source's stdin (e.g. a `git rev-list` commit list
            // piped into `git diff-tree --stdin`) on a thread, concurrent
            // with the drain below — writing it all up front would deadlock
            // once the source's stdout pipe fills. A short write (the source
            // exited early) surfaces as its non-zero exit status, which
            // callers already inspect. The scoped thread only borrows the
            // data, so `source_stdin` stays available to log after the
            // pipeline reaps the source.
            if let (Some(mut stdin), Some(data)) = (source_stdin_pipe, source_stdin.as_deref()) {
                s.spawn(move || {
                    let _ = stdin.write_all(data);
                    // `stdin` drops here, closing the pipe so the source sees EOF.
                });
            }

            // Drain `next` first (its `wait_with_output` reads its own
            // stdout/stderr), so `first`'s writes can complete. Its stdin is the
            // source's stdout (the OS pipe), not its own input, so it is logged
            // with `None` — the intermediate stream stays out of the deep log.
            let second_result = second_child.wait_with_output();
            record_captured(&mut second_trace, None, &second_result);

            // Reap `first`. Its stderr is already being drained; combine
            // the captured stderr with the exit status into an Output.
            let first_status = first_child.wait();
            let first_stderr = stderr_thread.join().unwrap();

            let first_result = first_status.and_then(|status| {
                first_stderr.map(|stderr| std::process::Output {
                    status,
                    stdout: Vec::new(),
                    stderr,
                })
            });
            // The source's own stdin (the commit list) is logged under `  < `,
            // symmetric with `run`. Only the intermediate diff stream — the
            // source's stdout, routed to the sink via OS pipe — stays out.
            record_captured(&mut first_trace, source_stdin.as_deref(), &first_result);

            (first_result, second_result)
        });

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
        use {signal_hook::consts::SIGPIPE, std::os::unix::process::CommandExt};

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
        // file is only re-added for user-source aliases (issue #2101); project
        // aliases and hooks leave it scrubbed so their bodies cannot inject
        // shell into the parent session.
        if let Some(ref path) = self.directive_cd_file {
            apply_cd_directive_env(&mut cmd, path);
        }
        if let Some(ref path) = self.directive_exec_file {
            cmd.env(DIRECTIVE_EXEC_FILE_ENV_VAR, path);
        }

        if let Err(e) = self.check_spawn_preconditions() {
            // Nothing spawned yet — emit a one-shot failed record (the trace
            // guard is constructed just before spawn, below).
            CommandTrace::record_failed(
                self.context.as_deref(),
                &cmd_str,
                self.stdin_data.is_some(),
                &e,
            );
            return Err(anyhow::Error::from(GitError::Other {
                message: format!("Failed to execute command ({}): {}", exec_mode, e),
            }));
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

        // Install the SIGINT/SIGTERM handler BEFORE spawn so a signal arriving
        // mid-spawn is queued, not default-killed.
        #[cfg(unix)]
        let signals = if self.forward_signals {
            Some(crate::signal_forwarder::ForegroundSignals::install()?)
        } else {
            None
        };

        #[cfg(unix)]
        if self.forward_signals && !self.share_parent_pgroup {
            // Isolate the child in its own process group so we can signal the whole tree.
            //
            // Skipped when the caller used `.inherit_stdin()`: a child that
            // reads the parent's controlling terminal must share the
            // foreground pgroup, otherwise calls like skim's (crossterm)
            // `tcsetattr` on `/dev/tty` raise SIGTTOU and stop the child
            // mid-render. The kernel already delivers tty-initiated signals
            // (Ctrl-C, Ctrl-Z, hangup) to every process in the shared
            // pgroup, so explicit forwarding is redundant in that case.
            cmd.process_group(0);
        }

        // Apply environment and spawn
        cmd.stdin(stdin_mode)
            .stdout(stdout_mode)
            .stderr(std::process::Stdio::inherit()) // Preserve TTY for errors
            // Prevent vergen "overridden" warning in nested cargo builds
            .env_remove("VERGEN_GIT_DESCRIBE");

        // Start the trace immediately before spawn, after the fallible
        // signal-handler install — so a pre-spawn early return can't drop the
        // guard unresolved, and the duration brackets the child.
        let mut trace = CommandTrace::new(self.context.as_deref(), &cmd_str)
            .reads_stdin(self.stdin_data.is_some());
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => {
                trace.fail(&e);
                return Err(anyhow::Error::from(GitError::Other {
                    message: format!("Failed to execute command ({}): {}", exec_mode, e),
                }));
            }
        };

        // Write stdin content if provided (ignore BrokenPipe - child may exit early)
        if let Some(ref content) = self.stdin_data
            && let Some(mut stdin) = child.stdin.take()
            && let Err(e) = stdin.write_all(content)
            && e.kind() != std::io::ErrorKind::BrokenPipe
        {
            trace.fail(&e);
            return Err(e.into());
        }
        // stdin handle is dropped here, closing the pipe

        // Start the listener now that the child PID is known. The handler
        // installed pre-spawn has been queueing signals; the listener
        // processes any queued signal on its first poll.
        #[cfg(unix)]
        let forwarder =
            signals.map(|s| s.forward_to_pid(child.id() as i32, self.share_parent_pgroup));

        let wait_result = child.wait();

        // Always tear down the listener, even on wait error, so the
        // signal-hook handle is released and the thread doesn't leak.
        #[cfg(unix)]
        let seen_signal = forwarder.and_then(|f| f.stop());

        let status = match wait_result {
            Ok(status) => status,
            Err(e) => {
                trace.fail(&e);
                return Err(anyhow::Error::from(GitError::Other {
                    message: format!("Failed to wait for command: {}", e),
                }));
            }
        };

        // Handle signals (Unix only).
        //
        // `seen_signal` records any signal forwarded by the listener thread,
        // covering the case where the child caught the signal and exited with
        // a code (no kernel `status.signal()` to read). The clean-exit gate
        // closes the wait-vs-handle.close window: if `child.wait()` returned
        // success and a signal then landed before `handle.close()` ran, the
        // signal arrived too late to have killed anything — the contract on
        // `signal: Some(_)` is "this child was killed by the signal" and
        // `interrupt_signal` callers in pipeline loops break on it.
        #[cfg(unix)]
        if let Some(sig) = seen_signal
            && !status.success()
        {
            trace.complete(false);
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
                trace.complete(true);
                external_log.record(Some(0));
                return Ok(());
            }
            trace.complete(false);
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
            trace.complete(false);
            external_log.record(status.code());
            return Err(WorktrunkError::ChildProcessExited {
                code,
                message: format!("exit status: {}", code),
                signal: None,
            }
            .into());
        }

        trace.complete(true);
        external_log.record(Some(0));

        Ok(())
    }

    /// Execute the command with delayed output streaming.
    ///
    /// Buffers stdout/stderr initially; if the command runs longer than
    /// `delay_ms`, switches to streaming both to **stderr** live (keeping our
    /// stdout clean for callers like `wt switch`). Fast commands stay quiet;
    /// slow ones (`git worktree add` on a large repo) show progress. Pass `-1`
    /// to never switch to streaming (always buffer); `0` streams immediately.
    ///
    /// `progress_message`, when set, prints to stderr at the moment streaming
    /// starts (the delay threshold is crossed).
    ///
    /// Like [`Cmd::stream`], this does **not** acquire the concurrency
    /// semaphore: a delayed-stream command runs in the foreground and would
    /// only ever contend with itself (and acquiring a permit while one is held
    /// could deadlock under `concurrency = 1`). On a non-zero exit it returns a
    /// [`StreamCommandError`] carrying the buffered output so callers can render
    /// the failure (e.g. git's `fatal: …`).
    ///
    /// # Panics
    ///
    /// Panics if called on a shell-wrapped command (`Cmd::shell()`); the
    /// delayed-stream path runs a program directly.
    pub fn delayed_stream(
        self,
        delay_ms: i64,
        progress_message: Option<String>,
    ) -> anyhow::Result<()> {
        assert!(
            !self.shell_wrap,
            "Cmd::delayed_stream() runs a program directly; Cmd::shell() is not supported"
        );
        debug_assert!(
            self.stdin_data.is_none()
                && self.timeout.is_none()
                && self.external_label.is_none()
                && self.directive_cd_file.is_none()
                && self.directive_exec_file.is_none(),
            "delayed_stream does not support stdin/timeout/external/directive options"
        );

        // Allow tests to override the delay threshold (-1 to disable, 0 for
        // immediate streaming).
        let delay_ms = std::env::var("WORKTRUNK_TEST_DELAYED_STREAM_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(delay_ms);

        let cmd_str = self.command_string();
        self.log_delayed_stream_start(&cmd_str, delay_ms);

        let mut trace = CommandTrace::new(self.context.as_deref(), &cmd_str);

        let mut cmd = self.direct_command();
        self.apply_common_settings(&mut cmd);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let child = match SharedChild::spawn(&mut cmd) {
            Ok(child) => child,
            Err(e) => {
                trace.fail(&e);
                return Err(e).with_context(|| format!("Failed to spawn: {}", cmd_str));
            }
        };

        let stdout = child.take_stdout().expect("stdout was piped");
        let stderr = child.take_stderr().expect("stderr was piped");

        // Shared state: when true, output streams directly; when false, buffers.
        let streaming = Arc::new(AtomicBool::new(false));
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let stdout_handle = spawn_delayed_reader(stdout, streaming.clone(), buffer.clone());
        let stderr_handle = spawn_delayed_reader(stderr, streaming.clone(), buffer.clone());

        let start = Instant::now();

        // Phase 1: If the delay threshold is enabled, wait that long for the
        // child to exit. If it finishes before the threshold, output stays
        // buffered (quiet).
        if delay_ms >= 0 {
            let delay = Duration::from_millis(delay_ms as u64);
            let remaining = delay.saturating_sub(start.elapsed());

            // Zero delay means "stream immediately", not "try a zero-timeout reap".
            if !remaining.is_zero() {
                match child.wait_timeout(remaining) {
                    Ok(Some(status)) => {
                        let _ = stdout_handle.join();
                        let _ = stderr_handle.join();
                        trace.complete(status.success());
                        return stream_exit_result(status, &buffer, &cmd_str);
                    }
                    // Threshold exceeded — fall through to streaming.
                    Ok(None) => {}
                    Err(e) => {
                        let _ = stdout_handle.join();
                        let _ = stderr_handle.join();
                        trace.fail(&e);
                        return Err(e).context("Failed to wait for command");
                    }
                }
            }

            // Delay threshold exceeded — switch to streaming.
            streaming.store(true, Ordering::Relaxed);
            if let Some(ref msg) = progress_message {
                let _ = writeln!(std::io::stderr(), "{}", msg);
            }
            for line in buffer.lock().unwrap().drain(..) {
                let _ = writeln!(std::io::stderr(), "{}", line);
            }
            let _ = std::io::stderr().flush();
        }

        // Phase 2: Block until the child exits (no polling).
        let wait_result = child.wait();
        let _ = stdout_handle.join();
        let _ = stderr_handle.join();
        let status = match wait_result {
            Ok(status) => status,
            Err(e) => {
                trace.fail(&e);
                return Err(e).context("Failed to wait for command");
            }
        };
        trace.complete(status.success());
        stream_exit_result(status, &buffer, &cmd_str)
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

/// Single-shot signal delivery to a specific PID. Used in shared-pgroup mode
/// where the child isn't a pgroup leader (so `killpg` would target a non-
/// existent group), and where the kernel has already broadcast tty signals
/// to the foreground pgroup — explicit forwarding only matters for
/// externally-delivered signals (e.g. `kill -TERM <wt-pid>`). No escalation:
/// see the call site in `Cmd::stream` for the rationale.
#[cfg(unix)]
pub fn forward_signal_to_pid(pid: i32, sig: i32) {
    let nix_sig = match sig {
        signal_hook::consts::SIGINT => nix::sys::signal::Signal::SIGINT,
        signal_hook::consts::SIGTERM => nix::sys::signal::Signal::SIGTERM,
        _ => return,
    };
    let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), nix_sig);
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
    fn test_scrub_directive_env_vars_covers_every_directive_variable() {
        assert_eq!(RETIRED_DIRECTIVE_FILE_ENV_VAR, "WORKTRUNK_DIRECTIVE_FILE");

        let mut cmd = std::process::Command::new("child");
        for var in [
            DIRECTIVE_CD_FILE_ENV_VAR,
            DIRECTIVE_EXEC_FILE_ENV_VAR,
            RETIRED_DIRECTIVE_FILE_ENV_VAR,
            SHELL_CWD_ENV_VAR,
        ] {
            cmd.env(var, "directive");
        }

        scrub_directive_env_vars(&mut cmd);

        for var in [
            DIRECTIVE_CD_FILE_ENV_VAR,
            DIRECTIVE_EXEC_FILE_ENV_VAR,
            RETIRED_DIRECTIVE_FILE_ENV_VAR,
            SHELL_CWD_ENV_VAR,
        ] {
            assert!(
                cmd.get_envs()
                    .any(|(key, value)| { key == std::ffi::OsStr::new(var) && value.is_none() }),
                "{var} should be removed from the child environment"
            );
        }
    }

    #[test]
    fn test_shell_cwd_from_prefers_absolute_inherited_value() {
        // Built from `temp_dir()` rather than platform literals: `/shell/cwd`
        // isn't absolute on Windows (no prefix), and a `cfg!(windows)` pair
        // would leave whichever arm this platform didn't take uncovered.
        // Neither path is touched — the predicate only inspects its shape.
        let root = std::env::temp_dir();
        let fallback = root.join("wt-shell-cwd-fallback");
        let absolute = root.join("wt-shell-cwd");
        assert!(absolute.is_absolute(), "temp_dir() should be absolute");

        // An absolute inherited value wins over the process cwd.
        assert_eq!(
            shell_cwd_from(Some(OsString::from(&absolute)), Some(&fallback)),
            Some(absolute.clone())
        );
        // A relative one names nothing from a child that runs elsewhere.
        assert_eq!(
            shell_cwd_from(Some(OsString::from("apps/gateway")), Some(&fallback)),
            Some(fallback.clone())
        );
        assert_eq!(
            shell_cwd_from(None, Some(&fallback)),
            Some(fallback.clone())
        );
        assert_eq!(
            shell_cwd_from(Some(OsString::from("apps/gateway")), None),
            None
        );
    }

    /// The only test allowed to set `FOREGROUND_THREAD` (a process-wide
    /// set-once): with it unset, `is_foreground_thread()` is false everywhere,
    /// which is the state every other test runs under.
    #[test]
    fn test_foreground_thread_exempt_after_init() {
        assert!(!is_foreground_thread());
        init_startup();
        assert!(is_foreground_thread());
        // Threads other than the initializing one stay capped.
        let from_spawned = std::thread::spawn(is_foreground_thread).join().unwrap();
        assert!(!from_spawned);
    }

    #[test]
    fn test_powershell_escape() {
        // Plain word: wrapped but unchanged inside the quotes.
        assert_eq!(powershell_escape("rg"), "'rg'");
        // Embedded single quote: doubled (PowerShell's only escape).
        assert_eq!(powershell_escape("can't"), "'can''t'");
        // `$(...)` is inert inside a PowerShell single-quoted string — no
        // expansion, no doubling needed.
        assert_eq!(powershell_escape("$(whoami)"), "'$(whoami)'");
        // Spaces are covered by the surrounding quotes.
        assert_eq!(powershell_escape("with space"), "'with space'");
        // Empty string still produces a valid empty literal.
        assert_eq!(powershell_escape(""), "''");
        // Multiple quotes each double independently.
        assert_eq!(powershell_escape("a'b'c"), "'a''b''c'");
    }

    #[test]
    fn test_fish_escape() {
        // Plain word: wrapped but unchanged inside the quotes.
        assert_eq!(fish_escape("rg"), "'rg'");
        // Two consecutive backslashes: each doubled, so fish's `eval`
        // collapses `\\\\` back to `\\` — POSIX `'…'` would corrupt this.
        assert_eq!(fish_escape(r"a\\b"), r"'a\\\\b'");
        // Trailing backslash: doubled, so the closing `'` is not swallowed
        // (the POSIX form `'end\'` is an unterminated string for fish).
        assert_eq!(fish_escape(r"end\"), r"'end\\'");
        // Embedded single quote: backslash-escaped (fish's escape inside `'…'`).
        assert_eq!(fish_escape("can't"), r"'can\'t'");
        // `$(...)` and backticks are inert inside a fish single-quoted string.
        assert_eq!(fish_escape("$(whoami)"), "'$(whoami)'");
        assert_eq!(fish_escape("`whoami`"), "'`whoami`'");
        // Empty string still produces a valid empty literal.
        assert_eq!(fish_escape(""), "''");
        // Backslash then quote: `\` doubled first, then `'` escaped, so the
        // escaping backslash of `\'` is not itself doubled.
        assert_eq!(fish_escape(r"\'"), r"'\\\''");
    }

    #[test]
    fn test_shell_escape_for_dispatch() {
        // Literal passes the value through untouched.
        assert_eq!(shell_escape_for(ShellEscapeMode::Literal, "can't"), "can't");
        // Posix uses the `'\''` idiom for an embedded quote.
        assert_eq!(
            shell_escape_for(ShellEscapeMode::Posix, "can't"),
            r"'can'\''t'"
        );
        // PowerShell doubles the embedded quote.
        assert_eq!(
            shell_escape_for(ShellEscapeMode::PowerShell, "can't"),
            "'can''t'"
        );
        // Fish backslash-escapes the embedded quote.
        assert_eq!(
            shell_escape_for(ShellEscapeMode::Fish, "can't"),
            r"'can\'t'"
        );
    }

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

    /// The `wait-timeout` crate has to stay out of the dependency graph, however
    /// it gets there.
    ///
    /// Its `SIGCHLD` handler pokes a socketpair with `send()` and `panic!`s on any
    /// errno but `WouldBlock`. That handler is `extern "C"`, so the panic cannot
    /// unwind — it goes straight to `abort()`. Under a sandbox that denies the send
    /// (the Codex CLI's `workspace-write` mode), every timed wait in `wt` became an
    /// uncatchable `SIGABRT` with no diagnostic (#3856). `shared_child` wakes
    /// through `signal_hook`, which discards wake-write errors by design.
    ///
    /// The lockfile is read at runtime, not `include_str!`d: a compile-time embed
    /// has to ship in every packaged build, which `embedded_assets_ship_in_package`
    /// enforces and `Cargo.lock` doesn't satisfy. Tests only ever run from the
    /// source tree, so the manifest dir is always there.
    #[test]
    fn test_wait_timeout_crate_stays_out_of_the_dependency_graph() {
        let lockfile = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.lock");
        let lockfile = std::fs::read_to_string(&lockfile).expect("read Cargo.lock");
        assert!(
            !lockfile.contains("name = \"wait-timeout\""),
            "wait-timeout is back in the dependency graph; its SIGCHLD handler \
             aborts wt when the self-pipe write fails (#3856)"
        );
    }

    /// A command that can't be spawned at all fails as a spawn error, not as a
    /// timeout — the deadline path never starts, so the caller doesn't wait it out.
    #[test]
    fn test_cmd_timeout_surfaces_a_spawn_failure() {
        let err = Cmd::new("worktrunk-no-such-program-3856")
            .timeout(Duration::from_secs(30))
            .run()
            .unwrap_err();
        assert_ne!(err.kind(), std::io::ErrorKind::TimedOut);
    }

    /// The timeout has to bound wall-clock, not just signal the direct child.
    /// A grandchild inherits the child's stderr pipe, so one that survives the
    /// kill holds the write end open and the output readers block on it — which
    /// used to make `run()` return `TimedOut` only once the *grandchild* exited
    /// (30 s here, ~127 s for the `git-remote-https` case this bound exists
    /// for). `; :` keeps the shell from `exec`ing sleep, so there really is a
    /// grandchild to leave behind.
    #[test]
    #[cfg(unix)]
    fn test_cmd_timeout_bounds_wall_clock_with_surviving_grandchild() {
        let start = std::time::Instant::now();
        let err = Cmd::new("sh")
            .args(["-c", "sleep 30; :"])
            .timeout(Duration::from_millis(200))
            .run()
            .unwrap_err();
        let elapsed = start.elapsed();

        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(
            elapsed < Duration::from_secs(10),
            "timeout waited on the grandchild: {elapsed:?}"
        );
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
    fn test_cmd_run_spawn_failure_is_errored() {
        let err = Cmd::new("/no/such/binary-7f3a9b2c").run().unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
    }

    #[test]
    fn test_cmd_run_missing_current_dir_is_errored() {
        let err = Cmd::new("sh")
            .args(["-c", "true"])
            .current_dir("/no/such/dir-7f3a9b2c")
            .run()
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
    }

    #[test]
    fn test_cmd_run_file_current_dir_is_errored() {
        // Any existing non-directory path will do, and the test binary is one,
        // so this creates nothing in the system temp directory — a shared
        // namespace where Windows can deny a fresh temp name outright ("Access
        // is denied.") rather than report a collision tempfile would retry.
        let err = Cmd::new("sh")
            .args(["-c", "true"])
            .current_dir(std::env::current_exe().unwrap())
            .run()
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotADirectory);
    }

    #[test]
    fn test_cmd_run_directory_program_path_is_errored() {
        let dir = tempfile::tempdir().unwrap();
        let err = Cmd::new(dir.path().to_string_lossy()).run().unwrap_err();
        assert_eq!(err.kind(), ErrorKind::PermissionDenied);
    }

    #[test]
    #[cfg(unix)]
    fn test_cmd_run_non_executable_program_path_is_errored() {
        use std::os::unix::fs::PermissionsExt;

        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::set_permissions(file.path(), std::fs::Permissions::from_mode(0o644)).unwrap();

        let err = Cmd::new(file.path().to_string_lossy()).run().unwrap_err();
        assert_eq!(err.kind(), ErrorKind::PermissionDenied);
    }

    #[test]
    fn test_cmd_stream_spawn_failure_is_errored() {
        // A non-existent absolute binary path should surface as a command
        // execution failure before the child can report only exit status 127.
        let result = Cmd::new("/no/such/binary-7f3a9b2c").stream();
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("Failed to execute command"),
            "expected spawn-failure message, got: {msg}"
        );
    }

    // Fault-injection coverage for the `CommandTrace` resolution arms across
    // `Cmd`'s execution modes. A non-existent *relative* program slips past
    // `check_spawn_preconditions` (which only stats absolute paths) and fails at
    // `spawn()`, exercising the `trace.fail` spawn-error arms; a non-existent
    // *absolute* program is caught by preconditions, exercising the
    // `record_failed` arms. None set `.context()`, so the no-context logging
    // branches are covered too.
    const MISSING_CMD: &str = "worktrunk-nonexistent-binary-7f3a9b2c";

    #[test]
    fn test_cmd_run_spawn_failure_resolves_trace() {
        let err = Cmd::new(MISSING_CMD).run().unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
    }

    #[test]
    #[cfg(unix)]
    fn test_cmd_run_with_stdin_closes_pipe() {
        // `cat` reads stdin to EOF; it can only exit once the pipe is closed,
        // so this pins the "drop stdin before wait" behavior (a regression here
        // hangs forever rather than failing).
        let output = Cmd::new("cat").stdin_bytes("piped body").run().unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout), "piped body");
    }

    #[test]
    #[cfg(unix)]
    fn test_cmd_delayed_stream_quiet_success() {
        // A fast command under a generous threshold exits during phase 1
        // (wait_timeout returns Some) and stays buffered/quiet.
        Cmd::new("true").delayed_stream(5_000, None).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn test_cmd_delayed_stream_streams_then_reports_failure() {
        // delay_ms=0 streams immediately (phase-1 threshold crossed → progress
        // message + buffer drain), and a non-zero exit surfaces as a
        // StreamCommandError carrying the buffered output.
        let err = Cmd::new("sh")
            .args(["-c", "echo to-stderr 1>&2; exit 3"])
            .delayed_stream(0, Some("working…".to_string()))
            .unwrap_err();
        let stream_err = err
            .downcast_ref::<StreamCommandError>()
            .expect("non-zero delayed_stream exit should be a StreamCommandError");
        assert_eq!(stream_err.exit_info, "exit code 3");
    }

    #[test]
    fn test_cmd_delayed_stream_spawn_failure_resolves_trace() {
        // delay_ms=-1 disables phase 1; the spawn failure resolves the trace
        // via `fail` rather than dropping it unresolved.
        let err = Cmd::new(MISSING_CMD).delayed_stream(-1, None).unwrap_err();
        assert!(err.to_string().contains("Failed to spawn"), "{err}");
    }

    #[test]
    #[cfg(unix)]
    fn test_cmd_pipe_into_succeeds() {
        let (source, sink) = Cmd::new("printf")
            .arg("hello")
            .pipe_into(Cmd::new("cat"))
            .unwrap();
        assert!(source.status.success() && sink.status.success());
        assert_eq!(String::from_utf8_lossy(&sink.stdout), "hello");
    }

    #[test]
    #[cfg(unix)]
    fn test_cmd_pipe_into_feeds_source_stdin() {
        // The source's stdin must flow through to the sink: `cat` echoes its
        // stdin to stdout, piped into a second `cat`, so the bytes round-trip.
        let (source, sink) = Cmd::new("cat")
            .stdin_bytes(b"piped stdin".to_vec())
            .pipe_into(Cmd::new("cat"))
            .unwrap();
        assert!(source.status.success() && sink.status.success());
        assert_eq!(String::from_utf8_lossy(&sink.stdout), "piped stdin");
    }

    #[test]
    fn test_cmd_pipe_into_source_spawn_failure_resolves_trace() {
        let err = Cmd::new(MISSING_CMD)
            .pipe_into(Cmd::new("cat"))
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
    }

    #[test]
    #[cfg(unix)]
    fn test_cmd_pipe_into_sink_spawn_failure_resolves_both_traces() {
        // The sink fails to spawn after the source is already running: the sink
        // trace fails and the source is torn down with complete(false).
        let err = Cmd::new("printf")
            .arg("hello")
            .pipe_into(Cmd::new(MISSING_CMD))
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
    }

    // An *absolute* non-existent program is rejected by
    // `check_spawn_preconditions` (which stats absolute paths) before any spawn,
    // exercising the `record_failed` precondition arms rather than the
    // spawn-error arms above.
    #[test]
    fn test_cmd_pipe_into_source_precondition_failure_resolves_trace() {
        let err = Cmd::new("/no/such/abs-source-7f3a9b2c")
            .pipe_into(Cmd::new("cat"))
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
    }

    #[test]
    #[cfg(unix)]
    fn test_cmd_pipe_into_sink_precondition_failure_resolves_trace() {
        // Source preconditions pass (relative program); the sink's fail.
        let err = Cmd::new("printf")
            .arg("x")
            .pipe_into(Cmd::new("/no/such/abs-sink-7f3a9b2c"))
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
    }

    #[test]
    fn test_stream_command_error_display_is_the_output() {
        // The Display impl exists only for the Error bound; callers read fields
        // via Repository::extract_failed_command, so nothing else exercises it.
        let err = StreamCommandError {
            output: "fatal: ref exists".to_string(),
            command: "git worktree add /x".to_string(),
            exit_info: "exit code 128".to_string(),
        };
        assert_eq!(err.to_string(), "fatal: ref exists");
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
    fn test_background_pid_deregisters_on_drop() {
        // Not a live PID: this exercises the registry bookkeeping only, and
        // nothing in this test signals anything.
        let pid = u32::MAX;
        {
            BACKGROUND_PIDS.lock().unwrap().insert(pid);
            let _guard = BackgroundPid(pid);
            assert!(BACKGROUND_PIDS.lock().unwrap().contains(&pid));
        }
        assert!(
            !BACKGROUND_PIDS.lock().unwrap().contains(&pid),
            "the guard should deregister its PID once the command finishes"
        );
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
        // No tracing subscriber is installed in unit tests, so
        // `tracing::enabled!(SUBPROCESS_FULL_TARGET, DEBUG)` is false and the
        // marker suggests `-vv`.
        assert!(marker.contains("rerun with -vv"));
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
