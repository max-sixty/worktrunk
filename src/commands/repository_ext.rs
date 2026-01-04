use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use super::worktree::{BranchDeletionMode, RemoveResult};
use anyhow::Context;
use color_print::cformat;
use worktrunk::git::{
    GitError, IntegrationReason, Repository, parse_porcelain_z, parse_untracked_files,
};
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{format_with_gutter, progress_message, warning_message};

/// Target for worktree removal.
#[derive(Debug)]
pub enum RemoveTarget<'a> {
    /// Remove worktree by branch name
    Branch(&'a str),
    /// Remove the current worktree (supports detached HEAD)
    Current,
}

/// CLI-only helpers implemented on [`Repository`] via an extension trait so we can keep orphan
/// implementations inside the binary crate.
pub trait RepositoryCliExt {
    /// Warn about untracked files being auto-staged.
    fn warn_if_auto_staging_untracked(&self) -> anyhow::Result<()>;

    /// Prepare a worktree removal by branch name or current worktree.
    ///
    /// Returns a `RemoveResult` describing what will be removed. The actual
    /// removal is performed by the output handler.
    fn prepare_worktree_removal(
        &self,
        target: RemoveTarget,
        deletion_mode: BranchDeletionMode,
        force_worktree: bool,
    ) -> anyhow::Result<RemoveResult>;

    /// Prepare the target worktree for push by auto-stashing non-overlapping changes when safe.
    fn prepare_target_worktree(
        &self,
        target_worktree: Option<&PathBuf>,
        target_branch: &str,
    ) -> anyhow::Result<Option<TargetWorktreeStash>>;

    /// Check if HEAD is a linear extension of the target branch.
    ///
    /// Returns true when:
    /// 1. The merge-base equals target's SHA (target hasn't advanced), AND
    /// 2. There are no merge commits between target and HEAD (history is linear)
    ///
    /// This detects branches that have merged the target into themselves — such
    /// branches need rebasing to linearize history even though merge-base equals target.
    fn is_rebased_onto(&self, target: &str) -> anyhow::Result<bool>;
}

impl RepositoryCliExt for Repository {
    fn warn_if_auto_staging_untracked(&self) -> anyhow::Result<()> {
        // Use -z for NUL-separated output to handle filenames with spaces/newlines
        let status = self
            .run_command(&["status", "--porcelain", "-z"])
            .context("Failed to get status")?;
        warn_about_untracked_files(&status)
    }

    fn prepare_worktree_removal(
        &self,
        target: RemoveTarget,
        deletion_mode: BranchDeletionMode,
        force_worktree: bool,
    ) -> anyhow::Result<RemoveResult> {
        let current_path = self.worktree_root()?.to_path_buf();
        let worktrees = self.list_worktrees()?;
        // Home worktree: prefer default branch's worktree, fall back to first worktree,
        // then repo base for bare repos with no worktrees.
        let home_worktree_path = self.home_path()?;

        // Resolve target to worktree path and branch
        let (worktree_path, branch_name, is_current) = match target {
            RemoveTarget::Branch(branch) => {
                match worktrees
                    .iter()
                    .find(|wt| wt.branch.as_deref() == Some(branch))
                {
                    Some(wt) => {
                        if !wt.path.exists() {
                            return Err(GitError::WorktreeMissing {
                                branch: branch.into(),
                            }
                            .into());
                        }
                        if wt.locked.is_some() {
                            return Err(GitError::WorktreeLocked {
                                branch: branch.into(),
                                reason: wt.locked.clone(),
                            }
                            .into());
                        }
                        let is_current = current_path == wt.path;
                        (wt.path.clone(), Some(branch.to_string()), is_current)
                    }
                    None => {
                        // No worktree found - check if the branch exists locally
                        if self.local_branch_exists(branch)? {
                            return Ok(RemoveResult::BranchOnly {
                                branch_name: branch.to_string(),
                                deletion_mode,
                            });
                        }
                        // Check if branch exists on a remote
                        let remotes = self.remotes_with_branch(branch)?;
                        if !remotes.is_empty() {
                            return Err(GitError::RemoteOnlyBranch {
                                branch: branch.into(),
                                remote: remotes[0].clone(),
                            }
                            .into());
                        }
                        return Err(GitError::NoWorktreeFound {
                            branch: branch.into(),
                        }
                        .into());
                    }
                }
            }
            RemoveTarget::Current => {
                let wt = worktrees
                    .iter()
                    .find(|wt| wt.path == current_path)
                    .ok_or_else(|| {
                        anyhow::anyhow!("Current worktree not found in worktree list")
                    })?;
                if wt.locked.is_some() {
                    // Use branch name if available, otherwise use directory name
                    let name = wt
                        .branch
                        .clone()
                        .unwrap_or_else(|| wt.dir_name().to_string());
                    return Err(GitError::WorktreeLocked {
                        branch: name,
                        reason: wt.locked.clone(),
                    }
                    .into());
                }
                (wt.path.clone(), wt.branch.clone(), true)
            }
        };

        // Create Repository at target for validation
        let target_repo = Repository::at(&worktree_path);

        // Cannot remove the main working tree (only linked worktrees can be removed)
        if !target_repo.is_in_worktree()? {
            return Err(GitError::CannotRemoveMainWorktree.into());
        }

        // Ensure the working tree is clean
        target_repo.ensure_clean_working_tree("remove worktree", branch_name.as_deref())?;

        // Compute main_path and changed_directory based on whether we're removing current
        let (main_path, changed_directory) = if is_current {
            (home_worktree_path, true)
        } else {
            (current_path, false)
        };

        // Resolve target branch for integration reason display
        // Skip if removing the default branch itself (avoids tautological "main (ancestor of main)")
        // Use .ok() to treat errors as unknown - safer than empty string for integration checks
        let default_branch = self.default_branch().ok();
        let target_branch = match (&default_branch, &branch_name) {
            (Some(db), Some(bn)) if db == bn => None,
            _ => default_branch,
        };

        // Pre-compute integration reason to avoid race conditions when removing
        // multiple worktrees in background mode.
        let integration_reason = compute_integration_reason(
            &main_path,
            branch_name.as_deref(),
            target_branch.as_deref(),
            deletion_mode,
        );

        Ok(RemoveResult::RemovedWorktree {
            main_path,
            worktree_path,
            changed_directory,
            branch_name,
            deletion_mode,
            target_branch,
            integration_reason,
            force_worktree,
        })
    }

    fn prepare_target_worktree(
        &self,
        target_worktree: Option<&PathBuf>,
        target_branch: &str,
    ) -> anyhow::Result<Option<TargetWorktreeStash>> {
        let Some(wt_path) = target_worktree else {
            return Ok(None);
        };

        let wt_repo = Repository::at(wt_path);
        if !wt_repo.is_dirty()? {
            return Ok(None);
        }

        let push_files = self.changed_files(target_branch, "HEAD")?;
        // Use -z for NUL-separated output: handles filenames with spaces and renames correctly
        // Format: "XY path\0" for normal files, "XY new_path\0old_path\0" for renames/copies
        let wt_status_output = wt_repo.run_command(&["status", "--porcelain", "-z"])?;

        let wt_files: Vec<String> = parse_porcelain_z(&wt_status_output);

        let overlapping: Vec<String> = push_files
            .iter()
            .filter(|f| wt_files.contains(f))
            .cloned()
            .collect();

        if !overlapping.is_empty() {
            return Err(GitError::ConflictingChanges {
                target_branch: target_branch.to_string(),
                files: overlapping,
                worktree_path: wt_path.clone(),
            }
            .into());
        }

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let stash_name = format!(
            "worktrunk autostash::{}::{}::{}",
            target_branch,
            process::id(),
            nanos
        );

        crate::output::print(progress_message(cformat!(
            "Stashing changes in <bold>{}</>...",
            format_path_for_display(wt_path)
        )))?;

        let stash_output =
            wt_repo.run_command(&["stash", "push", "--include-untracked", "-m", &stash_name])?;

        if stash_output.contains("No local changes to save") {
            return Ok(None);
        }

        let list_output = wt_repo.run_command(&["stash", "list", "--format=%gd%x00%gs%x00"])?;
        let mut parts = list_output.split('\0');
        let mut stash_ref = None;
        while let Some(id) = parts.next() {
            if id.is_empty() {
                continue;
            }
            if let Some(message) = parts.next()
                && (message == stash_name || message.ends_with(&stash_name))
            {
                stash_ref = Some(id.to_string());
                break;
            }
        }

        let Some(stash_ref) = stash_ref else {
            return Err(anyhow::anyhow!(
                "Failed to locate autostash entry '{}'",
                stash_name
            ));
        };

        Ok(Some(TargetWorktreeStash::new(wt_path, stash_ref)))
    }

    fn is_rebased_onto(&self, target: &str) -> anyhow::Result<bool> {
        let merge_base = self.merge_base("HEAD", target)?;
        let target_sha = self.run_command(&["rev-parse", target])?.trim().to_string();

        if merge_base != target_sha {
            return Ok(false); // Target has advanced past merge-base
        }

        // Check for merge commits — if present, history is not linear
        let merge_commits = self
            .run_command(&["rev-list", "--merges", &format!("{}..HEAD", target)])?
            .trim()
            .to_string();

        Ok(merge_commits.is_empty())
    }
}

/// Compute integration reason for branch deletion.
///
/// Returns `None` if:
/// - `deletion_mode` is `ForceDelete` (skip integration check)
/// - `branch_name` is `None` (detached HEAD)
/// - `target_branch` is `None` (no target to check against)
/// - Branch is not integrated into target (safe deletion not confirmed)
///
/// Note: Integration is computed even for `Keep` mode so we can inform the user
/// if the flag had an effect (branch was integrated) or not (branch was unmerged).
fn compute_integration_reason(
    main_path: &Path,
    branch_name: Option<&str>,
    target_branch: Option<&str>,
    deletion_mode: BranchDeletionMode,
) -> Option<IntegrationReason> {
    // Skip for force delete (we'll delete regardless of integration status)
    // But compute for keep mode so we can inform user if the flag had no effect
    if deletion_mode.is_force() {
        return None;
    }
    let (branch, target) = branch_name.zip(target_branch)?;
    let main_repo = Repository::at(main_path);
    let effective_target = main_repo.effective_integration_target(target);
    let mut provider =
        worktrunk::git::LazyGitIntegration::new(&main_repo, branch, &effective_target);
    worktrunk::git::check_integration(&mut provider)
}

/// Warn about untracked files that will be auto-staged.
fn warn_about_untracked_files(status_output: &str) -> anyhow::Result<()> {
    let files = parse_untracked_files(status_output);
    if files.is_empty() {
        return Ok(());
    }

    let count = files.len();
    let path_word = if count == 1 { "path" } else { "paths" };
    crate::output::print(warning_message(format!(
        "Auto-staging {count} untracked {path_word}:"
    )))?;

    let joined_files = files.join("\n");
    crate::output::print(format_with_gutter(&joined_files, None))?;

    Ok(())
}

/// Stash guard that auto-restores on drop.
///
/// Created by `prepare_target_worktree()` when the target worktree has changes
/// that don't conflict with the push. Automatically restores the stash when
/// dropped, ensuring cleanup happens in both success and error paths.
#[must_use = "stash guard restores immediately if dropped; hold it until push completes"]
pub(crate) struct TargetWorktreeStash {
    /// Inner data wrapped in Option so we can take() in Drop.
    /// None means already restored (or disarmed).
    inner: Option<StashData>,
}

struct StashData {
    repo: Repository,
    path: PathBuf,
    stash_ref: String,
}

impl StashData {
    /// Restore the stash, printing progress and warning on failure.
    fn restore(self) {
        let _ = crate::output::print(progress_message(cformat!(
            "Restoring stashed changes in <bold>{}</>...",
            format_path_for_display(&self.path)
        )));

        if let Err(_e) = self
            .repo
            .run_command(&["stash", "pop", "--quiet", &self.stash_ref])
        {
            let _ = crate::output::print(warning_message(cformat!(
                "Failed to restore stash <bold>{stash_ref}</> - run <bold>git stash pop {stash_ref}</> in <bold>{path}</>",
                stash_ref = self.stash_ref,
                path = format_path_for_display(&self.path),
            )));
        }
    }
}

impl Drop for TargetWorktreeStash {
    fn drop(&mut self) {
        if let Some(data) = self.inner.take() {
            data.restore();
        }
    }
}

impl TargetWorktreeStash {
    pub(crate) fn new(path: &Path, stash_ref: String) -> Self {
        Self {
            inner: Some(StashData {
                repo: Repository::at(path),
                path: path.to_path_buf(),
                stash_ref,
            }),
        }
    }

    /// Explicitly restore the stash now, preventing Drop from restoring again.
    ///
    /// Use this when you need the restore to happen at a specific point
    /// (e.g., before a success message). Drop handles errors/early returns.
    pub(crate) fn restore_now(&mut self) {
        if let Some(data) = self.inner.take() {
            data.restore();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_porcelain_z_modified_staged() {
        // "M  file.txt\0" - staged modification
        let output = "M  file.txt\0";
        assert_eq!(parse_porcelain_z(output), vec!["file.txt"]);
    }

    #[test]
    fn test_parse_porcelain_z_modified_unstaged() {
        // " M file.txt\0" - unstaged modification (this was the bug case)
        let output = " M file.txt\0";
        assert_eq!(parse_porcelain_z(output), vec!["file.txt"]);
    }

    #[test]
    fn test_parse_porcelain_z_modified_both() {
        // "MM file.txt\0" - both staged and unstaged
        let output = "MM file.txt\0";
        assert_eq!(parse_porcelain_z(output), vec!["file.txt"]);
    }

    #[test]
    fn test_parse_porcelain_z_untracked() {
        // "?? new.txt\0" - untracked file
        let output = "?? new.txt\0";
        assert_eq!(parse_porcelain_z(output), vec!["new.txt"]);
    }

    #[test]
    fn test_parse_porcelain_z_rename() {
        // "R  new.txt\0old.txt\0" - rename includes both paths
        let output = "R  new.txt\0old.txt\0";
        let result = parse_porcelain_z(output);
        assert_eq!(result, vec!["new.txt", "old.txt"]);
    }

    #[test]
    fn test_parse_porcelain_z_copy() {
        // "C  copy.txt\0original.txt\0" - copy includes both paths
        let output = "C  copy.txt\0original.txt\0";
        let result = parse_porcelain_z(output);
        assert_eq!(result, vec!["copy.txt", "original.txt"]);
    }

    #[test]
    fn test_parse_porcelain_z_multiple_files() {
        // Multiple files with different statuses
        let output = " M file1.txt\0M  file2.txt\0?? untracked.txt\0R  new.txt\0old.txt\0";
        let result = parse_porcelain_z(output);
        assert_eq!(
            result,
            vec![
                "file1.txt",
                "file2.txt",
                "untracked.txt",
                "new.txt",
                "old.txt"
            ]
        );
    }

    #[test]
    fn test_parse_porcelain_z_filename_with_spaces() {
        // "M  file with spaces.txt\0"
        let output = "M  file with spaces.txt\0";
        assert_eq!(parse_porcelain_z(output), vec!["file with spaces.txt"]);
    }

    #[test]
    fn test_parse_porcelain_z_empty() {
        assert_eq!(parse_porcelain_z(""), Vec::<String>::new());
    }

    #[test]
    fn test_parse_porcelain_z_short_entry_skipped() {
        // Entry too short to have path (malformed, shouldn't happen in practice)
        let output = "M\0";
        assert_eq!(parse_porcelain_z(output), Vec::<String>::new());
    }

    #[test]
    fn test_parse_porcelain_z_rename_missing_old_path() {
        // Rename without old path (malformed, but should handle gracefully)
        let output = "R  new.txt\0";
        let result = parse_porcelain_z(output);
        // Should include new.txt, old path is simply not added
        assert_eq!(result, vec!["new.txt"]);
    }

    #[test]
    fn test_parse_untracked_files_single() {
        assert_eq!(parse_untracked_files("?? new.txt\0"), vec!["new.txt"]);
    }

    #[test]
    fn test_parse_untracked_files_multiple() {
        assert_eq!(
            parse_untracked_files("?? file1.txt\0?? file2.txt\0?? file3.txt\0"),
            vec!["file1.txt", "file2.txt", "file3.txt"]
        );
    }

    #[test]
    fn test_parse_untracked_files_ignores_modified() {
        // Only untracked files should be collected
        assert_eq!(
            parse_untracked_files(" M modified.txt\0?? untracked.txt\0"),
            vec!["untracked.txt"]
        );
    }

    #[test]
    fn test_parse_untracked_files_ignores_staged() {
        assert_eq!(
            parse_untracked_files("M  staged.txt\0?? untracked.txt\0"),
            vec!["untracked.txt"]
        );
    }

    #[test]
    fn test_parse_untracked_files_empty() {
        assert!(parse_untracked_files("").is_empty());
    }

    #[test]
    fn test_parse_untracked_files_skips_rename_old_path() {
        // Rename entries have old path as second NUL-separated field
        // Should only have untracked file, not the rename paths
        assert_eq!(
            parse_untracked_files("R  new.txt\0old.txt\0?? untracked.txt\0"),
            vec!["untracked.txt"]
        );
    }

    #[test]
    fn test_parse_untracked_files_with_spaces() {
        assert_eq!(
            parse_untracked_files("?? file with spaces.txt\0"),
            vec!["file with spaces.txt"]
        );
    }

    #[test]
    fn test_parse_untracked_files_no_untracked() {
        // All files are tracked (modified, staged, etc.)
        assert!(parse_untracked_files(" M file1.txt\0M  file2.txt\0").is_empty());
    }

    #[test]
    fn test_stash_guard_restore_now_clears_inner() {
        // Create a guard - note: this doesn't actually create a stash since we're not
        // in a real git repo with that stash ref. We're just testing the state machine.
        let mut guard = TargetWorktreeStash::new(std::path::Path::new("/tmp"), "stash@{0}".into());

        // Inner should be populated
        assert!(guard.inner.is_some());

        // restore_now() should clear inner (the restore itself will fail since no real repo,
        // but that's expected - we're testing the state transition)
        guard.restore_now();

        // Inner should now be None
        assert!(guard.inner.is_none());

        // Calling restore_now() again is a no-op
        guard.restore_now();
        assert!(guard.inner.is_none());
    }

    #[test]
    fn test_stash_guard_drop_clears_inner() {
        // Test that Drop also consumes the inner
        let guard = TargetWorktreeStash::new(std::path::Path::new("/tmp"), "stash@{0}".into());

        // Just drop it - the restore will fail (no real repo) but Drop shouldn't panic
        drop(guard);
        // If we get here, Drop worked without panicking
    }
}
