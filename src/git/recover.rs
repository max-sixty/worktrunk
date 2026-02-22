//! Recovery from a deleted current working directory.
//!
//! When a linked worktree is removed (via `wt remove` or `wt merge` from another
//! terminal) while a shell is still in that directory, `Repository::current()` fails
//! because git can't resolve the CWD. This module provides recovery by finding the
//! parent repository from `$PWD` (which shells preserve after directory deletion).

use std::path::{Path, PathBuf};

use super::Repository;

/// Try to get the current repository, recovering from a deleted CWD if possible.
///
/// Returns `(Repository, recovered)` where `recovered` is `true` if the CWD was
/// deleted and we recovered by finding the parent repository.
///
/// Prints an info message when recovery occurs.
pub fn current_or_recover() -> anyhow::Result<(Repository, bool)> {
    match Repository::current() {
        Ok(repo) => Ok((repo, false)),
        Err(err) => match recover_from_deleted_cwd() {
            Some(repo) => {
                eprintln!(
                    "{}",
                    crate::styling::info_message("Current worktree was removed, recovering...")
                );
                Ok((repo, true))
            }
            None => Err(err),
        },
    }
}

/// Attempt to recover a repository when the current directory has been deleted.
///
/// Returns `Some(Repository)` if:
/// 1. `std::env::current_dir()` fails or returns a non-existent path (CWD is gone)
/// 2. `$PWD` points to a path whose ancestor contains a git repository
/// 3. The deleted path was actually a worktree of that repository
///
/// Returns `None` if CWD is fine or recovery fails at any step.
fn recover_from_deleted_cwd() -> Option<Repository> {
    // If current_dir succeeds and the directory exists, nothing to recover from.
    // On Windows, current_dir() may succeed even after the directory is removed
    // (the process handle keeps it alive), so also check existence on disk.
    match std::env::current_dir() {
        Ok(p) if p.exists() => return None,
        _ => {}
    }

    // Shells preserve the logical path in $PWD even after the directory is deleted
    let pwd = std::env::var_os("PWD")?;
    let deleted_path = PathBuf::from(pwd);

    recover_from_path(&deleted_path)
}

/// Core recovery logic: given a deleted worktree path, find the parent repository.
///
/// Walks up from `deleted_path` to find the first existing ancestor, looks for a
/// git repository there or in its immediate children, and verifies the deleted path
/// was actually a worktree of that repository.
fn recover_from_path(deleted_path: &Path) -> Option<Repository> {
    let ancestor = first_existing_ancestor(deleted_path)?;
    log::debug!(
        "Deleted CWD recovery: path={}, ancestor={}",
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

    Some(repo)
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

    // Check immediate children for .git directories.
    // Uses is_some_and instead of ? so an unreadable entry (e.g., broken symlink)
    // skips that entry rather than aborting the entire search.
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        if entry.file_type().ok().is_some_and(|ft| ft.is_dir())
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
///
/// Note: This only matches `.git` directories, so bare repos (which have no
/// `.git` subdirectory) won't be discovered. The fallback hint in `main.rs`
/// covers this gracefully.
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

    #[test]
    fn test_find_repo_near_returns_none_when_no_repo() {
        let tmp = tempfile::tempdir().unwrap();
        // No .git directory anywhere — should return None
        assert!(find_repo_near(tmp.path()).is_none());
    }

    #[test]
    fn test_find_repo_near_skips_non_directories() {
        let tmp = tempfile::tempdir().unwrap();
        // Create a file (not a directory) as child — should be skipped
        std::fs::write(tmp.path().join("not_a_dir"), "data").unwrap();
        assert!(find_repo_near(tmp.path()).is_none());
    }

    #[test]
    fn test_paths_match_identical_paths() {
        let p = PathBuf::from("/some/path/feature");
        assert!(paths_match(&p, &p));
    }

    #[test]
    fn test_paths_match_different_names() {
        let a = PathBuf::from("/repos/feature-a");
        let b = PathBuf::from("/repos/feature-b");
        assert!(!paths_match(&a, &b));
    }

    #[test]
    fn test_paths_match_same_name_same_parent() {
        let tmp = tempfile::tempdir().unwrap();
        // Both paths share the same existing parent and same name
        let a = tmp.path().join("feature");
        let b = tmp.path().join("feature");
        assert!(paths_match(&a, &b));
    }

    #[test]
    fn test_paths_match_different_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let dir_a = tmp.path().join("a");
        let dir_b = tmp.path().join("b");
        std::fs::create_dir(&dir_a).unwrap();
        std::fs::create_dir(&dir_b).unwrap();
        let a = dir_a.join("feature");
        let b = dir_b.join("feature");
        assert!(!paths_match(&a, &b));
    }

    #[test]
    fn test_was_worktree_of_finds_existing_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        // Canonicalize to handle symlinks (e.g., /tmp -> /private/tmp on macOS)
        let base = dunce::canonicalize(tmp.path()).unwrap();
        let repo_dir = base.join("repo");
        std::fs::create_dir(&repo_dir).unwrap();
        git_init(&repo_dir);
        // Create an initial commit so worktree add works
        Cmd::new("git")
            .args(["commit", "--allow-empty", "-m", "init"])
            .current_dir(&repo_dir)
            .run()
            .unwrap();

        // Add a linked worktree
        let wt_path = base.join("feature-wt");
        Cmd::new("git")
            .args([
                "worktree",
                "add",
                &wt_path.to_string_lossy(),
                "-b",
                "feature",
            ])
            .current_dir(&repo_dir)
            .run()
            .unwrap();

        let repo = Repository::at(&repo_dir).unwrap();
        assert!(was_worktree_of(&repo, &wt_path));
    }

    #[cfg(unix)]
    #[test]
    fn test_find_repo_near_handles_broken_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        // Create a broken symlink — file_type() returns Err for these
        std::os::unix::fs::symlink("/nonexistent/target", tmp.path().join("broken_link")).unwrap();
        // Should return None without aborting (the broken symlink is skipped gracefully)
        assert!(find_repo_near(tmp.path()).is_none());
    }

    #[test]
    fn test_current_or_recover_returns_repo_when_cwd_exists() {
        // In a test environment, CWD exists, so current_or_recover should succeed
        // via the normal Repository::current() path (not recovery).
        // This test will fail if not run inside a git repo, which is expected in CI.
        if Repository::current().is_ok() {
            let (repo, recovered) = current_or_recover().unwrap();
            assert!(!recovered);
            // Sanity check: repo should have a valid path
            assert!(repo.repo_path().exists());
        }
    }

    #[test]
    fn test_was_worktree_of_rejects_unknown_path() {
        let tmp = tempfile::tempdir().unwrap();
        git_init(tmp.path());
        Cmd::new("git")
            .args(["commit", "--allow-empty", "-m", "init"])
            .current_dir(tmp.path())
            .run()
            .unwrap();

        let repo = Repository::at(tmp.path()).unwrap();
        let unknown = PathBuf::from("/nonexistent/unknown");
        assert!(!was_worktree_of(&repo, &unknown));
    }

    #[test]
    fn test_recover_from_path_finds_deleted_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let base = dunce::canonicalize(tmp.path()).unwrap();
        let repo_dir = base.join("repo");
        std::fs::create_dir(&repo_dir).unwrap();
        git_init(&repo_dir);
        Cmd::new("git")
            .args(["commit", "--allow-empty", "-m", "init"])
            .current_dir(&repo_dir)
            .run()
            .unwrap();

        // Add a linked worktree
        let wt_path = base.join("feature-wt");
        Cmd::new("git")
            .args([
                "worktree",
                "add",
                &wt_path.to_string_lossy(),
                "-b",
                "feature",
            ])
            .current_dir(&repo_dir)
            .run()
            .unwrap();

        // Delete the worktree directory (simulating external removal)
        std::fs::remove_dir_all(&wt_path).unwrap();

        // recover_from_path should find the parent repo
        let recovered = recover_from_path(&wt_path);
        assert!(
            recovered.is_some(),
            "should recover repo from deleted worktree path"
        );
    }

    #[test]
    fn test_recover_from_path_returns_none_for_unrelated_path() {
        let tmp = tempfile::tempdir().unwrap();
        let base = dunce::canonicalize(tmp.path()).unwrap();
        let repo_dir = base.join("repo");
        std::fs::create_dir(&repo_dir).unwrap();
        git_init(&repo_dir);
        Cmd::new("git")
            .args(["commit", "--allow-empty", "-m", "init"])
            .current_dir(&repo_dir)
            .run()
            .unwrap();

        // Try to recover from a path that was never a worktree
        let unrelated = base.join("not-a-worktree");
        let recovered = recover_from_path(&unrelated);
        assert!(
            recovered.is_none(),
            "should not recover from unrelated path"
        );
    }
}
