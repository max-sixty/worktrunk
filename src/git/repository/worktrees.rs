//! Worktree management operations for Repository.

use std::path::{Path, PathBuf};

use color_print::cformat;
use dunce::canonicalize;
use normalize_path::NormalizePath;

use super::{GitError, Repository, ResolvedWorktree, WorktreeInfo};
use crate::path::format_path_for_display;

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
        let mut worktrees: Vec<_> = raw_worktrees.into_iter().filter(|wt| !wt.bare).collect();

        // Submodule path correction.
        //
        // Git's `get_main_worktree()` computes the main worktree path by stripping
        // a trailing `/.git` from the common dir. For submodules, the common dir is
        // `.git/modules/sub` (no trailing `/.git`), so git leaves it unchanged —
        // reporting the git data directory as the "main worktree" path. Git does not
        // consult `core.worktree` in this code path.
        //
        // We detect this by checking whether the first worktree's path equals
        // git_common_dir (which never holds for normal repos, where git_common_dir
        // is `.git` inside the worktree). When matched, we correct it using
        // repo_path(), which resolves the actual working directory via
        // `git rev-parse --show-toplevel` (which does read core.worktree).
        //
        // We fix this here rather than at each call site because list_worktrees()
        // is the single point where worktree paths enter the system — all consumers
        // (worktree_for_branch, current_worktree_info, resolve_worktree, etc.)
        // depend on paths being working directories. If git fixes this upstream,
        // the condition stops triggering.
        if let Some(first) = worktrees.first_mut()
            && canonicalize(&first.path).ok().as_deref() == Some(self.git_common_dir())
        {
            first.path = self.repo_path().to_path_buf();
        }

        Ok(worktrees)
    }

    /// Get the WorktreeInfo struct for the current worktree, if we're inside one.
    ///
    /// Returns `None` if not in a worktree (e.g., in bare repo directory).
    ///
    /// Note: For worktree-specific operations, use [`current_worktree()`](Self::current_worktree)
    /// to get a [`WorkingTree`](super::WorkingTree) instead.
    pub fn current_worktree_info(&self) -> anyhow::Result<Option<WorktreeInfo>> {
        // root() returns canonicalized path, so canonicalize worktree paths for comparison
        // to handle symlinks (e.g., macOS /var -> /private/var)
        let current_path = match self.current_worktree().root() {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };
        let worktrees = self.list_worktrees()?;
        Ok(worktrees.into_iter().find(|wt| {
            canonicalize(&wt.path)
                .map(|p| p == current_path)
                .unwrap_or(false)
        }))
    }

    /// Find the worktree path for a given branch, if one exists.
    pub fn worktree_for_branch(&self, branch: &str) -> anyhow::Result<Option<PathBuf>> {
        let worktrees = self.list_worktrees()?;

        Ok(worktrees
            .iter()
            .find(|wt| wt.branch.as_deref() == Some(branch))
            .map(|wt| wt.path.clone()))
    }

    /// The "home" worktree — main worktree for normal repos, default branch worktree for bare.
    ///
    /// Used as the default source for `copy-ignored` and the `{{ primary_worktree_path }}` template.
    /// Returns `None` for bare repos when no worktree has the default branch.
    pub fn primary_worktree(&self) -> anyhow::Result<Option<PathBuf>> {
        if self.is_bare()? {
            let Some(branch) = self.default_branch() else {
                return Ok(None);
            };
            self.worktree_for_branch(&branch)
        } else {
            Ok(Some(self.repo_path().to_path_buf()))
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

    /// Prune worktree entries whose directories no longer exist.
    ///
    /// Git tracks worktrees in `.git/worktrees/`. If a worktree directory is deleted
    /// externally (e.g., `rm -rf`), this method runs `git worktree prune` to clean
    /// up the entries.
    pub fn prune_worktrees(&self) -> anyhow::Result<()> {
        self.run_command(&["worktree", "prune"])?;
        Ok(())
    }

    /// Remove a worktree at the specified path.
    ///
    /// When `force` is true, passes `--force` to `git worktree remove`,
    /// allowing removal even when the worktree contains untracked files
    /// (like build artifacts such as `.vite/` or `node_modules/`).
    ///
    /// When the worktree contains initialized submodules, git refuses removal
    /// even for clean worktrees. This method detects that case and automatically
    /// retries with `--force`, which is safe because the caller has already
    /// validated worktree cleanliness via `ensure_clean()`.
    ///
    /// # Why git requires `--force` for submodules
    ///
    /// Git's `--force` flag on `worktree remove` bypasses two unrelated
    /// protections under one flag: dirty working tree checks AND the
    /// submodule structural check. We separate these concerns — our
    /// `ensure_clean()` handles dirty state, and the `--force` retry
    /// handles the submodule restriction.
    ///
    /// # TOCTOU note
    ///
    /// Git checks for submodules *before* checking for dirty files. If a
    /// file is modified between our `ensure_clean()` and the git command,
    /// git reports the submodule error (not the dirty error), so the
    /// `--force` retry fires and bypasses git's dirty check. This is the
    /// same TOCTOU window that exists for all removal (between
    /// `ensure_clean()` and the actual delete), but for non-submodule
    /// worktrees git's own dirty check acts as an accidental backstop
    /// that we lose here. The window is milliseconds.
    pub fn remove_worktree(&self, path: &std::path::Path, force: bool) -> anyhow::Result<()> {
        let path_str = path.to_str().ok_or_else(|| {
            anyhow::Error::from(GitError::Other {
                message: format!(
                    "Worktree path contains invalid UTF-8: {}",
                    format_path_for_display(path)
                ),
            })
        })?;
        let mut args = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        args.push(path_str);

        match self.run_command(&args) {
            Ok(_) => Ok(()),
            Err(e)
                if !force
                    && e.to_string()
                        .contains("submodules cannot be moved or removed") =>
            {
                // Submodule retry — see doc comment above for safety analysis.
                log::debug!("Retrying worktree removal with --force due to submodules");
                self.run_command(&["worktree", "remove", "--force", path_str])?;
                Ok(())
            }
            Err(e) => Err(e),
        }
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
                self.switch_previous().ok_or_else(|| {
                    GitError::Other {
                        message: cformat!(
                            "No previous branch found in history. Run <bright-black>wt list</> to see available worktrees."
                        ),
                    }
                    .into()
                })
            }
            "^" => self.default_branch().ok_or_else(|| {
                GitError::Other {
                    message: cformat!(
                        "Cannot determine default branch. Specify target explicitly or run <bright-black>wt config state default-branch set <bold>BRANCH</></>"
                    ),
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
                // root() returns canonicalized path, so canonicalize worktree paths
                // for comparison to handle symlinks (e.g., macOS /var -> /private/var)
                let worktrees = self.list_worktrees()?;
                let branch = worktrees
                    .iter()
                    .find(|wt| canonicalize(&wt.path).map(|p| p == path).unwrap_or(false))
                    .and_then(|wt| wt.branch.clone());
                Ok(ResolvedWorktree::Worktree { path, branch })
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
    /// Returns the primary worktree if it exists, otherwise the repo root.
    /// - Normal repos: the main worktree (repo root)
    /// - Bare repos: the default branch's worktree, or the bare repo directory
    pub fn home_path(&self) -> anyhow::Result<PathBuf> {
        Ok(self
            .primary_worktree()?
            .unwrap_or_else(|| self.repo_path().to_path_buf()))
    }
}
