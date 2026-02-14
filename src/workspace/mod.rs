//! VCS-agnostic workspace abstraction.
//!
//! This module provides the [`Workspace`] trait that captures the operations
//! commands need, independent of the underlying VCS (git, jj, etc.).
//!
//! The git implementation is on [`Repository`](crate::git::Repository) directly.
//! The jj implementation ([`JjWorkspace`]) shells out to `jj` CLI commands.
//! Commands that need git-specific features can downcast via
//! `workspace.as_any().downcast_ref::<Repository>()`.
//!
//! Use [`detect_vcs`] to determine which VCS manages a given path.

pub(crate) mod detect;
mod git;
pub(crate) mod jj;
pub mod types;

use std::any::Any;
use std::path::{Path, PathBuf};

use crate::git::WorktreeInfo;
pub use types::{IntegrationReason, LineDiff, path_dir_name};

pub use detect::detect_vcs;
pub use jj::JjWorkspace;

/// Outcome of a rebase operation on the VCS level.
pub enum RebaseOutcome {
    /// True rebase (history rewritten).
    Rebased,
    /// Fast-forward (HEAD moved forward, no rewrite).
    FastForward,
}

/// Version control system type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VcsKind {
    Git,
    Jj,
}

/// VCS-agnostic workspace item (worktree in git, workspace in jj).
#[derive(Debug, Clone)]
pub struct WorkspaceItem {
    /// Filesystem path to the workspace root.
    pub path: PathBuf,
    /// Workspace name. In git: derived from branch name (or directory name for
    /// detached HEAD). In jj: the native workspace name.
    pub name: String,
    /// Commit identifier. In git: commit SHA. In jj: change ID.
    pub head: String,
    /// Branch name (git) or bookmark name (jj). None for detached HEAD (git)
    /// or workspaces without bookmarks (jj).
    pub branch: Option<String>,
    /// Whether this is the default/primary workspace.
    pub is_default: bool,
    /// Lock reason, if locked.
    pub locked: Option<String>,
    /// Prunable reason, if prunable (directory deleted but VCS still tracks it).
    pub prunable: Option<String>,
}

impl WorkspaceItem {
    /// Create a `WorkspaceItem` from a git [`WorktreeInfo`].
    ///
    /// The `name` field uses the branch name when available, falling back
    /// to the directory name for detached HEAD worktrees.
    pub fn from_worktree(wt: WorktreeInfo, is_default: bool) -> Self {
        let name = wt
            .branch
            .clone()
            .unwrap_or_else(|| path_dir_name(&wt.path).to_string());

        Self {
            path: wt.path,
            name,
            head: wt.head,
            branch: wt.branch,
            is_default,
            locked: wt.locked,
            prunable: wt.prunable,
        }
    }
}

/// VCS-agnostic workspace operations.
///
/// Captures what commands need at the workspace-operation level, not the
/// VCS-command level. Each VCS implementation translates these operations
/// into the appropriate commands.
pub trait Workspace: Send + Sync {
    /// Which VCS backs this workspace.
    fn kind(&self) -> VcsKind;

    // ====== Discovery ======

    /// List all workspaces in the repository.
    fn list_workspaces(&self) -> anyhow::Result<Vec<WorkspaceItem>>;

    /// Resolve a workspace name to its filesystem path.
    fn workspace_path(&self, name: &str) -> anyhow::Result<PathBuf>;

    /// Path to the default/primary workspace.
    fn default_workspace_path(&self) -> anyhow::Result<Option<PathBuf>>;

    /// Name of the default/trunk branch. Returns `None` if unknown.
    /// Git: "main"/"master"/etc. Jj: `None` (uses `trunk()` revset).
    fn default_branch_name(&self) -> anyhow::Result<Option<String>>;

    // ====== Status per workspace ======

    /// Whether the workspace has uncommitted changes.
    fn is_dirty(&self, path: &Path) -> anyhow::Result<bool>;

    /// Line-level diff of uncommitted changes.
    fn working_diff(&self, path: &Path) -> anyhow::Result<LineDiff>;

    // ====== Comparison against trunk ======

    /// Commits ahead/behind between two refs.
    fn ahead_behind(&self, base: &str, head: &str) -> anyhow::Result<(usize, usize)>;

    /// Check if content identified by `id` is integrated into `target`.
    /// Returns the integration reason if integrated, `None` if not.
    fn is_integrated(&self, id: &str, target: &str) -> anyhow::Result<Option<IntegrationReason>>;

    /// Line-level diff stats between two refs (committed changes only).
    fn branch_diff_stats(&self, base: &str, head: &str) -> anyhow::Result<LineDiff>;

    // ====== Mutations ======

    /// Create a new workspace.
    /// - `name`: workspace/branch name
    /// - `base`: starting point (branch, commit, or None for default)
    /// - `path`: filesystem path for the new workspace
    fn create_workspace(&self, name: &str, base: Option<&str>, path: &Path) -> anyhow::Result<()>;

    /// Remove a workspace by name.
    fn remove_workspace(&self, name: &str) -> anyhow::Result<()>;

    // ====== Rebase ======

    /// Resolve the integration target (branch/bookmark to rebase onto).
    /// Git: validates ref exists, falls back to default branch.
    /// Jj: detects trunk bookmark.
    fn resolve_integration_target(&self, target: Option<&str>) -> anyhow::Result<String>;

    /// Whether the current workspace is already rebased onto `target`.
    /// Git: merge-base == target SHA, no merge commits between.
    /// Jj: target is ancestor of feature tip.
    fn is_rebased_onto(&self, target: &str, path: &Path) -> anyhow::Result<bool>;

    /// Rebase the current workspace onto `target`.
    /// Returns the outcome (Rebased vs FastForward).
    /// Implementations emit their own progress message when appropriate.
    fn rebase_onto(&self, target: &str, path: &Path) -> anyhow::Result<RebaseOutcome>;

    // ====== Identity ======

    /// Root path of the repository (git dir or jj repo root).
    fn root_path(&self) -> anyhow::Result<PathBuf>;

    /// Filesystem path of the current workspace/worktree.
    ///
    /// Git: uses `current_worktree().path()` (respects `-C` flag / base_path).
    /// Jj: uses `current_workspace().path` (found via cwd).
    fn current_workspace_path(&self) -> anyhow::Result<PathBuf>;

    /// Current workspace/branch name at the given path.
    /// Returns `None` for detached HEAD (git) or workspaces without bookmarks (jj).
    fn current_name(&self, path: &Path) -> anyhow::Result<Option<String>>;

    /// Project identifier for approval/hook scoping.
    /// Uses remote URL if available, otherwise the canonical repository path.
    fn project_identifier(&self) -> anyhow::Result<String>;

    // ====== Commit ======

    /// Commit staged/working changes with the given message.
    /// Returns the new commit identifier (SHA for git, change ID for jj).
    fn commit(&self, message: &str, path: &Path) -> anyhow::Result<String>;

    /// Subject lines of commits between `base` and `head`.
    fn commit_subjects(&self, base: &str, head: &str) -> anyhow::Result<Vec<String>>;

    // ====== Push ======

    /// Push current branch/bookmark to remote, fast-forward only.
    /// `target` is the branch/bookmark to update on the remote.
    fn push_to_target(&self, target: &str, path: &Path) -> anyhow::Result<()>;

    // ====== Capabilities ======

    /// Whether this VCS has a staging area (index).
    /// Git: true. Jj: false.
    fn has_staging_area(&self) -> bool;

    /// Downcast to concrete type for VCS-specific operations.
    fn as_any(&self) -> &dyn Any;
}

/// Detect VCS and open the appropriate workspace for the current directory.
pub fn open_workspace() -> anyhow::Result<Box<dyn Workspace>> {
    let cwd = std::env::current_dir()?;
    match detect_vcs(&cwd) {
        Some(VcsKind::Jj) => Ok(Box::new(JjWorkspace::from_current_dir()?)),
        Some(VcsKind::Git) => {
            let repo = crate::git::Repository::current()?;
            Ok(Box::new(repo))
        }
        None => anyhow::bail!("Not in a git or jj repository"),
    }
}
