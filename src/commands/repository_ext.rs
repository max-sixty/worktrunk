use rayon::prelude::*;
use std::path::Path;

use super::list::model::{BranchInfo, ListData, ListItem, WorktreeInfo};
use worktrunk::config::ProjectConfig;
use worktrunk::git::{GitError, GitResultExt, Repository};
use worktrunk::styling::{
    ERROR, ERROR_EMOJI, HINT, HINT_BOLD, HINT_EMOJI, WARNING, WARNING_BOLD, WARNING_EMOJI,
    format_with_gutter, println,
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
        let untracked = get_untracked_files(&status);

        if untracked.is_empty() {
            return Ok(());
        }

        let count = untracked.len();
        let file_word = if count == 1 { "file" } else { "files" };
        crate::output::warning(format!(
            "{WARNING}Auto-staging {count} untracked {file_word}:{WARNING:#}"
        ))?;

        let joined_files = untracked.join("\n");
        crate::output::gutter(format_with_gutter(&joined_files, "", None))?;

        Ok(())
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
}

fn load_project_config_at(repo_root: &Path) -> Result<Option<ProjectConfig>, GitError> {
    ProjectConfig::load(repo_root).git_context("Failed to load project config")
}

fn get_untracked_files(status_output: &str) -> Vec<String> {
    status_output
        .lines()
        .filter_map(|line| line.strip_prefix("?? "))
        .map(|filename| filename.to_string())
        .collect()
}
