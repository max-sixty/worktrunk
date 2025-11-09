use std::path::Path;
use worktrunk::config::ProjectConfig;
use worktrunk::git::{GitError, GitResultExt, Repository};
use worktrunk::styling::{
    ERROR, ERROR_EMOJI, HINT, HINT_BOLD, HINT_EMOJI, WARNING, format_with_gutter,
};

/// CLI-specific helpers that operate on `Repository` but live in the binary crate due to orphan rules.
pub trait RepositoryCliExt {
    /// Load the project configuration if it exists.
    fn load_project_config(&self) -> Result<Option<ProjectConfig>, GitError>;

    /// Load the project configuration, emitting a helpful hint if missing.
    fn require_project_config(&self) -> Result<ProjectConfig, GitError>;

    /// Warn about untracked files being auto-staged.
    fn warn_if_auto_staging_untracked(&self) -> Result<(), GitError>;
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
