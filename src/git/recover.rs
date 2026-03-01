//! Recovery from a deleted current working directory.
//!
//! When a linked worktree is removed (via `wt remove` or `wt merge` from another
//! terminal) while a shell is still in that directory, `Repository::current()` fails
//! because git can't resolve the CWD. This module provides recovery by finding the
//! parent repository from `$PWD` (which shells preserve after directory deletion).

use std::path::{Path, PathBuf};

use color_print::cformat;

use crate::styling::eprintln;

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

/// Determine the hint to show when the user's CWD has been removed.
///
/// Tries to find the parent repository and checks if `wt switch ^` would work
/// (i.e., the default branch has an existing worktree). Falls back to
/// progressively less specific hints when recovery or resolution fails.
pub fn cwd_removed_hint() -> String {
    let Some(repo) = Repository::current().ok().or_else(recover_from_deleted_cwd) else {
        return "Current directory was removed.".to_string();
    };
    hint_for_repo(&repo)
}

fn hint_for_repo(repo: &Repository) -> String {
    if let Some(branch) = repo.default_branch()
        && repo
            .worktree_for_branch(&branch)
            .ok()
            .flatten()
            .is_some_and(|p| p.exists())
    {
        return cformat!("Current directory was removed. Try: <bright-black>wt switch ^</>");
    }

    cformat!("Current directory was removed. Run <bright-black>wt list</> to see worktrees.")
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
/// Walks up from `deleted_path` checking each existing ancestor (and its immediate
/// children) for git repositories. Each candidate repo is validated with
/// `was_worktree_of` to ensure the deleted path actually belonged to it.
///
/// This handles both sibling layouts (worktree next to repo) and nested layouts
/// (worktree inside repo) without needing to know the template structure.
fn recover_from_path(deleted_path: &Path) -> Option<Repository> {
    let mut candidate = deleted_path.parent()?;
    loop {
        if candidate.is_dir() {
            log::debug!(
                "Deleted CWD recovery: path={}, checking ancestor={}",
                deleted_path.display(),
                candidate.display()
            );
            if let Some(repo) = find_validated_repo_near(candidate, deleted_path) {
                return Some(repo);
            }
        }
        candidate = candidate.parent()?;
    }
}

/// Look for a git repository at `dir` or its immediate children that recognizes
/// `deleted_path` as a (former) worktree.
///
/// Only checks for `.git` **directories** (main repos), not `.git` files
/// (which are linked worktrees — we need the main repo to recover).
fn find_validated_repo_near(dir: &Path, deleted_path: &Path) -> Option<Repository> {
    // Check the directory itself first
    if let Some(repo) = try_repo_at(dir)
        && was_worktree_of(&repo, deleted_path)
    {
        return Some(repo);
    }

    // Check immediate children for .git directories.
    // Uses is_some_and instead of ? so an unreadable entry (e.g., broken symlink)
    // skips that entry rather than aborting the entire search.
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        if entry.file_type().ok().is_some_and(|ft| ft.is_dir())
            && let Some(repo) = try_repo_at(&entry.path())
            && was_worktree_of(&repo, deleted_path)
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
/// `.git` subdirectory) won't be discovered. `cwd_removed_hint()` handles
/// this gracefully by falling back to progressively less specific hints.
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
///
/// Also matches when `deleted_path` is a subdirectory of a worktree (the shell
/// may have been deeper than the worktree root when it was removed).
fn was_worktree_of(repo: &Repository, deleted_path: &Path) -> bool {
    repo.list_worktrees().is_ok_and(|worktrees| {
        worktrees.iter().any(|wt| {
            deleted_path.starts_with(&wt.path)
                || (wt.is_prunable() && paths_match(&wt.path, deleted_path))
        })
    })
}

/// Compare worktree paths, accounting for the fact that the deleted path
/// may not be canonical (e.g., symlinks in parent directories).
///
/// Note: the symlink fallback only handles the case where `deleted_path` is
/// the worktree root itself. If `deleted_path` is deeper (e.g., `.../wt/src/`)
/// AND there are symlinks in the parent, this won't match. The `starts_with`
/// check in `was_worktree_of` handles the non-symlink descendant case.
fn paths_match(worktree_path: &Path, deleted_path: &Path) -> bool {
    // Direct comparison first (includes descendant check via starts_with)
    if deleted_path.starts_with(worktree_path) {
        return true;
    }

    // Symlink fallback: canonicalize parents and compare the final component.
    // Only handles exact match (same final component), not descendants.
    let wt_name = worktree_path.file_name();
    let del_name = deleted_path.file_name();
    if wt_name != del_name {
        return false;
    }

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
    use ansi_str::AnsiStr;

    fn git_init(path: &Path) {
        Cmd::new("git")
            .args(["init", "--quiet"])
            .current_dir(path)
            .run()
            .unwrap();
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
    fn test_recover_returns_none_when_cwd_exists() {
        // current_dir() succeeds in test environment, so recovery should return None
        assert!(recover_from_deleted_cwd().is_none());
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
    fn test_current_or_recover_returns_repo_when_cwd_exists() {
        // In a test environment, CWD exists, so current_or_recover should succeed
        // via the normal Repository::current() path (not recovery).
        // Tests run inside a git repo in CI, so Repository::current() succeeds.
        let (repo, recovered) = current_or_recover().unwrap();
        assert!(!recovered);
        assert!(repo.repo_path().exists());
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
        assert!(recover_from_path(&wt_path).is_some());
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
        assert!(recover_from_path(&unrelated).is_none());
    }

    #[test]
    fn test_recover_from_path_multi_repo_siblings() {
        let tmp = tempfile::tempdir().unwrap();
        let base = dunce::canonicalize(tmp.path()).unwrap();

        // Create two sibling repos
        let repo_a = base.join("alpha");
        let repo_b = base.join("beta");
        std::fs::create_dir(&repo_a).unwrap();
        std::fs::create_dir(&repo_b).unwrap();
        git_init(&repo_a);
        git_init(&repo_b);
        for repo in [&repo_a, &repo_b] {
            Cmd::new("git")
                .args(["commit", "--allow-empty", "-m", "init"])
                .current_dir(repo)
                .run()
                .unwrap();
        }

        // Add worktrees for both repos as siblings
        let wt_a = base.join("alpha.feature");
        let wt_b = base.join("beta.feature");
        Cmd::new("git")
            .args(["worktree", "add", &wt_a.to_string_lossy(), "-b", "feature"])
            .current_dir(&repo_a)
            .run()
            .unwrap();
        Cmd::new("git")
            .args(["worktree", "add", &wt_b.to_string_lossy(), "-b", "feature"])
            .current_dir(&repo_b)
            .run()
            .unwrap();

        // Delete beta's worktree (simulating wt merge from another terminal)
        std::fs::remove_dir_all(&wt_b).unwrap();

        // Recovery should find beta's repo, not alpha's
        let recovered = recover_from_path(&wt_b).unwrap();
        assert_eq!(dunce::canonicalize(recovered.repo_path()).unwrap(), repo_b);
    }

    #[test]
    fn test_recover_from_path_nested_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let base = dunce::canonicalize(tmp.path()).unwrap();

        let repo_dir = base.join("myrepo");
        std::fs::create_dir(&repo_dir).unwrap();
        git_init(&repo_dir);
        Cmd::new("git")
            .args(["commit", "--allow-empty", "-m", "init"])
            .current_dir(&repo_dir)
            .run()
            .unwrap();

        // Add a worktree nested under the repo
        let wt_path = repo_dir.join(".worktrees").join("feature");
        std::fs::create_dir_all(wt_path.parent().unwrap()).unwrap();
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

        // Delete the worktree
        std::fs::remove_dir_all(&wt_path).unwrap();

        // Recovery should find the repo
        assert!(recover_from_path(&wt_path).is_some());
    }

    #[test]
    fn test_recover_from_path_deep_pwd() {
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

        // Delete the worktree
        std::fs::remove_dir_all(&wt_path).unwrap();

        // Recover from a path deeper than the worktree root
        // (simulates $PWD being in a subdirectory when the worktree was removed)
        let deep_path = wt_path.join("src").join("lib.rs");
        assert!(recover_from_path(&deep_path).is_some());
    }

    #[test]
    fn test_hint_for_repo_suggests_switch() {
        // A normal repo with a main worktree should suggest `wt switch ^`.
        let tmp = tempfile::tempdir().unwrap();
        git_init(tmp.path());
        Cmd::new("git")
            .args(["commit", "--allow-empty", "-m", "init"])
            .current_dir(tmp.path())
            .run()
            .unwrap();
        let repo = Repository::at(tmp.path()).unwrap();
        let hint = hint_for_repo(&repo);
        insta::assert_snapshot!(hint.ansi_strip(), @"Current directory was removed. Try: wt switch ^");
    }

    #[test]
    fn test_hint_for_repo_fallback_to_list() {
        // A bare repo with no worktrees has no default branch worktree,
        // so it should suggest `wt list` instead of `wt switch ^`.
        let tmp = tempfile::tempdir().unwrap();
        Cmd::new("git")
            .args(["init", "--bare", "--quiet"])
            .current_dir(tmp.path())
            .run()
            .unwrap();
        let repo = Repository::at(tmp.path()).unwrap();
        let hint = hint_for_repo(&repo);
        insta::assert_snapshot!(hint.ansi_strip(), @"Current directory was removed. Run wt list to see worktrees.");
    }
}
