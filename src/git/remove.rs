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
//! 1. **fsmonitor daemon stopped** (best effort). `git fsmonitor--daemon stop`
//!    runs against the target worktree before its path disappears, preventing
//!    zombie daemons on platforms that use builtin fsmonitor.
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
//! let output = remove_worktree_with_cleanup(
//!     &repo,
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

use crate::git::{IntegrationReason, Repository};
use crate::utils::epoch_now;

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
    worktree_path: &Path,
    options: RemoveOptions,
) -> anyhow::Result<RemovalOutput> {
    // Stop fsmonitor daemon (best effort — prevents zombie daemons when using
    // builtin fsmonitor). Must happen while the worktree path still exists.
    let _ = repo
        .worktree_at(worktree_path)
        .run_command(&["fsmonitor--daemon", "stop"]);

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
            log::debug!("Failed to prune worktrees after rename: {e}");
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

    let (effective_target, reason) = repo.integration_reason(branch_name, target)?;

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
}
