//! Worktree removal with fast-path trash staging and safe branch deletion.
//!
//! Two entry points:
//!
//! - [`stage_worktree_removal`] — the ordered prelude every removal path runs
//!   in the foreground before the worktree directory stops existing: the
//!   ownership checks, fsmonitor stop, synchronized final gates, then the
//!   rename into trash. Staging and direct removal share the final safety gate.
//! - [`remove_worktree_with_cleanup`] — that prelude, plus the direct-removal
//!   fallback and branch deletion, run to completion synchronously.
//!
//! The split exists because the default path defers its deletion: `wt remove`
//! and `wt merge --remove` stage the worktree here, then hand the deletion to
//! a detached [`drain_trash`] so the command returns as soon as the workspace
//! is clear. If staging fails, they delegate guarded direct removal to a
//! detached worker so the caller and its shell can release the current path;
//! prune waits for noncurrent fallback removals. They build that tail rather than calling
//! [`remove_worktree_with_cleanup`]. The callers that run to completion —
//! `--foreground` removals, the TUI picker, and external tooling (e.g.
//! `worktrunk-sync`) — call it and get the fallback and integration-checked
//! branch deletion for free.
//!
//! # What happens during removal
//!
//! Steps 1-3 are [`stage_worktree_removal`]; 4-5 are the rest of
//! [`remove_worktree_with_cleanup`].
//!
//! 1. **Ownership and lock checks**. A `git worktree lock` is refused
//!    unconditionally (matching `git worktree remove`; `--force` does not
//!    override it). With built-in fsmonitor, an initial clean check can
//!    refuse dirty work without stopping the daemon. Clean checks are skipped with
//!    [`RemoveOptions::force_worktree`], which reports the current uncommitted
//!    paths, or why they could not be read, before deletion instead. Why the
//!    dirty gate precedes the stop below: [`stage_worktree_removal`], "Why this order".
//! 2. **fsmonitor daemon stopped** (best effort). [`stop_fsmonitor_daemon`]
//!    runs against the target worktree before its path disappears: it sends
//!    the graceful `git fsmonitor--daemon stop` IPC request, then verifies the
//!    daemon is actually gone and force-kills it by PID if it has wedged.
//!    Without this, a daemon that has stopped answering its socket leaks
//!    forever once its worktree is removed. With fsmonitor configured, the
//!    final clean check runs without it. The registry read lock precedes
//!    that final check and containment scan; it is released before staging
//!    needs the write lock for pruning.
//! 3. **Fast-path trash staging.** The worktree directory is renamed into
//!    `<git-common-dir>/wt/trash/<name>-<timestamp>/`. Same-filesystem renames
//!    are instant metadata operations, so the user's workspace clears
//!    immediately. The caller is responsible for eventually removing the
//!    staged path — either synchronously or via [`drain_trash`] in a
//!    background process.
//! 4. **Fallback removal.** If the rename fails (cross-filesystem, permission
//!    denied, Windows file locks), the code falls back to `git worktree remove`
//!    through [`Repository::remove_worktree`], which reacquires the registry
//!    lock and reruns the final safety gate before deleting files directly.
//! 5. **Branch deletion** (optional). When a branch name is supplied, the
//!    branch is deleted according to the requested [`BranchDeletionMode`]:
//!    - [`Keep`](BranchDeletionMode::Keep): never delete.
//!    - [`SafeDelete`](BranchDeletionMode::SafeDelete): delete only if
//!      [`Repository::integration_reason`] reports the branch as integrated
//!      into `target_branch` (or `HEAD` when unspecified).
//!    - [`ForceDelete`](BranchDeletionMode::ForceDelete): run `branch -D`
//!      without the integration check.
//!
//! # The dirty-worktree gate
//!
//! Under `core.fsmonitor` the daemon answers step 1's `git status`, as it
//! answers `git worktree remove`'s. That first gate refuses a dirty worktree
//! cheaply and leaves its daemon running. Without built-in fsmonitor, the
//! sole clean check runs after shutdown and any registry wait.
//!
//! Stopping the daemon is `wt`'s own step, not git's, and it opens a window
//! git's gate never has: writes during the shutdown. So with fsmonitor
//! configured the gate runs again after the stop, rescanning without the
//! daemon, and that rescan is what a clean removal passes. It costs one full
//! re-stat per removal where fsmonitor is configured, and also catches a
//! stale clean answer from the daemon, which makes `wt` stricter there than
//! `git worktree remove`. (`--no-optional-locks` cannot replace the rescan: it
//! governs index-lock acquisition, and git still queries the daemon under it.)
//!
//! # Example
//!
//! ```no_run
//! use std::path::Path;
//! use worktrunk::git::{
//!     BranchDeletionMode, RemoveOptions, Repository, remove_worktree_with_cleanup,
//! };
//!
//! let repo = Repository::current()?;
//! let snapshot = repo.capture_refs()?;
//! let output = remove_worktree_with_cleanup(
//!     &repo,
//!     &snapshot,
//!     Path::new("/repos/myproject.feature"),
//!     RemoveOptions {
//!         branch: Some("feature".into()),
//!         deletion_mode: BranchDeletionMode::SafeDelete,
//!         target_branch: Some("main".into()),
//!         force_worktree: false,
//!     },
//! )?;
//!
//! // Caller cleans up the staged trash entry (sync or background).
//! if let Some(staged) = output.staged_path {
//!     let _ = std::fs::remove_dir_all(staged);
//! }
//! # Ok::<(), anyhow::Error>(())
//! ```

use std::collections::HashSet;
use std::fs::{self, TryLockError};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context as _;

use crate::git::repository::WorkingTree;
use crate::git::{ErrorExt, GitError, IntegrationReason, Repository, WorktreeInfo, path_dir_name};
use crate::path::{canonicalize_with_parents, format_path_for_display};
use crate::progress::Progress;
use crate::remove_dir::remove_dir_with_progress;
use crate::shell_exec::Cmd;
use crate::styling::{eprintln, format_with_gutter, warning_message};
use crate::utils::epoch_now;

/// Bound on the graceful `git fsmonitor--daemon stop` IPC request.
///
/// `stop` is itself an IPC call to the daemon, so a wedged daemon (the failure
/// this whole helper exists for) makes it hang. The force-kill path below is
/// what actually reaps such a daemon; this timeout just stops the graceful
/// attempt from blocking `wt remove` while the daemon ignores it.
const FSMONITOR_STOP_TIMEOUT: Duration = Duration::from_secs(2);

/// Bound on the `lsof` socket→PID lookup.
#[cfg(unix)]
const FSMONITOR_LSOF_TIMEOUT: Duration = Duration::from_secs(2);

/// Stop the fsmonitor daemon serving `worktree`, force-killing it if it has
/// stopped answering its IPC socket.
///
/// `git fsmonitor--daemon` is a per-worktree, self-respawning filesystem-watch
/// cache git starts when `core.fsmonitor=true`. The graceful shutdown,
/// `git fsmonitor--daemon stop`, is an IPC request *to the daemon itself*: a
/// wedged daemon (one that has stopped answering its socket — the common
/// failure, which also hangs `git status` in that worktree) silently ignores
/// `stop` and then leaks forever once its worktree is gone, since nothing else
/// references it. Worktree removal is the one moment we can still identify the
/// daemon by its socket, so this verifies the daemon is actually gone after
/// `stop` and, on Unix, kills it by PID (SIGTERM, brief wait, SIGKILL) if not.
///
/// This is the single canonical fsmonitor-stop path. It runs **synchronously
/// while the worktree path still exists** (the socket lives under the
/// per-worktree git dir and is needed to resolve the owning PID), so its one
/// caller is [`stage_worktree_removal`], which every removal path runs in the
/// foreground before the directory is staged or pruned. The detached trash drain
/// never touches the daemon; keeping daemon management in
/// the Rust foreground avoids reimplementing socket/PID resolution and signal
/// escalation as a shell string.
///
/// Best-effort and fail-open: every step is bounded by a timeout and every
/// error is logged at debug level and swallowed. A failure here must never
/// fail or materially slow `wt remove`. The PID is only ever resolved from the
/// IPC socket *inside the specific worktree being removed*, so a signal can
/// only ever reach that worktree's own daemon, never another worktree's.
pub fn stop_fsmonitor_daemon(worktree: &WorkingTree) {
    // Graceful path first: a healthy daemon exits cleanly on this IPC request.
    let _ = Cmd::new("git")
        .args(["fsmonitor--daemon", "stop"])
        .scrub_git_discovery_env()
        .current_dir(worktree.path())
        .context(crate::git::repository::path_to_logging_context(
            worktree.path(),
        ))
        .timeout(FSMONITOR_STOP_TIMEOUT)
        .run();

    // Resolve the per-worktree git dir via git (handles the `.git` *file* a
    // linked worktree uses — never hand-construct `<path>/.git`). The daemon
    // binds its IPC socket at `<git-dir>/fsmonitor--daemon.ipc`.
    let socket = match worktree.git_dir() {
        Ok(git_dir) => git_dir.join(super::fsmonitor::IPC_SOCKET_NAME),
        Err(e) => {
            tracing::debug!(error = %e, "fsmonitor: could not resolve git dir, skipping force-kill: {e}");
            return;
        }
    };

    force_kill_fsmonitor_via_socket(&socket);
}

/// Unix: if `socket` still exists, find the daemon owning it via `lsof` and
/// terminate it (SIGTERM, bounded wait, SIGKILL).
///
/// `lsof -t -- <socket>` prints just the owning PID(s), one per line, and
/// exits 0 when found / 1 when nothing holds the socket. (`--` ends option
/// parsing so a socket path is never mistaken for a flag.) Matching by socket
/// path (not process name) guarantees a signal only ever reaches the daemon
/// for *this* worktree: a different worktree's daemon binds a different socket,
/// and once the daemon exits nothing holds the socket so `lsof` returns no
/// PID — a dead daemon's reused PID is therefore never reported here.
#[cfg(unix)]
fn force_kill_fsmonitor_via_socket(socket: &Path) {
    // No socket means `stop` already reaped a healthy daemon (or one never ran).
    if !socket.exists() {
        return;
    }

    let output = match Cmd::new("lsof")
        .arg("-t")
        .arg("--")
        .arg(socket)
        .timeout(FSMONITOR_LSOF_TIMEOUT)
        .run()
    {
        Ok(output) => output,
        Err(e) => {
            tracing::debug!(error = %e, "fsmonitor: lsof failed, cannot force-kill: {e}");
            return;
        }
    };

    let pids: Vec<u32> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse::<u32>().ok())
        .collect();
    super::fsmonitor::escalate_terminate(
        &super::fsmonitor::NixSignaller,
        &pids,
        super::fsmonitor::REAP_KILL_DEADLINE,
    );
}

/// Non-Unix: the daemon uses a named pipe rather than a Unix-domain socket, so
/// the `lsof`-by-socket reaping doesn't apply. The graceful IPC `stop` in
/// [`stop_fsmonitor_daemon`] is the only stop mechanism here.
#[cfg(not(unix))]
fn force_kill_fsmonitor_via_socket(_socket: &Path) {}

/// How the branch should be handled after worktree removal.
///
/// Replaces a two-boolean flag pair (`keep`/`force`) to make the three valid
/// states explicit and prevent invalid combinations (e.g. keep+force).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BranchDeletionMode {
    /// Keep the branch regardless of merge status (`--no-delete-branch`).
    Keep,
    /// Delete only if integrated into the target branch (default).
    #[default]
    SafeDelete,
    /// Delete the branch even if not merged (`-D`).
    ForceDelete,
}

impl BranchDeletionMode {
    /// Construct from CLI-style flags.
    ///
    /// `keep_branch` takes precedence over `force_delete`.
    pub fn from_flags(keep_branch: bool, force_delete: bool) -> Self {
        if keep_branch {
            Self::Keep
        } else if force_delete {
            Self::ForceDelete
        } else {
            Self::SafeDelete
        }
    }

    /// Whether the branch should be kept (never deleted).
    pub fn should_keep(&self) -> bool {
        matches!(self, Self::Keep)
    }

    /// Whether to force-delete even if unmerged.
    pub fn is_force(&self) -> bool {
        matches!(self, Self::ForceDelete)
    }
}

/// Outcome of a branch-deletion attempt.
pub enum BranchDeletionOutcome {
    /// Branch was not deleted — it was not integrated, and deletion was not forced.
    NotDeleted,
    /// Branch was integrated, but a fresh topology read found it checked out in
    /// a worktree before the deletion attempt. The ref is retained so that
    /// worktree's HEAD remains resolvable.
    RetainedCheckedOut { path: PathBuf },
    /// Branch was integrated but the atomic compare-and-swap deletion was
    /// rejected because the ref moved between the integration check and the
    /// delete attempt — e.g. a hook or concurrent process advanced it. The
    /// branch is retained (fail-closed), and the caller surfaces this as a
    /// warning so the user can decide whether to re-check and delete.
    RetainedRaced,
    /// Branch was force-deleted without an integration check.
    ForceDeleted,
    /// Branch was deleted because it was integrated (the specific reason is attached).
    Integrated(IntegrationReason),
}

/// Result of [`delete_branch_if_safe`].
pub struct BranchDeletionResult {
    pub outcome: BranchDeletionOutcome,
    /// The ref actually checked against.
    ///
    /// May differ from the caller-supplied target when the local branch is
    /// behind its upstream — in that case `integration_reason` substitutes the
    /// upstream ref so users don't get false negatives.
    pub integration_target: String,
}

/// Options for [`remove_worktree_with_cleanup`].
///
/// Typical usage:
///
/// ```
/// use worktrunk::git::{BranchDeletionMode, RemoveOptions};
///
/// let options = RemoveOptions {
///     branch: Some("feature".into()),
///     deletion_mode: BranchDeletionMode::SafeDelete,
///     target_branch: Some("main".into()),
///     force_worktree: false,
/// };
///
/// // Or, to delete a worktree without touching the branch:
/// let options = RemoveOptions {
///     branch: Some("feature".into()),
///     deletion_mode: BranchDeletionMode::Keep,
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone, Default)]
pub struct RemoveOptions {
    /// Branch name to delete alongside the worktree.
    ///
    /// `None` skips branch handling (useful for detached-HEAD worktrees).
    pub branch: Option<String>,
    /// How to handle the branch (default: [`BranchDeletionMode::SafeDelete`]).
    pub deletion_mode: BranchDeletionMode,
    /// Integration target for the safety check.
    ///
    /// Only consulted when `deletion_mode` is [`BranchDeletionMode::SafeDelete`].
    /// `None` falls back to `HEAD`.
    pub target_branch: Option<String>,
    /// Skip the clean check and pass `--force` to the `git worktree remove`
    /// fallback.
    ///
    /// Trash staging itself is unconditional and always preserves data (the
    /// renamed directory can be recovered from `<git-common-dir>/wt/trash/`
    /// until the caller deletes it).
    pub force_worktree: bool,
}

/// Result of [`remove_worktree_with_cleanup`].
///
/// `branch_result` is `None` when deletion was skipped (no branch supplied, or
/// `deletion_mode.should_keep()`). A deletion failure carries
/// [`GitError::BranchDeletionFailed`] with the removed worktree's path.
/// Callers finish teardown and cleanup before propagating it.
///
/// `staged_path` is `Some` only on the fast path. Callers are responsible for
/// cleaning up the staged directory; `wt remove` does this with a detached
/// [`drain_trash`] so the foreground command returns immediately.
pub struct RemovalOutput {
    pub branch_result: Option<anyhow::Result<BranchDeletionResult>>,
    /// Path to the staged trash directory on the fast path.
    ///
    /// `None` if the fast-path rename failed and the fallback `git worktree
    /// remove` was used.
    pub staged_path: Option<PathBuf>,
}

/// Remove a worktree with fsmonitor cleanup, fast-path trash staging, and
/// optional safe branch deletion.
///
/// See the [module-level docs](self) for the full flow.
///
/// # Errors
///
/// - Returns an error if the fast-path rename fails **and** the fallback
///   `git worktree remove` also fails.
/// - Branch-deletion errors are captured in
///   [`RemovalOutput::branch_result`] rather than returned — worktree removal
///   is already complete. Callers finish teardown and cleanup before
///   propagating the contextual deletion failure.
pub fn remove_worktree_with_cleanup(
    repo: &Repository,
    snapshot: &crate::git::RefSnapshot,
    worktree_path: &Path,
    options: RemoveOptions,
) -> anyhow::Result<RemovalOutput> {
    let staged_path = stage_worktree_removal(
        repo,
        worktree_path,
        options.branch.as_deref(),
        options.force_worktree,
    )?;
    if staged_path.is_none() {
        repo.remove_worktree(worktree_path, options.force_worktree, None)?;
    }

    // Delete branch if safe
    let branch_result = if let Some(branch) = options.branch.as_deref()
        && !options.deletion_mode.should_keep()
    {
        let target = options.target_branch.as_deref().unwrap_or("HEAD");
        Some(
            delete_branch_if_safe(
                repo,
                snapshot,
                branch,
                target,
                options.deletion_mode.is_force(),
            )
            .map_err(|error| {
                GitError::BranchDeletionFailed {
                    branch: branch.to_owned(),
                    removed_worktree: Some(worktree_path.to_path_buf()),
                    error,
                }
                .into()
            }),
        )
    } else {
        None
    };

    Ok(RemovalOutput {
        branch_result,
        staged_path,
    })
}

/// Check ownership, stop fsmonitor, run the final gates, and stage a worktree
/// for removal — steps 1-3 of the [module-level docs](self).
///
/// Every removal path runs this, in the foreground, before the worktree
/// directory stops existing: the synchronous
/// [`remove_worktree_with_cleanup`], and the default background path, which
/// stages here and hands the deletion to a detached [`drain_trash`]. Keeping the three
/// steps together is what makes the dirty-worktree gate a single decision
/// rather than a sequence each caller re-assembles — the order below is easy
/// to get subtly wrong, and getting it wrong destroys uncommitted work.
///
/// Returns `Some(staged_path)` when the worktree was renamed into
/// `<git-common-dir>/wt/trash/`, `None` when the rename failed
/// (cross-filesystem, permissions, Windows file locking) and the caller must
/// fall back to a direct `git worktree remove`. Either way the caller owns the
/// staged directory and must eventually delete it.
///
/// # Why this order
///
/// With built-in fsmonitor, a cheap pre-stop check leaves the daemon running
/// when the worktree is dirty. The decisive cleanliness check runs after
/// shutdown and any registry wait; it bypasses the built-in daemon to catch
/// shutdown-time writes. See "The dirty-worktree gate" in the
/// [module-level docs](self).
///
/// The daemon stop runs **before** the rename because on Windows the daemon
/// holds a handle on the worktree that would fail it, and git's graceful stop
/// resolves the daemon by worktree path — unreachable once the path moves.
///
/// The ownership check runs **before** the dirty-worktree gate, and outside
/// the `force_worktree` branch that skips it. Both matter. The gate reads `git
/// status` in the directory, so against a foreign occupant it reports *that*
/// repository's dirt as this worktree's and offers `--force` as the remedy — a
/// hint leading straight to the deletion the check refuses. And `--force` is
/// the user waiving their own uncommitted changes, never a claim about who
/// owns the directory, so it cannot be allowed to skip it; git's own
/// validation is likewise unconditional. The lock check sits with ownership:
/// `--force` does not override `git worktree lock`, and the check reads the
/// `locked` file rather than `list_worktrees()`, whose `RepoCache` entry
/// planning already warmed: it would answer from before the approval prompt
/// and the `pre-remove` hook, which is the window this call closes.
///
/// `wt remove` and `wt merge --remove` have already asked this during
/// planning, where the answer can precede the "Removing …" announcement. The
/// call here still re-reads the directory's `.git` entry, so it also closes the
/// window those two leave open: the approval prompt and the `pre-remove` hook
/// run between their check and this rename.
///
/// # Errors
///
/// The ownership, lock, dirty-worktree, containment, and cancellation gates error. A
/// failed rename is reported as `None`, not an error, and the daemon stop is
/// best-effort throughout.
pub fn stage_worktree_removal(
    repo: &Repository,
    worktree_path: &Path,
    branch: Option<&str>,
    force_worktree: bool,
) -> anyhow::Result<Option<PathBuf>> {
    let worktree = repo.worktree_at(worktree_path);
    let git_dir = require_removal_allowed(&worktree, branch, None)?;
    let fsmonitor = repo.is_builtin_fsmonitor_enabled();
    if force_worktree {
        // Disclosure does not veto the force removal, but an interrupted
        // status command still cancels before any directory is removed.
        match worktree.dirty_files() {
            Ok(dirty_files) if !dirty_files.is_empty() => {
                eprintln!(
                    "{}",
                    warning_message("Discarding uncommitted changes (--force):")
                );
                eprintln!("{}", format_with_gutter(&dirty_files.join("\n"), None));
            }
            Ok(_) => {}
            Err(error) => {
                if error.interrupt_signal().is_some() {
                    return Err(error);
                }
                eprintln!(
                    "{}",
                    warning_message(
                        "Cannot list uncommitted changes; discarding worktree contents (--force):"
                    )
                );
                eprintln!("{}", format_with_gutter(&error.display_message(), None));
            }
        }
    } else if fsmonitor {
        worktree.ensure_clean("remove worktree", branch, true)?;
    }

    stop_fsmonitor_daemon(&worktree);

    // Wait before the decisive cleanliness scan. Otherwise a concurrent
    // teardown can expose a half-removed registration, or a write during the
    // wait can escape a scan performed before it.
    let registry = repo.worktree_registry_read();

    // Shutdown may replace or lock the selected checkout. Refuse it before
    // a status query can inspect another repository's contents.
    require_removal_allowed(&worktree, branch, Some(&git_dir))?;

    // The daemon answered the gate above, and a write can land while it shuts
    // down. Recheck without it; with fsmonitor configured, a plain status
    // would start a new daemon in the worktree about to go.
    if !force_worktree {
        if fsmonitor {
            worktree.ensure_clean_without_fsmonitor("remove worktree", branch, true)?;
        } else {
            worktree.ensure_clean("remove worktree", branch, true)?;
        }
    }

    require_final_removal_allowed(&worktree, branch, &git_dir)?;
    // No external command runs before rename. Pruning afterward needs the
    // write side of this same lock.
    drop(registry);
    Ok(rename_into_trash(repo, worktree_path, &git_dir))
}

/// Final mutation boundary shared by trash staging and direct Git removal.
/// Callers hold the registry read or write lock before invoking it.
pub(in crate::git) fn require_final_removal_allowed(
    worktree: &WorkingTree,
    branch: Option<&str>,
    git_dir: &Path,
) -> anyhow::Result<()> {
    // Status and daemon shutdown may add a registered checkout or relocate
    // sibling Git storage inside the removal tree. Force never waives it.
    if let Some(nested) = worktree.repo().nested_worktree_unlocked(worktree.path())? {
        anyhow::bail!(
            "Cannot remove worktree @ {}; contains worktree or Git storage @ {}",
            format_path_for_display(worktree.path()),
            format_path_for_display(&nested),
        );
    }

    // No external command runs between these final topology/owner/lock checks
    // and deletion. A replacement must not retarget the old removal.
    require_removal_allowed(worktree, branch, Some(git_dir))?;
    #[cfg(unix)]
    if let Some(signal) = crate::signal_forwarder::operation_interrupt_signal() {
        return Err(crate::git::WorktrunkError::Interrupted { signal, hint: None }.into());
    }
    Ok(())
}

pub(in crate::git) fn require_removal_allowed(
    worktree: &WorkingTree,
    branch: Option<&str>,
    expected_registration: Option<&Path>,
) -> anyhow::Result<PathBuf> {
    let git_dir = worktree.ensure_holds_this_worktree()?;
    if expected_registration.is_some_and(|expected| !crate::path::paths_match(expected, &git_dir)) {
        anyhow::bail!(
            "Worktree registration changed during removal @ {}",
            format_path_for_display(worktree.path()),
        );
    }
    // Read the lock file freshly; planning already warmed the topology cache.
    if let Some(reason) = worktree.lock_reason()? {
        return Err(GitError::WorktreeLocked {
            branch: branch
                .unwrap_or_else(|| path_dir_name(worktree.path()))
                .to_string(),
            path: worktree.path().to_path_buf(),
            reason,
        }
        .into());
    }
    Ok(git_dir)
}

/// Rename a worktree into `<git-common-dir>/wt/trash/` and prune git metadata.
///
/// Returns `Some(staged_path)` on success, `None` if the rename failed. The
/// unguarded mutation: [`stage_worktree_removal`] is the only caller, and it
/// is what places the dirty-worktree gate ahead of this.
///
/// The metadata cleanup names this worktree
/// ([`prune_worktree_entry`](Repository::prune_worktree_entry)) rather than
/// sweeping the repository, so a sibling worktree whose directory happens to
/// be absent right now keeps its registration. A locked worktree never reaches
/// here — [`stage_worktree_removal`] rejects one before the rename.
fn rename_into_trash(repo: &Repository, worktree_path: &Path, git_dir: &Path) -> Option<PathBuf> {
    let trash_dir = repo.wt_trash_dir();
    let _ = std::fs::create_dir_all(&trash_dir);
    let staged_path = generate_removing_path(&trash_dir, git_dir);

    if std::fs::rename(worktree_path, &staged_path)
        .inspect_err(|e| {
            tracing::debug!(error = %e, "Failed to stage worktree into trash, falling back: {e}");
        })
        .is_ok()
    {
        // The rename moved the directory out from under `worktree_path`,
        // leaving its registration stale for the prune to delete.
        if let Err(e) = repo.prune_worktree_entry(worktree_path) {
            tracing::debug!(error = %e, "Failed to prune worktree entry after rename: {e}");
        }
        Some(staged_path)
    } else {
        None
    }
}

/// Capture fresh refs and run branch deletion.
///
/// Used after worktree removal and for branch-only deletion. Every path,
/// including the detached fallback, uses [`delete_branch_if_safe`] to recheck
/// integration and checkout ownership; safe deletion uses CAS against the captured tip.
pub fn execute_branch_deletion(
    repo: &Repository,
    branch_name: &str,
    target: &str,
    force_delete: bool,
    expected_tip: Option<&str>,
) -> anyhow::Result<BranchDeletionResult> {
    let snapshot = repo
        .capture_refs()
        .inspect_err(publish_branch_cancellation)?;
    if expected_tip.is_some_and(|tip| {
        snapshot
            .local_branch(branch_name)
            .map(|branch| branch.commit_sha.as_str())
            != Some(tip)
    }) {
        return Ok(BranchDeletionResult {
            outcome: BranchDeletionOutcome::RetainedRaced,
            integration_target: target.to_string(),
        });
    }
    delete_branch_if_safe(repo, &snapshot, branch_name, target, force_delete)
}

/// A captured Git child signal cancels the whole command, including pending
/// background hooks. Publish it at branch ownership, before callers finish a
/// partial removal; the shared dispatcher reads this existing operation latch.
fn publish_branch_cancellation(error: &anyhow::Error) {
    #[cfg(unix)]
    if let Some(signal) = error.interrupt_signal() {
        crate::signal_forwarder::cancel_foreground(signal);
    }
    #[cfg(not(unix))]
    let _ = error;
}

/// Delete a branch if its content is integrated into the target, or if
/// `force_delete` is set.
///
/// The integration check is the same logic `wt list` uses for its status
/// column — see [`IntegrationReason`] for the full set of recognised cases
/// (same-commit, ancestor, squash-merged, etc.).
///
/// Returns a [`BranchDeletionResult`] rather than raising an error for the
/// "not integrated" case — that's a normal outcome and the caller decides how
/// to surface it. Failures to read current topology or run the Git mutation
/// propagate as `Err`.
pub fn delete_branch_if_safe(
    repo: &Repository,
    snapshot: &crate::git::RefSnapshot,
    branch_name: &str,
    target: &str,
    force_delete: bool,
) -> anyhow::Result<BranchDeletionResult> {
    (|| {
        // Force-delete: skip integration check entirely (matches compute_integration_reason
        // behavior for the Worktree path). The user explicitly chose -D.
        if force_delete {
            // Git cannot protect an operation branch whose identity is unreadable.
            let _ = fresh_branch_checkout(repo, branch_name)?;
            repo.run_command(&["branch", "-D", "--", branch_name])?;
            return Ok(BranchDeletionResult {
                outcome: BranchDeletionOutcome::ForceDeleted,
                integration_target: target.to_string(),
            });
        }

        let (effective_target, reason) = repo.integration_reason(snapshot, branch_name, target)?;

        let outcome = match reason {
            Some(r) => {
                // Atomic compare-and-swap against the snapshotted SHA. If the ref
                // moved between `integration_reason` and the delete (e.g. a hook
                // advanced the branch), the `git update-ref` mutation fails
                // closed: the branch is retained and we surface a `RetainedRaced`
                // outcome rather than dropping the unmerged commits silently.
                //
                // Read the SHA from the snapshot inventory (`local_branch`) rather
                // than `resolve()`, so it reflects the same `refs/heads/` walk
                // `integration_reason` consulted.
                let Some(branch) = snapshot.local_branch(branch_name) else {
                    anyhow::bail!(
                        "Cannot safely delete branch {branch_name}: absent from ref snapshot"
                    );
                };
                cas_delete_branch_outcome(repo, branch_name, &branch.commit_sha, r)?
            }
            None => BranchDeletionOutcome::NotDeleted,
        };

        Ok(BranchDeletionResult {
            outcome,
            integration_target: effective_target,
        })
    })()
    .inspect_err(publish_branch_cancellation)
}

/// Find a live checkout of `branch` using a new repository cache.
///
/// Removal planning intentionally caches `git worktree list`, but branch
/// deletion happens after hooks and other concurrent actors may have changed
/// topology. Constructing a new [`Repository`] is the cache boundary: its first
/// inventory query executes a fresh `git worktree list --porcelain`, under the
/// same registry lock as the strict held-branch scan.
///
/// Git still lists stale registrations whose directories are gone, marking
/// them `prunable`; those have no live checkout to orphan and must not strand
/// the branch. Missing locked worktrees remain non-prunable, so their lock
/// continues to retain the branch conservatively.
fn fresh_branch_checkout(repo: &Repository, branch_name: &str) -> anyhow::Result<Option<PathBuf>> {
    let fresh_repo = Repository::at(repo.discovery_path())?;
    let _registry = fresh_repo.worktree_registry_read();
    let worktrees = fresh_repo.load_worktrees_unlocked()?;
    // Also guard incomplete registrations Git omits from its worktree list.
    let registrations = fresh_repo.worktree_registrations()?;
    let main = if fresh_repo.is_bare()? {
        None
    } else {
        Some(canonicalize_with_parents(fresh_repo.repo_path()?))
    };
    let mut checked_out = None;
    // Consult every surviving registration, including one added by an external
    // Git process after the inventory query. Vanished registrations are absent.
    let git_dirs = registrations
        .iter()
        .map(|(path, git_dir)| (path, git_dir.as_path()))
        .chain(
            main.as_ref()
                .map(|path| (path, fresh_repo.git_common_dir())),
        );
    for (path, git_dir) in git_dirs {
        let branches = super::held_branches(git_dir).with_context(|| {
            format!(
                "Cannot determine worktree branches @ {}",
                format_path_for_display(path)
            )
        })?;
        let worktree = worktrees
            .iter()
            .find(|wt| canonicalize_with_parents(&wt.path) == *path);
        if worktree.is_none_or(branch_checkout_requires_retention) && branches.contains(branch_name)
        {
            checked_out.get_or_insert_with(|| worktree.map_or(path, |wt| &wt.path).clone());
        }
    }
    Ok(checked_out)
}

/// Whether this worktree record retains its held branches.
///
/// Git normally reports a missing locked worktree as `locked` but not
/// `prunable`. Letting the lock win explicitly keeps the guard fail-closed if a
/// Git version or synthetic porcelain record ever carries both fields.
fn branch_checkout_requires_retention(worktree: &WorktreeInfo) -> bool {
    !worktree.is_prunable() || worktree.locked.is_some()
}

/// Atomically delete `refs/heads/<branch>` iff it currently points at
/// `expected_sha`, and translate the result into a [`BranchDeletionOutcome`].
///
/// `git update-ref -d <ref> <original-sha>` removes the ref only if its current
/// value still matches. The repository coordinator serializes these mutations
/// to avoid contention on Git's packed-refs lock.
///
/// A fresh topology read provides best-effort checkout protection before
/// acquiring the deletion mutex. Topology reads can run concurrently. Git has
/// no transaction spanning checkout state and a ref update: a checkout can
/// appear after this check, including during the mutex wait. The atomic SHA
/// comparison still protects concurrent commits.
///
/// On failure, a fresh exact ref read distinguishes actual SHA movement from
/// a lock or I/O error without parsing Git's localized diagnostics. A missing,
/// unchanged or unreadable ref propagates the original deletion error.
fn cas_delete_branch_outcome(
    repo: &Repository,
    branch_name: &str,
    expected_sha: &str,
    reason: IntegrationReason,
) -> anyhow::Result<BranchDeletionOutcome> {
    let ref_name = format!("refs/heads/{branch_name}");
    // update-ref bypasses Git's checked-out-branch protection. Sample topology
    // from a fresh Repository cache, independent of the planning-time cache.
    if let Some(path) = fresh_branch_checkout(repo, branch_name)? {
        return Ok(BranchDeletionOutcome::RetainedCheckedOut { path });
    }
    if repo
        .branch_deletions()
        .delete(repo, &ref_name, expected_sha)?
    {
        Ok(BranchDeletionOutcome::Integrated(reason))
    } else {
        Ok(BranchDeletionOutcome::RetainedRaced)
    }
}

/// Generate a staging path for worktree removal.
///
/// Places the staging directory inside `<git-common-dir>/wt/trash/` so it is
/// hidden from the user's workspace. For the main worktree, `.git/` is on the
/// same filesystem, so `rename()` is an instant metadata operation. Linked
/// worktrees on different mount points will get EXDEV and fall back to the
/// `git worktree remove` path.
///
/// Format: `<trash-dir>/<name>-<timestamp>`, where `<name>` is the final
/// component of the worktree's `git_dir`: its registration id under
/// `<common>/worktrees/`, which git keeps unique among a repository's
/// worktrees. The directory's basename is not unique — Codex places every
/// worktree at `<id>/<repo>` — and two removals sharing a staging path in the
/// same second would make the second rename fail onto the synchronous
/// fallback.
pub(crate) fn generate_removing_path(trash_dir: &Path, git_dir: &Path) -> PathBuf {
    let timestamp = epoch_now();
    let name = git_dir
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    trash_dir.join(format!("{}-{}", name, timestamp))
}

/// Entries in `trash_dir` named `<name>-<timestamp>`, as removal stages them.
///
/// Anything else in the directory was not staged by worktrunk and is left
/// alone. A missing directory has no entries.
pub fn staged_trash_entries(trash_dir: &Path) -> Vec<PathBuf> {
    let Ok(read_dir) = fs::read_dir(trash_dir) else {
        return Vec::new();
    };
    read_dir
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.file_name().to_str().is_some_and(|name| {
                name.rsplit_once('-')
                    .is_some_and(|(_, timestamp)| timestamp.parse::<u64>().is_ok())
            })
        })
        .map(|entry| entry.path())
        .collect()
}

/// Whether a drain holds `trash_dir`'s lock.
///
/// A removal that finds one running skips starting another: the running drain
/// lists the trash again before it exits. The check holds the lock for an
/// instant, so a drain starting at that moment may exit early; the check then
/// reports no drain running and the caller starts one.
pub fn trash_drain_running(trash_dir: &Path) -> bool {
    open_drain_lock(trash_dir)
        .is_ok_and(|lock| matches!(lock.try_lock(), Err(TryLockError::WouldBlock)))
}

fn open_drain_lock(trash_dir: &Path) -> anyhow::Result<fs::File> {
    let lock_path = trash_dir.with_extension("lock");
    fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("Failed to open {}", format_path_for_display(&lock_path)))
}

/// Delete every staged entry in `trash_dir`, unless another drain is running.
///
/// Concurrent deletions on one filesystem add CPU without adding throughput:
/// a 193-worktree prune ran 193 `rm -rf` processes using about three cores to
/// delete 1,500 files/s, where one process alone deleted 3,000–5,000 files/s
/// at under one core. So one drain per trash directory does the deleting.
///
/// The running drain holds an exclusive lock on `<trash_dir>.lock`. A drain
/// that finds the lock held exits and leaves its entry to the holder, which
/// lists the directory again after each pass. After releasing the lock, the
/// holder lists once more and, if it finds a new entry, takes the lock again:
/// an entry staged while the holder was releasing it would otherwise wait for
/// the next removal.
///
/// An entry that a drain can't fully delete, such as one holding another
/// user's files, stays in the trash for a later drain rather than being
/// retried in a loop.
pub fn drain_trash(trash_dir: &Path) -> anyhow::Result<()> {
    let mut undeletable = HashSet::new();
    loop {
        let lock = open_drain_lock(trash_dir)?;
        match lock.try_lock() {
            Err(TryLockError::WouldBlock) => return Ok(()),
            result => result
                .map_err(std::io::Error::from)
                .context("Failed to lock the trash for deletion")?,
        }
        loop {
            let pending: Vec<_> = staged_trash_entries(trash_dir)
                .into_iter()
                .filter(|entry| !undeletable.contains(entry))
                .collect();
            if pending.is_empty() {
                break;
            }
            for entry in pending {
                if !remove_dir_with_progress(&entry, &Progress::disabled()) {
                    undeletable.insert(entry);
                }
            }
        }
        drop(lock);
        if staged_trash_entries(trash_dir)
            .iter()
            .all(|entry| undeletable.contains(entry))
        {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::ErrorExt;
    use crate::testing::TestRepo;

    /// A failed staging rename falls back to Git. A nested checkout created
    /// while that fallback waits on teardown must still survive explicit force.
    #[test]
    fn failed_staging_fallback_rechecks_containment_after_registry_wait() {
        let mut test = TestRepo::with_initial_commit();
        let outer = test.add_worktree("outer");
        let repo = Repository::at(test.root_path()).unwrap();
        let trash = repo.wt_trash_dir();
        std::fs::create_dir_all(trash.parent().unwrap()).unwrap();
        std::fs::write(&trash, "block staging").unwrap();
        assert!(
            stage_worktree_removal(&repo, &outer, Some("outer"), true)
                .unwrap()
                .is_none()
        );

        let registry = repo.worktree_registry_write();
        std::thread::scope(|scope| {
            let (started, waiting) = std::sync::mpsc::channel();
            let fallback_repo = &repo;
            let path = &outer;
            let fallback = scope.spawn(move || {
                started.send(()).unwrap();
                fallback_repo.remove_worktree(path, true, None)
            });
            waiting.recv().unwrap();
            let inner = outer.join("inner");
            test.add_worktree_at_path("inner", &inner);
            std::fs::write(inner.join("precious.txt"), "nested work").unwrap();
            drop(registry);

            let error = fallback.join().unwrap().unwrap_err();
            assert!(error.to_string().contains("contains worktree"), "{error:#}");
            assert!(outer.join(".git").is_file());
            assert!(inner.join(".git").is_file());
            assert_eq!(
                std::fs::read_to_string(inner.join("precious.txt")).unwrap(),
                "nested work"
            );
        });
    }

    /// A separate store without a primary backlink must not invent a root
    /// outside a removal tree that actually contains the primary checkout.
    #[test]
    fn removal_preserves_unrecorded_separate_primary_inside_linked_checkout() {
        let test = TestRepo::with_initial_commit();
        let primary = test.home_path().join("primary");
        let store = test.home_path().join("store/git-data");
        std::fs::create_dir_all(store.parent().unwrap()).unwrap();
        test.run_git(&[
            "clone",
            "--separate-git-dir",
            store.to_str().unwrap(),
            test.root_path().to_str().unwrap(),
            primary.to_str().unwrap(),
        ]);
        let outer = test.home_path().join("outer");
        test.run_git_in(
            &primary,
            &["worktree", "add", "-b", "outer", outer.to_str().unwrap()],
        );
        assert!(!store.join("gitdir").exists());
        let contained = outer.join("primary");
        std::fs::rename(primary, &contained).unwrap();
        std::fs::write(contained.join("precious.txt"), "primary work").unwrap();

        let repo = Repository::at(&outer).unwrap();
        let error = stage_worktree_removal(&repo, &outer, Some("outer"), true).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Cannot determine main worktree path"),
            "{error:#}"
        );
        assert!(outer.join(".git").is_file());
        assert!(contained.join(".git").is_file());
        assert_eq!(
            std::fs::read_to_string(contained.join("precious.txt")).unwrap(),
            "primary work"
        );
    }

    /// Already latched cancellation preserves the checkout and registration.
    #[cfg(unix)]
    #[test]
    fn stage_refuses_cancelled_operation() {
        const CHILD_ENV: &str = "WORKTRUNK_TEST_REMOVE_CANCELLED_STAGING_CHILD";
        if std::env::var_os(CHILD_ENV).is_none() {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "git::remove::tests::stage_refuses_cancelled_operation",
                ])
                .env(CHILD_ENV, "1")
                .stdout(std::process::Stdio::piped());
            let output = crate::shell_exec::spawn(&mut command)
                .unwrap()
                .wait_with_output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(output.status.success(), "{stdout}");
            assert!(stdout.contains(" 1 passed;"), "{stdout}");
            return;
        }
        let mut test = TestRepo::with_initial_commit();
        let worktree = test.add_worktree("feature");
        let repo = Repository::at(test.root_path()).unwrap();
        let registration = repo.worktree_at(&worktree).git_dir().unwrap();
        let index = std::fs::read(registration.join("index")).unwrap();
        let operation = crate::signal_forwarder::CommandOperation::start().unwrap();
        let foreground = crate::signal_forwarder::ForegroundSignals::install(
            crate::shell_exec::TerminalInterrupt::Cancels,
        )
        .unwrap();
        signal_hook::low_level::raise(signal_hook::consts::SIGTERM).unwrap();
        assert_eq!(
            operation.interrupt_signal(),
            Some(signal_hook::consts::SIGTERM)
        );
        let error = stage_worktree_removal(&repo, &worktree, Some("feature"), false).unwrap_err();
        assert_eq!(error.interrupt_signal(), Some(signal_hook::consts::SIGTERM));
        assert!(worktree.join(".git").is_file());
        assert_eq!(std::fs::read(registration.join("index")).unwrap(), index);
        assert!(!repo.wt_dir().join("retained").exists());
        drop(foreground);
        drop(operation);
    }

    /// A drain deletes worktrunk's staged entries and leaves anything else; a
    /// drain that finds another holding the lock leaves the entries to it.
    #[test]
    fn drain_trash_deletes_staged_entries_once_the_lock_is_free() {
        let temp = tempfile::tempdir().unwrap();
        let trash = temp.path().join("trash");
        let staged = trash.join("repo.feature-branch-1700000000");
        let foreign = trash.join("not-staged");
        fs::create_dir_all(staged.join("node_modules/pkg")).unwrap();
        fs::write(staged.join("node_modules/pkg/index.js"), "x").unwrap();
        fs::create_dir_all(&foreign).unwrap();
        fs::write(trash.join("stray-file-1700000000"), "a staged file").unwrap();
        assert_eq!(staged_trash_entries(&trash).len(), 2);

        assert!(!trash_drain_running(&trash));
        let holder = fs::File::create(temp.path().join("trash.lock")).unwrap();
        holder.lock().unwrap();
        assert!(trash_drain_running(&trash));
        drain_trash(&trash).unwrap();
        assert!(staged.exists(), "the lock holder owns the deletion");

        drop(holder);
        // A child that a concurrent test forks while `holder` is open inherits
        // the lock until it execs, so a drain can still find it held for an
        // instant and leave the deletion to that phantom holder.
        crate::testing::wait_for("a drain to delete the staged entries", || {
            drain_trash(&trash).unwrap();
            staged_trash_entries(&trash).is_empty()
        });
        assert!(foreign.exists());
        assert!(staged_trash_entries(&temp.path().join("missing")).is_empty());
    }

    /// An entry the drain can't delete ends the drain rather than looping.
    #[cfg(unix)]
    #[test]
    fn drain_trash_stops_at_an_undeletable_entry() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let trash = temp.path().join("trash");
        fs::create_dir(&trash).unwrap();
        fs::write(trash.join("stuck-1700000000"), "x").unwrap();
        // Unlinking an entry needs write permission on the trash itself.
        fs::set_permissions(&trash, fs::Permissions::from_mode(0o555)).unwrap();

        drain_trash(&trash).unwrap();

        fs::set_permissions(&trash, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Same-basename worktrees need distinct staging paths. Test staging
    /// directly: a prune batch may share one detached drain, so per-branch
    /// cleanup logs do not prove whether each worktree was staged.
    #[test]
    fn stages_worktrees_sharing_a_basename() {
        let mut test = TestRepo::with_initial_commit();
        let parent = test.root_path().parent().unwrap().to_path_buf();
        let worktrees = [
            ("one", parent.join("aaaa/repo")),
            ("two", parent.join("bbbb/repo")),
        ];
        for (branch, path) in &worktrees {
            test.add_worktree_at_path(branch, path);
        }
        let repo = Repository::at(test.root_path()).unwrap();
        let mut staged_paths = Vec::new();
        for (branch, path) in &worktrees {
            let git_dir = repo.worktree_at(path).git_dir().unwrap();
            let registration = git_dir.file_name().unwrap().to_string_lossy();
            let staged = stage_worktree_removal(&repo, path, Some(branch), false)
                .unwrap()
                .expect("each worktree must stage successfully");
            assert!(!path.exists());
            assert!(staged.exists());
            // Pin the registration prefix independently of the wall clock.
            assert!(
                staged
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(&format!("{registration}-"))
            );
            staged_paths.push(staged);
        }
        assert_ne!(staged_paths[0], staged_paths[1]);
        assert!(staged_paths.iter().all(|path| path.exists()));
    }

    /// A `git worktree lock` must stop the rename even when the caller skipped
    /// `prepare_worktree_removal` (merge used to construct a plan by hand).
    #[test]
    fn stage_refuses_locked_worktree() {
        let mut test = TestRepo::with_initial_commit();
        let worktree_path = test.add_worktree("feature");
        test.lock_worktree("feature", Some("keep"));
        let repo = Repository::at(test.root_path()).unwrap();

        let err =
            stage_worktree_removal(&repo, &worktree_path, Some("feature"), false).unwrap_err();
        assert!(
            matches!(
                err.downcast_ref::<GitError>(),
                Some(GitError::WorktreeLocked { branch, reason, .. })
                    if branch == "feature" && reason.as_deref() == Some("keep")
            ),
            "expected WorktreeLocked, got {err:?}"
        );
        assert!(
            worktree_path.exists(),
            "locked worktree must still be on disk"
        );
    }

    /// `--force` waives dirt, not a lock — same as `git worktree remove --force`.
    #[test]
    fn stage_refuses_locked_worktree_even_with_force() {
        let mut test = TestRepo::with_initial_commit();
        let worktree_path = test.add_worktree("feature");
        test.lock_worktree("feature", None);
        let repo = Repository::at(test.root_path()).unwrap();

        let err = stage_worktree_removal(&repo, &worktree_path, Some("feature"), true).unwrap_err();
        match err.downcast_ref::<GitError>() {
            Some(GitError::WorktreeLocked { reason: None, .. }) => {}
            other => panic!("expected WorktreeLocked without a reason, got {other:?}"),
        }
        assert!(worktree_path.exists());
    }

    #[test]
    fn final_removal_preserves_registered_ignored_nested_checkout() {
        let mut test = TestRepo::with_initial_commit();
        std::fs::write(test.root_path().join(".gitignore"), ".storage/\n").unwrap();
        test.commit("Ignore storage");
        let outer = test.add_worktree("outer");
        let inner_path = outer.join(".storage/inner");
        let inner = test.add_worktree_at_path("inner", &inner_path);
        std::fs::write(inner.join("notes.txt"), "independent work").unwrap();
        let repo = Repository::at(test.root_path()).unwrap();
        assert!(repo.worktree_at(&outer).dirty_files().unwrap().is_empty());
        let error =
            crate::git::stage_worktree_removal(&repo, &outer, Some("outer"), true).unwrap_err();
        assert!(error.to_string().contains("contains worktree"), "{error:#}");
        assert!(outer.join(".git").is_file());
        assert!(inner.join(".git").is_file());
        assert_eq!(
            std::fs::read_to_string(inner.join("notes.txt")).unwrap(),
            "independent work"
        );
    }

    #[cfg(unix)]
    #[test]
    fn final_removal_preserves_contained_registration_storage() {
        for contained in [true, false] {
            let mut test = TestRepo::with_initial_commit();
            std::fs::write(test.root_path().join(".gitignore"), ".storage/\n").unwrap();
            test.commit("Ignore storage");
            let outer = test.add_worktree("outer");
            let inner = test.add_worktree("inner");
            let repo = Repository::at(test.root_path()).unwrap();
            let registration = repo.worktree_at(&inner).git_dir().unwrap();
            let storage = if contained {
                outer.join(".storage/inner-registration")
            } else {
                test.root_path().join("external-registration")
            };
            std::fs::create_dir_all(storage.parent().unwrap()).unwrap();
            std::fs::rename(&registration, &storage).unwrap();
            std::fs::write(
                storage.join("commondir"),
                format!("{}\n", repo.git_common_dir().display()),
            )
            .unwrap();
            std::os::unix::fs::symlink(&storage, &registration).unwrap();
            assert!(repo.worktree_at(&outer).dirty_files().unwrap().is_empty());
            let result = crate::git::stage_worktree_removal(&repo, &outer, Some("outer"), true);
            if contained {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("contains worktree or Git storage")
                );
                assert!(outer.join(".git").is_file());
            } else {
                assert!(result.unwrap().is_some());
                assert!(!outer.exists());
            }
            assert!(registration.is_symlink());
        }
    }

    #[test]
    fn nested_worktree_refuses_contained_common_git_storage() {
        let seed = TestRepo::with_initial_commit();
        std::fs::write(seed.root_path().join(".gitignore"), ".store/\n").unwrap();
        seed.run_git(&["add", ".gitignore"]);
        seed.run_git(&["commit", "-m", "ignore moved store"]);
        let directory = crate::testing::test_tempdir();
        let original = directory.path().join("bare.git");
        let outer = directory.path().join("outer");
        seed.run_git(&[
            "clone",
            "--bare",
            seed.root_path().to_str().unwrap(),
            original.to_str().unwrap(),
        ]);
        seed.run_git_in(
            &original,
            &["worktree", "add", "-b", "outer", outer.to_str().unwrap()],
        );
        std::fs::write(outer.join("only-history.txt"), "sole unpushed history").unwrap();
        seed.run_git_in(&outer, &["add", "only-history.txt"]);
        seed.run_git_in(&outer, &["commit", "-m", "sole history"]);
        let head = String::from_utf8(
            seed.git_command()
                .args(["rev-parse", "HEAD"])
                .current_dir(&outer)
                .run()
                .unwrap()
                .stdout,
        )
        .unwrap();
        let store = outer.join(".store");
        std::fs::rename(&original, &store).unwrap();
        std::fs::write(
            outer.join(".git"),
            format!("gitdir: {}\n", store.join("worktrees/outer").display()),
        )
        .unwrap();
        let repo = Repository::at(&outer).unwrap();
        assert!(repo.is_bare().unwrap());
        let error = crate::git::remove::stage_worktree_removal(&repo, &outer, Some("outer"), true)
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("contains worktree or Git storage"),
            "{error:#}"
        );
        seed.run_git_in(&outer, &["cat-file", "-e", head.trim()]);
    }

    /// A registry wait precedes the decisive clean check, so a concurrent
    /// write cannot be discarded after metadata teardown releases the lock.
    #[test]
    fn stage_rechecks_cleanliness_after_registry_wait() {
        let mut test = TestRepo::with_initial_commit();
        let worktree_path = test.add_worktree("feature");
        let repo = Repository::at(test.root_path()).unwrap();
        let worker_repo = repo.clone();
        let worker_path = worktree_path.clone();
        let registry_guard = repo.worktree_registry_write();
        let (started, waiting) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            started.send(()).unwrap();
            stage_worktree_removal(&worker_repo, &worker_path, Some("feature"), false)
        });
        waiting.recv().unwrap();
        std::fs::write(worktree_path.join("late.txt"), "late work").unwrap();
        drop(registry_guard);

        let error = worker.join().unwrap().unwrap_err();
        assert!(
            error.to_string().contains("uncommitted changes"),
            "{error:#}"
        );
        assert!(worktree_path.join(".git").is_file());
        assert_eq!(
            std::fs::read_to_string(worktree_path.join("late.txt")).unwrap(),
            "late work"
        );
    }

    /// When the branch tip moves between snapshot capture and the deletion
    /// attempt, the atomic compare-and-swap rejects the delete and surfaces
    /// `RetainedRaced` rather than dropping the new commits silently.
    ///
    /// The setup mimics a hook (or any concurrent writer) that advances the
    /// branch after the planner observed it as integrated: we capture refs
    /// when `feature` is at the same commit as `main` (trivially integrated),
    /// then move `feature` forward, then call `delete_branch_if_safe` with
    /// the stale snapshot. Integration check still says "integrated" (the
    /// snapshotted SHA is reachable from main), but CAS catches the live
    /// tip having moved and refuses the delete.
    #[test]
    fn cas_rejects_delete_when_branch_advances() {
        let test = TestRepo::with_initial_commit();
        test.run_git(&["branch", "feature"]);
        let repo = Repository::at(test.root_path()).unwrap();

        // Snapshot captures `feature` at the initial commit (same as `main`).
        let snapshot = repo.capture_refs().unwrap();
        let original_sha = snapshot.local_branch("feature").unwrap().commit_sha.clone();

        // Race: advance `feature` after the snapshot.
        test.run_git(&["checkout", "feature"]);
        std::fs::write(test.root_path().join("race.txt"), "boom\n").unwrap();
        test.run_git(&["add", "race.txt"]);
        test.run_git(&["commit", "-m", "post-snapshot advance"]);
        test.run_git(&["checkout", "main"]);

        let advanced_sha = test.git_output(&["rev-parse", "feature"]);
        assert_ne!(
            original_sha, advanced_sha,
            "test setup: tip must have moved"
        );

        let result = delete_branch_if_safe(&repo, &snapshot, "feature", "main", false).unwrap();
        assert!(
            matches!(result.outcome, BranchDeletionOutcome::RetainedRaced),
            "expected RetainedRaced, got a different outcome"
        );

        // Branch survives, still at the post-race SHA.
        let live = test.git_output(&["rev-parse", "feature"]);
        assert_eq!(live, advanced_sha, "branch must not be deleted nor reset");
    }

    /// Plain integrated case still deletes via CAS. Sanity check that the
    /// new code path doesn't break the common case.
    #[test]
    fn cas_deletes_when_branch_unchanged() {
        let test = TestRepo::with_initial_commit();
        test.run_git(&["branch", "feature"]);
        let repo = Repository::at(test.root_path()).unwrap();

        let snapshot = repo.capture_refs().unwrap();
        let result = delete_branch_if_safe(&repo, &snapshot, "feature", "main", false).unwrap();
        assert!(
            matches!(result.outcome, BranchDeletionOutcome::Integrated(_)),
            "expected Integrated, got a different outcome"
        );

        // Ref should be gone.
        let mut rev_parse = std::process::Command::new("git");
        crate::testing::configure_git_cmd(&mut rev_parse);
        let exit = rev_parse
            .args(["rev-parse", "--verify", "--quiet", "refs/heads/feature"])
            .current_dir(test.root_path())
            .status()
            .unwrap();
        assert!(!exit.success(), "branch should have been deleted");
    }

    /// A packed-ref lock failure must preserve the branch and the original
    /// Git error; an unchanged ref is not evidence of branch movement.
    #[test]
    fn cas_propagates_error_when_packed_refs_locked() {
        let test = TestRepo::with_initial_commit();
        test.run_git(&["branch", "feature"]);
        test.run_git(&["pack-refs", "--all"]);
        test.run_git(&["config", "core.packedRefsTimeout", "0"]);
        let repo = Repository::at(test.root_path()).unwrap();
        let snapshot = repo.capture_refs().unwrap();
        let expected_sha = &snapshot.local_branch("feature").unwrap().commit_sha;
        std::fs::write(repo.git_common_dir().join("packed-refs.lock"), "").unwrap();

        let result = delete_branch_if_safe(&repo, &snapshot, "feature", "main", false);
        let error = result.err().expect("lock failure must propagate");
        let message = error.display_message();
        assert!(message.contains("packed-refs.lock"), "{message}");
        assert_eq!(test.git_output(&["rev-parse", "feature"]), *expected_sha);
    }

    /// A branch whose name starts with `-` must still force-delete: the
    /// `git branch -D -- <name>` separator stops git from parsing `-x` as an
    /// option. Created via `update-ref` since `git branch` rejects leading-dash
    /// names.
    #[test]
    fn force_delete_handles_flag_like_branch_name() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();
        let head = repo.run_command(&["rev-parse", "HEAD"]).unwrap();
        test.run_git(&["update-ref", "refs/heads/-x", head.trim()]);

        let snapshot = repo.capture_refs().unwrap();
        let result = delete_branch_if_safe(&repo, &snapshot, "-x", "main", true).unwrap();
        assert!(
            matches!(result.outcome, BranchDeletionOutcome::ForceDeleted),
            "expected ForceDeleted, got a different outcome"
        );
        assert!(
            repo.run_command(&["rev-parse", "--verify", "--quiet", "refs/heads/-x"])
                .is_err(),
            "flag-like branch should have been force-deleted"
        );
    }

    /// ForceDelete remains a direct `git branch -D` operation. If topology
    /// changes after planning, Git's own checked-out-branch protection rejects
    /// it rather than the SafeDelete topology guard translating it into a
    /// retained outcome.
    #[test]
    fn force_delete_uses_git_checkout_protection() {
        let test = TestRepo::with_initial_commit();
        test.create_branch("feature");
        let repo = Repository::at(test.root_path()).unwrap();
        let snapshot = repo.capture_refs().unwrap();

        let checkout = test.home_path().join("repo.feature-force-race");
        test.run_git(&["worktree", "add", checkout.to_str().unwrap(), "feature"]);

        assert!(
            delete_branch_if_safe(&repo, &snapshot, "feature", "main", true).is_err(),
            "git branch -D must refuse a branch checked out after planning"
        );
        assert!(
            repo.run_command(&["rev-parse", "--verify", "refs/heads/feature"])
                .is_ok(),
            "Git's refusal must preserve the branch"
        );
    }

    /// A lock is the user's explicit protection for a temporarily absent
    /// worktree. It must win even over a synthetic record that is also marked
    /// prunable; an ordinary unlocked prunable record remains stale.
    #[test]
    fn locked_prunable_checkout_still_requires_retention() {
        let mut worktree = WorktreeInfo {
            path: PathBuf::from("/missing/feature"),
            head: "0123456789abcdef".to_string(),
            branch: Some("feature".to_string()),
            bare: false,
            detached: false,
            locked: Some("detachable media".to_string()),
            prunable: Some("gitdir file points to non-existent location".to_string()),
        };

        assert!(branch_checkout_requires_retention(&worktree));
        worktree.locked = None;
        assert!(!branch_checkout_requires_retention(&worktree));
    }

    /// When the branch ref vanishes between snapshot capture and the CAS
    /// delete, `git update-ref` fails *and* the ref is already absent, so
    /// the outcome is a real error (propagated) — distinct from the
    /// `RetainedRaced` case where the ref moved but still exists.
    #[test]
    fn cas_propagates_error_when_ref_vanished() {
        let test = TestRepo::with_initial_commit();
        test.run_git(&["branch", "feature"]);
        let repo = Repository::at(test.root_path()).unwrap();

        // Snapshot captures `feature`, then it is deleted out-of-band.
        let snapshot = repo.capture_refs().unwrap();
        test.run_git(&["branch", "-D", "feature"]);

        // Integration still reads "integrated" from the stale snapshot, the CAS
        // update-ref fails, and rev-parse confirms the ref is gone → error.
        let result = delete_branch_if_safe(&repo, &snapshot, "feature", "main", false);
        assert!(
            result.is_err(),
            "expected a propagated error when the ref vanished, got Ok"
        );
    }

    /// A branch created after the snapshot has no checked SHA. Even if a live
    /// resolution finds it integrated, safe deletion must not force-delete it
    /// without a compare-and-swap protecting a subsequent commit.
    #[test]
    fn retains_branch_when_snapshot_lacks_expected_sha() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        // Capture refs BEFORE `feature` exists, so the snapshot carries no SHA
        // for it.
        let snapshot = repo.capture_refs().unwrap();
        assert!(
            snapshot.local_branch("feature").is_none(),
            "test setup: snapshot must predate the branch"
        );

        // Create `feature` at main's commit → trivially integrated (same
        // commit), resolvable live but missing from the stale snapshot.
        test.run_git(&["branch", "feature"]);

        let (_, reason) = repo
            .integration_reason(&snapshot, "feature", "main")
            .unwrap();
        assert!(reason.is_some(), "the live branch must appear integrated");
        let result = delete_branch_if_safe(&repo, &snapshot, "feature", "main", false);
        let error = result.err().expect("missing checked SHA must fail closed");
        assert!(
            error.to_string().contains("absent from ref snapshot"),
            "unexpected error: {error}"
        );
        assert_eq!(
            test.git_output(&["rev-parse", "feature"]),
            test.git_output(&["rev-parse", "main"])
        );
    }

    #[test]
    fn test_branch_deletion_outcome_matching() {
        // Ensure the match patterns work correctly
        let outcomes = [
            (BranchDeletionOutcome::NotDeleted, false),
            (
                BranchDeletionOutcome::RetainedCheckedOut {
                    path: PathBuf::from("/tmp/feature"),
                },
                false,
            ),
            (BranchDeletionOutcome::RetainedRaced, false),
            (BranchDeletionOutcome::ForceDeleted, true),
            (
                BranchDeletionOutcome::Integrated(IntegrationReason::SameCommit),
                true,
            ),
        ];
        for (outcome, expected_deleted) in outcomes {
            let deleted = matches!(
                outcome,
                BranchDeletionOutcome::ForceDeleted | BranchDeletionOutcome::Integrated(_)
            );
            assert_eq!(deleted, expected_deleted);
        }
    }

    #[test]
    fn test_branch_deletion_mode_from_flags() {
        assert_eq!(
            BranchDeletionMode::from_flags(false, false),
            BranchDeletionMode::SafeDelete
        );
        assert_eq!(
            BranchDeletionMode::from_flags(false, true),
            BranchDeletionMode::ForceDelete
        );
        assert_eq!(
            BranchDeletionMode::from_flags(true, false),
            BranchDeletionMode::Keep
        );
        // keep takes precedence over force
        assert_eq!(
            BranchDeletionMode::from_flags(true, true),
            BranchDeletionMode::Keep
        );
    }

    #[test]
    fn test_branch_deletion_mode_helpers() {
        assert!(BranchDeletionMode::Keep.should_keep());
        assert!(!BranchDeletionMode::SafeDelete.should_keep());
        assert!(!BranchDeletionMode::ForceDelete.should_keep());

        assert!(BranchDeletionMode::ForceDelete.is_force());
        assert!(!BranchDeletionMode::SafeDelete.is_force());
        assert!(!BranchDeletionMode::Keep.is_force());
    }

    #[test]
    fn test_remove_options_default() {
        let opts = RemoveOptions::default();
        assert!(opts.branch.is_none());
        assert_eq!(opts.deletion_mode, BranchDeletionMode::SafeDelete);
        assert!(opts.target_branch.is_none());
        assert!(!opts.force_worktree);
    }

    #[test]
    fn test_generate_removing_path() {
        let trash_dir = PathBuf::from("/some/path/.git/wt/trash");
        let git_dir = PathBuf::from("/some/path/.git/worktrees/repo1");
        let removing_path = generate_removing_path(&trash_dir, &git_dir);
        // Format: <trash>/<registration>-<timestamp>
        let name = removing_path.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("repo1-"));
        assert!(removing_path.starts_with(&trash_dir));
    }

    /// A linked worktree uses a `.git` *file* pointing at
    /// `<common>/.git/worktrees/<name>`, not a `.git` directory. The fsmonitor
    /// IPC socket the force-kill path resolves must land under that
    /// per-worktree git dir, never under a hand-constructed `<path>/.git`.
    #[test]
    fn test_fsmonitor_socket_resolves_to_linked_worktree_git_dir() {
        use crate::git::Repository;

        let tmp = tempfile::tempdir().unwrap();
        let git = |dir: &Path| crate::testing::configure_git_env(Cmd::new("git")).current_dir(dir);

        let main = tmp.path().join("repo");
        std::fs::create_dir(&main).unwrap();
        git(&main).args(["init", "-b", "main"]).run().unwrap();
        git(&main)
            .args(["commit", "--allow-empty", "-m", "init"])
            .run()
            .unwrap();

        let linked = tmp.path().join("repo.feature");
        git(&main)
            .args(["worktree", "add", linked.to_str().unwrap(), "-b", "feature"])
            .run()
            .unwrap();
        // The defining property of a linked worktree: `.git` is a file.
        assert!(linked.join(".git").is_file());

        let repo = Repository::at(&main).unwrap();
        let wt = repo.worktree_at(&linked);
        let git_dir = wt.git_dir().unwrap();

        // git_dir points into the shared common dir's worktrees/ subtree,
        // not the worktree's own directory.
        assert!(
            git_dir.ends_with("worktrees/repo.feature"),
            "expected per-worktree git dir, got {}",
            git_dir.display()
        );
        let socket = git_dir.join("fsmonitor--daemon.ipc");
        assert!(
            !socket.starts_with(&linked),
            "socket must resolve via the .git file, not <worktree>/.git: {}",
            socket.display()
        );

        // No daemon ever ran, so the socket is absent and the whole force-kill
        // path is a no-op that returns cleanly.
        assert!(!socket.exists());
        stop_fsmonitor_daemon(&wt);
    }

    /// Fail-open contract: when the per-worktree git dir can't be resolved
    /// (the path is not a git worktree), `stop_fsmonitor_daemon` logs and
    /// returns without panicking and without attempting a force-kill.
    #[test]
    fn test_fsmonitor_stop_unresolvable_git_dir_is_noop() {
        use crate::git::Repository;

        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("repo");
        std::fs::create_dir(&main).unwrap();
        crate::testing::configure_git_env(Cmd::new("git"))
            .current_dir(&main)
            .args(["init", "-b", "main"])
            .run()
            .unwrap();
        let repo = Repository::at(&main).unwrap();

        // A path that is not a git worktree: `git_dir()` errors.
        let not_a_worktree = tmp.path().join("nope");
        std::fs::create_dir(&not_a_worktree).unwrap();
        let wt = repo.worktree_at(&not_a_worktree);
        assert!(wt.git_dir().is_err(), "precondition: git dir unresolvable");

        // Hits the git_dir() Err arm: log + early return, no panic.
        stop_fsmonitor_daemon(&wt);
    }

    /// A socket file that no process holds: the force-kill path runs `lsof`,
    /// resolves no owning PID, and is a clean no-op — nothing is signalled and
    /// the path is left intact. Exercises the real `lsof` lookup without a
    /// live daemon.
    #[cfg(unix)]
    #[test]
    fn test_fsmonitor_force_kill_unheld_socket_is_noop() {
        use crate::git::Repository;

        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("repo");
        std::fs::create_dir(&main).unwrap();
        crate::testing::configure_git_env(Cmd::new("git"))
            .current_dir(&main)
            .args(["init", "-b", "main"])
            .run()
            .unwrap();
        let repo = Repository::at(&main).unwrap();
        let wt = repo.worktree_at(&main);
        let socket = wt.git_dir().unwrap().join("fsmonitor--daemon.ipc");

        // Plant a regular file where the IPC socket would be. No process holds
        // it, so `lsof` resolves no PID and nothing is signalled.
        std::fs::write(&socket, b"").unwrap();
        stop_fsmonitor_daemon(&wt);
        assert!(socket.exists(), "no-op path must not delete the socket");
    }
}
