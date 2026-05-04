//! `wt step promote` — swap a branch into the main worktree.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::Context;
use color_print::cformat;
use path_slash::PathExt as _;
use worktrunk::copy::{copy_dir_recursive, copy_leaf};
use worktrunk::git::Repository;
use worktrunk::progress::Progress;
use worktrunk::styling::{eprintln, hint_message, info_message, success_message, warning_message};

use super::shared::list_and_filter_ignored_entries;

/// Move a file or directory, falling back to copy+delete on cross-device errors.
fn move_entry(src: &Path, dest: &Path, is_dir: bool) -> anyhow::Result<()> {
    // Ensure parent directory exists
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .context(format!("creating parent directory for {}", dest.display()))?;
    }

    match fs::rename(src, dest) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::CrossesDevices => copy_and_remove(src, dest, is_dir),
        Err(e) => Err(anyhow::Error::from(e).context(format!(
            "moving {} to {}",
            src.display(),
            dest.display()
        ))),
    }
}

/// Copy then delete — fallback when `rename` fails with EXDEV (cross-device).
fn copy_and_remove(src: &Path, dest: &Path, is_dir: bool) -> anyhow::Result<()> {
    if is_dir {
        copy_dir_recursive(src, dest, None, true, &Progress::disabled())?;
        fs::remove_dir_all(src).context(format!("removing source directory {}", src.display()))?;
    } else {
        copy_leaf(src, dest, None, true)?;

        fs::remove_file(src).context(format!("removing source file {}", src.display()))?;
    }
    Ok(())
}

const PROMOTE_STAGING_DIR: &str = "staging/promote";

/// Move gitignored files from both worktrees into a staging directory.
///
/// Called BEFORE the branch exchange because `git switch` silently overwrites
/// ignored files that collide with tracked files on the target branch.
///
/// Returns the staging directory path and the count of entries staged.
fn stage_ignored(
    repo: &Repository,
    path_a: &Path,
    entries_a: &[(PathBuf, bool)],
    path_b: &Path,
    entries_b: &[(PathBuf, bool)],
) -> anyhow::Result<(PathBuf, usize)> {
    let staging_dir = repo.wt_dir().join(PROMOTE_STAGING_DIR);
    fs::create_dir_all(&staging_dir).context("creating promote staging directory")?;

    let staging_a = staging_dir.join("a");
    let staging_b = staging_dir.join("b");
    let mut count = 0;

    // Move A's entries → staging/a
    for (src_entry, is_dir) in entries_a {
        let relative = src_entry
            .strip_prefix(path_a)
            .context("entry not under worktree A")?;
        let staging_entry = staging_a.join(relative);
        if fs::symlink_metadata(src_entry).is_ok() {
            move_entry(src_entry, &staging_entry, *is_dir)
                .context(format!("staging {}", relative.display()))?;
            count += 1;
        }
    }

    // Move B's entries → staging/b
    for (src_entry, is_dir) in entries_b {
        let relative = src_entry
            .strip_prefix(path_b)
            .context("entry not under worktree B")?;
        let staging_entry = staging_b.join(relative);
        if fs::symlink_metadata(src_entry).is_ok() {
            move_entry(src_entry, &staging_entry, *is_dir)
                .context(format!("staging {}", relative.display()))?;
            count += 1;
        }
    }

    // Clean up empty staging directory (can happen if all entries vanished
    // between listing and staging due to TOCTOU)
    if count == 0 && staging_dir.exists() {
        let _ = fs::remove_dir_all(&staging_dir);
    }

    Ok((staging_dir, count))
}

/// Distribute staged files to their new worktrees after a branch exchange.
///
/// B's original files (in staging/b) go to worktree A (which now has B's branch).
/// A's original files (in staging/a) go to worktree B (which now has A's branch).
fn distribute_staged(
    staging_dir: &Path,
    path_a: &Path,
    entries_a: &[(PathBuf, bool)],
    path_b: &Path,
    entries_b: &[(PathBuf, bool)],
) -> anyhow::Result<usize> {
    let staging_a = staging_dir.join("a");
    let staging_b = staging_dir.join("b");
    let mut count = 0;

    // Move B's staged entries → A (A now has B's branch)
    for (src_entry, is_dir) in entries_b {
        let relative = src_entry
            .strip_prefix(path_b)
            .context("entry not under worktree B")?;
        let staging_entry = staging_b.join(relative);
        let dest_entry = path_a.join(relative);
        if fs::symlink_metadata(&staging_entry).is_ok() {
            move_entry(&staging_entry, &dest_entry, *is_dir)
                .context(format!("distributing {}", relative.display()))?;
            count += 1;
        }
    }

    // Move A's staged entries → B (B now has A's branch)
    for (src_entry, is_dir) in entries_a {
        let relative = src_entry
            .strip_prefix(path_a)
            .context("entry not under worktree A")?;
        let staging_entry = staging_a.join(relative);
        let dest_entry = path_b.join(relative);
        if fs::symlink_metadata(&staging_entry).is_ok() {
            move_entry(&staging_entry, &dest_entry, *is_dir)
                .context(format!("distributing {}", relative.display()))?;
            count += 1;
        }
    }

    // Clean up staging directory (best-effort — files are already distributed)
    let _ = fs::remove_dir_all(staging_dir);

    Ok(count)
}

/// Result of a promote operation
pub enum PromoteResult {
    /// Branch was promoted successfully
    Promoted,
    /// Already in canonical state (requested branch is already in main)
    AlreadyInMain(String),
}

/// Exchange branches between two worktrees.
///
/// Steps: detach target → detach main → switch main → switch target.
/// Both worktrees must be clean (verified by caller). On failure, attempts
/// best-effort rollback — but failure here is near-impossible given the
/// preconditions (`ensure_clean` passed, branches exist, detach released locks).
fn exchange_branches(
    main_wt: &worktrunk::git::WorkingTree<'_>,
    main_branch: &str,
    target_wt: &worktrunk::git::WorkingTree<'_>,
    target_branch: &str,
) -> anyhow::Result<()> {
    let steps: &[(&worktrunk::git::WorkingTree<'_>, &[&str], &str)] = &[
        (target_wt, &["switch", "--detach"], "detach target"),
        (main_wt, &["switch", "--detach"], "detach main"),
        (main_wt, &["switch", target_branch], "switch main"),
        (target_wt, &["switch", main_branch], "switch target"),
    ];

    for (wt, args, label) in steps {
        if let Err(e) = wt.run_command(args) {
            // Best-effort rollback: try to re-attach both branches.
            let _ = main_wt.run_command(&["switch", main_branch]);
            let _ = target_wt.run_command(&["switch", target_branch]);
            return Err(e.context(format!("branch exchange failed at: {label}")));
        }
    }

    Ok(())
}

/// Handle `wt step promote` command
///
/// Promotes a branch to the main worktree, exchanging it with whatever branch is currently there.
///
/// ## Interruption recovery
///
/// The swap uses a staging directory at `.git/wt/staging/promote/` and proceeds
/// in three phases:
///
/// 1. **Stage**: move ignored files from both worktrees into staging (`a/`, `b/`)
/// 2. **Exchange**: detach + `git switch` to swap branches
/// 3. **Distribute**: move staged files to their new worktrees, then delete staging
///
/// A hard kill at any phase leaves files in staging, never deleted. The next run
/// detects the leftover directory and bails with a recovery path. A kill during
/// `git switch` may leave a worktree detached (fix: `git switch <branch>`).
pub fn handle_promote(branch: Option<&str>) -> anyhow::Result<PromoteResult> {
    use worktrunk::git::GitError;

    let repo = Repository::current()?;
    let worktrees = repo.list_worktrees()?;

    if worktrees.is_empty() {
        anyhow::bail!("No worktrees found");
    }

    // For normal repos, worktrees[0] is the main worktree
    // For bare repos, there's no main worktree - we don't support promote there
    if repo.is_bare()? {
        anyhow::bail!("wt step promote is not supported in bare repositories");
    }

    let main_wt = &worktrees[0];
    let main_path = &main_wt.path;
    let main_branch = main_wt
        .branch
        .clone()
        .ok_or_else(|| GitError::DetachedHead {
            action: Some("promote".into()),
        })?;

    // Resolve the branch to promote (default_branch computed lazily, only when needed)
    let target_branch = match branch {
        Some(b) => b.to_string(),
        None => {
            let current_wt = repo.current_worktree();
            if !current_wt.is_linked()? {
                // From main worktree with no args: restore default branch
                repo.default_branch()
                    .ok_or_else(|| anyhow::anyhow!("Could not determine default branch"))?
            } else {
                // From other worktree with no args: promote current branch
                current_wt.branch()?.ok_or_else(|| GitError::DetachedHead {
                    action: Some("promote".into()),
                })?
            }
        }
    };

    // Check if target is already in main worktree
    if target_branch == main_branch {
        return Ok(PromoteResult::AlreadyInMain(target_branch));
    }

    // Find the worktree with the target branch
    let target_wt = worktrees
        .iter()
        .skip(1) // Skip main worktree
        .find(|wt| wt.branch.as_deref() == Some(&target_branch))
        .ok_or_else(|| GitError::WorktreeNotFound {
            branch: target_branch.clone(),
        })?;

    let target_path = &target_wt.path;

    // Bail early if a leftover staging dir exists from a previous interrupted promote —
    // it may contain the user's only copy of files from the failed swap.
    // Check BEFORE ensure_clean so users see the recovery path first.
    let staging_path = repo.wt_dir().join(PROMOTE_STAGING_DIR);
    if staging_path.exists() {
        let display = staging_path.to_slash_lossy();
        return Err(anyhow::anyhow!(
            "Files may need manual recovery from: {display}\n\
             Remove it to retry: rm -rf \"{display}\""
        )
        .context("Found leftover staging directory from an interrupted promote"));
    }

    // Ensure both worktrees are clean
    let main_working_tree = repo.worktree_at(main_path);
    let target_working_tree = repo.worktree_at(target_path);

    main_working_tree.ensure_clean("promote", Some(&main_branch), false)?;
    target_working_tree.ensure_clean("promote", Some(&target_branch), false)?;

    // Check if we're restoring canonical state (promoting default branch back to main worktree)
    // Only lookup default_branch if needed for messaging (already resolved if no-arg from main)
    let default_branch = repo.default_branch();
    let is_restoring = default_branch.as_ref() == Some(&target_branch);

    if is_restoring {
        // Restoring default branch to main worktree - no warning needed
        eprintln!("{}", info_message("Restoring main worktree"));
    } else {
        // Creating mismatch - show warning and how to restore
        eprintln!(
            "{}",
            warning_message("Promoting creates mismatched worktree state (shown as ⚑ in wt list)",)
        );
        // Only show restore hint if we know the default branch
        if let Some(default) = &default_branch {
            eprintln!(
                "{}",
                hint_message(cformat!(
                    "Run <underline>wt step promote {default}</> to restore canonical locations"
                ))
            );
        }
    }

    // Discover gitignored entries BEFORE branch exchange — .gitignore rules belong
    // to the current branch and will change after `git switch`.
    let worktree_paths: Vec<PathBuf> = worktrees.iter().map(|wt| wt.path.clone()).collect();
    let no_excludes: &[String] = &[];
    let main_entries =
        list_and_filter_ignored_entries(main_path, &main_branch, &worktree_paths, no_excludes)?;
    let target_entries =
        list_and_filter_ignored_entries(target_path, &target_branch, &worktree_paths, no_excludes)?;

    // Move gitignored files to staging BEFORE branch exchange.
    // `git switch` silently overwrites ignored files that collide with tracked
    // files on the target branch — staging them first prevents data loss.
    let staged = if !main_entries.is_empty() || !target_entries.is_empty() {
        let (dir, count) = stage_ignored(
            &repo,
            main_path,
            &main_entries,
            target_path,
            &target_entries,
        )
        .context(format!(
            "Failed to stage ignored files. Already-staged files may be recoverable from: {}",
            staging_path.to_slash_lossy()
        ))?;
        if count > 0 { Some((dir, count)) } else { None }
    } else {
        None
    };

    // Exchange branches (detach both, then switch to swapped branches).
    // Failure is near-impossible (both worktrees verified clean, branches exist).
    // If it somehow fails, stale staging detection recovers on next run.
    exchange_branches(
        &main_working_tree,
        &main_branch,
        &target_working_tree,
        &target_branch,
    )?;

    // Distribute staged files to their new worktrees (after branch exchange)
    let swapped = if let Some((ref staging_dir, _)) = staged {
        distribute_staged(
            staging_dir,
            main_path,
            &main_entries,
            target_path,
            &target_entries,
        )
        .context(format!(
            "Failed to distribute staged files. Staged files may be recoverable from: {}",
            staging_dir.display()
        ))?
    } else {
        0
    };

    // Print success messages only after everything succeeded
    eprintln!(
        "{}",
        success_message(cformat!(
            "Promoted: main worktree now has <bold>{target_branch}</>; {} now has <bold>{main_branch}</>",
            worktrunk::path::format_path_for_display(target_path)
        ))
    );
    if swapped > 0 {
        let path_word = if swapped == 1 { "path" } else { "paths" };
        eprintln!(
            "{}",
            success_message(format!("Swapped {swapped} gitignored {path_word}"))
        );
    }

    Ok(PromoteResult::Promoted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_move_entry_file() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("source.txt");
        let dest = tmp.path().join("subdir/dest.txt");

        fs::write(&src, "content").unwrap();
        move_entry(&src, &dest, false).unwrap();

        assert!(!src.exists());
        assert_eq!(fs::read_to_string(&dest).unwrap(), "content");
    }

    #[test]
    fn test_move_entry_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("srcdir");
        let dest = tmp.path().join("nested/destdir");

        fs::create_dir_all(src.join("inner")).unwrap();
        fs::write(src.join("inner/file.txt"), "nested").unwrap();
        fs::write(src.join("root.txt"), "root").unwrap();

        move_entry(&src, &dest, true).unwrap();

        assert!(!src.exists());
        assert_eq!(
            fs::read_to_string(dest.join("inner/file.txt")).unwrap(),
            "nested"
        );
        assert_eq!(fs::read_to_string(dest.join("root.txt")).unwrap(), "root");
    }

    #[test]
    fn test_copy_and_remove_file() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("source.txt");
        let dest = tmp.path().join("dest.txt");

        fs::write(&src, "content").unwrap();
        copy_and_remove(&src, &dest, false).unwrap();

        assert!(!src.exists());
        assert_eq!(fs::read_to_string(&dest).unwrap(), "content");
    }

    #[test]
    fn test_copy_and_remove_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("srcdir");
        let dest = tmp.path().join("destdir");

        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("sub/file.txt"), "nested").unwrap();
        fs::write(src.join("root.txt"), "root").unwrap();

        copy_and_remove(&src, &dest, true).unwrap();

        assert!(!src.exists());
        assert_eq!(
            fs::read_to_string(dest.join("sub/file.txt")).unwrap(),
            "nested"
        );
        assert_eq!(fs::read_to_string(dest.join("root.txt")).unwrap(), "root");
    }
}
