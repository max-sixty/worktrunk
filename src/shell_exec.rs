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
//! ## Spawning children
//!
//! Worktrunk's command spawns use [`spawn`] or [`spawn_shared_child`]. Their
//! shared lock covers descriptor setup and process creation, never child waits:
//! children still execute concurrently. On macOS, the standard library creates
//! pipes and sockets before marking them close-on-exec. Serializing their setup
//! with spawns prevents a child from retaining another command's output or wake.
//!
//! ## Process groups and signal handling (Unix, `Cmd::stream`)
//!
//! Foreground children share the caller's process group, including concurrent
//! hooks with piped output and closed stdin. Group membership is independent
//! of input allocation: programs may still open `/dev/tty` for terminal control.
//! The shell owns the foreground job and the kernel delivers Ctrl-C, Ctrl-Z
//! and hangup to it. No terminal handoff or private foreground group is needed.
//!
//! [`Cmd::forward_signals`] observes direct exit while waiting, preserving a
//! child's normal exit when it handles Ctrl-C.
//! SIGTERM addressed only to wt is forwarded
//! to owned direct children and cancels the command. Child signal exits propagate
//! through `WorktrunkError::ChildProcessExited` to stop foreground loops.
//! Detached background commands and timeout-bounded captures have separate
//! ownership: those commands may create a private group for tree cancellation.
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
//! [`wait_shared_child`] is the canonical wait for shared child handles. macOS
//! kernel exit events avoid SharedChild's stop-sensitive `waitid` path while
//! retaining its synchronized reaping and PID-safe signal delivery. Other
//! platforms use SharedChild's native waits and deadlines. macOS waits do not
//! change SIGCHLD dispositions or masks, including when a deadline expires.
//! Foreground waits publish native INT/TERM to current foreground scopes.
//! Captures and pagers do not publish cancellation themselves; command consumers
//! decide whether to propagate their typed failures to the foreground operation.
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
//!
//! **When a timed wait fails.** A wall-clock deadline failure retires the
//! owned child. An advisory output-delay failure switches to streaming and
//! retries the canonical wait; if that wait also fails, it retires the child
//! before joining output readers and returns the underlying error.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs::Metadata;
use std::io::{BufRead, BufReader, ErrorKind, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Context;
use shared_child::SharedChild;

use crate::git::{ErrorExt, GitError, WorktrunkError};
use crate::styling::eprintln;
use crate::sync::Semaphore;
use crate::trace::CommandTrace;

#[cfg(unix)]
pub mod pipe;

/// Prepare complete input before spawning, with no producer that can block.
/// The anonymous file is removed when its last handle closes, even on interruption.
/// This is seekable stdin, unlike the pipes connecting live pipeline stages.
pub fn buffered_stdin(bytes: &[u8]) -> std::io::Result<Stdio> {
    let mut file = tempfile::tempfile()?;
    file.write_all(bytes)?;
    file.rewind()?;
    Ok(file.into())
}

// Published Linux/Windows targets use atomic descriptors or std's spawn lock.
#[cfg(target_vendor = "apple")]
static PROCESS_CREATION_LOCK: Mutex<()> = Mutex::new(());

/// Serialize process creation with descriptor creation that sets close-on-exec
/// after its native syscall. Otherwise a concurrent spawn can inherit endpoints
/// between creation and CLOEXEC, keeping unrelated output or wakeups alive.
pub(crate) fn with_process_creation_guard<T>(
    create: impl FnOnce() -> std::io::Result<T>,
) -> std::io::Result<T> {
    #[cfg(target_vendor = "apple")]
    let _guard = PROCESS_CREATION_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    create()
}

/// Spawn a child under the shared process-creation guard.
pub fn spawn(command: &mut Command) -> std::io::Result<std::process::Child> {
    with_process_creation_guard(|| command.spawn())
}

/// Spawn a shared child under the same process-creation lock as [`spawn`].
pub fn spawn_shared_child(command: &mut Command) -> std::io::Result<SharedChild> {
    // SharedChild::new calls try_wait and can reap a short-lived child here.
    // Keep SharedChild::spawn's unreaped ownership for process-tree cleanup.
    with_process_creation_guard(|| SharedChild::spawn(command))
}

/// Create wakeup endpoints under the same guard as process creation.
#[cfg(unix)]
pub fn socket_pair() -> std::io::Result<(
    std::os::unix::net::UnixStream,
    std::os::unix::net::UnixStream,
)> {
    with_process_creation_guard(std::os::unix::net::UnixStream::pair)
}

/// Create a stream socket without holding the spawn guard during connection I/O.
pub fn stream_socket(domain: socket2::Domain) -> std::io::Result<socket2::Socket> {
    with_process_creation_guard(|| socket2::Socket::new(domain, socket2::Type::STREAM, None))
}

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
/// started. A command marked [`Cmd::finish_once_started`] doesn't register
/// either, so it runs to completion once spawned.
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

/// Whether a cancellation would signal `pid` right now.
#[cfg(all(test, unix))]
pub(crate) fn is_cancellable_pid(pid: u32) -> bool {
    BACKGROUND_PIDS.lock().unwrap().contains(&pid)
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
/// running is signalled rather than left to finish as an orphan — except
/// commands marked [`Cmd::finish_once_started`], which run to completion.
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
    terminate_pid(pid as i32);
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

/// POSIX shell configuration for command execution (sh or Git Bash).
#[derive(Debug, Clone)]
pub struct ShellConfig {
    /// Path to the shell executable
    pub executable: PathBuf,
    /// Arguments to pass before the command (e.g., ["-c"] for sh)
    pub args: Vec<String>,
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
    if let Ok(git_path) = which::which("git")
        && let Some(bash_path) = git_bash_beside_git(&git_path)
    {
        return Some(bash_path);
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

/// Derive Git Bash's location from a Git for Windows `git.exe` path.
///
/// `git.exe` sits one level below the install root (`Git/cmd`, `Git/bin`) or
/// below `usr` (`Git/usr/bin`), or two levels below the root in
/// `Git/mingw64/bin` — the copy MSYS puts first on PATH inside a Git Bash
/// session. bash.exe is at `Git/bin/bash.exe` or `Git/usr/bin/bash.exe`.
#[cfg(any(windows, test))]
fn git_bash_beside_git(git_path: &Path) -> Option<PathBuf> {
    let git_dir = git_path.parent()?;
    let root = git_dir.parent()?;
    // Only known nested runtime layouts justify another ascent. A shallow
    // Git/cmd or Git/bin install must not select a sibling ../bin/bash.exe.
    let nested_runtime = git_dir
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("bin"))
        && root
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                ["usr", "mingw32", "mingw64"]
                    .iter()
                    .any(|runtime| name.eq_ignore_ascii_case(runtime))
            });
    std::iter::once(root)
        .chain(root.parent().filter(|_| nested_runtime))
        .find_map(|root| {
            [root.join("bin"), root.join("usr").join("bin")]
                .into_iter()
                .map(|dir| dir.join("bash.exe"))
                .find(|bash_path| bash_path.exists())
        })
}

/// Environment variable naming the directive file for `cd` path changes.
///
/// Shell wrappers set this to a temp file; wt writes a raw absolute path to
/// it (one line, no shell escaping). The wrapper `cd`s to that path after wt
/// exits. Because the file contents are a literal path, there is no shell
/// injection surface — this is safe to pass through to alias/hook shell bodies.
pub const DIRECTIVE_CD_FILE_ENV_VAR: &str = "WORKTRUNK_DIRECTIVE_CD_FILE";

/// Retired shell-command directive variable.
///
/// Current wrappers no longer set it. Its presence tells `--execute` that the
/// live shell still has an old wrapper loaded, and subprocesses must not
/// inherit it: an older parent wrapper would source anything written there
/// after wt exits.
pub const DIRECTIVE_EXEC_FILE_ENV_VAR: &str = "WORKTRUNK_DIRECTIVE_EXEC_FILE";

/// Retired single-file directive env var.
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
///   worktree in turn), and the `--execute` program (run in the switch target)
///   — the cwd carries `wt`'s intent, so the inherited context is scrubbed and
///   the command's `git` calls discover the worktree from the cwd. The inherited
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
///   user's context, so it is forwarded untouched.
///
/// - **`wt`'s own git plumbing** splits on the same question. Repo-level
///   ([`Cmd`] via `Repository::run_command`) keeps the inherited context on
///   purpose (relative values absolutized, see issue #1914): `wt` honoring
///   the context it was handed is the point of running `wt` under `git`. Its
///   cwd is `discovery_path`, which is often but not always where the user
///   invoked `wt` — `Repository::at` is handed a `wt`-chosen worktree at
///   several sites (the post-switch hook repo, the pipeline repo, `finish`'s
///   destination repo, the `pre-remove` render repo). The exemption rests on
///   scope rather than cwd: repo-level questions are worktree-agnostic within
///   one repository, and every worktree-scoped answer routes through
///   [`crate::git::WorkingTree`], which scrubs.
///   **Worktree-local** plumbing — [`crate::git::WorkingTree::run_command`],
///   `TempIndex::command`, `list_ignored_entries` — relocates git into a
///   worktree `wt` resolved, so it scrubs, the same way hooks and `for-each`
///   do. Otherwise a `!wt` alias from a linked worktree (`GIT_DIR` pinned to
///   that worktree's private gitdir) makes `status` / `read-tree` on a
///   *different* worktree use the invoking tree's index.
///
/// Every site uses this one list; a site that needs its own value for a
/// scrubbed var sets it *after* the scrub rather than subsetting the list
/// ([`Cmd::scrub_git_discovery_env`]). That includes `GIT_OBJECT_DIRECTORY`:
/// a redirected repository re-sets its own immediately after, so the only
/// value a worktree-local scrub drops is an **inherited** one — which git
/// supplies to push-quarantine hooks, and which is pinned to the invoking
/// context exactly as `GIT_DIR` is. Dropping it is the deliberate call, not
/// an artifact of reusing the list.
///
/// Any new spawn site whose cwd names a `wt`-chosen worktree must apply this
/// scrub, via this helper or [`Cmd::scrub_git_discovery_env`].
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
/// for, and why `-c` precedence keeps this list short: `tests/AGENTS.md` →
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
// Shell Escaping
// ============================================================================

/// How to expand a value into a command template.
///
/// Hooks and aliases are POSIX shell command lines. Filesystem paths and
/// `--execute` argv elements use literal expansion because no shell parses
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellEscapeMode {
    /// Substitute values verbatim — used for filesystem paths.
    Literal,
    /// POSIX single-quoting (`'it'\''s'`) for hooks and aliases.
    Posix,
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
    let context = crate::trace::emit::diagnostic_context().or(context);
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
/// the still-owned child's process tree; scoped readers share the same deadline,
/// including when an exited child's descendants retain its pipes.
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
    cancellable: bool,
) -> std::io::Result<std::process::Output> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let child = spawn_shared_child(cmd)?;
    let _tracked = cancellable
        .then(|| track_if_cancellable(child.id()))
        .flatten();

    let deadline = Instant::now() + timeout;
    let child_stdout = child.take_stdout();
    let child_stderr = child.take_stderr();
    #[cfg(unix)]
    let readers = (|| {
        Ok::<_, std::io::Error>((
            child_stdout
                .map(|stream| pipe::PipeReader::new(stream, Some(deadline), None))
                .transpose()?,
            child_stderr
                .map(|stream| pipe::PipeReader::new(stream, Some(deadline), None))
                .transpose()?,
        ))
    })();
    #[cfg(not(unix))]
    let readers = Ok::<_, std::io::Error>((child_stdout, child_stderr));
    let (mut child_stdout, mut child_stderr) = match readers {
        Ok(readers) => readers,
        Err(error) => {
            kill_timed_out_tree(child.id());
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };

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

        let collect = || {
            let stdout = output_reader_result(stdout_thread.join());
            let stderr = output_reader_result(stderr_thread.join());
            Ok::<_, std::io::Error>((stdout?, stderr?))
        };
        // Keep the group leader unreaped until bounded EOF. A descendant-held
        // pipe can then time out without surrendering the identity used by cleanup.
        #[cfg(unix)]
        let output = match collect() {
            Ok(output) => output,
            Err(error) => {
                kill_timed_out_tree(child.id());
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        let status = match wait_shared_child(&child, Some(deadline)) {
            Ok(status) => status,
            Err(error) => {
                kill_timed_out_tree(child.id());
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        #[cfg(not(unix))]
        let output = collect()?;
        Ok(std::process::Output {
            status,
            stdout: output.0,
            stderr: output.1,
        })
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
/// [`shared_child::SharedChild`]: bounded readers finish before waiting/reaping,
/// and an expired direct wait leaves the child unreaped. An exited child stays
/// a zombie reserving its pid until the caller's own `wait()`. It cannot name a different
/// process group by the time the signal lands.
///
/// The same unreaped zombie means the escalation's liveness probe reads the
/// group as alive for the entire grace, so its final SIGKILL fires even when
/// every member exited on the TERM. Accepted: that sweep is a no-op against a
/// dead group (see [`terminate_process_group`]), and holding the zombie
/// is what pins the pgid.
#[cfg(unix)]
fn kill_timed_out_tree(pid: u32) {
    terminate_process_group(pid as i32);
}

/// `taskkill /T` walks the child tree Windows has no process group for; `/F`
/// forces. Best-effort — the pid may already be gone, or have left children
/// that detached from it.
#[cfg(windows)]
fn kill_timed_out_tree(pid: u32) {
    let mut command = Command::new("taskkill");
    command
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let _ = spawn(&mut command).and_then(|mut child| child.wait());
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
    /// Environment mutations in call order: `Some(value)` sets, `None`
    /// removes. One ordered list rather than a set list plus a remove list, so
    /// the last builder call naming a variable wins — the property
    /// [`Cmd::scrub_git_discovery_env`] relies on when a caller drops the whole
    /// inherited git context and then supplies its own value for one of those
    /// vars (`TempIndex`'s `GIT_INDEX_FILE`, a redirected repository's
    /// `GIT_OBJECT_DIRECTORY`).
    env_ops: Vec<(OsString, Option<OsString>)>,
    /// If true, wrap command through ShellConfig (for stream())
    shell_wrap: bool,
    /// Stdout configuration for stream() (defaults to inherit)
    stdout_cfg: Option<std::process::Stdio>,
    /// Stdin configuration for stream() (defaults to null, or a file for stdin_data)
    stdin_cfg: Option<std::process::Stdio>,
    /// Observe terminal signals and forward PID-targeted cancellation during stream().
    forward_signals: bool,
    /// If true, treat a SIGPIPE exit as success. This is the default for pager
    /// producers, where the consumer closing early is expected. Direct user
    /// programs can opt out with [`Cmd::propagate_sigpipe`].
    ignore_sigpipe: bool,
    /// When set, log this command to the command log after execution.
    /// The label identifies what triggered the command (e.g., "pre-merge user:lint").
    external_label: Option<String>,
    /// When set, re-adds `WORKTRUNK_DIRECTIVE_CD_FILE` after the security scrub
    /// in `apply_common_settings`. Used by aliases and foreground hooks — their
    /// shell bodies may emit cd directives (the file holds a raw path, no shell
    /// injection surface).
    directive_cd_file: Option<std::path::PathBuf>,
    /// If true, cancellation stops this command from spawning but never
    /// signals it once running. Set via [`Cmd::finish_once_started`].
    finish_once_started: bool,
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

/// A child from [`Cmd::spawn_captured`], traced from spawn until
/// [`Self::wait`] reaps it.
struct CapturedChild {
    child: std::process::Child,
    trace: CommandTrace,
    _tracked: Option<BackgroundPid>,
}

impl CapturedChild {
    fn wait(self) -> std::io::Result<std::process::Output> {
        let CapturedChild {
            child,
            mut trace,
            _tracked,
        } = self;
        let result = child.wait_with_output();
        record_captured(&mut trace, None, &result);
        result
    }
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
    /// Native status, retaining signal identity for cancellation and rendering.
    pub status: std::process::ExitStatus,
}

impl std::fmt::Display for StreamCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Callers use Repository::extract_failed_command() to access fields
        // directly. This Display impl exists only to satisfy the Error trait
        // bound.
        write!(f, "{}", self.output)
    }
}

impl StreamCommandError {
    pub fn exit_info(&self) -> String {
        self.status
            .code()
            .map(|code| format!("exit code {code}"))
            .unwrap_or_else(|| "killed by signal".to_string())
    }
}

impl std::error::Error for StreamCommandError {}

/// Shared state between [`Cmd::delayed_stream`] and its two reader threads.
///
/// `streaming` lives *inside* the mutex rather than beside it as an atomic so
/// that a reader cannot consult it without also holding the buffer. The
/// ordering guarantee this type exists for depends on the check and the write
/// being one critical section (see [`spawn_delayed_reader`]), and an atomic
/// makes a lock-free read the natural thing to write.
#[derive(Default)]
struct DelayedOutput {
    /// Once set, readers write to stderr instead of buffering.
    streaming: bool,
    /// Lines held back before the switch: drained to stderr behind the
    /// progress message, or joined into the error body when the command fails
    /// before streaming ever starts.
    lines: Vec<String>,
}

/// Convert a finished child's exit status into `Ok(())` or a
/// [`StreamCommandError`] carrying the buffered output.
fn stream_exit_result(
    status: std::process::ExitStatus,
    state: &Arc<Mutex<DelayedOutput>>,
    cmd_str: &str,
) -> anyhow::Result<()> {
    if status.success() {
        return Ok(());
    }
    let lines = &state.lock().unwrap().lines;
    Err(StreamCommandError {
        output: lines.join("\n"),
        command: cmd_str.to_string(),
        status,
    }
    .into())
}

/// Decode a streamed line, trimming a trailing newline/carriage return and
/// replacing invalid UTF-8 without dropping later command diagnostics.
pub fn output_line(bytes: &[u8]) -> std::borrow::Cow<'_, str> {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    String::from_utf8_lossy(bytes)
}

/// Spawn a reader thread for [`Cmd::delayed_stream`]: each line is written to
/// stderr live once the switch to streaming has happened, or buffered (for the
/// quiet-fast and pre-threshold cases) until then. Both the child's stdout and
/// stderr use this; everything routes to stderr to keep our stdout clean.
///
/// One [`DelayedOutput`] guard covers both reading `streaming` and writing the
/// line, and the switch sets that same field under that same lock. So each
/// line lands on one side of the switch or the other: it is either buffered,
/// and so drained in order behind the progress message, or written after that
/// drain has finished. Were the flag readable without the lock, a reader could
/// observe the flip and print between the progress message and the drain — or
/// ahead of the progress message entirely, which is what a zero delay makes
/// routine, since nothing has to be buffered first.
fn spawn_delayed_reader<R: Read + Send + 'static>(
    stream: R,
    state: Arc<Mutex<DelayedOutput>>,
) -> std::thread::JoinHandle<std::io::Result<()>> {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut bytes = Vec::new();
        while reader.read_until(b'\n', &mut bytes)? != 0 {
            let line = output_line(&bytes);
            let mut state = state
                .lock()
                .map_err(|_| std::io::Error::other("Command output state poisoned"))?;
            if state.streaming {
                eprintln!("{}", line);
            } else {
                state.lines.push(line.into_owned());
            }
            bytes.clear();
        }
        Ok(())
    })
}

/// Convert a reader's thread panic or I/O failure into the output error channel.
fn output_reader_result<T>(joined: std::thread::Result<std::io::Result<T>>) -> std::io::Result<T> {
    joined
        .map_err(|_| std::io::Error::other("Command output reader panicked"))
        .and_then(|result| result)
}

/// Join every reader before returning the first I/O or thread failure.
fn join_delayed_readers(
    handles: [std::thread::JoinHandle<std::io::Result<()>>; 2],
) -> std::io::Result<()> {
    let mut result = Ok(());
    for handle in handles {
        let reader_result = output_reader_result(handle.join());
        result = result.and(reader_result);
    }
    result
}

/// The batch launcher a bare program name resolves to on PATH, if any.
///
/// `std::process::Command` resolves a bare name like `az` to `az.exe` only: it
/// neither walks `PATHEXT` nor falls back to `az.cmd`. Some CLIs ship only a
/// batch launcher — Azure CLI's WinGet install has `az.cmd` plus an
/// extensionless bash script, and no `az.exe` — so a bare spawn fails with
/// `NotFound` although the shell runs them. `which` follows `PATHEXT` and skips
/// files Windows can't execute; when what it finds is a `.cmd` or `.bat`, its
/// full path is what to spawn, and std then applies its batch-file argument
/// quoting. Any other result keeps the bare name and std's own search order.
///
/// Resolved once per name: PATH doesn't change during a run, and the search
/// stats every `PATHEXT` candidate in every PATH directory.
#[cfg(windows)]
fn windows_batch_launcher(program: &str) -> Option<PathBuf> {
    use std::collections::HashMap;

    static RESOLVED: OnceLock<Mutex<HashMap<String, Option<PathBuf>>>> = OnceLock::new();

    let path = Path::new(program);
    if path.extension().is_some() || path.components().count() != 1 {
        return None;
    }
    let mut resolved = RESOLVED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    resolved
        .entry(program.to_string())
        .or_insert_with(|| {
            which::which(program).ok().filter(|found| {
                found.extension().is_some_and(|ext| {
                    ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat")
                })
            })
        })
        .clone()
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
            env_ops: Vec::new(),
            shell_wrap,
            stdout_cfg: None,
            stdin_cfg: None,
            forward_signals: false,
            ignore_sigpipe: true,
            external_label: None,
            directive_cd_file: None,
            finish_once_started: false,
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
        #[cfg(windows)]
        let mut cmd = match windows_batch_launcher(&self.program) {
            Some(launcher) => Command::new(launcher),
            None => Command::new(&self.program),
        };
        #[cfg(not(windows))]
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args);
        cmd
    }

    /// [`Self::check_spawn_preconditions`], after checking that this thread's
    /// commands haven't been cancelled. Background callers check after taking
    /// their semaphore permit: a command can be cancelled while parked on the
    /// semaphore, and that is the common case in a large fan-out.
    fn check_before_spawn(&self) -> std::io::Result<()> {
        if background_cancelled() {
            return Err(cancelled_error());
        }
        self.check_spawn_preconditions()
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

        // Before `self.env_ops`, so a per-command env can override the floor.
        apply_hermetic_test_env(cmd);

        // In builder-call order, so the last mutation naming a variable wins.
        for (key, val) in &self.env_ops {
            match val {
                Some(val) => cmd.env(key, val),
                None => cmd.env_remove(key),
            };
        }

        // Prevent subprocesses from writing shell directives (security).
        // Applied last so it can't be re-added by user-provided envs.
        // `stream()` selectively re-adds `WORKTRUNK_DIRECTIVE_CD_FILE` for
        // trusted contexts.
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

    /// Set complete input bytes, delivered through an anonymous temporary file.
    ///
    /// The child reads these bytes followed by EOF, without a pipe writer that
    /// can block on an unread input. This takes precedence over `.stdin(Stdio)`.
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

    /// Let this command run to completion once spawned, even if background
    /// work is cancelled meanwhile ([`cancel_background_commands`]). A
    /// cancelled command that hasn't spawned yet still never does.
    ///
    /// For commands whose interruption leaves debris SIGTERM doesn't clean:
    /// `git merge-tree` writes its external merge driver's inputs as
    /// `.merge_file_XXXXXX` files in its cwd and removes them only on a normal
    /// return, so a signal mid-driver strands them in the user's worktree.
    ///
    /// Only affects `.run()`.
    pub fn finish_once_started(mut self) -> Self {
        self.finish_once_started = true;
        self
    }

    /// [`track_if_cancellable`], unless [`Cmd::finish_once_started`] opted out.
    fn track_if_cancellable(&self, pid: u32) -> Option<BackgroundPid> {
        (!self.finish_once_started)
            .then(|| track_if_cancellable(pid))
            .flatten()
    }

    /// Set an environment variable.
    ///
    /// Accepts the same types as [`Command::env`]: string literals, `String`,
    /// `&Path`, `PathBuf`, `OsString`, etc.
    pub fn env(mut self, key: impl AsRef<OsStr>, val: impl AsRef<OsStr>) -> Self {
        self.env_ops.push((
            key.as_ref().to_os_string(),
            Some(val.as_ref().to_os_string()),
        ));
        self
    }

    /// Remove an environment variable.
    ///
    /// A later [`Cmd::env`] for the same variable overrides this.
    pub fn env_remove(mut self, key: impl AsRef<OsStr>) -> Self {
        self.env_ops.push((key.as_ref().to_os_string(), None));
        self
    }

    /// Scrub inherited git-discovery vars ([`INHERITED_GIT_PATH_VARS`]) from the
    /// child environment. Applied by every spawn site whose `current_dir` names
    /// a worktree `wt` chose — relocated user commands (hooks, `wt step
    /// for-each`, the `--execute` program) and `wt`'s own worktree-local
    /// plumbing ([`crate::git::WorkingTree::run_command`], `TempIndex`) alike —
    /// so git discovers the repository from that working directory, not a
    /// `GIT_DIR`/`GIT_WORK_TREE` `wt` inherited. See
    /// [`scrub_git_discovery_env_vars`] for the site classification (issue #3373).
    ///
    /// Applied after the inherited-`GIT_*` absolutization in
    /// `apply_common_settings`, so it also drops the relative-path
    /// absolutization that would otherwise re-add these vars.
    ///
    /// A site that supplies its own value for one of the scrubbed vars calls
    /// [`Cmd::env`] *after* this: `env_ops` is call-ordered, so the set wins.
    pub fn scrub_git_discovery_env(mut self) -> Self {
        for var in INHERITED_GIT_PATH_VARS {
            self.env_ops.push((OsString::from(*var), None));
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
    /// parent's input, use [`Cmd::inherit_stdin()`] instead. Foreground group
    /// membership and access to `/dev/tty` are independent of stdin.
    ///
    /// Only affects `.stream()`. For `.run()`, stdin defaults to null unless
    /// data is provided via `.stdin_bytes()`.
    pub fn stdin(mut self, cfg: std::process::Stdio) -> Self {
        self.stdin_cfg = Some(cfg);
        self
    }

    /// Inherit the parent's stdin for interactive input.
    ///
    /// Foreground group membership is independent of this setting.
    pub fn inherit_stdin(mut self) -> Self {
        self.stdin_cfg = Some(std::process::Stdio::inherit());
        self
    }

    /// Preserve terminal Ctrl-C delivery and forward SIGTERM to live children.
    ///
    /// The child shares the caller's foreground group. A handled Ctrl-C keeps
    /// its normal status; cancellation and signal exits stop foreground loops.
    /// Only affects `.stream()` on Unix. No-op on Windows.
    pub fn forward_signals(mut self) -> Self {
        self.forward_signals = true;
        self
    }

    /// Preserve SIGPIPE as exit status 141 instead of treating it as pager
    /// completion.
    ///
    /// `Cmd::stream()` historically ignores SIGPIPE because many callers feed
    /// a pager that may quit before consuming all output. Use this for a child
    /// whose status belongs to the user, such as `wt switch --execute`.
    pub fn propagate_sigpipe(mut self) -> Self {
        self.ignore_sigpipe = false;
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
            self.directive_cd_file.is_none(),
            "directive_*_file is only applied by .stream(), not .run()"
        );

        let cmd_str = self.command_string();
        let external_log = ExternalCommandLog::new(self.external_label.clone(), cmd_str.clone());
        self.log_run_start(&cmd_str);

        // Limit concurrent commands (background threads only; see CMD_SEMAPHORE)
        let _guard = (!is_foreground_thread()).then(|| semaphore().acquire());

        let mut trace = CommandTrace::new(self.context.as_deref(), &cmd_str)
            .reads_stdin(self.stdin_data.is_some());

        if let Err(e) = self.check_before_spawn() {
            trace.fail(&e);
            external_log.record(None);
            return Err(e);
        }

        let mut cmd = self.direct_command();
        self.apply_common_settings(&mut cmd);

        // Preparation failures and child outcomes resolve the same trace.
        let result = (|| {
            let stdin = self
                .stdin_data
                .as_deref()
                .map(buffered_stdin)
                .transpose()?
                .unwrap_or_else(Stdio::null);
            cmd.stdin(stdin)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if let Some(timeout) = self.timeout {
                run_with_timeout_impl(&mut cmd, timeout, !self.finish_once_started)
            } else {
                let child = spawn(&mut cmd)?;
                let _tracked = self.track_if_cancellable(child.id());
                child.wait_with_output()
            }
        })();

        record_captured(&mut trace, self.stdin_data.as_deref(), &result);

        let exit_code = result.as_ref().ok().and_then(|output| output.status.code());
        external_log.record(exit_code);

        result
    }

    /// Run `cmds` as concurrent child processes and return each one's output,
    /// in input order.
    ///
    /// Every child is spawned before any is waited on, so a batch of short
    /// commands costs about one command's latency without a thread per
    /// command. Outputs are read one child at a time, in order, so a child
    /// that writes more than a pipe buffer blocks until its turn: use this
    /// for commands with small output.
    ///
    /// The caller waits only on its own children, which makes this safe where
    /// a thread pool is not: inside a cache initializer that pool jobs also
    /// read. A rayon thread that waits runs other pool jobs, and a job that
    /// reads the cache being initialized then deadlocks.
    ///
    /// On a background thread the batch takes one semaphore permit, as
    /// [`Self::pipe_into`] does: one permit per child could deadlock two
    /// batches that each hold part of the pool. A batch runs at most
    /// `max_concurrent_commands()` children at once; other threads' commands
    /// are not counted against it. Stdin, timeouts,
    /// `external()` logging and shell commands are not supported.
    pub fn run_concurrently(cmds: &[Cmd]) -> Vec<std::io::Result<std::process::Output>> {
        assert!(
            cmds.iter().all(|cmd| !cmd.shell_wrap
                && cmd.stdin_data.is_none()
                && cmd.timeout.is_none()
                && cmd.external_label.is_none()),
            "run_concurrently supports captured commands without stdin, timeout, external() or shell"
        );

        let _guard = (!is_foreground_thread()).then(|| semaphore().acquire());

        let mut results = Vec::with_capacity(cmds.len());
        for chunk in cmds.chunks(max_concurrent_commands()) {
            let children: Vec<_> = chunk.iter().map(Cmd::spawn_captured).collect();
            results.extend(
                children
                    .into_iter()
                    .map(|child| child.and_then(CapturedChild::wait)),
            );
        }
        results
    }

    /// Spawn `self` with piped stdout/stderr and null stdin, without waiting.
    /// A failure to spawn resolves the trace before returning.
    fn spawn_captured(&self) -> std::io::Result<CapturedChild> {
        let cmd_str = self.command_string();
        self.log_run_start(&cmd_str);
        let mut trace = CommandTrace::new(self.context.as_deref(), &cmd_str);

        if let Err(e) = self.check_before_spawn() {
            trace.fail(&e);
            return Err(e);
        }

        let mut cmd = self.direct_command();
        self.apply_common_settings(&mut cmd);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        match spawn(&mut cmd) {
            Ok(child) => {
                let tracked = self.track_if_cancellable(child.id());
                Ok(CapturedChild {
                    child,
                    trace,
                    _tracked: tracked,
                })
            }
            Err(e) => {
                trace.fail(&e);
                Err(e)
            }
        }
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
            self.directive_cd_file.is_none() && next.directive_cd_file.is_none(),
            "directive_*_file is only applied by .stream(), not pipe_into"
        );

        let first_cmd_str = self.command_string();
        let second_cmd_str = next.command_string();
        self.log_run_start(&first_cmd_str);
        next.log_run_start(&second_cmd_str);

        let _guard = (!is_foreground_thread()).then(|| semaphore().acquire());

        // Validate both commands before spawning either. Nothing has spawned
        // yet, so a cancellation or precondition failure emits a one-shot
        // failed record rather than holding a guard across an execution that
        // never happens.
        if let Err(e) = self.check_before_spawn() {
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

        let mut first_trace = CommandTrace::new(self.context.as_deref(), &first_cmd_str)
            .reads_stdin(source_stdin.is_some());
        let input = source_stdin
            .as_deref()
            .map(buffered_stdin)
            .transpose()
            .inspect_err(|error| {
                first_trace.fail(error);
            })?
            .unwrap_or_else(Stdio::null);
        let mut first = self.direct_command();
        self.apply_common_settings(&mut first);
        first
            .stdin(input)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut first_child = match spawn(&mut first) {
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
        let second_child = match spawn(&mut second) {
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
    /// - Optionally observes terminal signals and forwards cancellation (`.forward_signals()`)
    /// - Does not use concurrency limiting (streaming commands run sequentially by nature)
    /// - Does not support timeout (interactive commands should not be time-limited)
    ///
    /// Shell commands created via `Cmd::shell()` are executed through the platform's
    /// shell (`sh -c` on Unix, Git Bash on Windows).
    ///
    /// Returns error if command exits with non-zero status.
    pub fn stream(mut self) -> anyhow::Result<()> {
        #[cfg(unix)]
        use signal_hook::consts::SIGPIPE;

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
        // The CD file is safe to pass through because it contains a raw path.
        if let Some(ref path) = self.directive_cd_file {
            apply_cd_directive_env(&mut cmd, path);
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

        let stdin = match self.stdin_data.as_deref() {
            Some(data) => buffered_stdin(data)
                .inspect_err(|error| {
                    CommandTrace::record_failed(self.context.as_deref(), &cmd_str, true, error);
                })
                .context("Failed to prepare command input")?,
            None => self.stdin_cfg.unwrap_or_else(Stdio::null),
        };
        cmd.stdin(stdin);

        // Install the SIGINT/SIGTERM handler BEFORE spawn so a signal arriving
        // mid-spawn is queued, not default-killed.
        #[cfg(unix)]
        let signals = if self.forward_signals {
            Some(crate::signal_forwarder::ForegroundSignals::install()?)
        } else {
            None
        };

        // Apply environment and spawn
        cmd.stdout(stdout_mode)
            .stderr(std::process::Stdio::inherit()) // Preserve TTY for errors
            // Prevent vergen "overridden" warning in nested cargo builds
            .env_remove("VERGEN_GIT_DESCRIBE");

        // Start the trace immediately before spawn, after the fallible
        // signal-handler install — so a pre-spawn early return can't drop the
        // guard unresolved, and the duration brackets the child.
        let mut trace = CommandTrace::new(self.context.as_deref(), &cmd_str)
            .reads_stdin(self.stdin_data.is_some());
        #[cfg(unix)]
        let spawned = match &signals {
            Some(signals) => signals.spawn(&mut cmd),
            None => spawn_shared_child(&mut cmd).map_err(anyhow::Error::from),
        };
        #[cfg(not(unix))]
        let spawned = spawn_shared_child(&mut cmd).map_err(anyhow::Error::from);
        let child = match spawned {
            Ok(child) => Arc::new(child),
            Err(error) if error.interrupt_signal().is_some() => {
                trace.fail(&error);
                return Err(error);
            }
            Err(e) => {
                trace.fail(&e);
                return Err(anyhow::Error::from(GitError::Other {
                    message: format!("Failed to execute command ({}): {}", exec_mode, e),
                }));
            }
        };

        #[cfg(unix)]
        let waited = match signals {
            Some(signals) => signals
                .wait(&child)
                .map(|outcome| (outcome.status, outcome.cancellation)),
            None => wait_shared_child(&child, None).map(|status| (status, None)),
        };
        #[cfg(not(unix))]
        let waited = wait_shared_child(&child, None).map(|status| (status, None::<i32>));
        let (status, cancellation) = waited
            .inspect_err(|error| {
                let _ = child.kill();
                let _ = child.wait();
                trace.fail(error);
            })
            .context("Failed to wait for command")?;
        #[cfg(unix)]
        let child_signal = std::os::unix::process::ExitStatusExt::signal(&status);

        // SIGPIPE is expected when a pager exits before its producer finishes.
        #[cfg(unix)]
        if child_signal == Some(SIGPIPE) && self.ignore_sigpipe && cancellation.is_none() {
            trace.complete(true);
            external_log.record(Some(0));
            return Ok(());
        }

        if !status.success() || cancellation.is_some() {
            let error = WorktrunkError::from_child_status(&status, cancellation);
            trace.complete(status.success());
            external_log.record(error.exit_code());
            return Err(error.into());
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
    /// starts. It arrives pre-rendered with its ANSI already in it, so — like
    /// every relayed line — it goes out through [`crate::styling`]'s
    /// `eprintln!` rather than a raw handle: anstream is the only thing that
    /// strips those escapes when stderr is redirected and honors `NO_COLOR`,
    /// and a single raw write here is enough to make one line of a log
    /// disagree with the rest. No `flush()` follows, and none did any work
    /// before: `AutoStream<Stderr>` writes through a locked `std::io::Stderr`,
    /// which is unbuffered. anstream's macro does not flush either, so a
    /// buffered stream would still need one.
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
                && self.directive_cd_file.is_none(),
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

        let child = match spawn_shared_child(&mut cmd) {
            Ok(child) => child,
            Err(e) => {
                trace.fail(&e);
                return Err(e).with_context(|| format!("Failed to spawn: {}", cmd_str));
            }
        };

        let stdout = child.take_stdout().expect("stdout was piped");
        let stderr = child.take_stderr().expect("stderr was piped");
        #[cfg(unix)]
        let prepared = (|| {
            let (control, cancel) = socket_pair()?;
            let control = Arc::new(control);
            Ok::<_, std::io::Error>((
                pipe::PipeReader::new(stdout, None, Some(control.clone()))?,
                pipe::PipeReader::new(stderr, None, Some(control))?,
                cancel,
            ))
        })();
        #[cfg(unix)]
        let (stdout, stderr, cancel) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                trace.fail(&error);
                return Err(error).context("Failed to prepare command output");
            }
        };
        #[cfg(unix)]
        let mut cancel = Some(cancel);

        // Shared state: readers stream directly once `streaming` is set, and
        // buffer until then.
        let state = Arc::new(Mutex::new(DelayedOutput::default()));
        let stdout_handle = spawn_delayed_reader(stdout, state.clone());
        let stderr_handle = spawn_delayed_reader(stderr, state.clone());

        let start = Instant::now();

        let wait_result = 'wait: {
            // Phase 1: wait up to the presentation threshold; an early exit stays quiet.
            if delay_ms >= 0 {
                let delay = Duration::from_millis(delay_ms as u64);
                let remaining = delay.saturating_sub(start.elapsed());

                // Zero delay streams immediately, without a zero-timeout reap.
                if !remaining.is_zero() {
                    match wait_shared_child(&child, Some(Instant::now() + remaining)) {
                        Ok(status) => break 'wait Ok(status),
                        // This deadline controls presentation, not child lifetime.
                        // On a timeout or wait-setup error, stream output and retry.
                        outcome => {
                            tracing::debug!(?outcome, "No exit status yet; switching to streaming");
                        }
                    }
                }

                // Switch to streaming under the readers' lock so no line can
                // print between the progress message and the buffered output.
                let mut state = state.lock().unwrap();
                state.streaming = true;
                if let Some(ref msg) = progress_message {
                    eprintln!("{}", msg);
                }
                for line in state.lines.drain(..) {
                    eprintln!("{}", line);
                }
            }

            // Phase 2: block until the child exits.
            wait_shared_child(&child, None)
        };
        #[cfg(unix)]
        if match &wait_result {
            Ok(status) => matches!(
                std::os::unix::process::ExitStatusExt::signal(status),
                Some(signal_hook::consts::SIGINT | signal_hook::consts::SIGTERM)
            ),
            Err(_) => true,
        } {
            cancel.take();
        }
        if wait_result.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let output_result = join_delayed_readers([stdout_handle, stderr_handle]);
        let status = match wait_result {
            Ok(status) => status,
            Err(e) => {
                trace.fail(&e);
                return Err(e).context("Failed to wait for command");
            }
        };
        if let Err(error) = output_result {
            trace.fail(&error);
            return Err(error).context("Failed to read command output");
        }
        trace.complete(status.success());
        stream_exit_result(status, &state, &cmd_str)
    }
}

// ============================================================================
// Signal forwarding helpers (Unix only)
// ============================================================================

/// Wait for a shared child without holding its signaling lock while stopped.
///
/// Darwin can return a stop from SharedChild's waitid(WEXITED) path. Its
/// subsequent blocking Child::wait holds that lock, preventing CONT and kill.
/// On macOS, subscribe to kernel exit events before try_wait so exit cannot
/// be lost between check and sleep. Other platforms use SharedChild directly.
/// A deadline returns TimedOut without reaping a still-running child.
pub fn wait_shared_child(
    child: &SharedChild,
    deadline: Option<Instant>,
) -> std::io::Result<std::process::ExitStatus> {
    #[cfg(target_os = "macos")]
    let status = wait_macos_child(child, deadline)?;
    #[cfg(not(target_os = "macos"))]
    let status = match deadline {
        Some(deadline) => child.wait_deadline(deadline)?,
        None => Some(child.wait()?),
    };
    let status =
        status.ok_or_else(|| std::io::Error::new(ErrorKind::TimedOut, "command timed out"))?;
    Ok(status)
}

/// Wait for a foreground child and publish native cancellation to current scopes.
/// Captures and diff-preview pagers use the neutral wait instead; their command
/// consumers decide whether to propagate typed cancellation.
pub fn wait_foreground_child(child: &SharedChild) -> std::io::Result<std::process::ExitStatus> {
    let status = wait_shared_child(child, None)?;
    #[cfg(unix)]
    if let Some(signal @ (signal_hook::consts::SIGINT | signal_hook::consts::SIGTERM)) =
        std::os::unix::process::ExitStatusExt::signal(&status)
    {
        crate::signal_forwarder::cancel_foreground(signal);
    }
    Ok(status)
}

#[cfg(target_os = "macos")]
fn wait_macos_child(
    child: &SharedChild,
    deadline: Option<Instant>,
) -> std::io::Result<Option<std::process::ExitStatus>> {
    use nix::errno::Errno;
    use nix::sys::event::{EvFlags, EventFilter, FilterFlag, KEvent, Kqueue};
    let queue = Kqueue::new()?;
    let exit = KEvent::new(
        child.id() as usize,
        EventFilter::EVFILT_PROC,
        EvFlags::EV_ADD | EvFlags::EV_ONESHOT,
        FilterFlag::NOTE_EXIT,
        0,
        0,
    );
    loop {
        let subscribe = queue.kevent(
            &[exit],
            &mut [],
            Some(nix::libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            }),
        );
        // A cached status wins over ESRCH if another waiter reaped the child.
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        match subscribe {
            Err(Errno::EINTR) => continue,
            // Darwin can remove the owned process from kqueue lookup before
            // its exit is waitable. Like NOTE_EXIT below, ESRCH means it can
            // no longer stop; finish synchronized reaping rather than fail.
            Err(Errno::ESRCH) => return child.wait().map(Some),
            Err(error) => return Err(error.into()),
            Ok(_) => break,
        }
    }
    let mut events = [exit];
    loop {
        let timeout = deadline.map(|deadline| {
            let duration = deadline.saturating_duration_since(Instant::now());
            nix::libc::timespec {
                tv_sec: duration.as_secs().min(i64::MAX as u64) as _,
                tv_nsec: duration.subsec_nanos().into(),
            }
        });
        let count = match queue.kevent(&[], &mut events, timeout) {
            Ok(count) => count,
            Err(Errno::EINTR) => continue,
            Err(e) => return Err(e.into()),
        };
        let observed = child.try_wait()?;
        if let Some(status) = observed {
            return Ok(Some(status));
        }
        if count == 0 {
            return Ok(None);
        }
        if events[0].flags().contains(EvFlags::EV_ERROR) {
            return Err(Errno::from_raw(events[0].data() as i32).into());
        }
        // NOTE_EXIT precedes waitable zombie state on Darwin. Once exit has
        // begun, this child cannot stop again; the ordinary synchronized wait
        // is now safe and closes the exit-event vs waitable-status gap.
        return child.wait().map(Some);
    }
}

/// Probe whether an owned background process group still exists.
#[cfg(unix)]
fn process_group_alive(pgid: i32) -> bool {
    match nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pgid), None) {
        Ok(_) => true,
        Err(nix::errno::Errno::ESRCH) => false,
        Err(_) => true,
    }
}

/// Poll [`process_group_alive`] until the group is gone or `grace` expires,
/// returning `true` when the group died within the grace. The first probe is
/// immediate, so an already-empty group costs no sleep at all, and a group
/// whose members exit (and are reaped) mid-grace is noticed within one poll
/// interval rather than at the deadline.
#[cfg(unix)]
fn group_died_within(pgid: i32, grace: Duration) -> bool {
    const POLL_INTERVAL: Duration = Duration::from_millis(20);
    let deadline = Instant::now() + grace;
    loop {
        if !process_group_alive(pgid) {
            return true;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        std::thread::sleep(remaining.min(POLL_INTERVAL));
    }
}

/// Request cancellation of a recorded background worker by PID. Foreground
/// delivery instead uses SharedChild's synchronized signaling ownership.
#[cfg(unix)]
pub fn terminate_pid(pid: i32) {
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid),
        nix::sys::signal::Signal::SIGTERM,
    );
}

/// Terminate an owned timeout or tether group: TERM, 200 ms grace, then KILL.
/// Foreground cancellation uses native job delivery instead of this teardown.
///
/// The group probe counts unreaped exits as alive. Capture timeouts retain
/// their leader, so they always use the full grace; signaling an already dead
/// group cannot change its exit status. Live survivors, including stopped or
/// TERM-ignoring processes, are killed at the deadline.
///
/// Tether may have reaped its leader, allowing an early return once the group
/// disappears. Reaping unpins the numeric pgid, leaving the existing recycling
/// exposure between the group probe and the subsequent signal.
#[cfg(unix)]
pub fn terminate_process_group(pgid: i32) {
    use nix::sys::signal::Signal;

    let pgid = nix::unistd::Pid::from_raw(pgid);
    let _ = nix::sys::signal::killpg(pgid, Signal::SIGTERM);
    if !group_died_within(pgid.as_raw(), Duration::from_millis(200)) {
        let _ = nix::sys::signal::killpg(pgid, Signal::SIGKILL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// macOS sets close-on-exec after creating stdio pipes and wakeup sockets.
    /// Concurrent spawns must not inherit these endpoints and delay EOF.
    #[cfg(target_os = "macos")]
    #[test]
    fn test_parallel_commands_do_not_inherit_sibling_descriptors() {
        const SCRIPT: &str = "
import errno, os, stat
counts = [0, 0]
for name in os.listdir('/dev/fd'):
    fd = int(name)
    if fd <= 2:
        continue
    try:
        mode = os.fstat(fd).st_mode
    except OSError as error:
        if error.errno != errno.EBADF:
            raise
        continue
    counts[0] += stat.S_ISFIFO(mode)
    counts[1] += stat.S_ISSOCK(mode)
print(*counts)
";
        let inspect = || Cmd::new("/usr/bin/python3").args(["-c", SCRIPT]).run();
        let descriptor_counts = |output: std::io::Result<std::process::Output>| {
            let output = output.unwrap();
            assert!(output.status.success(), "{output:?}");
            String::from_utf8(output.stdout)
                .unwrap()
                .split_whitespace()
                .map(|count| count.parse::<usize>().unwrap())
                .collect::<Vec<_>>()
        };
        // A test runner may itself have passed inheritable descriptors.
        let inherited = descriptor_counts(inspect());
        let barrier = std::sync::Barrier::new(16);
        let stop = AtomicBool::new(false);
        struct StopCreators<'a>(&'a AtomicBool);
        impl Drop for StopCreators<'_> {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Relaxed);
            }
        }
        let results = std::thread::scope(|scope| {
            // Stop creators before scope joins its threads, including on panic.
            let _stop_creators = StopCreators(&stop);
            for index in 0..8 {
                let stop = &stop;
                scope.spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        match index % 3 {
                            0 => drop(socket_pair().unwrap()),
                            1 => drop(stream_socket(socket2::Domain::UNIX).unwrap()),
                            _ => drop(stream_socket(socket2::Domain::IPV4).unwrap()),
                        }
                    }
                });
            }
            let workers: Vec<_> = (0..16)
                .map(|_| {
                    scope.spawn(|| {
                        (0..32)
                            .map(|_| {
                                barrier.wait();
                                inspect()
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            workers
                .into_iter()
                .flat_map(|worker| worker.join().unwrap())
                .collect::<Vec<_>>()
        });
        for output in results {
            assert_eq!(
                descriptor_counts(output),
                inherited,
                "inherited sibling pipe or wakeup socket"
            );
        }
    }

    /// The property the worktree-local scrub sites rest on: `TempIndex` and a
    /// redirected repository's object store scrub the whole
    /// [`INHERITED_GIT_PATH_VARS`] list and then set their own value for one of
    /// those vars. If `Cmd` ever applies removes after sets again, that set is
    /// silently dropped and those sites fall back to the ambient index / object
    /// store — so pin call order rather than the split-vector shape.
    #[test]
    fn test_env_mutations_apply_in_call_order() {
        let scrubbed = Cmd::new("child")
            .scrub_git_discovery_env()
            .env("GIT_INDEX_FILE", "chosen-index");
        let mut cmd = std::process::Command::new("child");
        scrubbed.apply_common_settings(&mut cmd);

        let env = |var: &str| {
            cmd.get_envs()
                .find(|(key, _)| *key == std::ffi::OsStr::new(var))
                .map(|(_, value)| value)
        };

        assert_eq!(
            env("GIT_INDEX_FILE"),
            Some(Some(std::ffi::OsStr::new("chosen-index"))),
            "a set after the scrub must win"
        );
        for var in INHERITED_GIT_PATH_VARS
            .iter()
            .filter(|var| **var != "GIT_INDEX_FILE")
        {
            assert_eq!(
                env(var),
                Some(None),
                "{var} should be removed from the child environment"
            );
        }

        // And the reverse order still removes, so `env_remove` isn't inert.
        let set_then_removed = Cmd::new("child")
            .env("GIT_INDEX_FILE", "chosen-index")
            .scrub_git_discovery_env();
        let mut cmd = std::process::Command::new("child");
        set_then_removed.apply_common_settings(&mut cmd);
        assert_eq!(
            cmd.get_envs()
                .find(|(key, _)| *key == std::ffi::OsStr::new("GIT_INDEX_FILE"))
                .map(|(_, value)| value),
            Some(None),
            "a scrub after the set must win"
        );
    }

    #[test]
    fn test_scrub_directive_env_vars_covers_every_directive_variable() {
        assert_eq!(RETIRED_DIRECTIVE_FILE_ENV_VAR, "WORKTRUNK_DIRECTIVE_FILE");
        assert_eq!(DIRECTIVE_EXEC_FILE_ENV_VAR, "WORKTRUNK_DIRECTIVE_EXEC_FILE");

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
    fn test_shell_escape_for_dispatch() {
        // Literal passes the value through untouched.
        assert_eq!(shell_escape_for(ShellEscapeMode::Literal, "can't"), "can't");
        // Posix uses the `'\''` idiom for an embedded quote.
        assert_eq!(
            shell_escape_for(ShellEscapeMode::Posix, "can't"),
            r"'can'\''t'"
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
    fn test_unix_shell_is_sh() {
        let config = ShellConfig::get().unwrap();
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
    fn test_git_bash_beside_git_install_layouts() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Git");
        for sub in ["bin", "usr/bin", "cmd", "mingw32/bin", "mingw64/bin"] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
        }
        let bin_bash = root.join("bin").join("bash.exe");
        std::fs::write(&bin_bash, "").unwrap();
        std::fs::write(root.join("usr").join("bin").join("bash.exe"), "").unwrap();

        for git in [
            "cmd/git.exe",
            "bin/git.exe",
            "mingw32/bin/git.exe",
            "mingw64/bin/git.exe",
        ] {
            assert_eq!(
                git_bash_beside_git(&root.join(git)),
                Some(bin_bash.clone()),
                "{git}"
            );
        }
        assert_eq!(
            git_bash_beside_git(&root.join("usr/bin/git.exe")),
            Some(root.join("usr").join("bin").join("bash.exe"))
        );

        // A minimal install with only usr/bin/bash.exe, reached from mingw64/bin
        std::fs::remove_file(&bin_bash).unwrap();
        assert_eq!(
            git_bash_beside_git(&root.join("mingw64/bin/git.exe")),
            Some(root.join("usr").join("bin").join("bash.exe"))
        );

        // A missing Git Bash does not authorize searching outside this install.
        // In particular shallow cmd/bin layouts must stop at Git, even if its
        // parent has an unrelated (or attacker-controlled) bin/bash.exe.
        std::fs::remove_file(root.join("usr/bin/bash.exe")).unwrap();
        std::fs::create_dir(dir.path().join("bin")).unwrap();
        std::fs::write(dir.path().join("bin/bash.exe"), "outside install").unwrap();
        for git in [
            "cmd/git.exe",
            "bin/git.exe",
            "usr/bin/git.exe",
            "mingw32/bin/git.exe",
            "mingw64/bin/git.exe",
        ] {
            assert_eq!(git_bash_beside_git(&root.join(git)), None, "{git}");
        }
    }

    #[test]
    #[cfg(windows)]
    fn test_windows_uses_git_bash() {
        let config = ShellConfig::get().unwrap();
        assert_eq!(config.name, "Git Bash");
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
            .stdin_bytes("unconsumed input")
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
        // A complete buffer can exceed both input and output pipe capacity.
        // Empty input must still supply EOF rather than inherit the caller.
        for input in [Vec::new(), b"hello from stdin\n".repeat(100_000)] {
            let output = Cmd::new("cat").stdin_bytes(input.as_slice()).run().unwrap();
            assert!(output.status.success());
            assert_eq!(output.stdout, input);
        }
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
    fn test_cmd_stream_can_propagate_sigpipe() {
        use crate::git::WorktrunkError;

        let err = Cmd::new("sh")
            .args(["-c", "kill -PIPE $$"])
            .propagate_sigpipe()
            .stream()
            .unwrap_err();
        let wt_err = err.downcast_ref::<WorktrunkError>().unwrap();
        assert!(matches!(
            wt_err,
            WorktrunkError::ChildProcessExited {
                code: 141,
                signal: Some(13),
                ..
            }
        ));
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
    fn test_cmd_delayed_stream_quiet_success() {
        // A fast command under a generous threshold exits during phase 1
        // (wait_timeout returns Some) and stays buffered/quiet.
        Cmd::new("true").delayed_stream(5_000, None).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn test_cmd_delayed_stream_retains_output_after_invalid_utf8() {
        let error = Cmd::new("python3")
            .args([
                "-c",
                "import sys; sys.stdout.buffer.write(b'\\xffinvalid\\r\\n' + b'x' * 250000 + b'\\nTAIL'); sys.stdout.flush(); sys.exit(17)",
            ])
            .delayed_stream(-1, None)
            .unwrap_err();
        let command = error.downcast_ref::<StreamCommandError>().unwrap();
        assert_eq!(command.status.code(), Some(17), "{error:?}");
        assert_eq!(
            command.output,
            format!("�invalid\n{}\nTAIL", "x".repeat(250000)),
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_cmd_delayed_stream_crosses_the_threshold() {
        // A command that outlives the threshold leaves phase 1 with no status
        // (`wait_timeout` returns `Ok(None)`), which switches the readers to
        // streaming, and phase 2 then reports the real exit.
        //
        // What pins the switch is where the late output goes: a streamed line
        // is written to stderr instead of the buffer, so the error carries
        // none of it. Had phase 1 returned a status instead, `late` would
        // still be buffered and would show up here. The other thresholds never
        // reach this arm — `0` streams without waiting, `-1` skips phase 1.
        //
        // The child writes 450 ms after the threshold passes. Only the switch
        // has to land in that window, and it follows the wait immediately, so
        // the margin covers a deschedule far longer than anything the suite
        // produces. The threshold stays well above zero for the opposite
        // reason: were `remaining` to reach it already spent, phase 1 would
        // skip the wait entirely and the test would pass without reaching the
        // arm it exists to cover.
        let err = Cmd::new("sh")
            .args(["-c", "sleep 0.5; echo late 1>&2; exit 3"])
            .delayed_stream(50, None)
            .unwrap_err();
        let stream_err = err
            .downcast_ref::<StreamCommandError>()
            .expect("non-zero delayed_stream exit should be a StreamCommandError");
        assert_eq!(stream_err.exit_info(), "exit code 3");
        assert_eq!(
            stream_err.output, "",
            "output written after the switch must stream, not buffer"
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_cmd_delayed_stream_flushes_what_it_buffered() {
        // Output written *before* the threshold is buffered, and the switch to
        // streaming drains that buffer to stderr before phase 2 begins. This
        // ordering is what reaches the drain loop:
        // `test_cmd_delayed_stream_crosses_the_threshold` writes after the
        // switch, so its line streams from the reader thread and the buffer
        // stays empty throughout.
        //
        // The error's `output` is empty either way — the reader relays a line
        // it picks up after the switch, and the drain empties one it picked up
        // before — so the assertion pins the switch, not the drain itself. A
        // unit test cannot capture stderr, so the window is the only lever on
        // which of the two runs; the integration test next door
        // (`test_delayed_stream_progress_strips_ansi_when_piped`) is what
        // asserts on the relayed text.
        //
        // The threshold is the whole window the reader has to get `early` into
        // the buffer: spawning `sh` and reading its first line has to land
        // inside it, or the switch goes first and the drain runs over nothing.
        // 250 ms absorbs a slow spawn on a loaded runner, and the child's
        // 500 ms still outlasts the threshold by the same margin.
        let err = Cmd::new("sh")
            .args(["-c", "echo early 1>&2; sleep 0.5; exit 3"])
            .delayed_stream(250, None)
            .unwrap_err();
        let stream_err = err
            .downcast_ref::<StreamCommandError>()
            .expect("non-zero delayed_stream exit should be a StreamCommandError");
        assert_eq!(stream_err.exit_info(), "exit code 3");
        assert_eq!(
            stream_err.output, "",
            "output buffered before the switch must be drained to stderr, not reported"
        );
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
        assert_eq!(stream_err.exit_info(), "exit code 3");
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
    fn test_cmd_run_concurrently_overlaps_children_and_keeps_order() {
        // The first child waits for a file that only the last child creates, so
        // it succeeds only if both run at once. A spawn failure in between
        // stays in its own slot rather than shifting the results after it.
        let dir = tempfile::tempdir().unwrap();
        let flag = dir.path().join("flag");
        let wait_for_flag = format!(
            "for _ in $(seq 500); do [ -e '{}' ] && exit 0; sleep 0.01; done; exit 1",
            flag.display()
        );
        let results = Cmd::run_concurrently(&[
            Cmd::new("sh").args(["-c", wait_for_flag.as_str()]),
            Cmd::new(MISSING_CMD),
            Cmd::new("sh").args(["-c", "exit 3"]),
            Cmd::new("touch").arg(flag.to_str().unwrap()),
        ]);
        let [waiter, missing, exit3, toucher]: [_; 4] = results.try_into().unwrap();
        assert!(
            waiter.unwrap().status.success(),
            "children ran one at a time"
        );
        assert_eq!(missing.unwrap_err().kind(), ErrorKind::NotFound);
        assert_eq!(exit3.unwrap().status.code(), Some(3));
        assert!(toucher.unwrap().status.success());
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
        #[cfg(unix)]
        use std::os::unix::process::ExitStatusExt;
        #[cfg(windows)]
        use std::os::windows::process::ExitStatusExt;

        // The Display impl exists only for the Error bound; callers read fields
        // via Repository::extract_failed_command, so nothing else exercises it.
        let err = StreamCommandError {
            output: "fatal: ref exists".to_string(),
            command: "git worktree add /x".to_string(),
            status: std::process::ExitStatus::from_raw(if cfg!(unix) { 128 << 8 } else { 128 }),
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
    fn test_group_died_within_immediate_for_reaped_group() {
        // A reaped child leaves an empty group: the first (immediate) probe
        // reads ESRCH and the grace loop returns without sleeping. This pins
        // the early exit that keeps escalation cheap for the callers whose
        // children are reaped concurrently (tether).
        use std::os::unix::process::CommandExt;
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", ":"]).process_group(0);
        let mut child = cmd.spawn().unwrap();
        let pgid = child.id() as i32;
        child.wait().unwrap();

        let start = Instant::now();
        // Grace far longer than any plausible scheduling stall, so returning
        // early is structurally distinguishable from having slept it, and the
        // bound below is a safety net rather than a race.
        assert!(super::group_died_within(pgid, Duration::from_secs(30)));
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "an empty group must exit the grace loop on the first probe; took {:?}",
            start.elapsed()
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_group_died_within_times_out_on_live_group() {
        // Probing our own (live) process group runs the full grace and
        // reports the group still alive.
        let pgid = nix::unistd::getpgrp().as_raw();
        let grace = Duration::from_millis(50);
        let start = Instant::now();
        assert!(!super::group_died_within(pgid, grace));
        assert!(start.elapsed() >= grace);
    }

    #[test]
    #[cfg(unix)]
    fn test_escalation_full_grace_and_inert_sweep_when_leader_unreaped() {
        // Replays `kill_timed_out_tree`'s position: the caller holds the group
        // leader unreaped while escalating. The child has already exited when
        // escalation starts (stdout EOF is the barrier — the pipe closes when
        // the process exits, so this needs no scheduling assumptions), but
        // nobody reaps it, so the liveness probe counts the zombie, the grace
        // runs to its deadline, and the final group SIGKILL fires against the
        // dead group. The recorded exit must come through untouched — signals
        // to a fully-exited group are discarded.
        use std::os::unix::process::CommandExt;
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "exit 7"])
            .stdout(std::process::Stdio::piped())
            .process_group(0);
        let mut child = cmd.spawn().unwrap();
        let pid = child.id() as i32;
        let mut eof = Vec::new();
        child.stdout.take().unwrap().read_to_end(&mut eof).unwrap();

        let start = Instant::now();
        super::terminate_process_group(pid);
        assert!(
            start.elapsed() >= Duration::from_millis(200),
            "with the leader unreaped the group must read alive for the whole grace"
        );

        let status = child.wait().unwrap();
        assert_eq!(
            status.code(),
            Some(7),
            "the TERM and the post-grace SIGKILL must not alter the recorded exit"
        );
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
