use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use super::list::model::{BranchInfo, ListData, ListItem, WorktreeInfo};
use super::worktree::RemoveResult;
use worktrunk::config::ProjectConfig;
use worktrunk::git::{GitError, GitResultExt, Repository};
use worktrunk::styling::{
    CYAN, CYAN_BOLD, ERROR, ERROR_EMOJI, HINT, HINT_BOLD, HINT_EMOJI, WARNING, WARNING_BOLD,
    WARNING_EMOJI, format_with_gutter, println,
};

/// CLI-only helpers implemented on [`Repository`] via an extension trait so we can keep orphan
/// implementations inside the binary crate.
pub trait RepositoryCliExt {
    /// Load the project configuration if it exists.
    fn load_project_config(&self) -> Result<Option<ProjectConfig>, GitError>;

    /// Load the project configuration, emitting a helpful hint if missing.
    fn require_project_config(&self) -> Result<ProjectConfig, GitError>;

    /// Warn about untracked files being auto-staged.
    fn warn_if_auto_staging_untracked(&self) -> Result<(), GitError>;

    /// Gather enriched list data for worktrees (and optional branches).
    fn gather_list_data(
        &self,
        show_branches: bool,
        fetch_ci: bool,
        check_conflicts: bool,
    ) -> Result<Option<ListData>, GitError>;

    /// Remove the currently checked-out worktree or switch back to the default branch if invoked
    /// from the primary repo root.
    fn remove_current_worktree(&self, no_delete_branch: bool) -> Result<RemoveResult, GitError>;

    /// Remove a worktree identified by branch name.
    fn remove_worktree_by_name(
        &self,
        branch_name: &str,
        no_delete_branch: bool,
    ) -> Result<RemoveResult, GitError>;

    /// Prepare the target worktree for push by auto-stashing non-overlapping changes when safe.
    fn prepare_target_worktree(
        &self,
        target_worktree: Option<&PathBuf>,
        target_branch: &str,
    ) -> Result<Option<TargetWorktreeStash>, GitError>;
}

impl RepositoryCliExt for Repository {
    fn load_project_config(&self) -> Result<Option<ProjectConfig>, GitError> {
        let repo_root = self.worktree_root()?;
        load_project_config_at(&repo_root)
    }

    fn require_project_config(&self) -> Result<ProjectConfig, GitError> {
        let repo_root = self.worktree_root()?;
        let config_path = repo_root.join(".config").join("wt.toml");

        match load_project_config_at(&repo_root)? {
            Some(cfg) => Ok(cfg),
            None => {
                use worktrunk::styling::eprintln;
                eprintln!("{ERROR_EMOJI} {ERROR}No project configuration found{ERROR:#}");
                eprintln!(
                    "{HINT_EMOJI} {HINT}Create a config file at: {HINT_BOLD}{}{HINT_BOLD:#}{HINT:#}",
                    config_path.display()
                );
                Err(GitError::CommandFailed(
                    "No project configuration found".to_string(),
                ))
            }
        }
    }

    fn warn_if_auto_staging_untracked(&self) -> Result<(), GitError> {
        let status = self
            .run_command(&["status", "--porcelain"])
            .git_context("Failed to get status")?;
        AutoStageWarning::from_status(&status).emit()
    }

    fn gather_list_data(
        &self,
        show_branches: bool,
        fetch_ci: bool,
        check_conflicts: bool,
    ) -> Result<Option<ListData>, GitError> {
        let worktrees = self.list_worktrees()?;

        if worktrees.worktrees.is_empty() {
            return Ok(None);
        }

        let primary = worktrees.worktrees[0].clone();
        let current_worktree_path = self.worktree_root().ok();

        let worktree_results: Vec<Result<WorktreeInfo, GitError>> = worktrees
            .worktrees
            .par_iter()
            .map(|wt| WorktreeInfo::from_worktree(wt, &primary, fetch_ci, check_conflicts))
            .collect();

        let mut items = Vec::new();
        for result in worktree_results {
            match result {
                Ok(info) => items.push(ListItem::Worktree(info)),
                Err(e) => return Err(e),
            }
        }

        if show_branches {
            let available_branches = self.available_branches()?;
            let primary_branch = primary.branch.as_deref();

            let branch_results: Vec<(String, Result<BranchInfo, GitError>)> = available_branches
                .par_iter()
                .map(|branch| {
                    let result = BranchInfo::from_branch(
                        branch,
                        self,
                        primary_branch,
                        fetch_ci,
                        check_conflicts,
                    );
                    (branch.clone(), result)
                })
                .collect();

            for (branch, result) in branch_results {
                match result {
                    Ok(info) => items.push(ListItem::Branch(info)),
                    Err(e) => {
                        println!(
                            "{WARNING_EMOJI} {WARNING}Failed to enrich branch {WARNING_BOLD}{branch}{WARNING_BOLD:#}: {e}{WARNING:#}"
                        );
                        println!(
                            "{HINT_EMOJI} {HINT}This branch will be shown with limited information{HINT:#}"
                        );
                    }
                }
            }
        }

        items.sort_by_key(|item| {
            let is_primary = item.is_primary();
            let is_current = item
                .worktree_path()
                .and_then(|p| current_worktree_path.as_ref().map(|cp| p == cp))
                .unwrap_or(false);

            let priority = if is_primary {
                0
            } else if is_current {
                1
            } else {
                2
            };

            (priority, std::cmp::Reverse(item.commit_timestamp()))
        });

        Ok(Some(ListData {
            items,
            current_worktree_path,
        }))
    }

    fn remove_current_worktree(&self, no_delete_branch: bool) -> Result<RemoveResult, GitError> {
        self.ensure_clean_working_tree()?;

        if self.is_in_worktree()? {
            let worktree_root = self.worktree_root()?;
            let current_branch = self
                .current_branch()?
                .ok_or_else(|| GitError::message("Not on a branch"))?;
            let worktrees = self.list_worktrees()?;
            let primary_worktree_dir = worktrees.worktrees[0].path.clone();

            return Ok(RemoveResult::RemovedWorktree {
                primary_path: primary_worktree_dir,
                worktree_path: worktree_root,
                changed_directory: true,
                branch_name: current_branch,
                no_delete_branch,
            });
        }

        let current_branch = self.current_branch()?;
        let default_branch = self.default_branch()?;

        if current_branch.as_deref() == Some(&default_branch) {
            return Ok(RemoveResult::AlreadyOnDefault(default_branch));
        }

        if let Err(err) = self.run_command(&["switch", &default_branch]) {
            return Err(match err {
                GitError::CommandFailed(msg) => GitError::SwitchFailed {
                    branch: default_branch.clone(),
                    error: msg,
                },
                other => other,
            });
        }

        Ok(RemoveResult::SwitchedToDefault(default_branch))
    }

    fn remove_worktree_by_name(
        &self,
        branch_name: &str,
        no_delete_branch: bool,
    ) -> Result<RemoveResult, GitError> {
        let worktree_path = match self.worktree_for_branch(branch_name)? {
            Some(path) => path,
            None => {
                return Err(GitError::NoWorktreeFound {
                    branch: branch_name.to_string(),
                });
            }
        };

        if !worktree_path.exists() {
            return Err(GitError::WorktreeMissing {
                branch: branch_name.to_string(),
            });
        }

        let target_repo = Repository::at(&worktree_path);
        target_repo.ensure_clean_working_tree()?;

        let current_worktree = self.worktree_root()?;
        let removing_current = current_worktree == worktree_path;

        let (primary_path, changed_directory) = if removing_current {
            let worktrees = self.list_worktrees()?;
            (worktrees.worktrees[0].path.clone(), true)
        } else {
            (current_worktree, false)
        };

        Ok(RemoveResult::RemovedWorktree {
            primary_path,
            worktree_path,
            changed_directory,
            branch_name: branch_name.to_string(),
            no_delete_branch,
        })
    }

    fn prepare_target_worktree(
        &self,
        target_worktree: Option<&PathBuf>,
        target_branch: &str,
    ) -> Result<Option<TargetWorktreeStash>, GitError> {
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
            });
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

        crate::output::progress(format!(
            "{CYAN}Stashing changes in {CYAN_BOLD}{}{CYAN_BOLD:#}{CYAN}...{CYAN:#}",
            wt_path.display()
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
            return Err(GitError::CommandFailed(format!(
                "Failed to locate autostash entry '{}'",
                stash_name
            )));
        };

        Ok(Some(TargetWorktreeStash::new(wt_path, stash_ref)))
    }
}

fn load_project_config_at(repo_root: &Path) -> Result<Option<ProjectConfig>, GitError> {
    ProjectConfig::load(repo_root).git_context("Failed to load project config")
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

    fn emit(&self) -> Result<(), GitError> {
        if self.files.is_empty() {
            return Ok(());
        }

        let count = self.files.len();
        let file_word = if count == 1 { "file" } else { "files" };
        crate::output::warning(format!(
            "{WARNING}Auto-staging {count} untracked {file_word}:{WARNING:#}"
        ))?;

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

    pub(crate) fn restore(self) -> Result<(), GitError> {
        crate::output::progress(format!(
            "{CYAN}Restoring stashed changes in {CYAN_BOLD}{}{CYAN_BOLD:#}{CYAN}...{CYAN:#}",
            self.path.display()
        ))?;

        if let Err(_e) = self
            .repo
            .run_command(&["stash", "pop", "--quiet", &self.stash_ref])
        {
            crate::output::warning(format!(
                "{WARNING}Failed to restore stash {WARNING_BOLD}{stash_ref}{WARNING_BOLD:#}{WARNING} - run 'git stash pop {stash_ref}' in {WARNING_BOLD}{path}{WARNING_BOLD:#}{WARNING:#}",
                stash_ref = self.stash_ref,
                path = self.path.display(),
            ))?;
        }

        Ok(())
    }
}
