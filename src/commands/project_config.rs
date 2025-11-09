use std::path::Path;
use worktrunk::config::{Command, ProjectConfig};
use worktrunk::git::{GitError, GitResultExt, HookType, Repository};

fn load_project_config_at(repo_root: &Path) -> Result<Option<ProjectConfig>, GitError> {
    ProjectConfig::load(repo_root).git_context("Failed to load project config")
}

/// Extension methods for accessing project config data via Repository instances.
pub trait ProjectConfigRepoExt {
    /// Load the project configuration if it exists.
    fn load_project_config(&self) -> Result<Option<ProjectConfig>, GitError>;

    /// Load the project configuration, emitting a helpful hint if missing.
    fn require_project_config(&self) -> Result<ProjectConfig, GitError>;
}

impl ProjectConfigRepoExt for Repository {
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
                use worktrunk::styling::{
                    ERROR, ERROR_EMOJI, HINT, HINT_BOLD, HINT_EMOJI, eprintln,
                };

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
}

/// Collect commands for the given hook types, preserving order of the provided hooks.
pub fn collect_commands_for_hooks(
    project_config: &ProjectConfig,
    hooks: &[HookType],
) -> Vec<Command> {
    let mut commands = Vec::new();
    for hook in hooks {
        let cfg = match hook {
            HookType::PostCreate => &project_config.post_create_command,
            HookType::PostStart => &project_config.post_start_command,
            HookType::PreCommit => &project_config.pre_commit_command,
            HookType::PreMerge => &project_config.pre_merge_command,
            HookType::PostMerge => &project_config.post_merge_command,
        };
        if let Some(config) = cfg {
            commands.extend(config.commands_with_phase(*hook));
        }
    }
    commands
}
