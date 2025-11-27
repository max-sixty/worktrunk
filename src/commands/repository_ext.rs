use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use super::worktree::RemoveResult;
use anyhow::Context;
use color_print::cformat;
use worktrunk::config::ProjectConfig;
use worktrunk::git::{GitError, Repository};
use worktrunk::path::format_path_for_display;
use worktrunk::styling::format_with_gutter;

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

    /// Prepare the target worktree for push by auto-stashing non-overlapping changes when safe.
    fn prepare_target_worktree(
        &self,
        target_worktree: Option<&PathBuf>,
        target_branch: &str,
    ) -> anyhow::Result<Option<TargetWorktreeStash>>;
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
        let status = self
            .run_command(&["status", "--porcelain"])
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
                // No worktree found - check if the branch exists
                if self.local_branch_exists(branch_name)? {
                    // Branch exists but no worktree - return BranchOnly to attempt branch deletion
                    return Ok(RemoveResult::BranchOnly {
                        branch_name: branch_name.to_string(),
                        no_delete_branch,
                        force_delete,
                    });
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
        target_repo.ensure_clean_working_tree(Some("remove worktree"))?;

        let current_worktree = self.worktree_root()?;
        let removing_current = current_worktree == worktree_path;

        let (main_path, changed_directory) = if removing_current {
            let worktrees = self.list_worktrees()?;
            (worktrees.main().path.clone(), true)
        } else {
            (current_worktree, false)
        };

        Ok(RemoveResult::RemovedWorktree {
            main_path,
            worktree_path,
            changed_directory,
            branch_name: branch_name.to_string(),
            no_delete_branch,
            force_delete,
            target_branch: None,
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
        let wt_status_output = wt_repo.run_command(&["status", "--porcelain"])?;

        let wt_files: Vec<String> = wt_status_output
            .lines()
            .filter_map(|line| {
                line.split_once(' ')
                    .map(|(_, filename)| filename.trim().to_string())
            })
            .collect();

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

        crate::output::progress(cformat!(
            "Stashing changes in <bold>{}</>...",
            format_path_for_display(wt_path)
        ))?;

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
}

fn load_project_config_at(repo_root: &Path) -> anyhow::Result<Option<ProjectConfig>> {
    ProjectConfig::load(repo_root).context("Failed to load project config")
}

struct AutoStageWarning {
    files: Vec<String>,
}

impl AutoStageWarning {
    fn from_status(status_output: &str) -> Self {
        let files = status_output
            .lines()
            .filter_map(|line| line.strip_prefix("?? "))
            .map(|filename| filename.to_string())
            .collect();

        Self { files }
    }

    fn emit(&self) -> anyhow::Result<()> {
        if self.files.is_empty() {
            return Ok(());
        }

        let count = self.files.len();
        let file_word = if count == 1 { "file" } else { "files" };
        crate::output::warning(format!("Auto-staging {count} untracked {file_word}:"))?;

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
        crate::output::progress(cformat!(
            "Restoring stashed changes in <bold>{}</>...",
            format_path_for_display(&self.path)
        ))?;

        if let Err(_e) = self
            .repo
            .run_command(&["stash", "pop", "--quiet", &self.stash_ref])
        {
            crate::output::warning(cformat!(
                "Failed to restore stash <bold>{stash_ref}</> - run 'git stash pop {stash_ref}' in <bold>{path}</>",
                stash_ref = self.stash_ref,
                path = format_path_for_display(&self.path),
            ))?;
        }

        Ok(())
    }
}
