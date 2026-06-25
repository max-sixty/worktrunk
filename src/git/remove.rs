//! Worktree removal with fast-path trash staging and safe branch deletion.
//!
//! This is the canonical removal flow used by `wt remove`, `wt merge --remove`,
//! and the TUI picker. External tooling (e.g. `worktrunk-sync`) can call it via
//! [`remove_worktree_with_cleanup`] to get the same semantics without
//! reimplementing the fsmonitor cleanup, trash-path staging, and
//! integration-check branch deletion.
//!
//! # What happens during removal
//!
//! 1. **fsmonitor daemon stopped** (best effort). [`stop_fsmonitor_daemon`]
//!    runs against the target worktree before its path disappears: it sends
//!    the graceful `git fsmonitor--daemon stop` IPC request, then verifies the
//!    daemon is actually gone and force-kills it by PID if it has wedged.
//!    Without this, a daemon that has stopped answering its socket leaks
//!    forever once its worktree is removed.
//! 2. **Fast-path trash staging.** The worktree directory is renamed into
//!    `<git-common-dir>/wt/trash/<name>-<timestamp>/`. Same-filesystem renames
//!    are instant metadata operations, so the user's workspace clears
//!    immediately. The caller is responsible for eventually removing the
//!    staged path — either synchronously or via a background process.
//! 3. **Fallback removal.** If the rename fails (cross-filesystem, permission
//!    denied, Windows file locks), the code falls back to `git worktree remove`
//!    (optionally with `--force`), which deletes files directly.
//! 4. **Branch deletion** (optional). When a branch name is supplied, the
//!    branch is deleted according to the requested [`BranchDeletionMode`]:
//!    - [`Keep`](BranchDeletionMode::Keep): never delete.
//!    - [`SafeDelete`](BranchDeletionMode::SafeDelete): delete only if
//!      [`Repository::integration_reason`] reports the branch as integrated
//!      into `target_branch` (or `HEAD` when unspecified).
//!    - [`ForceDelete`](BranchDeletionMode::ForceDelete): run `branch -D`
//!      without the integration check.
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
use crate::git::{IntegrationReason, Repository};
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
/// per-worktree git dir and is needed to resolve the owning PID), so every
/// removal path — the library `remove_worktree_with_cleanup`, the foreground
/// handler, and the background `spawn_background_removal` — calls it in the
/// foreground before the directory is staged or pruned. The detached
/// `rm -rf` background process never touches the daemon; keeping daemon
/// management in the Rust foreground avoids reimplementing socket/PID
/// resolution and signal escalation as a shell string.
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
    /// Pass `--force` to the `git worktree remove` fallback.
    ///
    /// Does not affect the fast path — trash staging is unconditional and
    /// always preserves data (the renamed directory can be recovered from
    /// `<git-common-dir>/wt/trash/` until the caller deletes it).
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
    // Stop the fsmonitor daemon, force-killing a wedged one. Must happen while
    // the worktree path still exists (the IPC socket lives under its git dir).
    stop_fsmonitor_daemon(&repo.worktree_at(worktree_path));

    // Fast path: rename into .git/wt/trash/ (instant on same filesystem),
    // then prune git metadata. Falls back to `git worktree remove` if the
    // rename fails (cross-filesystem, permissions, Windows file locking).
    let staged_path = stage_worktree_removal(repo, worktree_path);
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

/// Rename a worktree into `<git-common-dir>/wt/trash/` and prune git metadata.
///
/// Returns `Some(staged_path)` on success, `None` if the rename failed (e.g.
/// cross-filesystem, permissions, Windows file locking). Callers that see
/// `None` should fall back to a direct `git worktree remove`.
///
/// This is a lower-level building block exposed for callers that want to
/// stage the directory up-front and defer the `rm -rf` to a detached
/// background process (the pattern `wt remove` uses internally).
pub fn stage_worktree_removal(repo: &Repository, worktree_path: &Path) -> Option<PathBuf> {
    let trash_dir = repo.wt_trash_dir();
    let _ = std::fs::create_dir_all(&trash_dir);
    let staged_path = generate_removing_path(&trash_dir, worktree_path);

    if std::fs::rename(worktree_path, &staged_path).is_ok() {
        if let Err(e) = repo.prune_worktrees() {
            tracing::debug!(error = %e, "Failed to prune worktrees after rename: {e}");
        }
        Some(staged_path)
    } else {
        None
    }
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
/// to surface it. Only `git branch -D` failures propagate as `Err`.
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
        repo.run_command(&["branch", "-D", branch_name])?;
        return Ok(BranchDeletionResult {
            outcome: BranchDeletionOutcome::ForceDeleted,
            integration_target: target.to_string(),
        });
    }

    let (effective_target, reason) = repo.integration_reason(snapshot, branch_name, target)?;

    let outcome = match reason {
        Some(r) => {
            repo.run_command(&["branch", "-D", branch_name])?;
            BranchDeletionOutcome::Integrated(r)
        }
        None => BranchDeletionOutcome::NotDeleted,
    };

    Ok(BranchDeletionResult {
        outcome,
        integration_target: effective_target,
    })
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

    #[test]
    fn test_branch_deletion_outcome_matching() {
        // Ensure the match patterns work correctly
        let outcomes = [
            (BranchDeletionOutcome::NotDeleted, false),
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
        let gitconfig = tmp.path().join("gitconfig");
        std::fs::write(
            &gitconfig,
            "[init]\n\tdefaultBranch = main\n[user]\n\tname = t\n\temail = t@t\n",
        )
        .unwrap();
        let git = |dir: &Path| {
            Cmd::new("git")
                .current_dir(dir)
                .env("GIT_CONFIG_GLOBAL", &gitconfig)
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
        };

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
        let gitconfig = tmp.path().join("gitconfig");
        std::fs::write(
            &gitconfig,
            "[init]\n\tdefaultBranch = main\n[user]\n\tname = t\n\temail = t@t\n",
        )
        .unwrap();
        let main = tmp.path().join("repo");
        std::fs::create_dir(&main).unwrap();
        Cmd::new("git")
            .current_dir(&main)
            .env("GIT_CONFIG_GLOBAL", &gitconfig)
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
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
        let gitconfig = tmp.path().join("gitconfig");
        std::fs::write(
            &gitconfig,
            "[init]\n\tdefaultBranch = main\n[user]\n\tname = t\n\temail = t@t\n",
        )
        .unwrap();
        let main = tmp.path().join("repo");
        std::fs::create_dir(&main).unwrap();
        Cmd::new("git")
            .current_dir(&main)
            .env("GIT_CONFIG_GLOBAL", &gitconfig)
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
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
