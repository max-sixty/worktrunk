//! WorkingTree - a borrowed handle for worktree-specific git operations.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};
use dunce::canonicalize;

use super::{GitError, LineDiff, Repository};

/// Get a short display name for a path, used in logging context.
pub fn path_to_logging_context(path: &Path) -> String {
    if path.to_str() == Some(".") {
        ".".to_string()
    } else {
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(".")
            .to_string()
    }
}

/// A borrowed handle for running git commands in a specific worktree.
///
/// This type borrows a [`Repository`] and holds a path to a specific worktree.
/// All worktree-specific operations (like `branch`, `is_dirty`) are on this type.
///
/// For an owned equivalent that can be cloned across threads, see [`super::super::BranchRef`].
///
/// # Examples
///
/// ```no_run
/// use worktrunk::git::Repository;
///
/// let repo = Repository::current()?;
/// let wt = repo.current_worktree();
///
/// // Worktree-specific operations
/// let _ = wt.is_dirty();
/// let _ = wt.branch();
///
/// // View at a different worktree
/// let _other = repo.worktree_at("/path/to/other/worktree");
/// # Ok::<(), anyhow::Error>(())
/// ```
#[derive(Debug)]
#[must_use]
pub struct WorkingTree<'a> {
    pub(super) repo: &'a Repository,
    pub(super) path: PathBuf,
}

impl<'a> WorkingTree<'a> {
    /// Run a git command in this worktree and return stdout.
    pub fn run_command(&self, args: &[&str]) -> anyhow::Result<String> {
        use crate::shell_exec::run;

        let mut cmd = Command::new("git");
        cmd.args(args);
        cmd.current_dir(&self.path);

        let output = run(&mut cmd, Some(&path_to_logging_context(&self.path)))
            .with_context(|| format!("Failed to execute: git {}", args.join(" ")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stderr = stderr.replace('\r', "\n");
            for line in stderr.trim().lines() {
                log::debug!("  ! {}", line);
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            let error_msg = [stderr.trim(), stdout.trim()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            bail!("{}", error_msg);
        }

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        if !stdout.is_empty() {
            for line in stdout.trim().lines() {
                log::debug!("  {}", line);
            }
        }
        Ok(stdout)
    }

    // =========================================================================
    // Worktree-specific methods
    // =========================================================================

    /// Get the branch checked out in this worktree, or None if in detached HEAD state.
    ///
    /// Result is cached in the repository's shared cache (keyed by worktree path).
    pub fn branch(&self) -> anyhow::Result<Option<String>> {
        Ok(self
            .repo
            .cache
            .current_branches
            .entry(self.path.clone())
            .or_insert_with(|| {
                self.run_command(&["branch", "--show-current"])
                    .ok()
                    .and_then(|s| {
                        let branch = s.trim();
                        if branch.is_empty() {
                            None // Detached HEAD
                        } else {
                            Some(branch.to_string())
                        }
                    })
            })
            .clone())
    }

    /// Check if the working tree has uncommitted changes.
    pub fn is_dirty(&self) -> anyhow::Result<bool> {
        let stdout = self.run_command(&["status", "--porcelain"])?;
        Ok(!stdout.trim().is_empty())
    }

    /// Get the root directory of this worktree (top-level of the working tree).
    ///
    /// Returns the canonicalized absolute path to the top-level directory.
    /// This could be the main worktree or a linked worktree.
    /// Result is cached in the repository's shared cache (keyed by worktree path).
    pub fn root(&self) -> anyhow::Result<PathBuf> {
        Ok(self
            .repo
            .cache
            .worktree_roots
            .entry(self.path.clone())
            .or_insert_with(|| {
                self.run_command(&["rev-parse", "--show-toplevel"])
                    .ok()
                    .map(|s| PathBuf::from(s.trim()))
                    .and_then(|p| canonicalize(&p).ok())
                    .unwrap_or_else(|| self.path.clone())
            })
            .clone())
    }

    /// Get the git directory (may be different from common-dir in worktrees).
    ///
    /// Always returns an absolute path, resolving any relative paths returned by git.
    pub fn git_dir(&self) -> anyhow::Result<PathBuf> {
        let stdout = self.run_command(&["rev-parse", "--git-dir"])?;
        let path = PathBuf::from(stdout.trim());

        // Resolve relative paths against the worktree's directory
        if path.is_relative() {
            canonicalize(self.path.join(&path)).context("Failed to resolve git directory")
        } else {
            Ok(path)
        }
    }

    /// Check if a rebase is in progress.
    pub fn is_rebasing(&self) -> anyhow::Result<bool> {
        let git_dir = self.git_dir()?;
        Ok(git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists())
    }

    /// Check if a merge is in progress.
    pub fn is_merging(&self) -> anyhow::Result<bool> {
        let git_dir = self.git_dir()?;
        Ok(git_dir.join("MERGE_HEAD").exists())
    }

    /// Check if this is a linked worktree (vs the main worktree).
    ///
    /// Returns `true` for linked worktrees (created via `git worktree add`),
    /// `false` for the main worktree (original clone location).
    ///
    /// Implementation: compares `git_dir` vs `common_dir`. In linked worktrees,
    /// the `.git` file points to `.git/worktrees/NAME`, so they differ. In the
    /// main worktree, both point to the same `.git` directory.
    ///
    /// For bare repos, all worktrees are "linked" (returns `true`).
    pub fn is_linked(&self) -> anyhow::Result<bool> {
        let git_dir = self.git_dir()?;
        let common_dir = self.repo.git_common_dir();
        Ok(git_dir != common_dir)
    }

    /// Ensure this worktree is clean (no uncommitted changes).
    ///
    /// Returns an error if there are uncommitted changes.
    /// - `action` describes what was blocked (e.g., "remove worktree").
    /// - `branch` identifies which branch for multi-worktree operations.
    pub fn ensure_clean(&self, action: &str, branch: Option<&str>) -> anyhow::Result<()> {
        if self.is_dirty()? {
            return Err(GitError::UncommittedChanges {
                action: Some(action.into()),
                branch: branch.map(String::from),
            }
            .into());
        }
        Ok(())
    }

    /// Get line diff statistics for working tree changes (unstaged + staged).
    pub fn working_tree_diff_stats(&self) -> anyhow::Result<LineDiff> {
        let stdout = self.run_command(&["diff", "--numstat", "HEAD"])?;
        LineDiff::from_numstat(&stdout)
    }

    /// Get line diff statistics between working tree and a specific ref.
    pub fn working_tree_diff_vs_ref(&self, ref_name: &str) -> anyhow::Result<LineDiff> {
        let stdout = self.run_command(&["diff", "--numstat", ref_name])?;
        LineDiff::from_numstat(&stdout)
    }

    /// Check if HEAD tree matches a base branch tree.
    ///
    /// This runs `rev-parse --verify` + `diff-tree --quiet` and returns the result.
    /// Can be called directly for parallel execution with `git status`.
    pub fn trees_match_base<'b>(
        &self,
        base_branch: Option<&'b str>,
    ) -> anyhow::Result<TreesMatchResult<'b>> {
        let Some(branch) = base_branch else {
            return Ok(TreesMatchResult::NoBaseBranch);
        };

        // Check if branch exists
        if self
            .run_command(&["rev-parse", "--verify", branch])
            .is_err()
        {
            return Ok(TreesMatchResult::BranchNotFound);
        }

        // Check if trees match
        let trees_match = self
            .run_command(&["diff-tree", "--quiet", "HEAD", branch])
            .is_ok();

        if trees_match {
            Ok(TreesMatchResult::TreesMatch { branch })
        } else {
            Ok(TreesMatchResult::TreesDiffer)
        }
    }
}

/// Result of checking if HEAD tree matches a base branch tree.
#[derive(Debug, Clone, Copy)]
pub enum TreesMatchResult<'a> {
    /// No base branch specified (main worktree).
    NoBaseBranch,
    /// Base branch doesn't exist.
    BranchNotFound,
    /// Trees differ between HEAD and base branch.
    TreesDiffer,
    /// Trees match - includes branch name for computing diff if needed.
    TreesMatch { branch: &'a str },
}
