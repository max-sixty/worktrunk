//! Worktree removal with fast-path trash staging and safe branch deletion.
//!
//! Two entry points:
//!
//! - [`stage_worktree_removal`] — the ordered prelude every removal path runs
//!   in the foreground before the worktree directory stops existing: the
//!   dirty-worktree gate, the fsmonitor stop, then safe metadata teardown. It
//!   owns the gate, so it is the one place removal's data safety is decided.
//! - [`remove_worktree_with_cleanup`] — that prelude, plus the direct-removal
//!   fallback and branch deletion, run to completion synchronously.
//!
//! The split exists because the default path defers its deletion: `wt remove`
//! and `wt merge --remove` stage the worktree here, then hand the `rm -rf` to
//! a detached process so the command returns as soon as the workspace is
//! clear. They build that tail themselves rather than calling
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
//! 1. **Lock and clean checks**. A `git worktree lock` is refused
//!    unconditionally (matching `git worktree remove`; `--force` does not
//!    override it). The dirty-worktree gate is skipped with
//!    [`RemoveOptions::force_worktree`]. Why the dirty gate precedes the
//!    stop below: [`stage_worktree_removal`], "Why this order".
//! 2. **fsmonitor daemon stopped** (best effort). [`stop_fsmonitor_daemon`]
//!    runs against the target worktree before its path disappears: it sends
//!    the graceful `git fsmonitor--daemon stop` IPC request, then verifies the
//!    daemon is actually gone and force-kills it by PID if it has wedged.
//!    Without this, a daemon that has stopped answering its socket leaks
//!    forever once its worktree is removed.
//! 3. **Fast-path staging.** The worktree directory is renamed into
//!    `<git-common-dir>/wt/retained/<name>-<timestamp>/`, unregistered, then
//!    moved to `wt/trash/`. A failure to unregister preserves the checkout in
//!    the unswept retained directory and reports its recovery path.
//!    Same-filesystem renames are instant metadata operations, so the user's workspace clears
//!    immediately. The caller is responsible for eventually removing the
//!    staged path — either synchronously or via a background process.
//! 4. **Fallback removal.** If the rename fails (cross-filesystem, permission
//!    denied, Windows file locks), the code falls back to `git worktree remove`
//!    (optionally with `--force`), which deletes files directly.
//! 5. **Branch deletion** (optional). When a branch name is supplied, the
//!    branch is deleted according to the requested [`BranchDeletionMode`]:
//!    - [`Keep`](BranchDeletionMode::Keep): never delete.
//!    - [`SafeDelete`](BranchDeletionMode::SafeDelete): delete only if
//!      [`Repository::integration_reason`] reports the branch as integrated
//!      into `target_branch` (or `HEAD` when unspecified).
//!    - [`ForceDelete`](BranchDeletionMode::ForceDelete): run `branch -D`
//!      without the integration check.
//!
//! # Cleanliness before and after fsmonitor shutdown
//!
//! The early gate follows ordinary `git status`, including `core.fsmonitor`,
//! so a dirty worktree is refused before touching its daemon. Shutdown is an
//! external command and can overlap writers. The final gate therefore uses
//! `core.fsmonitor=false` to rescan staged, modified and untracked files without
//! restarting the daemon. It pays the full filesystem scan on a successful
//! non-forced removal so work appearing during shutdown cannot be discarded.
//! As with Git, updates after that final check still have a filesystem race.
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

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::git::repository::WorkingTree;
use crate::git::{
    CleanCheckMode, GitError, IntegrationReason, Repository, WorktreeInfo, WorktreePruneMode,
    path_dir_name,
};
use crate::shell_exec::Cmd;
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
/// foreground before the directory is staged or pruned. The detached `rm -rf`
/// background process never touches the daemon; keeping daemon management in
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
        .current_dir(worktree.path())
        .scrub_git_discovery_env()
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
        .arg(socket.to_string_lossy().into_owned())
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
    /// Ownership and locks are checked regardless. Staging preserves the
    /// directory under `<git-common-dir>/wt/retained/` until unregistering
    /// succeeds; only then may callers delete the authorized payload.
    pub force_worktree: bool,
}

/// Result of [`remove_worktree_with_cleanup`].
///
/// `branch_result` is `None` when deletion was skipped (no branch supplied, or
/// `deletion_mode.should_keep()`). Otherwise it carries the raw result so
/// callers can decide how to surface branch-deletion failures — the
/// foreground removal path reports them to the user, the TUI picker ignores
/// them (best-effort), and external tools can do whatever fits.
///
/// `staged_path` is `Some` only after successful fast-path unregistering.
/// Callers own its cleanup, usually from trash and otherwise from retained
/// staging if promotion to trash failed. `wt remove` spawns a detached
/// background `rm -rf` so the foreground command returns immediately.
pub struct RemovalOutput {
    pub branch_result: Option<anyhow::Result<BranchDeletionResult>>,
    /// Path to the safely unregistered payload on the fast path.
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
///   is considered the primary operation, and callers can decide how to
///   handle a residual branch-deletion failure.
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
        repo.remove_worktree(worktree_path, options.force_worktree)?;
    }

    // Delete branch if safe
    let branch_result = if let Some(branch) = options.branch.as_deref()
        && !options.deletion_mode.should_keep()
    {
        let target = options.target_branch.as_deref().unwrap_or("HEAD");
        Some(delete_branch_if_safe(
            repo,
            snapshot,
            branch,
            target,
            options.deletion_mode.is_force(),
        ))
    } else {
        None
    };

    Ok(RemovalOutput {
        branch_result,
        staged_path,
    })
}

/// Gate, stop the fsmonitor daemon, and stage a worktree for removal — steps
/// 1-3 of the [module-level docs](self), in that order.
///
/// Every removal path runs this, in the foreground, before the worktree
/// directory stops existing: the synchronous
/// [`remove_worktree_with_cleanup`], and the default background path, which
/// stages here and hands the `rm -rf` to a detached process. Keeping the three
/// steps together is what makes the dirty-worktree gate a single decision
/// rather than a sequence each caller re-assembles — the order below is easy
/// to get subtly wrong, and getting it wrong destroys uncommitted work.
///
/// Returns `Some(staged_path)` after the worktree was moved aside and safely
/// unregistered, usually under `<git-common-dir>/wt/trash/`. Returns `None`
/// when the initial rename failed
/// (cross-filesystem, permissions, Windows file locking) and the caller must
/// fall back to a direct `git worktree remove`. Either way the caller owns the
/// staged directory and must eventually delete it.
///
/// # Why this order
///
/// The early gate runs before daemon shutdown to refuse already-dirty
/// worktrees without altering their daemon. After shutdown, a full scan with
/// fsmonitor disabled catches staged, modified and untracked files that
/// appeared during it. Ownership and locks are then rechecked immediately
/// before the rename. See the [module-level docs](self).
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
/// Ownership, lock, index, and dirty-worktree checks error. The initial rename
/// failing is reported as `None`, and the daemon stop is best effort. Failure
/// to unregister after staging errors with the preserved checkout's location;
/// callers must not arrange deletion after that error.
pub fn stage_worktree_removal(
    repo: &Repository,
    worktree_path: &Path,
    branch: Option<&str>,
    force_worktree: bool,
) -> anyhow::Result<Option<PathBuf>> {
    let worktree = repo.worktree_at(worktree_path);
    let git_dir = require_removal_allowed(&worktree, branch, None)?;
    if !force_worktree {
        worktree.ensure_clean(
            "remove worktree",
            branch,
            true,
            CleanCheckMode::ConfiguredFsmonitor,
        )?;
    }

    stop_fsmonitor_daemon(&repo.worktree_at(worktree_path));

    // Shutdown can overlap writers. Rescan after it without restarting the
    // daemon: an index-only check would miss unstaged and untracked files.
    if !force_worktree {
        // A replacement must be refused as an ownership change, rather than
        // reporting its dirt and suggesting --force against the wrong tree.
        require_removal_allowed(&worktree, branch, Some(&git_dir))?;
        worktree.ensure_clean("remove worktree", branch, true, CleanCheckMode::FullScan)?;
    }

    // No external command runs between this final owner/lock check and the
    // rename. A same-repository replacement must not retarget the old removal.
    require_removal_allowed(&worktree, branch, Some(&git_dir))?;

    rename_into_trash(repo, worktree_path, &git_dir, force_worktree)
}

/// The owner and lock gates are unconditional, including explicit force.
/// Called before inspecting/stopping the worktree, and again before moving it.
fn require_removal_allowed(
    worktree: &WorkingTree,
    branch: Option<&str>,
    expected_registration: Option<&Path>,
) -> anyhow::Result<PathBuf> {
    let git_dir = worktree.ensure_holds_this_worktree()?;
    if expected_registration.is_some_and(|expected| !crate::path::paths_match(expected, &git_dir)) {
        anyhow::bail!(
            "Worktree registration changed during removal @ {}",
            worktree.path().display(),
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

/// Stage a worktree outside swept trash, unregister it, then promote to trash.
///
/// Returns `Some(staged_path)` on success, `None` if the initial rename failed. The
/// unguarded mutation: [`stage_worktree_removal`] is the only caller, and it
/// is what places the dirty-worktree gate ahead of this.
///
/// The metadata cleanup names this worktree
/// ([`prune_worktree_entry`](Repository::prune_worktree_entry)) rather than
/// sweeping the repository, so a sibling worktree whose directory happens to
/// be absent right now keeps its registration. A locked worktree never reaches
/// here — [`stage_worktree_removal`] rejects one before the rename.
///
/// Removal of this live worktree has already been authorized by that gate.
/// Like `git worktree remove`, it includes clean in-progress operation state.
/// Already-stale registrations retain operation state as well as staged work.
/// Live removal still checks its index again before unregistering: a paused
/// operation must not hide staged work added after the live-removal gate.
fn rename_into_trash(
    repo: &Repository,
    worktree_path: &Path,
    git_dir: &Path,
    force_worktree: bool,
) -> anyhow::Result<Option<PathBuf>> {
    // Nothing enters swept trash until its metadata was safely unregistered.
    // A failure must leave both payload and registration recoverable, even if
    // another process has already occupied the original checkout path.
    let retained_dir = repo.wt_dir().join("retained");
    if let Err(e) = std::fs::create_dir_all(&retained_dir) {
        tracing::debug!(error = %e, "Failed to prepare worktree staging, falling back: {e}");
        return Ok(None);
    }
    let retained_path = generate_removing_path(&retained_dir, git_dir);
    if let Err(e) = renamore::rename_exclusive(worktree_path, &retained_path) {
        tracing::debug!(error = %e, "Failed to stage worktree, falling back: {e}");
        return Ok(None);
    }

    if let Err(e) = repo.prune_worktree_entry(
        worktree_path,
        WorktreePruneMode::removed_live(force_worktree),
    ) {
        anyhow::bail!(
            "Worktree removal stopped: {e:#}. Worktree files are preserved at {}; run git -C {} worktree repair {} to reconnect them",
            retained_path.display(),
            shell_escape::escape(repo.git_common_dir().to_string_lossy()),
            shell_escape::escape(retained_path.to_string_lossy()),
        );
    }

    let trash_dir = repo.wt_trash_dir();
    let staged_path = generate_removing_path(&trash_dir, git_dir);
    if std::fs::create_dir_all(&trash_dir)
        .and_then(|()| renamore::rename_exclusive(&retained_path, &staged_path))
        .is_ok()
    {
        Ok(Some(staged_path))
    } else {
        // Unregistering committed the removal. Caller cleanup can safely
        // delete this payload directly even if promoting it to trash failed.
        Ok(Some(retained_path))
    }
}

/// Capture fresh refs and run a planned branch deletion.
///
/// The spelling of "the plan said delete; do it now" for every synchronous
/// site that doesn't already hold a fresh snapshot (the background fast path,
/// prune's synchronous fallback, branch-only removal, a picker row). The
/// invariant it carries — no deletion ever runs against the planning-time
/// snapshot, so the topology guard and CAS in [`delete_branch_if_safe`]
/// re-decide against live state — also holds on the two paths that don't call
/// it: the foreground removal captures its own fresh snapshot just before
/// [`remove_worktree_with_cleanup`], and the detached fallback can't call
/// Rust, so it gets the guarantee from a fresh porcelain topology read followed
/// by an `update-ref -d <ref> <sha>` shell tail instead. A hook or concurrent
/// process that advanced the branch after the integration check cannot lose
/// that commit: the CAS rejects the deletion. Checkout protection is best
/// effort, retaining any checkout observed by the fresh topology read.
pub fn execute_branch_deletion(
    repo: &Repository,
    branch_name: &str,
    target: &str,
    force_delete: bool,
) -> anyhow::Result<BranchDeletionResult> {
    let snapshot = repo.capture_refs()?;
    delete_branch_if_safe(repo, &snapshot, branch_name, target, force_delete)
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
    // Force-delete: skip integration check entirely (matches compute_integration_reason
    // behavior for the Worktree path). The user explicitly chose -D.
    if force_delete {
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
}

/// Find a live checkout of `branch` using a new repository cache.
///
/// Removal planning intentionally caches `git worktree list`, but branch
/// deletion happens after hooks and other concurrent actors may have changed
/// topology. Constructing a new [`Repository`] is the cache boundary: its first
/// `list_worktrees` call executes a fresh `git worktree list --porcelain`.
///
/// Git still lists stale registrations whose directories are gone, marking
/// them `prunable`; those have no live checkout to orphan and must not strand
/// the branch. Missing locked worktrees remain non-prunable, so their lock
/// continues to retain the branch conservatively.
fn fresh_branch_checkout(repo: &Repository, branch_name: &str) -> anyhow::Result<Option<PathBuf>> {
    let fresh_repo = Repository::at(repo.discovery_path())?;
    Ok(fresh_repo
        .list_worktrees()?
        .iter()
        .find(|worktree| branch_checkout_requires_retention(worktree, branch_name))
        .map(|worktree| worktree.path.clone()))
}

/// Whether deleting `branch` would risk orphaning this worktree record.
///
/// Git normally reports a missing locked worktree as `locked` but not
/// `prunable`. Letting the lock win explicitly keeps the guard fail-closed if a
/// Git version or synthetic porcelain record ever carries both fields.
fn branch_checkout_requires_retention(worktree: &WorktreeInfo, branch_name: &str) -> bool {
    worktree.branch.as_deref() == Some(branch_name)
        && (!worktree.is_prunable() || worktree.locked.is_some())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::ErrorExt;
    use crate::testing::TestRepo;

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

    /// Registry serialization starts after the fast-path rename, so worktree
    /// staging can overlap while metadata teardown remains exclusive.
    #[test]
    fn stages_worktree_before_waiting_for_registry_lock() {
        let mut test = TestRepo::with_initial_commit();
        let worktree_path = test.add_worktree("feature");
        let repo = Repository::at(test.root_path()).unwrap();
        let worker_repo = repo.clone();
        let worker_path = worktree_path.clone();

        let registry_guard = repo.worktree_registry_write();
        let worker = std::thread::spawn(move || {
            stage_worktree_removal(&worker_repo, &worker_path, Some("feature"), false)
        });

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while worktree_path.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let renamed_before_unlock = !worktree_path.exists();

        drop(registry_guard);
        let result = worker.join().expect("staging thread should not panic");
        assert!(
            renamed_before_unlock,
            "worktree should be renamed before registry teardown acquires the lock; result: {result:?}"
        );
        assert!(
            result.unwrap().is_some(),
            "worktree should use the rename fast path"
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

        assert!(branch_checkout_requires_retention(&worktree, "feature"));
        worktree.locked = None;
        assert!(!branch_checkout_requires_retention(&worktree, "feature"));
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
