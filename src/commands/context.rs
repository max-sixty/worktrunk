use std::path::PathBuf;

use anyhow::Context;
use worktrunk::config::UserConfig;
use worktrunk::git::Repository;
use worktrunk::workspace::{Workspace, open_workspace};

use super::command_executor::CommandContext;

/// Shared execution context for command handlers that operate on the current worktree.
///
/// Centralizes the common "workspace + branch + config + cwd" setup so individual handlers
/// can focus on their core logic while sharing consistent error messaging.
///
/// Holds a `Box<dyn Workspace>` for VCS-agnostic operations. Commands that need
/// git-specific features (hooks, staging) can access `&Repository` via [`repo()`](Self::repo).
///
/// This helper is used for commands that explicitly act on "where the user is standing"
/// (e.g., `step commit` and `merge`) and therefore need all of these pieces together.
/// Commands that inspect multiple worktrees or run without a config/branch requirement
/// (`list`, `select`, some `worktree` helpers) call `open_workspace()` directly so they
/// can operate in broader contexts without forcing config loads or branch resolution.
pub struct CommandEnv {
    pub workspace: Box<dyn Workspace>,
    /// Current branch name, if on a branch (None in detached HEAD state).
    pub branch: Option<String>,
    pub config: UserConfig,
    pub worktree_path: PathBuf,
}

impl CommandEnv {
    /// Build the command environment from a pre-opened workspace.
    ///
    /// `action` describes what command is running (e.g., "merge", "squash").
    /// Used in error messages when the environment can't be loaded.
    /// Requires a branch (can't merge/squash in detached HEAD).
    pub fn with_workspace(
        workspace: Box<dyn Workspace>,
        action: &str,
        config: UserConfig,
    ) -> anyhow::Result<Self> {
        let worktree_path = workspace.current_workspace_path()?;
        let branch = workspace.current_name(&worktree_path)?;

        // Require a branch (can't merge/squash in detached HEAD)
        if branch.is_none() {
            return Err(worktrunk::git::GitError::DetachedHead {
                action: Some(action.into()),
            }
            .into());
        }

        Ok(Self {
            workspace,
            branch,
            config,
            worktree_path,
        })
    }

    /// Build the command environment from a pre-opened workspace, without requiring a branch.
    ///
    /// Use this for commands that can operate in detached HEAD state,
    /// such as running hooks (where `{{ branch }}` expands to "HEAD" if detached).
    pub fn with_workspace_branchless(workspace: Box<dyn Workspace>) -> anyhow::Result<Self> {
        let worktree_path = workspace.current_workspace_path()?;
        let branch = workspace
            .current_name(&worktree_path)
            .context("Failed to determine current branch")?;
        let config = UserConfig::load().context("Failed to load config")?;

        Ok(Self {
            workspace,
            branch,
            config,
            worktree_path,
        })
    }

    /// Open a workspace and load the command environment without requiring a branch.
    ///
    /// Convenience wrapper that calls `open_workspace()` then `with_workspace_branchless()`.
    pub fn for_action_branchless() -> anyhow::Result<Self> {
        Self::with_workspace_branchless(open_workspace()?)
    }

    /// Access the underlying git `Repository`.
    ///
    /// Returns `None` if this is a non-git workspace (e.g., jj).
    /// For git workspaces, this is always `Some`.
    pub fn repo(&self) -> Option<&Repository> {
        self.workspace.as_any().downcast_ref::<Repository>()
    }

    /// Access the underlying git `Repository`, returning an error if not git.
    ///
    /// Use in code paths that require git-specific features (hooks, staging).
    pub fn require_repo(&self) -> anyhow::Result<&Repository> {
        self.repo()
            .ok_or_else(|| anyhow::anyhow!("This command requires a git repository"))
    }

    /// Build a `CommandContext` tied to this environment.
    pub fn context(&self, yes: bool) -> CommandContext<'_> {
        CommandContext::new(
            self.workspace.as_ref(),
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
        self.workspace.project_identifier().ok()
    }

    /// Get all resolved config with defaults applied.
    pub fn resolved(&self) -> worktrunk::config::ResolvedConfig {
        self.config.resolved(self.project_id().as_deref())
    }
}
