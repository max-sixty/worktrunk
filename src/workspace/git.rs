//! Git implementation of the [`Workspace`] trait.
//!
//! Implements [`Workspace`] directly on [`Repository`], mapping git-specific
//! types to the VCS-agnostic [`WorkspaceItem`] and [`Workspace`] interface.
//!
//! Commands that need git-specific features (staging, interactive squash)
//! can downcast via `workspace.as_any().downcast_ref::<Repository>()`.

use std::any::Any;
use std::path::{Path, PathBuf};

use anyhow::Context;
use color_print::cformat;

use crate::git::{Repository, check_integration, compute_integration_lazy};

use super::types::{IntegrationReason, LineDiff, path_dir_name};
use crate::styling::{eprintln, progress_message};

use super::{RebaseOutcome, VcsKind, Workspace, WorkspaceItem};

impl Workspace for Repository {
    fn kind(&self) -> VcsKind {
        VcsKind::Git
    }

    fn list_workspaces(&self) -> anyhow::Result<Vec<WorkspaceItem>> {
        let worktrees = self.list_worktrees()?;
        let primary_path = self.primary_worktree()?;

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
        let worktrees = self.list_worktrees()?;

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
        self.primary_worktree()
    }

    fn default_branch_name(&self) -> anyhow::Result<Option<String>> {
        Ok(self.default_branch())
    }

    fn is_dirty(&self, path: &Path) -> anyhow::Result<bool> {
        self.worktree_at(path).is_dirty()
    }

    fn working_diff(&self, path: &Path) -> anyhow::Result<LineDiff> {
        self.worktree_at(path).working_tree_diff_stats()
    }

    fn ahead_behind(&self, base: &str, head: &str) -> anyhow::Result<(usize, usize)> {
        // Note: this calls Repository::ahead_behind inherent method directly
        Repository::ahead_behind(self, base, head)
    }

    fn is_integrated(&self, id: &str, target: &str) -> anyhow::Result<Option<IntegrationReason>> {
        let signals = compute_integration_lazy(self, id, target)?;
        Ok(check_integration(&signals))
    }

    fn branch_diff_stats(&self, base: &str, head: &str) -> anyhow::Result<LineDiff> {
        Repository::branch_diff_stats(self, base, head)
    }

    fn create_workspace(&self, name: &str, base: Option<&str>, path: &Path) -> anyhow::Result<()> {
        self.create_worktree(name, base, path)
    }

    fn remove_workspace(&self, name: &str) -> anyhow::Result<()> {
        let path = Workspace::workspace_path(self, name)?;
        self.remove_worktree(&path, false)
    }

    fn resolve_integration_target(&self, target: Option<&str>) -> anyhow::Result<String> {
        self.require_target_ref(target)
    }

    fn is_rebased_onto(&self, target: &str, _path: &Path) -> anyhow::Result<bool> {
        // Call the inherent method via fully-qualified syntax
        Repository::is_rebased_onto(self, target)
    }

    fn rebase_onto(&self, target: &str, _path: &Path) -> anyhow::Result<RebaseOutcome> {
        // Detect fast-forward: merge-base == HEAD means HEAD is behind target
        let merge_base = self
            .merge_base("HEAD", target)?
            .context("Cannot rebase: no common ancestor with target branch")?;
        let head_sha = self.run_command(&["rev-parse", "HEAD"])?.trim().to_string();
        let is_fast_forward = merge_base == head_sha;

        // Only show progress for true rebases (fast-forwards are instant)
        if !is_fast_forward {
            eprintln!(
                "{}",
                progress_message(cformat!("Rebasing onto <bold>{target}</>..."))
            );
        }

        let rebase_result = self.run_command(&["rebase", target]);

        if let Err(e) = rebase_result {
            let is_rebasing = self
                .worktree_state()?
                .is_some_and(|s| s.starts_with("REBASING"));
            if is_rebasing {
                let git_output = e.to_string();
                return Err(crate::git::GitError::RebaseConflict {
                    target_branch: target.to_string(),
                    git_output,
                }
                .into());
            }
            return Err(crate::git::GitError::Other {
                message: format!("Failed to rebase onto {target}: {e}"),
            }
            .into());
        }

        // Verify rebase completed successfully
        if self.worktree_state()?.is_some() {
            return Err(crate::git::GitError::RebaseConflict {
                target_branch: target.to_string(),
                git_output: String::new(),
            }
            .into());
        }

        if is_fast_forward {
            Ok(RebaseOutcome::FastForward)
        } else {
            Ok(RebaseOutcome::Rebased)
        }
    }

    fn root_path(&self) -> anyhow::Result<PathBuf> {
        Ok(self.repo_path().to_path_buf())
    }

    fn current_workspace_path(&self) -> anyhow::Result<PathBuf> {
        Ok(self.current_worktree().path().to_path_buf())
    }

    fn current_name(&self, path: &Path) -> anyhow::Result<Option<String>> {
        self.worktree_at(path).branch()
    }

    fn project_identifier(&self) -> anyhow::Result<String> {
        // Call the inherent method via fully-qualified syntax
        Repository::project_identifier(self)
    }

    fn commit(&self, message: &str, path: &Path) -> anyhow::Result<String> {
        self.worktree_at(path)
            .run_command(&["commit", "-m", message])
            .context("Failed to create commit")?;
        let sha = self
            .worktree_at(path)
            .run_command(&["rev-parse", "HEAD"])?
            .trim()
            .to_string();
        Ok(sha)
    }

    fn commit_subjects(&self, base: &str, head: &str) -> anyhow::Result<Vec<String>> {
        let range = format!("{base}..{head}");
        let output = self.run_command(&["log", "--format=%s", &range])?;
        Ok(output.lines().map(|l| l.to_string()).collect())
    }

    fn push_to_target(&self, target: &str, _path: &Path) -> anyhow::Result<()> {
        self.run_command(&["push", "origin", &format!("HEAD:{target}")])?;
        Ok(())
    }

    fn has_staging_area(&self) -> bool {
        true
    }

    fn as_any(&self) -> &dyn Any {
        self
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

    /// Exercise all `Workspace` trait methods on a real git repository.
    ///
    /// This covers the `Workspace for Repository` implementation which
    /// maps `Repository` methods into the VCS-agnostic trait.
    #[test]
    fn test_workspace_trait_on_real_repo() {
        use std::process::Command;

        use super::super::{VcsKind, Workspace};
        use crate::git::Repository;

        let temp = tempfile::tempdir().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();

        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(&repo_path)
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@test.com")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@test.com")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };

        git(&["init", "-b", "main"]);
        std::fs::write(repo_path.join("file.txt"), "hello\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "initial"]);

        let repo = Repository::at(&repo_path).unwrap();
        let ws: &dyn Workspace = &repo;

        // kind
        assert_eq!(ws.kind(), VcsKind::Git);

        // has_staging_area
        assert!(ws.has_staging_area());

        // list_workspaces
        let items = ws.list_workspaces().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "main");
        assert!(items[0].is_default);

        // default_workspace_path
        assert!(ws.default_workspace_path().unwrap().is_some());

        // default_branch_name
        let _ = ws.default_branch_name().unwrap();

        // is_dirty — clean state
        assert!(!ws.is_dirty(&repo_path).unwrap());

        // working_diff — clean
        let diff = ws.working_diff(&repo_path).unwrap();
        assert_eq!(diff.added, 0);
        assert_eq!(diff.deleted, 0);

        // Make dirty
        std::fs::write(repo_path.join("file.txt"), "modified\n").unwrap();
        assert!(ws.is_dirty(&repo_path).unwrap());
        let diff = ws.working_diff(&repo_path).unwrap();
        assert!(diff.added > 0 || diff.deleted > 0);
        git(&["checkout", "--", "."]);

        // Create feature branch with a commit
        git(&["checkout", "-b", "feature"]);
        std::fs::write(repo_path.join("feature.txt"), "feature\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "feature commit"]);

        // ahead_behind
        let (ahead, behind) = ws.ahead_behind("main", "feature").unwrap();
        assert_eq!(ahead, 1);
        assert_eq!(behind, 0);

        // is_integrated — feature is NOT an ancestor of main
        let integrated = ws.is_integrated("feature", "main").unwrap();
        assert!(integrated.is_none());

        // branch_diff_stats
        let diff = ws.branch_diff_stats("main", "feature").unwrap();
        assert!(diff.added > 0);

        // Switch back to main for workspace mutation tests
        git(&["checkout", "main"]);

        // create_workspace (also covers Repository::create_worktree)
        let wt_path = temp.path().join("test-wt");
        ws.create_workspace("test-branch", None, &wt_path).unwrap();

        // workspace_path — found by branch name
        let path = ws.workspace_path("test-branch").unwrap();
        assert_eq!(
            dunce::canonicalize(&path).unwrap(),
            dunce::canonicalize(&wt_path).unwrap()
        );

        // remove_workspace
        ws.remove_workspace("test-branch").unwrap();

        // workspace_path — not found
        assert!(ws.workspace_path("nonexistent").is_err());

        // create_workspace with base revision (covers the `if let Some(base_ref)` branch)
        let wt_path2 = temp.path().join("test-wt2");
        ws.create_workspace("from-feature", Some("feature"), &wt_path2)
            .unwrap();
        ws.remove_workspace("from-feature").unwrap();

        // as_any downcast
        let repo_ref = ws.as_any().downcast_ref::<Repository>();
        assert!(repo_ref.is_some());
    }
}
