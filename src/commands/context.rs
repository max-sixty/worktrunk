use anyhow::Context;
use std::path::PathBuf;
use worktrunk::config::UserConfig;
use worktrunk::git::Repository;

use super::command_executor::CommandContext;

/// Shared execution context for command handlers that operate on the current worktree.
///
/// Centralizes the common "repo + branch + config + cwd" setup so individual handlers
/// can focus on their core logic while sharing consistent error messaging.
///
/// This helper is used for commands that explicitly act on "where the user is standing"
/// (e.g., `beta` and `merge`) and therefore need all of these pieces together. Commands that
/// inspect multiple worktrees or run without a config/branch requirement (`list`, `select`,
/// some `worktree` helpers) still call `Repository::current()` directly so they can operate in
/// broader contexts without forcing config loads or branch resolution.
pub struct CommandEnv {
    pub repo: Repository,
    /// Current branch name, if on a branch (None in detached HEAD state).
    pub branch: Option<String>,
    pub config: UserConfig,
    pub worktree_path: PathBuf,
}

impl CommandEnv {
    /// Load the command environment for a specific action.
    ///
    /// `action` describes what command is running (e.g., "merge", "squash").
    /// Used in error messages when the environment can't be loaded.
    pub fn for_action(action: &str, config: UserConfig) -> anyhow::Result<Self> {
        let repo = Repository::current()?;
        let worktree_path = std::env::current_dir().context("Failed to get current directory")?;
        let branch = repo.require_current_branch(action)?;

        Ok(Self {
            repo,
            branch: Some(branch),
            config,
            worktree_path,
        })
    }

    /// Load the command environment without requiring a branch.
    ///
    /// Use this for commands that can operate in detached HEAD state,
    /// such as running hooks (where `{{ branch }}` expands to "HEAD" if detached).
    pub fn for_action_branchless() -> anyhow::Result<Self> {
        let repo = Repository::current()?;
        let worktree_path = std::env::current_dir().context("Failed to get current directory")?;
        // Propagate git errors (broken repo, missing git) but allow None for detached HEAD
        let branch = repo
            .current_worktree()
            .branch()
            .context("Failed to determine current branch")?;
        let config = UserConfig::load().context("Failed to load config")?;

        Ok(Self {
            repo,
            branch,
            config,
            worktree_path,
        })
    }

    /// Build a `CommandContext` tied to this environment.
    pub fn context(&self, yes: bool) -> CommandContext<'_> {
        CommandContext::new(
            &self.repo,
            &self.config,
            self.branch.as_deref(),
            &self.worktree_path,
            yes,
        )
    }

    /// Get branch name, returning error if in detached HEAD state.
    pub fn require_branch(&self, action: &str) -> anyhow::Result<&str> {
        self.branch.as_deref().ok_or_else(|| {
            worktrunk::git::GitError::DetachedHead {
                action: Some(action.into()),
            }
            .into()
        })
    }

    /// Get the project identifier for per-project config lookup.
    ///
    /// Uses the remote URL if available, otherwise the canonical repository path.
    /// Returns None only if the path is not valid UTF-8.
    pub fn project_id(&self) -> Option<String> {
        self.repo.project_identifier().ok()
    }

    /// Get the commit generation config, merging project-specific settings.
    pub fn commit_generation(&self) -> worktrunk::config::CommitGenerationConfig {
        self.config.commit_generation(self.project_id().as_deref())
    }

    /// Get the commit config, merging project-specific settings.
    pub fn commit(&self) -> Option<worktrunk::config::CommitConfig> {
        self.config.commit(self.project_id().as_deref())
    }

    /// Get the merge config, merging project-specific settings.
    pub fn merge(&self) -> Option<worktrunk::config::MergeConfig> {
        self.config.merge(self.project_id().as_deref())
    }
}
