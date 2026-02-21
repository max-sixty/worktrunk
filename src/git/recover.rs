//! Recovery from a deleted current working directory.
//!
//! When a linked worktree is removed (via `wt remove` or `wt merge` from another
//! terminal) while a shell is still in that directory, `Repository::current()` fails
//! because git can't resolve the CWD. This module provides recovery by finding the
//! parent repository from `$PWD` (which shells preserve after directory deletion).

use std::path::{Path, PathBuf};

use super::Repository;

/// Result of recovering from a deleted working directory.
pub struct RecoveredRepo {
    /// A valid repository discovered from an ancestor directory.
    pub repo: Repository,
    /// The deleted worktree path (from `$PWD`).
    pub deleted_path: PathBuf,
}

/// Attempt to recover a repository when the current directory has been deleted.
///
/// Returns `Some(RecoveredRepo)` if:
/// 1. `std::env::current_dir()` fails (CWD is gone)
/// 2. `$PWD` points to a path whose ancestor contains a git repository
/// 3. The deleted path was actually a worktree of that repository
///
/// Returns `None` if CWD is fine or recovery fails at any step.
pub fn recover_from_deleted_cwd() -> Option<RecoveredRepo> {
    // If current_dir succeeds, nothing to recover from
    if std::env::current_dir().is_ok() {
        return None;
    }

    // Shells preserve the logical path in $PWD even after the directory is deleted
    let pwd = std::env::var_os("PWD")?;
    let deleted_path = PathBuf::from(pwd);

    // Walk up from $PWD to find the first existing ancestor
    let ancestor = first_existing_ancestor(&deleted_path)?;
    log::debug!(
        "Deleted CWD recovery: $PWD={}, ancestor={}",
        deleted_path.display(),
        ancestor.display()
    );

    // Look for a git repository at the ancestor or its immediate children
    let repo = find_repo_near(&ancestor)?;

    // Verify the deleted path was actually a worktree of this repo
    if !was_worktree_of(&repo, &deleted_path) {
        log::debug!("Deleted CWD recovery: path was not a worktree of discovered repo");
        return None;
    }

    Some(RecoveredRepo { repo, deleted_path })
}

/// Walk up from `path` to find the first existing ancestor directory.
fn first_existing_ancestor(path: &Path) -> Option<PathBuf> {
    let mut candidate = path.parent()?;
    loop {
        if candidate.is_dir() {
            return Some(candidate.to_path_buf());
        }
        candidate = candidate.parent()?;
    }
}

/// Look for a git repository at `dir` or its immediate children.
///
/// Only checks for `.git` **directories** (main repos), not `.git` files
/// (which are linked worktrees — we need the main repo to recover).
fn find_repo_near(dir: &Path) -> Option<Repository> {
    // Check the directory itself first
    if let Some(repo) = try_repo_at(dir) {
        return Some(repo);
    }

    // Check immediate children for .git directories
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        if entry.file_type().ok()?.is_dir()
            && let Some(repo) = try_repo_at(&entry.path())
        {
            return Some(repo);
        }
    }

    None
}

/// Try to discover a repository at the given path.
///
/// Returns `Some(repo)` if the path contains a `.git` directory (not a file)
/// and `Repository::at()` succeeds.
fn try_repo_at(dir: &Path) -> Option<Repository> {
    let git_path = dir.join(".git");
    // Only match .git directories (main repos), not .git files (linked worktrees)
    if git_path.is_dir() {
        Repository::at(dir).ok()
    } else {
        None
    }
}

/// Check if the deleted path was a worktree of the given repository.
///
/// Uses `list_worktrees()` which includes prunable entries — a deleted worktree
/// directory will show up as prunable, confirming it belonged to this repo.
fn was_worktree_of(repo: &Repository, deleted_path: &Path) -> bool {
    let worktrees = match repo.list_worktrees() {
        Ok(wt) => wt,
        Err(_) => return false,
    };

    // Canonicalize the deleted path's parent for comparison, since worktree paths
    // from git are typically canonical. We can't canonicalize the deleted path
    // itself (it doesn't exist), but we can check if any worktree path matches.
    worktrees.iter().any(|wt| {
        wt.path == deleted_path || (wt.is_prunable() && paths_match(&wt.path, deleted_path))
    })
}

/// Compare worktree paths, accounting for the fact that the deleted path
/// may not be canonical (e.g., symlinks in parent directories).
fn paths_match(worktree_path: &Path, deleted_path: &Path) -> bool {
    // Direct comparison first
    if worktree_path == deleted_path {
        return true;
    }

    // Try canonicalizing parents and comparing the final component.
    // The deleted path doesn't exist, but its parent might.
    let wt_name = worktree_path.file_name();
    let del_name = deleted_path.file_name();
    if wt_name != del_name {
        return false;
    }

    // If both parents can be canonicalized and they match, the paths match
    let wt_parent = worktree_path
        .parent()
        .and_then(|p| dunce::canonicalize(p).ok());
    let del_parent = deleted_path
        .parent()
        .and_then(|p| dunce::canonicalize(p).ok());
    matches!((wt_parent, del_parent), (Some(a), Some(b)) if a == b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell_exec::Cmd;

    fn git_init(path: &Path) {
        Cmd::new("git")
            .args(["init", "--quiet"])
            .current_dir(path)
            .run()
            .unwrap();
    }

    #[test]
    fn test_first_existing_ancestor_finds_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let existing = tmp.path().join("a");
        std::fs::create_dir(&existing).unwrap();
        let deleted = existing.join("b").join("c");

        let result = first_existing_ancestor(&deleted);
        assert_eq!(result, Some(existing));
    }

    #[test]
    fn test_first_existing_ancestor_returns_none_for_root() {
        // A path with no existing ancestors (besides root) should still find something
        let result = first_existing_ancestor(Path::new("/nonexistent/deep/path"));
        assert!(result.is_some()); // / exists
    }

    #[test]
    fn test_try_repo_at_rejects_git_file() {
        let tmp = tempfile::tempdir().unwrap();
        // Create a .git file (not directory) — simulates a linked worktree
        std::fs::write(tmp.path().join(".git"), "gitdir: /some/path").unwrap();
        assert!(try_repo_at(tmp.path()).is_none());
    }

    #[test]
    fn test_try_repo_at_accepts_git_dir() {
        let tmp = tempfile::tempdir().unwrap();
        git_init(tmp.path());
        assert!(try_repo_at(tmp.path()).is_some());
    }

    #[test]
    fn test_find_repo_near_finds_repo_in_child() {
        let tmp = tempfile::tempdir().unwrap();
        let child = tmp.path().join("myrepo");
        std::fs::create_dir(&child).unwrap();
        git_init(&child);

        let repo = find_repo_near(tmp.path());
        assert!(repo.is_some());
    }

    #[test]
    fn test_recover_returns_none_when_cwd_exists() {
        // current_dir() succeeds in test environment, so recovery should return None
        assert!(recover_from_deleted_cwd().is_none());
    }
}
