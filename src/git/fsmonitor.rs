//! Identify and reap orphaned `git fsmonitor--daemon` processes.
//!
//! Git's builtin fsmonitor (`core.fsmonitor=true`) starts one
//! `git fsmonitor--daemon` per worktree. Each daemon owns a Unix domain socket
//! at `<git-dir-for-worktree>/fsmonitor--daemon.ipc`. The daemon is a stateless
//! filesystem-watch cache: if its worktree is gone, git will never talk to it
//! or respawn it, and it will live until the machine reboots. Daemons leak
//! whenever a worktree is destroyed by a path that bypasses `wt remove`'s
//! `git fsmonitor--daemon stop` call — plain `git worktree remove`, manual
//! `rm -rf`, or a crashed `wt`.
//!
//! This module is the defense-in-depth sweep: it finds daemons that are
//! *provably* orphaned and terminates them. It rides the existing repo-wide
//! internal cleanup op (the same opportunistic cadence as the stale-trash
//! sweep), runs after primary user output, and is strictly best-effort —
//! every failure path logs at debug level and continues. It never fails or
//! materially slows the operation it rides on.
//!
//! # Data-safety contract
//!
//! A daemon is reaped only when it is provably not serving any live worktree.
//! Two classes qualify, both identified through the IPC socket alone (never by
//! matching process names broadly):
//!
//! 1. **Unresolvable socket** — `lsof` reports the socket as the bare name
//!    `fsmonitor--daemon.ipc` with no directory. That happens only when the
//!    socket's containing directory (the worktree's git-dir) no longer exists.
//!    A live worktree's git-dir always exists, so a bare-name socket cannot
//!    belong to any live worktree of any repo. Reaped unconditionally — this
//!    is the class behind large machine-wide accumulations.
//! 2. **Resolved, but not a live worktree of this repo** — the socket resolves
//!    to `<git-dir>/fsmonitor--daemon.ipc` where `<git-dir>` is under this
//!    repo's git-common-dir but is not the git-dir of any live (non-prunable,
//!    on-disk) worktree. This is a worktree that was removed while its git
//!    metadata lingered (e.g. a not-yet-pruned `prunable` entry). Class 2
//!    requires a trustworthy live-worktree set: if `git worktree list` fails,
//!    the set is unknowable and class 2 is skipped entirely, so a transient
//!    git failure can never escalate to reaping a live worktree's daemon.
//!    Only class 1, which holds regardless of the live set, survives that.
//!
//! A daemon whose socket resolves to a *live* worktree's git-dir is NEVER
//! reaped, even if it appears wedged. Killing a live worktree's daemon
//! implicitly is out of scope by design (no implicit destructive side
//! effects); the residual wedged-but-live case is handled by the user via the
//! documented manual workaround.
//!
//! # Signal discipline
//!
//! Orphans get `SIGTERM`, then a single bounded poll budget
//! (`REAP_KILL_DEADLINE`) for them to exit, then `SIGKILL` for any
//! survivor. The wait is always bounded — never indefinite.
//!
//! # Platform
//!
//! Unix only. The daemon uses a named pipe on Windows (not a socket), and
//! `lsof` is not the discovery tool there; the reap is a no-op on Windows
//! rather than guessing. The public entry point compiles on all platforms so
//! callers need no `cfg`; the Unix-specific machinery is gated internally.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::Repository;
#[cfg(doc)]
use super::WorkingTree;

/// Basename of every fsmonitor IPC socket. Also the exact string `lsof`
/// prints when the socket's directory has been deleted (orphan class 1).
const IPC_SOCKET_NAME: &str = "fsmonitor--daemon.ipc";

/// How long to wait for `SIGTERM`'d daemons to exit before escalating to
/// `SIGKILL`. Bounded so the sweep can never stall `wt`. Daemons are wedged
/// in their IPC handling, not in signal handling, so they almost always die
/// on `SIGTERM` well within this budget; the `SIGKILL` tail only fires for a
/// daemon that ignores `SIGTERM`.
///
/// Shared with [`crate::git::remove`]'s `force_kill_fsmonitor_via_socket` so
/// both fsmonitor-stop paths apply the same grace window.
#[cfg(unix)]
pub(crate) const REAP_KILL_DEADLINE: std::time::Duration = std::time::Duration::from_millis(1500);

/// One running fsmonitor daemon: its PID and the IPC socket path `lsof`
/// reported for it. `socket` is [`None`] when `lsof` printed the bare,
/// unresolvable socket name (orphan class 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonProcess {
    pub pid: u32,
    pub socket: Option<PathBuf>,
}

/// Abstraction over process signalling so the SIGTERM→wait→SIGKILL escalation
/// is unit-testable without spawning real daemons.
pub trait ProcessSignaller {
    /// Send `SIGTERM` to `pid`. Best-effort; errors are swallowed by the
    /// caller (the process may already be gone).
    fn term(&self, pid: u32);
    /// Send `SIGKILL` to `pid`.
    fn kill(&self, pid: u32);
    /// Whether `pid` is still alive.
    fn is_alive(&self, pid: u32) -> bool;
}

/// Extract the IPC socket path from `lsof -F n` output for a single daemon
/// PID.
///
/// `lsof -a -p <pid> -U -F n` prints one record per field-prefixed line. Unix
/// socket name lines start with `n`; connected-pair endpoints look like
/// `n->0x…` and are skipped. The fsmonitor socket line is the one whose name
/// ends in `IPC_SOCKET_NAME`.
///
/// Returns:
/// - `Some(path)` for a resolved socket path (an absolute path ending in the
///   socket name).
/// - `None` when the only match is the bare, directory-less socket name,
///   which signals an orphan whose git-dir was deleted.
///
/// A daemon with no fsmonitor socket line at all also yields `None`; the
/// caller treats a `None` socket as orphan class 1, which is correct: a
/// `git fsmonitor--daemon` process holding no resolvable fsmonitor socket is
/// not serving a live worktree.
pub fn parse_lsof_socket_path(lsof_stdout: &str) -> Option<PathBuf> {
    for line in lsof_stdout.lines() {
        let Some(name) = line.strip_prefix('n') else {
            continue;
        };
        // Skip socket-pair endpoints ("n->0x…"); only named sockets resolve
        // to a filesystem path.
        if name.starts_with("->") {
            continue;
        }
        if name == IPC_SOCKET_NAME {
            // Bare name: the socket's directory is gone → unresolvable.
            return None;
        }
        if Path::new(name).file_name().and_then(|f| f.to_str()) == Some(IPC_SOCKET_NAME) {
            return Some(PathBuf::from(name));
        }
    }
    None
}

/// Canonicalize a resolved socket path so its git-dir compares exactly
/// against the canonicalized live-worktree git-dirs.
///
/// `lsof` prints the socket path verbatim (un-normalized), while
/// [`WorkingTree::git_dir`] canonicalizes (resolving e.g. macOS
/// `/var`→`/private/var`). Without aligning the two, a live worktree whose
/// path contains a symlink would never match the live set and its daemon
/// would be misclassified as an orphan — a data-safety violation.
///
/// Canonicalizes the git-dir (the socket's parent) and rejoins the socket
/// name. The caller only ever passes a resolved socket ending in the IPC
/// name, so it always has a parent and a file name. Any failure to resolve
/// the parent (the git-dir is gone — the orphan case) keeps the lexical
/// path: it still won't match any canonicalized live git-dir (correct: it's
/// an orphan) and git never relocates a worktree's admin dir, so the
/// repo-prefix check stays valid.
#[cfg(unix)]
fn canonicalize_socket(socket: &Path) -> PathBuf {
    match (
        socket.parent().and_then(|p| dunce::canonicalize(p).ok()),
        socket.file_name(),
    ) {
        (Some(real_parent), Some(name)) => real_parent.join(name),
        _ => socket.to_path_buf(),
    }
}

/// Derive the worktree git-dir that owns a resolved IPC socket path.
///
/// The socket always lives at `<git-dir>/fsmonitor--daemon.ipc`, so the
/// git-dir is simply the socket's parent. Returns `None` if the path has no
/// parent (it never should — a resolved socket path is always absolute).
pub fn socket_path_to_git_dir(socket: &Path) -> Option<PathBuf> {
    socket.parent().map(Path::to_path_buf)
}

/// Select the PIDs of daemons that are provably orphaned.
///
/// This is the data-safety core. A daemon is selected only when:
///
/// - its socket is unresolvable (`socket == None`) — provably not a live
///   worktree of any repo (orphan class 1); or
/// - `live_git_dirs` is `Some` *and* the socket's git-dir is under
///   `repo_common_dir` *and* is not in that set (orphan class 2).
///
/// `live_git_dirs` is `None` when the live-worktree set is unknowable (the
/// `git worktree list` underlying it failed). Class 2 is then **disabled
/// entirely** — only the provably-safe unresolvable class is reaped. Treating
/// an unknowable set as empty would reap every resolved-socket daemon under
/// the repo, including live worktrees', violating the invariant that a daemon
/// serving a live worktree is never reaped.
///
/// A daemon whose git-dir is in the live set is never selected. A daemon whose
/// resolved git-dir is outside `repo_common_dir` belongs to another repository
/// and is left for that repository's own sweep; only the unresolvable class is
/// reaped repo-agnostically, because it cannot be attributed to — or endanger
/// — any live worktree.
///
/// All path inputs are expected pre-canonicalized by the caller so equality
/// comparisons are exact.
pub fn classify_orphans(
    daemons: &[DaemonProcess],
    live_git_dirs: Option<&HashSet<PathBuf>>,
    repo_common_dir: &Path,
) -> Vec<u32> {
    daemons
        .iter()
        .filter(|d| match &d.socket {
            None => true,
            Some(socket) => match (live_git_dirs, socket_path_to_git_dir(socket)) {
                (Some(live), Some(git_dir)) => {
                    !live.contains(&git_dir) && git_dir.starts_with(repo_common_dir)
                }
                // Unknowable live set, or a socket with no parent: never a
                // class-2 orphan — leave it for a run with a trustworthy set.
                _ => false,
            },
        })
        .map(|d| d.pid)
        .collect()
}

/// `SIGTERM` every PID, wait up to [`REAP_KILL_DEADLINE`] for them to exit,
/// then `SIGKILL` any survivor.
///
/// The poll loop returns early as soon as every PID is gone, so the deadline
/// is only reached when a daemon ignores `SIGTERM`. Returns the count of
/// daemons confirmed gone (terminated or already absent).
#[cfg(unix)]
pub(crate) fn escalate_terminate<S: ProcessSignaller>(
    signaller: &S,
    pids: &[u32],
    deadline: std::time::Duration,
) -> usize {
    if pids.is_empty() {
        return 0;
    }
    for &pid in pids {
        signaller.term(pid);
    }

    let poll = std::time::Duration::from_millis(50);
    let start = std::time::Instant::now();
    loop {
        if pids.iter().all(|&pid| !signaller.is_alive(pid)) {
            return pids.len();
        }
        if start.elapsed() >= deadline {
            break;
        }
        std::thread::sleep(poll);
    }

    let mut gone = 0;
    for &pid in pids {
        if signaller.is_alive(pid) {
            signaller.kill(pid);
        }
        if !signaller.is_alive(pid) {
            gone += 1;
        }
    }
    gone
}

/// Real signal delivery via `nix`, used in production.
#[cfg(unix)]
pub(crate) struct NixSignaller;

#[cfg(unix)]
impl ProcessSignaller for NixSignaller {
    fn term(&self, pid: u32) {
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid as i32),
            nix::sys::signal::Signal::SIGTERM,
        );
    }

    fn kill(&self, pid: u32) {
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid as i32),
            nix::sys::signal::Signal::SIGKILL,
        );
    }

    fn is_alive(&self, pid: u32) -> bool {
        // Signal 0 (`None`) performs existence/permission checking without
        // delivering a signal: `Ok` or `EPERM` means the process exists;
        // `ESRCH` means it doesn't.
        match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None) {
            Ok(()) => true,
            Err(nix::errno::Errno::EPERM) => true,
            Err(_) => false,
        }
    }
}

/// Enumerate running `git fsmonitor--daemon` processes and resolve each one's
/// IPC socket via `lsof`.
///
/// `pgrep -f 'git fsmonitor--daemon'` lists candidate PIDs; `lsof` resolves
/// each PID's Unix socket. Both run through [`crate::shell_exec::Cmd`] with a
/// short timeout. Any failure (tool missing, non-zero exit, unparsable
/// output) is treated as "no daemons" — the sweep is best-effort.
#[cfg(unix)]
fn enumerate_daemons() -> Vec<DaemonProcess> {
    use crate::shell_exec::Cmd;

    let timeout = std::time::Duration::from_secs(5);

    let Ok(output) = Cmd::new("pgrep")
        .args(["-f", "git fsmonitor--daemon"])
        .timeout(timeout)
        .run()
    else {
        return Vec::new();
    };
    // pgrep exits 1 when nothing matched — that is the common, healthy case.
    if !output.status.success() {
        return Vec::new();
    }
    let pids: Vec<u32> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|l| l.trim().parse::<u32>().ok())
        .collect();

    pids.into_iter()
        .filter_map(|pid| {
            let out = Cmd::new("lsof")
                .args(["-a", "-p", &pid.to_string(), "-U", "-F", "n"])
                .timeout(timeout)
                .run()
                .ok()?;
            // lsof exits non-zero when a PID vanished mid-scan; skip it
            // (a gone process is not an orphan we need to reap).
            if !out.status.success() {
                return None;
            }
            daemon_from_lsof_stdout(pid, &String::from_utf8_lossy(&out.stdout))
        })
        .collect()
}

/// Classify the lsof output for one PID into an [`Option<DaemonProcess>`].
///
/// `pgrep -f "git fsmonitor--daemon"` matches the full command line, so a
/// non-daemon process whose argv merely contains that string (a debugger
/// `gdb …/git fsmonitor--daemon`, a wrapper) is also enumerated as a
/// candidate. A real fsmonitor daemon always holds its IPC socket (the
/// resolved path or, on some platforms, the bare socket name), so its lsof
/// output contains [`IPC_SOCKET_NAME`]. Output that does not contain
/// [`IPC_SOCKET_NAME`] cannot belong to a daemon — return [`None`] rather
/// than misclassify as a class-1 (unresolvable-socket) orphan, which would
/// SIGTERM/SIGKILL the unrelated process during the sweep.
#[cfg(unix)]
fn daemon_from_lsof_stdout(pid: u32, stdout: &str) -> Option<DaemonProcess> {
    if !stdout.contains(IPC_SOCKET_NAME) {
        return None;
    }
    let socket = parse_lsof_socket_path(stdout).map(|s| canonicalize_socket(&s));
    Some(DaemonProcess { pid, socket })
}

/// Canonicalized git-dirs of every live (non-prunable, on-disk) worktree in
/// `repo`, or [`None`] if the worktree list itself could not be obtained.
///
/// The `Option` distinguishes "this repo has no qualifying live worktrees"
/// (empty set — every resolved-socket daemon under the repo is then a class-2
/// orphan) from "the live set is unknowable" (`None` — `git worktree list`
/// failed). The caller must NOT treat the unknowable case as an empty set: a
/// resolved-socket daemon would then be misclassified as an orphan and a live
/// worktree's daemon could be reaped. `None` disables class-2 reaping entirely
/// (see [`classify_orphans`]); only the provably-safe unresolvable-socket
/// class survives an unknowable live set.
///
/// `git_dir()` is canonicalized and process-cached, so this is cheap on the
/// `wt remove` path where the cache is already warm. A worktree whose
/// `git_dir()` resolution fails individually is dropped from the set, which
/// leaves a residual risk: that worktree's daemon then looks class-2 and
/// could be reaped despite being live. This residual is accepted, not
/// eliminated — per-worktree resolution failure is far rarer than the
/// whole-list failure that `None` already covers, and the worst case is a
/// respawnable fsmonitor cache daemon restarting on the next `git status`,
/// never user data. The whole-list failure, which would endanger every live
/// worktree at once, is the one elevated to `None`.
#[cfg(unix)]
fn live_git_dirs(repo: &Repository) -> Option<HashSet<PathBuf>> {
    let worktrees = repo.list_worktrees().ok()?;
    Some(
        worktrees
            .iter()
            .filter(|wt| !wt.is_prunable() && wt.path.exists())
            .filter_map(|wt| repo.worktree_at(&wt.path).git_dir().ok())
            .collect(),
    )
}

/// Reap orphaned fsmonitor daemons for `repo`.
///
/// Fire-and-forget defense-in-depth: enumerate daemons, classify the provably
/// orphaned ones (see the module docstring's data-safety contract), and
/// escalate `SIGTERM`→`SIGKILL` with a bounded wait. Best-effort throughout;
/// any failure logs at debug level and returns. No-op on non-Unix.
///
/// Call after primary user output so the bounded wait can never delay a
/// user-visible message.
pub fn reap_orphan_fsmonitor_daemons(repo: &Repository) {
    #[cfg(unix)]
    {
        let daemons = enumerate_daemons();
        if daemons.is_empty() {
            return;
        }

        let live = live_git_dirs(repo);
        let common_dir = dunce::canonicalize(repo.git_common_dir())
            .unwrap_or_else(|_| repo.git_common_dir().to_path_buf());

        let orphans = classify_orphans(&daemons, live.as_ref(), &common_dir);
        if orphans.is_empty() {
            return;
        }

        log::debug!(
            "Reaping {} orphaned fsmonitor daemon(s): {:?}",
            orphans.len(),
            orphans
        );
        let gone = escalate_terminate(&NixSignaller, &orphans, REAP_KILL_DEADLINE);
        log::debug!(
            "Orphaned fsmonitor reap: {gone}/{} terminated",
            orphans.len()
        );
    }
    #[cfg(not(unix))]
    {
        let _ = repo;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Only the Unix-gated FakeSignaller uses these.
    #[cfg(unix)]
    use std::cell::RefCell;
    #[cfg(unix)]
    use std::collections::HashMap;

    #[test]
    fn parses_resolved_socket_path() {
        let lsof = "p10033\nf21\nn->0x62fc003fda86ee70\nf24\nn/Users/me/repo/.git/worktrees/repo.feat/fsmonitor--daemon.ipc\n";
        assert_eq!(
            parse_lsof_socket_path(lsof),
            Some(PathBuf::from(
                "/Users/me/repo/.git/worktrees/repo.feat/fsmonitor--daemon.ipc"
            ))
        );
    }

    #[test]
    fn bare_socket_name_is_unresolvable() {
        // A deleted worktree's directory is gone, so lsof prints just the
        // basename — this must classify as unresolvable (orphan class 1).
        let lsof = "p10311\nf24\nnfsmonitor--daemon.ipc\n";
        assert_eq!(parse_lsof_socket_path(lsof), None);
    }

    #[test]
    fn no_fsmonitor_socket_yields_none() {
        let lsof = "p999\nf3\nn->0xdead\nf4\nn->0xbeef\n";
        assert_eq!(parse_lsof_socket_path(lsof), None);
    }

    #[cfg(unix)]
    #[test]
    fn canonicalize_socket_resolves_symlinked_git_dir_and_falls_back_when_gone() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real-gitdir");
        std::fs::create_dir(&real).unwrap();
        let link = tmp.path().join("link-gitdir");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        // Existing (symlinked) git-dir: parent resolves to the real path so
        // the result compares equal to a canonicalized live git-dir.
        let via_link = link.join(IPC_SOCKET_NAME);
        let resolved = canonicalize_socket(&via_link);
        assert_eq!(
            resolved,
            dunce::canonicalize(&real).unwrap().join(IPC_SOCKET_NAME)
        );

        // Deleted git-dir: canonicalization fails, the lexical path is kept
        // (still won't match any live git-dir → correctly an orphan).
        let gone = tmp.path().join("deleted-gitdir").join(IPC_SOCKET_NAME);
        assert_eq!(canonicalize_socket(&gone), gone);
    }

    #[test]
    fn git_dir_is_socket_parent() {
        let socket = Path::new("/r/.git/worktrees/r.x/fsmonitor--daemon.ipc");
        assert_eq!(
            socket_path_to_git_dir(socket),
            Some(PathBuf::from("/r/.git/worktrees/r.x"))
        );
    }

    /// `pgrep -f "git fsmonitor--daemon"` is a substring match against the
    /// full command line, so a non-daemon process whose argv merely contains
    /// that string (a debugger `gdb …/git fsmonitor--daemon`, a wrapper)
    /// is also enumerated. Its `lsof` output won't reference the IPC socket
    /// — `daemon_from_lsof_stdout` must return `None` rather than
    /// misclassify it as a class-1 orphan, which would SIGTERM/SIGKILL the
    /// unrelated process during the sweep.
    #[cfg(unix)]
    #[test]
    fn daemon_from_lsof_skips_pgrep_false_positive() {
        let stdout = "p1234\nn/usr/lib/libc.so\nn/tmp/some-other.sock\nn/dev/null\n";
        assert_eq!(daemon_from_lsof_stdout(1234, stdout), None);
    }

    /// A real daemon with a resolved IPC-socket path classifies with a
    /// `Some(path)` socket; the caller then decides class-2 based on whether
    /// the worktree is live.
    #[cfg(unix)]
    #[test]
    fn daemon_from_lsof_keeps_resolved_socket() {
        let stdout = "p1234\nn/r/.git/worktrees/r.x/fsmonitor--daemon.ipc\n";
        let d = daemon_from_lsof_stdout(1234, stdout).expect("real daemon");
        assert_eq!(d.pid, 1234);
        assert!(d.socket.is_some());
    }

    /// When `lsof` reports the socket only as the bare name (no parent
    /// path), the daemon still classifies with `socket = None` — the
    /// existing class-1 "worktree-gone" reap path. The new guard must
    /// preserve this.
    #[cfg(unix)]
    #[test]
    fn daemon_from_lsof_keeps_bare_socket_name_as_class_one() {
        let stdout = "p1234\nnfsmonitor--daemon.ipc\n";
        let d = daemon_from_lsof_stdout(1234, stdout).expect("bare-name daemon");
        assert_eq!(d.pid, 1234);
        assert!(
            d.socket.is_none(),
            "bare name must remain unresolved (class 1)"
        );
    }

    fn daemon(pid: u32, socket: Option<&str>) -> DaemonProcess {
        DaemonProcess {
            pid,
            socket: socket.map(PathBuf::from),
        }
    }

    #[test]
    fn live_worktree_daemon_is_never_reaped() {
        let common = PathBuf::from("/r/.git");
        let live_git_dir = PathBuf::from("/r/.git/worktrees/r.live");
        let live: HashSet<PathBuf> = [live_git_dir.clone()].into_iter().collect();

        let daemons = vec![daemon(
            1,
            Some("/r/.git/worktrees/r.live/fsmonitor--daemon.ipc"),
        )];
        let orphans = classify_orphans(&daemons, Some(&live), &common);
        assert!(
            orphans.is_empty(),
            "a daemon mapping to a live worktree must never be selected"
        );
    }

    #[test]
    fn unknowable_live_set_spares_resolved_socket_daemons() {
        // `git worktree list` failed → live set is `None`. A resolved-socket
        // daemon under this repo (which would be a class-2 orphan with a
        // trustworthy set) must NOT be reaped: we cannot prove it isn't a
        // live worktree. Only the unresolvable class-1 daemon is reaped.
        let common = PathBuf::from("/r/.git");
        let daemons = vec![
            daemon(10, None),
            daemon(
                20,
                Some("/r/.git/worktrees/r.maybe-live/fsmonitor--daemon.ipc"),
            ),
        ];
        let orphans = classify_orphans(&daemons, None, &common);
        assert_eq!(
            orphans,
            vec![10],
            "an unknowable live set must spare every resolved-socket daemon"
        );
    }

    #[test]
    fn classifies_each_orphan_class_and_spares_others() {
        let common = PathBuf::from("/r/.git");
        let live: HashSet<PathBuf> = [PathBuf::from("/r/.git/worktrees/r.live")]
            .into_iter()
            .collect();

        let daemons = vec![
            // class 1: unresolvable socket → reap regardless of repo.
            daemon(10, None),
            // class 2: resolved, under this repo's common-dir, not live → reap.
            daemon(20, Some("/r/.git/worktrees/r.gone/fsmonitor--daemon.ipc")),
            // live worktree → spare.
            daemon(30, Some("/r/.git/worktrees/r.live/fsmonitor--daemon.ipc")),
            // resolved but belongs to a different repo → spare (left for
            // that repo's own sweep).
            daemon(40, Some("/other/.git/worktrees/o.x/fsmonitor--daemon.ipc")),
        ];

        let mut orphans = classify_orphans(&daemons, Some(&live), &common);
        orphans.sort_unstable();
        assert_eq!(orphans, vec![10, 20]);
    }

    /// Programmable signaller: `survives` PIDs stay alive through `SIGTERM`
    /// and only die on `SIGKILL`; everything else dies on `SIGTERM`.
    ///
    /// `escalate_terminate` is Unix-only, so its test scaffolding is gated to
    /// match — on non-Unix the fake would be dead code.
    #[cfg(unix)]
    struct FakeSignaller {
        alive: RefCell<HashMap<u32, bool>>,
        survives_term: HashSet<u32>,
        term_calls: RefCell<Vec<u32>>,
        kill_calls: RefCell<Vec<u32>>,
    }

    #[cfg(unix)]
    impl FakeSignaller {
        fn new(pids: &[u32], survives_term: &[u32]) -> Self {
            Self {
                alive: RefCell::new(pids.iter().map(|&p| (p, true)).collect()),
                survives_term: survives_term.iter().copied().collect(),
                term_calls: RefCell::new(Vec::new()),
                kill_calls: RefCell::new(Vec::new()),
            }
        }
    }

    #[cfg(unix)]
    impl ProcessSignaller for FakeSignaller {
        fn term(&self, pid: u32) {
            self.term_calls.borrow_mut().push(pid);
            if !self.survives_term.contains(&pid) {
                self.alive.borrow_mut().insert(pid, false);
            }
        }
        fn kill(&self, pid: u32) {
            self.kill_calls.borrow_mut().push(pid);
            self.alive.borrow_mut().insert(pid, false);
        }
        fn is_alive(&self, pid: u32) -> bool {
            *self.alive.borrow().get(&pid).unwrap_or(&false)
        }
    }

    #[cfg(unix)]
    #[test]
    fn sigterm_alone_terminates_responsive_daemons() {
        let fake = FakeSignaller::new(&[1, 2], &[]);
        let gone = escalate_terminate(&fake, &[1, 2], std::time::Duration::from_millis(200));
        assert_eq!(gone, 2);
        assert_eq!(*fake.term_calls.borrow(), vec![1, 2]);
        assert!(
            fake.kill_calls.borrow().is_empty(),
            "responsive daemons must not be SIGKILL'd"
        );
    }

    #[cfg(unix)]
    #[test]
    fn escalates_to_sigkill_after_bounded_wait() {
        // PID 2 ignores SIGTERM; it must be SIGKILL'd after the deadline,
        // and the call must still return bounded.
        let fake = FakeSignaller::new(&[1, 2], &[2]);
        let start = std::time::Instant::now();
        let gone = escalate_terminate(&fake, &[1, 2], std::time::Duration::from_millis(150));
        let elapsed = start.elapsed();

        assert_eq!(gone, 2);
        assert_eq!(*fake.kill_calls.borrow(), vec![2]);
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "escalation must stay bounded, took {elapsed:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn empty_pid_list_is_a_noop() {
        let fake = FakeSignaller::new(&[], &[]);
        assert_eq!(
            escalate_terminate(&fake, &[], std::time::Duration::from_millis(10)),
            0
        );
        assert!(fake.term_calls.borrow().is_empty());
    }

    /// End-to-end: `NixSignaller` actually delivers `SIGTERM` and a
    /// responsive child exits from it. The `FakeSignaller` tests above cover
    /// the escalation logic; this asserts the real signal-delivery path that
    /// production calls go through.
    ///
    /// The `gone` count returned by `escalate_terminate` is not asserted on
    /// here: a SIGTERM'd *direct* child becomes a zombie until `wait()`, and
    /// `kill(pid, 0)` (the liveness probe) reports zombies as alive — so the
    /// count under-reports for this test setup. Production daemons are not
    /// children of `wt`; their parent reaps them and `is_alive` flips to
    /// false promptly, so the count is meaningful there.
    #[cfg(unix)]
    #[test]
    fn nix_signaller_terminates_responsive_child_with_sigterm() {
        use std::process::Command;

        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id();

        escalate_terminate(&NixSignaller, &[pid], std::time::Duration::from_millis(500));

        // `sleep` exits on SIGTERM; it must be reaped and have a
        // signal-derived (non-success) status.
        let status = child.wait().unwrap();
        assert!(!status.success());
    }

    /// End-to-end: a child that ignores `SIGTERM` is SIGKILL'd after the
    /// bounded wait via the real `NixSignaller`.
    #[cfg(unix)]
    #[test]
    fn nix_signaller_escalates_to_sigkill_when_sigterm_ignored() {
        use std::os::unix::process::CommandExt;
        use std::process::Command;
        use std::time::Duration;

        // `trap '' TERM` makes SIGTERM a no-op; only SIGKILL can stop it. The
        // child touches `ready` *after* the trap is installed; the test waits
        // for that file so the first SIGTERM can't race trap installation
        // (which would let the child die on SIGTERM and report signal 15).
        //
        // The trap also forces the shell to fork a separate `sleep` child
        // rather than exec it, so the tree is sh → sleep. `process_group(0)`
        // makes sh a group leader (the `sleep` inherits its group) so the
        // orphaned grandchild can be reaped after sh is killed — see the group
        // SIGKILL below.
        let tmp = tempfile::tempdir().unwrap();
        let ready = tmp.path().join("ready");
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(format!(
                "trap '' TERM; : > {}; sleep 30",
                ready.to_str().unwrap()
            ))
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = child.id();

        let wait_deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !ready.exists() {
            assert!(
                std::time::Instant::now() < wait_deadline,
                "child never installed SIGTERM trap"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        // Short escalation deadline keeps the test fast; the FakeSignaller
        // tests already cover that the production `REAP_KILL_DEADLINE` value
        // flows through unchanged.
        escalate_terminate(&NixSignaller, &[pid], Duration::from_millis(200));

        // `escalate_terminate` SIGKILLs only sh's pid, orphaning the forked
        // `sleep` grandchild — reparented to init, it would linger ~30s holding
        // this test's inherited stdout/stderr, which nextest reports as a leak.
        // sh leads its own process group, so SIGKILL the whole group (negative
        // pid) to reap the grandchild before the test returns. sh is already a
        // zombie here, so this only reaches the `sleep`.
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(-(pid as i32)),
            nix::sys::signal::Signal::SIGKILL,
        );

        // Must have been SIGKILLed despite ignoring SIGTERM.
        use std::os::unix::process::ExitStatusExt;
        let status = child.wait().unwrap();
        assert_eq!(
            status.signal(),
            Some(nix::sys::signal::Signal::SIGKILL as i32)
        );
    }
}
