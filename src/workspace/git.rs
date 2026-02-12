//! Git implementation of the [`Workspace`] trait.
//!
//! Delegates to [`Repository`] methods, mapping git-specific types
//! to the VCS-agnostic [`WorkspaceItem`] and [`Workspace`] interface.

use std::path::{Path, PathBuf};

use crate::git::{
    IntegrationReason, LineDiff, Repository, check_integration, compute_integration_lazy,
    path_dir_name,
};

use super::{VcsKind, Workspace, WorkspaceItem};

/// Git-backed workspace implementation.
///
/// Wraps a [`Repository`] and implements [`Workspace`] by delegating to
/// existing git operations. The `Repository` is cloneable (shares cache
/// via `Arc`), so `GitWorkspace` is cheap to clone.
#[derive(Debug, Clone)]
pub struct GitWorkspace {
    repo: Repository,
}

impl GitWorkspace {
    /// Create a new `GitWorkspace` wrapping the given repository.
    pub fn new(repo: Repository) -> Self {
        Self { repo }
    }

    /// Access the underlying [`Repository`].
    pub fn repo(&self) -> &Repository {
        &self.repo
    }
}

impl From<Repository> for GitWorkspace {
    fn from(repo: Repository) -> Self {
        Self::new(repo)
    }
}

impl Workspace for GitWorkspace {
    fn kind(&self) -> VcsKind {
        VcsKind::Git
    }

    fn list_workspaces(&self) -> anyhow::Result<Vec<WorkspaceItem>> {
        let worktrees = self.repo.list_worktrees()?;
        let primary_path = self.repo.primary_worktree()?;

        Ok(worktrees
            .into_iter()
            .map(|wt| {
                let is_default = primary_path
                    .as_ref()
                    .is_some_and(|primary| *primary == wt.path);
                WorkspaceItem::from_worktree(wt, is_default)
            })
            .collect())
    }

    fn workspace_path(&self, name: &str) -> anyhow::Result<PathBuf> {
        // Single pass: list worktrees once, check both branch name and dir name
        let worktrees = self.repo.list_worktrees()?;

        // Prefer branch name match
        if let Some(wt) = worktrees
            .iter()
            .find(|wt| wt.branch.as_deref() == Some(name))
        {
            return Ok(wt.path.clone());
        }

        // Fall back to directory name match
        worktrees
            .iter()
            .find(|wt| path_dir_name(&wt.path) == name)
            .map(|wt| wt.path.clone())
            .ok_or_else(|| anyhow::anyhow!("No workspace found for name: {name}"))
    }

    fn default_workspace_path(&self) -> anyhow::Result<Option<PathBuf>> {
        self.repo.primary_worktree()
    }

    fn default_branch_name(&self) -> anyhow::Result<Option<String>> {
        Ok(self.repo.default_branch())
    }

    fn is_dirty(&self, path: &Path) -> anyhow::Result<bool> {
        self.repo.worktree_at(path).is_dirty()
    }

    fn working_diff(&self, path: &Path) -> anyhow::Result<LineDiff> {
        self.repo.worktree_at(path).working_tree_diff_stats()
    }

    fn ahead_behind(&self, base: &str, head: &str) -> anyhow::Result<(usize, usize)> {
        self.repo.ahead_behind(base, head)
    }

    fn is_integrated(&self, id: &str, target: &str) -> anyhow::Result<Option<IntegrationReason>> {
        let signals = compute_integration_lazy(&self.repo, id, target)?;
        Ok(check_integration(&signals))
    }

    fn branch_diff_stats(&self, base: &str, head: &str) -> anyhow::Result<LineDiff> {
        self.repo.branch_diff_stats(base, head)
    }

    fn create_workspace(&self, name: &str, base: Option<&str>, path: &Path) -> anyhow::Result<()> {
        self.repo.create_worktree(name, base, path)
    }

    fn remove_workspace(&self, name: &str) -> anyhow::Result<()> {
        let path = self.workspace_path(name)?;
        self.repo.remove_worktree(&path, false)
    }

    fn has_staging_area(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::git::WorktreeInfo;

    use super::super::WorkspaceItem;

    #[test]
    fn test_from_worktree_with_branch() {
        let wt = WorktreeInfo {
            path: PathBuf::from("/repos/myrepo.feature"),
            head: "abc123".into(),
            branch: Some("feature".into()),
            bare: false,
            detached: false,
            locked: None,
            prunable: None,
        };

        let item = WorkspaceItem::from_worktree(wt, false);

        assert_eq!(item.name, "feature");
        assert_eq!(item.head, "abc123");
        assert_eq!(item.branch, Some("feature".into()));
        assert_eq!(item.path, PathBuf::from("/repos/myrepo.feature"));
        assert!(!item.is_default);
    }

    #[test]
    fn test_from_worktree_detached() {
        let wt = WorktreeInfo {
            path: PathBuf::from("/repos/myrepo.detached"),
            head: "def456".into(),
            branch: None,
            bare: false,
            detached: true,
            locked: None,
            prunable: None,
        };

        let item = WorkspaceItem::from_worktree(wt, true);

        // Falls back to directory name when no branch
        assert_eq!(item.name, "myrepo.detached");
        assert_eq!(item.head, "def456");
        assert_eq!(item.branch, None);
        assert!(item.is_default);
    }

    #[test]
    fn test_from_worktree_locked() {
        let wt = WorktreeInfo {
            path: PathBuf::from("/repos/myrepo.locked"),
            head: "789abc".into(),
            branch: Some("locked-branch".into()),
            bare: false,
            detached: false,
            locked: Some("in use".into()),
            prunable: None,
        };

        let item = WorkspaceItem::from_worktree(wt, false);

        assert_eq!(item.locked, Some("in use".into()));
        assert_eq!(item.prunable, None);
    }

    #[test]
    fn test_from_worktree_prunable() {
        let wt = WorktreeInfo {
            path: PathBuf::from("/repos/myrepo.gone"),
            head: "000000".into(),
            branch: Some("gone-branch".into()),
            bare: false,
            detached: false,
            locked: None,
            prunable: Some("directory missing".into()),
        };

        let item = WorkspaceItem::from_worktree(wt, false);

        assert_eq!(item.prunable, Some("directory missing".into()));
    }
}
