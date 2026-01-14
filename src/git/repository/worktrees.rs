//! Worktree management operations for Repository.

use std::path::{Path, PathBuf};

use normalize_path::NormalizePath;

use super::{GitError, Repository, ResolvedWorktree, WorktreeInfo};

impl Repository {
    /// List all worktrees for this repository.
    ///
    /// Returns a list of worktrees with bare entries filtered out.
    ///
    /// **Ordering:** Git lists the main worktree first. For normal repos, `[0]` is
    /// the main worktree. For bare repos, the bare entry is filtered out, so `[0]`
    /// is the first linked worktree (no semantic "main" exists).
    ///
    /// Returns an empty vec for bare repos with no linked worktrees.
    pub fn list_worktrees(&self) -> anyhow::Result<Vec<WorktreeInfo>> {
        let stdout = self.run_command(&["worktree", "list", "--porcelain"])?;
        let raw_worktrees = WorktreeInfo::parse_porcelain_list(&stdout)?;
        Ok(raw_worktrees.into_iter().filter(|wt| !wt.bare).collect())
    }

    /// Get the WorktreeInfo struct for the current worktree, if we're inside one.
    ///
    /// Returns `None` if not in a worktree (e.g., in bare repo directory).
    ///
    /// Note: For worktree-specific operations, use [`current_worktree()`](Self::current_worktree)
    /// to get a [`WorkingTree`](super::WorkingTree) instead.
    pub fn current_worktree_info(&self) -> anyhow::Result<Option<WorktreeInfo>> {
        let current_path = match self.current_worktree().root() {
            Ok(p) => p.to_path_buf(),
            Err(_) => return Ok(None),
        };
        let worktrees = self.list_worktrees()?;
        Ok(worktrees.into_iter().find(|wt| wt.path == current_path))
    }

    /// Find the worktree path for a given branch, if one exists.
    pub fn worktree_for_branch(&self, branch: &str) -> anyhow::Result<Option<PathBuf>> {
        let worktrees = self.list_worktrees()?;

        Ok(worktrees
            .iter()
            .find(|wt| wt.branch.as_deref() == Some(branch))
            .map(|wt| wt.path.clone()))
    }

    /// The primary worktree — where established files (ignored files, configs) typically live.
    ///
    /// - Normal repos: the main worktree (repo root) — this is where you cloned and set things up
    /// - Bare repos: the default branch's worktree — no main worktree exists
    ///
    /// Returns `None` for bare repos when no worktree has the default branch.
    ///
    /// For template var `{{ primary_worktree_path }}`.
    pub fn primary_worktree(&self) -> anyhow::Result<Option<PathBuf>> {
        if self.is_bare()? {
            let Some(branch) = self.default_branch() else {
                return Ok(None);
            };
            self.worktree_for_branch(&branch)
        } else {
            Ok(Some(self.repo_path()?))
        }
    }

    /// Find the worktree at a given path, returning its branch if known.
    ///
    /// Returns `Some((path, branch))` if a worktree exists at the path,
    /// where `branch` is `None` for detached HEAD worktrees.
    pub fn worktree_at_path(
        &self,
        path: &Path,
    ) -> anyhow::Result<Option<(PathBuf, Option<String>)>> {
        let worktrees = self.list_worktrees()?;
        // Use lexical normalization so comparison works even when path doesn't exist
        let normalized_path = path.normalize();

        Ok(worktrees
            .iter()
            .find(|wt| wt.path.normalize() == normalized_path)
            .map(|wt| (wt.path.clone(), wt.branch.clone())))
    }

    /// Remove a worktree at the specified path.
    ///
    /// When `force` is true, passes `--force` to `git worktree remove`,
    /// allowing removal even when the worktree contains untracked files
    /// (like build artifacts such as `.vite/` or `node_modules/`).
    pub fn remove_worktree(&self, path: &std::path::Path, force: bool) -> anyhow::Result<()> {
        let path_str = path.to_str().ok_or_else(|| {
            anyhow::Error::from(GitError::Other {
                message: format!("Worktree path contains invalid UTF-8: {}", path.display()),
            })
        })?;
        let mut args = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        args.push(path_str);
        self.run_command(&args)?;
        Ok(())
    }

    /// Resolve a worktree name, expanding "@" to current, "-" to previous, and "^" to main.
    ///
    /// # Arguments
    /// * `name` - The worktree name to resolve:
    ///   - "@" for current HEAD
    ///   - "-" for previous branch (via worktrunk.history)
    ///   - "^" for default branch
    ///   - any other string is returned as-is
    ///
    /// # Returns
    /// - `Ok(name)` if not a special symbol
    /// - `Ok(current_branch)` if "@" and on a branch
    /// - `Ok(previous_branch)` if "-" and worktrunk.history has a previous branch
    /// - `Ok(default_branch)` if "^"
    /// - `Err(DetachedHead)` if "@" and in detached HEAD state
    /// - `Err` if "-" but no previous branch in history
    pub fn resolve_worktree_name(&self, name: &str) -> anyhow::Result<String> {
        match name {
            "@" => self.current_worktree().branch()?.ok_or_else(|| {
                GitError::DetachedHead {
                    action: Some("resolve '@' to current branch".into()),
                }
                .into()
            }),
            "-" => {
                // Read from worktrunk.history (recorded by wt switch operations)
                self.get_switch_previous().ok_or_else(|| {
                    GitError::Other {
                        message:
                            "No previous branch found in history. Use 'wt list' to see available worktrees."
                                .into(),
                    }
                    .into()
                })
            }
            "^" => self.default_branch().ok_or_else(|| {
                GitError::Other {
                    message: "Cannot determine default branch. Specify target explicitly or run 'wt config state default-branch set <branch>'.".into(),
                }
                .into()
            }),
            _ => Ok(name.to_string()),
        }
    }

    /// Resolve a worktree by name, returning its path and branch (if known).
    ///
    /// Unlike `resolve_worktree_name` which returns a branch name, this returns
    /// the worktree path directly. This is useful for commands like `wt remove`
    /// that operate on worktrees, not branches.
    ///
    /// # Arguments
    /// * `name` - The worktree name to resolve:
    ///   - "@" for current worktree (works even in detached HEAD)
    ///   - "-" for previous branch's worktree
    ///   - "^" for main worktree
    ///   - any other string is treated as a branch name
    ///
    /// # Returns
    /// - `Worktree { path, branch }` if a worktree exists
    /// - `BranchOnly { branch }` if only the branch exists (no worktree)
    /// - `Err` if neither worktree nor branch exists
    pub fn resolve_worktree(&self, name: &str) -> anyhow::Result<ResolvedWorktree> {
        match name {
            "@" => {
                // Current worktree by path - works even in detached HEAD
                // If worktree_root fails (e.g., in bare repo directory), give a clear error
                let path = self
                    .current_worktree()
                    .root()
                    .map_err(|_| GitError::NotInWorktree {
                        action: Some("resolve '@'".into()),
                    })?;
                let worktrees = self.list_worktrees()?;
                let branch = worktrees
                    .iter()
                    .find(|wt| wt.path == path)
                    .and_then(|wt| wt.branch.clone());
                Ok(ResolvedWorktree::Worktree {
                    path: path.to_path_buf(),
                    branch,
                })
            }
            _ => {
                // Resolve to branch name first, then find its worktree
                let branch = self.resolve_worktree_name(name)?;
                match self.worktree_for_branch(&branch)? {
                    Some(path) => Ok(ResolvedWorktree::Worktree {
                        path,
                        branch: Some(branch),
                    }),
                    None => Ok(ResolvedWorktree::BranchOnly { branch }),
                }
            }
        }
    }

    /// Find the "home" path - where to cd when leaving a worktree.
    ///
    /// This is the preferred destination after removing the current worktree
    /// or after merge removes the worktree. Priority:
    /// 1. The default branch's worktree (if it exists)
    /// 2. The first worktree in the list
    /// 3. The repo base directory (for bare repos with no worktrees)
    pub fn home_path(&self) -> anyhow::Result<PathBuf> {
        let worktrees = self.list_worktrees()?;
        let default_branch = self.default_branch().unwrap_or_default();

        if let Some(home) = WorktreeInfo::find_home(&worktrees, &default_branch) {
            return Ok(home.path.clone());
        }

        // No worktrees - fall back to repo base (bare repo case)
        self.repo_path()
    }
}
