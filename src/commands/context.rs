use std::path::PathBuf;

use anyhow::Context;
use once_cell::sync::OnceCell;
use worktrunk::config::UserConfig;
use worktrunk::git::Repository;

use super::command_executor::CommandContext;

/// Shared execution context for command handlers that operate on the current worktree.
///
/// Centralizes the common "repo + branch + cwd" setup so individual handlers can focus on
/// their core logic. Config and repo_root are loaded lazily on first access.
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
    pub worktree_path: PathBuf,
    // Lazy-loaded: defer config reads until needed.
    config: OnceCell<UserConfig>,
}

impl CommandEnv {
    /// Load the command environment for a specific action.
    ///
    /// Only loads the essentials (repo, branch, worktree_path). Config and repo_root
    /// are deferred until first access via `.config()` or `.context()`.
    ///
    /// `action` describes what command is running (e.g., "merge", "squash").
    /// Used in error messages when the environment can't be loaded.
    pub fn for_action(action: &str) -> anyhow::Result<Self> {
        let repo = Repository::current()?;
        let worktree_path = std::env::current_dir().context("Failed to get current directory")?;
        let branch = repo.require_current_branch(action)?;

        Ok(Self {
            repo,
            branch: Some(branch),
            worktree_path,
            config: OnceCell::new(),
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

        Ok(Self {
            repo,
            branch,
            worktree_path,
            config: OnceCell::new(),
        })
    }

    /// Get config, loading lazily on first access.
    pub fn config(&self) -> anyhow::Result<&UserConfig> {
        self.config
            .get_or_try_init(|| UserConfig::load().context("Failed to load config"))
    }

    /// Build a `CommandContext` tied to this environment.
    ///
    /// Loads config if not already loaded.
    pub fn context(&self, yes: bool) -> anyhow::Result<CommandContext<'_>> {
        Ok(CommandContext::new(
            &self.repo,
            self.config()?,
            self.branch.as_deref(),
            &self.worktree_path,
            yes,
        ))
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
