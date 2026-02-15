use super::worktree::{BranchDeletionMode, RemoveResult, get_path_mismatch};
use anyhow::Context;
use worktrunk::config::UserConfig;
use worktrunk::git::{GitError, IntegrationReason, Repository, parse_untracked_files};
use worktrunk::styling::{eprintln, format_with_gutter, warning_message};

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
    ///
    /// The `config` parameter is used to compute the expected worktree path
    /// for path mismatch detection.
    fn prepare_worktree_removal(
        &self,
        target: RemoveTarget,
        deletion_mode: BranchDeletionMode,
        force_worktree: bool,
        config: &UserConfig,
    ) -> anyhow::Result<RemoveResult>;
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
        config: &UserConfig,
    ) -> anyhow::Result<RemoveResult> {
        let current_path = self.current_worktree().root()?.to_path_buf();
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
                            // Directory missing - prune and continue
                            self.prune_worktrees()?;
                            return Ok(RemoveResult::BranchOnly {
                                branch_name: branch.to_string(),
                                deletion_mode,
                                pruned: true,
                            });
                        }
                        if wt.locked.is_some() {
                            return Err(GitError::WorktreeLocked {
                                branch: branch.into(),
                                path: wt.path.clone(),
                                reason: wt.locked.clone(),
                            }
                            .into());
                        }
                        let is_current = current_path == wt.path;
                        (wt.path.clone(), Some(branch.to_string()), is_current)
                    }
                    None => {
                        // No worktree found - check if the branch exists locally
                        let branch_handle = self.branch(branch);
                        if branch_handle.exists_locally()? {
                            return Ok(RemoveResult::BranchOnly {
                                branch_name: branch.to_string(),
                                deletion_mode,
                                pruned: false,
                            });
                        }
                        // Check if branch exists on a remote
                        let remotes = branch_handle.remotes()?;
                        if !remotes.is_empty() {
                            return Err(GitError::RemoteOnlyBranch {
                                branch: branch.into(),
                                remote: remotes[0].clone(),
                            }
                            .into());
                        }
                        return Err(GitError::BranchNotFound {
                            branch: branch.into(),
                            show_create_hint: false,
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
                        path: wt.path.clone(),
                        reason: wt.locked.clone(),
                    }
                    .into());
                }
                (wt.path.clone(), wt.branch.clone(), true)
            }
        };

        // Cannot remove the main working tree (only linked worktrees can be removed)
        let target_wt = self.worktree_at(&worktree_path);
        if !target_wt.is_linked()? {
            return Err(GitError::CannotRemoveMainWorktree.into());
        }

        // Check working tree cleanliness (unless --force, which passes through to git)
        if !force_worktree {
            target_wt.ensure_clean("remove worktree", branch_name.as_deref(), true)?;
        }

        // Compute main_path and changed_directory based on whether we're removing current
        let (main_path, changed_directory) = if is_current {
            (home_worktree_path, true)
        } else {
            (current_path, false)
        };

        // Resolve target branch for integration reason display
        // Skip if removing the default branch itself (avoids tautological "main (ancestor of main)")
        let default_branch = self.default_branch();
        let target_branch = match (&default_branch, &branch_name) {
            (Some(db), Some(bn)) if db == bn => None,
            _ => default_branch,
        };

        // Pre-compute integration reason to avoid race conditions when removing
        // multiple worktrees in background mode.
        let integration_reason = compute_integration_reason(
            self,
            branch_name.as_deref(),
            target_branch.as_deref(),
            deletion_mode,
        );

        // Compute expected_path for path mismatch detection
        // Only set if actual path differs from expected (path mismatch)
        let expected_path = branch_name
            .as_ref()
            .and_then(|branch| get_path_mismatch(self, branch, &worktree_path, config));

        // Capture commit SHA before removal for post-remove hook template variables.
        // This ensures {{ commit }} references the removed worktree's state.
        let removed_commit = target_wt
            .run_command(&["rev-parse", "HEAD"])
            .ok()
            .map(|s| s.trim().to_string());

        Ok(RemoveResult::RemovedWorktree {
            main_path,
            worktree_path,
            changed_directory,
            branch_name,
            deletion_mode,
            target_branch,
            integration_reason,
            force_worktree,
            expected_path,
            removed_commit,
        })
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
    repo: &Repository,
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
    // On error, return None (informational only)
    let (_, reason) = repo.integration_reason(branch, target).ok()?;
    reason
}

/// Warn about untracked files that will be auto-staged.
fn warn_about_untracked_files(status_output: &str) -> anyhow::Result<()> {
    let files = parse_untracked_files(status_output);
    if files.is_empty() {
        return Ok(());
    }

    let count = files.len();
    let path_word = if count == 1 { "path" } else { "paths" };
    eprintln!(
        "{}",
        warning_message(format!("Auto-staging {count} untracked {path_word}:"))
    );

    let joined_files = files.join("\n");
    eprintln!("{}", format_with_gutter(&joined_files, None));

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use worktrunk::git::parse_porcelain_z;

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
}
