use anyhow::Context;
use std::path::PathBuf;
use worktrunk::config::WorktrunkConfig;
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
    pub config: WorktrunkConfig,
    pub worktree_path: PathBuf,
    pub repo_root: PathBuf,
}

impl CommandEnv {
    /// Load the command environment for a specific action.
    ///
    /// `action` describes what command is running (e.g., "merge", "squash").
    /// Used in error messages when the environment can't be loaded.
    pub fn for_action(action: &str) -> anyhow::Result<Self> {
        let repo = Repository::current();
        let worktree_path = std::env::current_dir().context("Failed to get current directory")?;
        let branch = repo.require_current_branch(action)?;
        let config = WorktrunkConfig::load().context("Failed to load config")?;
        let repo_root = repo.worktree_base()?;

        Ok(Self {
            repo,
            branch: Some(branch),
            config,
            worktree_path,
            repo_root,
        })
    }

    /// Load the command environment without requiring a branch.
    ///
    /// Use this for commands that can operate in detached HEAD state,
    /// such as running hooks (where `{{ branch }}` expands to "HEAD" if detached).
    pub fn for_action_branchless() -> anyhow::Result<Self> {
        let repo = Repository::current();
        let worktree_path = std::env::current_dir().context("Failed to get current directory")?;
        // Propagate git errors (broken repo, missing git) but allow None for detached HEAD
        let branch = repo
            .current_branch()
            .context("Failed to determine current branch")?;
        let config = WorktrunkConfig::load().context("Failed to load config")?;
        let repo_root = repo.worktree_base()?;

        Ok(Self {
            repo,
            branch,
            config,
            worktree_path,
            repo_root,
        })
    }

    /// Build a `CommandContext` tied to this environment.
    pub fn context(&self, force: bool) -> CommandContext<'_> {
        CommandContext::new(
            &self.repo,
            &self.config,
            self.branch.as_deref(),
            &self.worktree_path,
            &self.repo_root,
            force,
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
}
