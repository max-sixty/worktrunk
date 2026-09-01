//! Worktree removal with fast-path trash staging and safe branch deletion.
//!
//! Two entry points:
//!
//! - [`stage_worktree_removal`] — the ordered prelude every removal path runs
//!   in the foreground before the worktree directory stops existing: the
//!   dirty-worktree gate, the fsmonitor stop, then the rename into trash. It
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
//! 1. **Clean check** (skipped with [`RemoveOptions::force_worktree`]). The
//!    final dirty-worktree gate, immediately before the mutation. Why it
//!    precedes the stop below: [`stage_worktree_removal`], "Why this order".
//! 2. **fsmonitor daemon stopped** (best effort). [`stop_fsmonitor_daemon`]
//!    runs against the target worktree before its path disappears: it sends
//!    the graceful `git fsmonitor--daemon stop` IPC request, then verifies the
//!    daemon is actually gone and force-kills it by PID if it has wedged.
//!    Without this, a daemon that has stopped answering its socket leaks
//!    forever once its worktree is removed.
//! 3. **Fast-path trash staging.** The worktree directory is renamed into
//!    `<git-common-dir>/wt/trash/<name>-<timestamp>/`. Same-filesystem renames
//!    are instant metadata operations, so the user's workspace clears
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
//! # The dirty-worktree gate follows git
//!
//! Under `core.fsmonitor` the daemon answers step 1's `git status`, so a
//! daemon returning a stale clean answer would let removal delete uncommitted
//! work. That is git's own line, not a gap in `wt`: `git worktree remove`
//! refuses a dirty worktree off the same daemon-served status, and on detected
//! event loss the builtin daemon forces a client rescan or exits. A stale
//! clean answer therefore requires an *undetected* loss, which lies to the
//! user's own `git status` in that worktree just as readily.
//!
//! Matching git is the decision. Scoping `-c core.fsmonitor=` to that one call
//! would make `wt` stricter than the command it replaces, and would restore
//! the full re-stat the ordering above avoids — the dominant per-removal cost
//! at rust-lang/rust scale. (`--no-optional-locks` is not that knob: it
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

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::git::repository::WorkingTree;
use crate::git::{IntegrationReason, Repository, WorktreeInfo};
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
        .context(crate::git::repository::path_to_logging_context(
            worktree.path(),
        ))
        .timeout(FSMONITOR_STOP_TIMEOUT)
        .run();

    // Resolve the per-worktree git dir via git (handles the `.git` *file* a
    // linked worktree uses — never hand-construct `<path>/.git`). The daemon
    // binds its IPC socket at `<git-dir>/fsmonitor--daemon.ipc`.
    let socket = match worktree.git_dir() {
        Ok(git_dir) => git_dir.join("fsmonitor--daemon.ipc"),
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
    /// a worktree immediately before deletion. The ref is retained so that
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
/// `deletion_mode.should_keep()`). Otherwise it carries the raw result so
/// callers can decide how to surface branch-deletion failures — the
/// foreground removal path reports them to the user, the TUI picker ignores
/// them (best-effort), and external tools can do whatever fits.
///
/// `staged_path` is `Some` only on the fast path. Callers are responsible for
/// cleaning up the staged directory; `wt remove` does this with a detached
/// background `rm -rf` so the foreground command returns immediately.
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
/// Returns `Some(staged_path)` when the worktree was renamed into
/// `<git-common-dir>/wt/trash/`, `None` when the rename failed
/// (cross-filesystem, permissions, Windows file locking) and the caller must
/// fall back to a direct `git worktree remove`. Either way the caller owns the
/// staged directory and must eventually delete it.
///
/// # Why this order
///
/// The gate runs **before** the daemon stop because the fsmonitor daemon
/// serves its `git status`; stopping first would force a full re-stat, the
/// dominant per-removal cost on a large repo. That the gate therefore trusts
/// the daemon is a deliberate match to `git worktree remove`, whose own gate
/// trusts it identically — see "The dirty-worktree gate follows git" in the
/// [module-level docs](self) for why that is the decision and not a gap.
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
/// validation is likewise unconditional.
///
/// `wt remove` and `wt merge --remove` have already asked this during
/// planning, where the answer can precede the "Removing …" announcement. The
/// call here still re-reads the directory's `.git` entry, so it also closes the
/// window those two leave open: the approval prompt and the `pre-remove` hook
/// run between their check and this rename.
///
/// # Errors
///
/// The ownership check and the dirty-worktree gate error. A failed rename is
/// reported as `None`, not an error, and the daemon stop is best-effort
/// throughout.
pub fn stage_worktree_removal(
    repo: &Repository,
    worktree_path: &Path,
    branch: Option<&str>,
    force_worktree: bool,
) -> anyhow::Result<Option<PathBuf>> {
    repo.worktree_at(worktree_path)
        .ensure_holds_this_worktree()?;

    if !force_worktree {
        repo.worktree_at(worktree_path)
            .ensure_clean("remove worktree", branch, true)?;
    }

    stop_fsmonitor_daemon(&repo.worktree_at(worktree_path));

    Ok(rename_into_trash(repo, worktree_path))
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
/// here — `prepare_worktree_removal` rejects one before staging.
fn rename_into_trash(repo: &Repository, worktree_path: &Path) -> Option<PathBuf> {
    let trash_dir = repo.wt_trash_dir();
    let _ = std::fs::create_dir_all(&trash_dir);
    let staged_path = generate_removing_path(&trash_dir, worktree_path);

    if std::fs::rename(worktree_path, &staged_path).is_ok() {
        // The rename moved the directory out from under `worktree_path`, so
        // git resolves the entry by its recorded path and skips the clean
        // check.
        if let Err(e) = repo.prune_worktree_entry(worktree_path) {
            tracing::debug!(error = %e, "Failed to prune worktree entry after rename: {e}");
        }
        Some(staged_path)
    } else {
        None
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
/// process that checked out or advanced the branch since planning retains it,
/// rather than orphaning a worktree or losing a commit.
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
            // `update-ref` deliberately bypasses `git branch`'s checked-out
            // branch protection, so sample topology from a brand-new
            // Repository immediately before the ref mutation. Never consult
            // `repo.list_worktrees()` here: callers often used that planning
            // cache before a pre-remove hook or another process had a chance to
            // add a checkout.
            //
            // Git exposes no transaction spanning worktree registration and ref
            // updates. Keeping this fresh read directly adjacent to the CAS
            // minimizes that unavoidable TOCTOU window; either command failing
            // leaves the branch intact.
            if let Some(path) = fresh_branch_checkout(repo, branch_name)? {
                return Ok(BranchDeletionResult {
                    outcome: BranchDeletionOutcome::RetainedCheckedOut { path },
                    integration_target: effective_target,
                });
            }

            // Atomic compare-and-swap against the snapshotted SHA. If the ref
            // moved between `integration_reason` and the delete (e.g. a hook
            // advanced the branch), `git update-ref -d <ref> <expected>` fails
            // closed: the branch is retained and we surface a `RetainedRaced`
            // outcome rather than dropping the unmerged commits silently.
            //
            // Read the SHA from the snapshot inventory (`local_branch`) rather
            // than `resolve()`, so it reflects the same `refs/heads/` walk
            // `integration_reason` consulted.
            match snapshot.local_branch(branch_name) {
                Some(b) => cas_delete_branch_outcome(repo, branch_name, &b.commit_sha, r)?,
                // Snapshot doesn't carry the branch SHA — extremely unusual
                // (the caller just captured refs, the branch is present in the
                // integration check). Fall through to a non-CAS delete rather
                // than failing the whole operation: this preserves the
                // pre-CAS behavior in a corner case rather than introducing a
                // new error class.
                None => {
                    repo.run_command(&["branch", "-D", "--", branch_name])?;
                    BranchDeletionOutcome::Integrated(r)
                }
            }
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
/// `git update-ref -d <ref> <oid>` is git's compare-and-swap delete primitive:
/// the ref is removed only if its current value matches `<oid>`. If it has
/// moved (a hook or concurrent process advanced the branch), the command
/// exits non-zero with a `cannot lock ref` message and the ref is left alone —
/// fail-closed semantics that protect unmerged commits.
///
/// When `update-ref` fails, distinguishes "ref moved" (the ref still exists
/// → `RetainedRaced`) from "real error" (the ref is gone or git itself
/// failed → propagate the original error) by re-checking with `rev-parse
/// --verify --quiet`, which has a structured exit code (0 = present, 1 =
/// absent) rather than relying on locale-sensitive error-message text.
///
/// A `packed-refs.lock` that stays contended past git's ~1 s retry budget
/// (concurrent deletes of packed branches — `wt step prune`'s parallel
/// removals — on slow ref-store I/O) also fails the CAS with the ref
/// present, and reads as `RetainedRaced` even though the tip never moved.
/// Accepted: it is fail-closed, empirically unobserved at 24-way concurrency
/// on local disks, and the next prune collects the branch.
fn cas_delete_branch_outcome(
    repo: &Repository,
    branch_name: &str,
    expected_sha: &str,
    reason: IntegrationReason,
) -> anyhow::Result<BranchDeletionOutcome> {
    let ref_name = format!("refs/heads/{branch_name}");
    let update_err = match repo.run_command(&["update-ref", "-d", &ref_name, expected_sha]) {
        Ok(_) => return Ok(BranchDeletionOutcome::Integrated(reason)),
        Err(e) => e,
    };

    // CAS failed. Re-check the ref to distinguish a race rejection (ref
    // moved → still present) from a true error (refs DB I/O, permissions,
    // git missing → propagate). `rev-parse --verify --quiet` returns exit
    // 0 when the ref exists, exit 1 when it does not — no message parsing.
    if repo
        .run_command(&["rev-parse", "--verify", "--quiet", &ref_name])
        .is_ok()
    {
        Ok(BranchDeletionOutcome::RetainedRaced)
    } else {
        Err(update_err)
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
/// Format: `<trash-dir>/<name>-<timestamp>`
pub(crate) fn generate_removing_path(trash_dir: &Path, worktree_path: &Path) -> PathBuf {
    let timestamp = epoch_now();
    let name = worktree_path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    trash_dir.join(format!("{}-{}", name, timestamp))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestRepo;

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
    /// delete, `git update-ref -d` fails *and* the ref is already absent, so
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

    /// When the branch is integrated but absent from the captured snapshot
    /// (created after capture), `integration_reason` still resolves it via the
    /// live `rev-parse` fallback, yet the snapshot carries no SHA to CAS
    /// against. The delete falls back to a plain `branch -D` and reports
    /// `Integrated` — the non-CAS arm that exists for exactly this skew.
    #[test]
    fn deletes_via_fallback_when_branch_absent_from_snapshot() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        // Capture refs BEFORE `feature` exists, so the snapshot carries no SHA
        // for it (forcing the snapshot-miss, non-CAS arm).
        let snapshot = repo.capture_refs().unwrap();
        assert!(
            snapshot.local_branch("feature").is_none(),
            "test setup: snapshot must predate the branch"
        );

        // Create `feature` at main's commit → trivially integrated (same
        // commit), resolvable live but missing from the stale snapshot.
        test.run_git(&["branch", "feature"]);

        let result = delete_branch_if_safe(&repo, &snapshot, "feature", "main", false).unwrap();
        assert!(
            matches!(result.outcome, BranchDeletionOutcome::Integrated(_)),
            "expected Integrated via the non-CAS fallback"
        );

        // Branch was deleted.
        let mut rev_parse = std::process::Command::new("git");
        crate::testing::configure_git_cmd(&mut rev_parse);
        let exit = rev_parse
            .args(["rev-parse", "--verify", "--quiet", "refs/heads/feature"])
            .current_dir(test.root_path())
            .status()
            .unwrap();
        assert!(!exit.success(), "branch should have been deleted");
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
        let path = PathBuf::from("/foo/bar/feature-branch");
        let removing_path = generate_removing_path(&trash_dir, &path);
        // Format: <trash>/<name>-<timestamp>
        let name = removing_path.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("feature-branch-"));
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
