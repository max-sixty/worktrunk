//! VCS-agnostic workspace abstraction.
//!
//! This module provides the [`Workspace`] trait that captures the operations
//! commands need, independent of the underlying VCS (git, jj, etc.).
//!
//! The git implementation ([`GitWorkspace`](git::GitWorkspace)) delegates to
//! [`Repository`](crate::git::Repository) methods. The jj implementation
//! ([`JjWorkspace`](jj::JjWorkspace)) shells out to `jj` CLI commands.
//!
//! Use [`detect_vcs`] to determine which VCS manages a given path.

pub(crate) mod detect;
mod git;
pub(crate) mod jj;

use std::path::{Path, PathBuf};

use crate::git::{IntegrationReason, LineDiff, WorktreeInfo, path_dir_name};

pub use detect::detect_vcs;
pub use git::GitWorkspace;
pub use jj::JjWorkspace;

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

    // ====== Capabilities ======

    /// Whether this VCS has a staging area (index).
    /// Git: true. Jj: false.
    fn has_staging_area(&self) -> bool;
}
