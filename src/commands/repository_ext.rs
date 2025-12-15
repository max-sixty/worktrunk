use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use super::worktree::RemoveResult;
use anyhow::Context;
use color_print::cformat;
use worktrunk::config::ProjectConfig;
use worktrunk::git::{GitError, Repository};
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{format_with_gutter, progress_message, warning_message};

/// CLI-only helpers implemented on [`Repository`] via an extension trait so we can keep orphan
/// implementations inside the binary crate.
pub trait RepositoryCliExt {
    /// Load the project configuration if it exists.
    fn load_project_config(&self) -> anyhow::Result<Option<ProjectConfig>>;

    /// Load the project configuration, emitting a helpful hint if missing.
    fn require_project_config(&self) -> anyhow::Result<ProjectConfig>;

    /// Warn about untracked files being auto-staged.
    fn warn_if_auto_staging_untracked(&self) -> anyhow::Result<()>;

    /// Remove a worktree identified by branch name.
    fn remove_worktree_by_name(
        &self,
        branch_name: &str,
        no_delete_branch: bool,
        force_delete: bool,
    ) -> anyhow::Result<RemoveResult>;

    /// Remove the current worktree (handles detached HEAD state).
    ///
    /// This method removes the worktree we're currently in, even if HEAD is detached.
    /// It finds the branch from:
    /// 1. The worktree's metadata (if not detached)
    /// 2. The reflog (if detached from a branch)
    fn remove_current_worktree(
        &self,
        no_delete_branch: bool,
        force_delete: bool,
    ) -> anyhow::Result<RemoveResult>;

    /// Prepare the target worktree for push by auto-stashing non-overlapping changes when safe.
    fn prepare_target_worktree(
        &self,
        target_worktree: Option<&PathBuf>,
        target_branch: &str,
    ) -> anyhow::Result<Option<TargetWorktreeStash>>;

    /// Check if HEAD is rebased onto the target branch.
    ///
    /// Returns true if the merge-base equals the target's SHA (HEAD is based on target).
    fn is_rebased_onto(&self, target: &str) -> anyhow::Result<bool>;
}

impl RepositoryCliExt for Repository {
    fn load_project_config(&self) -> anyhow::Result<Option<ProjectConfig>> {
        let repo_root = self.worktree_root()?;
        load_project_config_at(&repo_root)
    }

    fn require_project_config(&self) -> anyhow::Result<ProjectConfig> {
        let repo_root = self.worktree_root()?;
        let config_path = repo_root.join(".config").join("wt.toml");

        match load_project_config_at(&repo_root)? {
            Some(cfg) => Ok(cfg),
            None => Err(GitError::ProjectConfigNotFound { config_path }.into()),
        }
    }

    fn warn_if_auto_staging_untracked(&self) -> anyhow::Result<()> {
        // Use -z for NUL-separated output to handle filenames with spaces/newlines
        let status = self
            .run_command(&["status", "--porcelain", "-z"])
            .context("Failed to get status")?;
        AutoStageWarning::from_status(&status).emit()
    }

    fn remove_worktree_by_name(
        &self,
        branch_name: &str,
        no_delete_branch: bool,
        force_delete: bool,
    ) -> anyhow::Result<RemoveResult> {
        let worktree_path = match self.worktree_for_branch(branch_name)? {
            Some(path) => path,
            None => {
                // No worktree found - check if the branch exists locally
                if self.local_branch_exists(branch_name)? {
                    // Branch exists but no worktree - return BranchOnly to attempt branch deletion
                    return Ok(RemoveResult::BranchOnly {
                        branch_name: branch_name.to_string(),
                        no_delete_branch,
                        force_delete,
                    });
                }
                // Check if branch exists on a remote
                let remotes = self.remotes_with_branch(branch_name)?;
                if !remotes.is_empty() {
                    return Err(GitError::RemoteOnlyBranch {
                        branch: branch_name.into(),
                        remote: remotes[0].clone(),
                    }
                    .into());
                }
                return Err(GitError::NoWorktreeFound {
                    branch: branch_name.into(),
                }
                .into());
            }
        };

        if !worktree_path.exists() {
            return Err(GitError::WorktreeMissing {
                branch: branch_name.into(),
            }
            .into());
        }

        let target_repo = Repository::at(&worktree_path);
        target_repo.ensure_clean_working_tree(Some("remove worktree"), Some(branch_name))?;

        let current_worktree = self.worktree_root()?;
        let removing_current = current_worktree == worktree_path;

        // Cannot remove the main working tree (only linked worktrees can be removed)
        if removing_current && !self.is_in_worktree()? {
            return Err(GitError::CannotRemoveMainWorktree.into());
        }

        let (main_path, changed_directory) = if removing_current {
            let worktrees = self.list_worktrees()?;
            (worktrees.main().path.clone(), true)
        } else {
            (current_worktree, false)
        };

        // Resolve default branch for integration reason display
        // Skip if removing the default branch itself (avoids tautological "main (ancestor of main)")
        let default_branch = self.default_branch().ok();
        let target_branch = match &default_branch {
            Some(db) if db == branch_name => None,
            _ => default_branch,
        };

        Ok(RemoveResult::RemovedWorktree {
            main_path,
            worktree_path,
            changed_directory,
            branch_name: Some(branch_name.to_string()),
            no_delete_branch,
            force_delete,
            target_branch,
        })
    }

    fn remove_current_worktree(
        &self,
        no_delete_branch: bool,
        force_delete: bool,
    ) -> anyhow::Result<RemoveResult> {
        // Cannot remove the main working tree (only linked worktrees can be removed)
        if !self.is_in_worktree()? {
            return Err(GitError::CannotRemoveMainWorktree.into());
        }

        // Get current worktree path
        let current_path = self.worktree_root()?;

        // Find this worktree in the list to get its metadata
        let worktrees = self.list_worktrees()?;
        let current_wt = worktrees
            .worktrees
            .iter()
            .find(|wt| wt.path == current_path);

        // Get branch name if available (None for detached HEAD)
        let branch_name = current_wt.and_then(|wt| wt.branch.clone());

        // Ensure the working tree is clean
        self.ensure_clean_working_tree(Some("remove worktree"), branch_name.as_deref())?;

        // Get main worktree path (we're removing current, so we'll cd to main)
        let main_path = worktrees.main().path.clone();

        // Resolve default branch for integration reason display
        // Skip if removing the default branch itself (avoids tautological "main (ancestor of main)")
        let default_branch = self.default_branch().ok();
        let target_branch = match (&default_branch, &branch_name) {
            (Some(db), Some(bn)) if db == bn => None,
            _ => default_branch,
        };

        Ok(RemoveResult::RemovedWorktree {
            main_path,
            worktree_path: current_path,
            changed_directory: true,
            branch_name,
            no_delete_branch,
            force_delete,
            target_branch,
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
        Ok(merge_base == target_sha)
    }
}

fn load_project_config_at(repo_root: &Path) -> anyhow::Result<Option<ProjectConfig>> {
    ProjectConfig::load(repo_root).context("Failed to load project config")
}

/// Parse `git status --porcelain -z` output into a list of affected filenames.
///
/// The -z format uses NUL separators and handles renames specially:
/// - Normal entries: `XY path\0`
/// - Renames/copies: `XY new_path\0old_path\0`
///
/// This correctly handles filenames with spaces and ensures both old and new
/// paths are included for renames/copies (important for overlap detection).
fn parse_porcelain_z(output: &str) -> Vec<String> {
    let mut files = Vec::new();
    let mut entries = output.split('\0').filter(|s| !s.is_empty()).peekable();

    while let Some(entry) = entries.next() {
        // Each entry is "XY path" where XY is exactly 2 status chars
        if entry.len() < 3 {
            continue;
        }

        let status = &entry[0..2];
        let path = &entry[3..];
        files.push(path.to_string());

        // For renames (R) and copies (C), the next NUL-separated field is the old path
        if (status.starts_with('R') || status.starts_with('C'))
            && let Some(old_path) = entries.next()
        {
            files.push(old_path.to_string());
        }
    }

    files
}

struct AutoStageWarning {
    files: Vec<String>,
}

impl AutoStageWarning {
    /// Parse `git status --porcelain -z` output for untracked files.
    ///
    /// Format: "XY path\0" where XY is the status code and path follows a space.
    /// Untracked files have status "??".
    fn from_status(status_output: &str) -> Self {
        let mut files = Vec::new();
        let mut entries = status_output
            .split('\0')
            .filter(|s| !s.is_empty())
            .peekable();

        while let Some(entry) = entries.next() {
            // Format: "XY PATH" where XY is 2 status chars, space, then path
            if entry.len() < 3 {
                continue;
            }

            let status = &entry[0..2];
            let path = &entry[3..];

            // Only collect untracked files
            if status == "??" {
                files.push(path.to_string());
            }

            // Skip old path for renames/copies (we don't care about them here)
            if status.starts_with('R') || status.starts_with('C') {
                entries.next();
            }
        }

        Self { files }
    }

    fn emit(&self) -> anyhow::Result<()> {
        if self.files.is_empty() {
            return Ok(());
        }

        let count = self.files.len();
        let path_word = if count == 1 { "path" } else { "paths" };
        crate::output::print(warning_message(format!(
            "Auto-staging {count} untracked {path_word}:"
        )))?;

        let joined_files = self.files.join("\n");
        crate::output::gutter(format_with_gutter(&joined_files, "", None))?;

        Ok(())
    }
}

pub(crate) struct TargetWorktreeStash {
    repo: Repository,
    path: PathBuf,
    stash_ref: String,
}

impl TargetWorktreeStash {
    pub(crate) fn new(path: &Path, stash_ref: String) -> Self {
        Self {
            repo: Repository::at(path),
            path: path.to_path_buf(),
            stash_ref,
        }
    }

    pub(crate) fn restore(self) -> anyhow::Result<()> {
        crate::output::print(progress_message(cformat!(
            "Restoring stashed changes in <bold>{}</>...",
            format_path_for_display(&self.path)
        )))?;

        if let Err(_e) = self
            .repo
            .run_command(&["stash", "pop", "--quiet", &self.stash_ref])
        {
            crate::output::print(warning_message(cformat!(
                "Failed to restore stash <bold>{stash_ref}</> - run <bold>git stash pop {stash_ref}</> in <bold>{path}</>",
                stash_ref = self.stash_ref,
                path = format_path_for_display(&self.path),
            )))?;
        }

        Ok(())
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
    fn test_auto_stage_warning_from_status_untracked() {
        let warning = AutoStageWarning::from_status("?? new.txt\0");
        assert_eq!(warning.files, vec!["new.txt"]);
    }

    #[test]
    fn test_auto_stage_warning_from_status_multiple_untracked() {
        let warning = AutoStageWarning::from_status("?? file1.txt\0?? file2.txt\0?? file3.txt\0");
        assert_eq!(warning.files, vec!["file1.txt", "file2.txt", "file3.txt"]);
    }

    #[test]
    fn test_auto_stage_warning_from_status_ignores_modified() {
        // Only untracked files should be collected
        let warning = AutoStageWarning::from_status(" M modified.txt\0?? untracked.txt\0");
        assert_eq!(warning.files, vec!["untracked.txt"]);
    }

    #[test]
    fn test_auto_stage_warning_from_status_ignores_staged() {
        let warning = AutoStageWarning::from_status("M  staged.txt\0?? untracked.txt\0");
        assert_eq!(warning.files, vec!["untracked.txt"]);
    }

    #[test]
    fn test_auto_stage_warning_from_status_empty() {
        let warning = AutoStageWarning::from_status("");
        assert!(warning.files.is_empty());
    }

    #[test]
    fn test_auto_stage_warning_from_status_skips_rename_old_path() {
        // Rename entries have old path as second NUL-separated field
        let warning = AutoStageWarning::from_status("R  new.txt\0old.txt\0?? untracked.txt\0");
        // Should only have untracked file, not the rename paths
        assert_eq!(warning.files, vec!["untracked.txt"]);
    }

    #[test]
    fn test_auto_stage_warning_from_status_with_spaces() {
        let warning = AutoStageWarning::from_status("?? file with spaces.txt\0");
        assert_eq!(warning.files, vec!["file with spaces.txt"]);
    }

    #[test]
    fn test_auto_stage_warning_from_status_no_untracked() {
        // All files are tracked (modified, staged, etc.)
        let warning = AutoStageWarning::from_status(" M file1.txt\0M  file2.txt\0");
        assert!(warning.files.is_empty());
    }
}
