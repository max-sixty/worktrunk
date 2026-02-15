//! WorkingTree - a borrowed handle for worktree-specific git operations.

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};

use crate::shell_exec::Cmd;
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
    /// Get a reference to the repository this worktree belongs to.
    pub fn repo(&self) -> &Repository {
        self.repo
    }

    /// Get the path this WorkingTree was created with.
    ///
    /// This is the path passed to `worktree_at()` or `base_path()` for `current_worktree()`.
    /// For the canonical git-determined root, use [`root()`](Self::root) instead.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Run a git command in this worktree and return stdout.
    pub fn run_command(&self, args: &[&str]) -> anyhow::Result<String> {
        let output = self.run_command_output(args)?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stderr = stderr.replace('\r', "\n");
            let stdout = String::from_utf8_lossy(&output.stdout);
            let error_msg = [stderr.trim(), stdout.trim()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            bail!("{}", error_msg);
        }

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        Ok(stdout)
    }

    /// Run a git command in this worktree and return the raw Output.
    ///
    /// Use this when you need to check exit codes directly (e.g., for commands
    /// where non-zero exit is not an error condition).
    pub fn run_command_output(&self, args: &[&str]) -> anyhow::Result<std::process::Output> {
        Cmd::new("git")
            .args(args.iter().copied())
            .current_dir(&self.path)
            .context(path_to_logging_context(&self.path))
            .run()
            .with_context(|| format!("Failed to execute: git {}", args.join(" ")))
    }

    // =========================================================================
    // Worktree-specific methods
    // =========================================================================

    /// Get the branch checked out in this worktree, or None if in detached HEAD state.
    ///
    /// Result is cached in the repository's shared cache (keyed by worktree path).
    /// Errors (e.g., permission denied, corrupted `.git`) are propagated, not swallowed.
    pub fn branch(&self) -> anyhow::Result<Option<String>> {
        // Check cache first
        if let Some(cached) = self.repo.cache.current_branches.get(&self.path) {
            return Ok(cached.clone());
        }

        // Not cached - run git command and propagate errors
        let stdout = self
            .run_command(&["branch", "--show-current"])
            .context("Failed to determine current branch")?;

        let branch = stdout.trim();
        let result = if branch.is_empty() {
            None // Detached HEAD
        } else {
            Some(branch.to_string())
        };

        // Cache the successful result
        self.repo
            .cache
            .current_branches
            .insert(self.path.clone(), result.clone());

        Ok(result)
    }

    /// Check if the working tree has uncommitted changes.
    ///
    /// Note: This does NOT detect files hidden via `git update-index --assume-unchanged`
    /// or `--skip-worktree`. We intentionally skip that check because:
    /// 1. Detecting hidden files requires `git ls-files -v` which lists ALL tracked files
    /// 2. On large repos (70k+ files), this adds noticeable latency to every clean check
    /// 3. Users who use skip-worktree are power users who understand the implications
    /// 4. A warning wouldn't prevent data loss anyway — it's informational only
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
    /// Always returns a canonicalized absolute path, resolving symlinks.
    /// This ensures consistent comparison with `git_common_dir()`.
    pub fn git_dir(&self) -> anyhow::Result<PathBuf> {
        let stdout = self.run_command(&["rev-parse", "--git-dir"])?;
        let path = PathBuf::from(stdout.trim());

        // Always canonicalize to resolve symlinks (e.g., /var -> /private/var on macOS)
        let absolute_path = if path.is_relative() {
            self.path.join(&path)
        } else {
            path
        };
        canonicalize(&absolute_path).context("Failed to resolve git directory")
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
    /// - `force_hint` when true, the error hint mentions `--force` as an alternative.
    pub fn ensure_clean(
        &self,
        action: &str,
        branch: Option<&str>,
        force_hint: bool,
    ) -> anyhow::Result<()> {
        if self.is_dirty()? {
            return Err(GitError::UncommittedChanges {
                action: Some(action.into()),
                branch: branch.map(String::from),
                force_hint,
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

    /// Determine whether there are staged changes in the index.
    ///
    /// Returns `Ok(true)` when staged changes are present, `Ok(false)` otherwise.
    ///
    /// Note: The index is per-worktree in git, so this checks this specific
    /// worktree's staging area.
    pub fn has_staged_changes(&self) -> anyhow::Result<bool> {
        // Exit code 0 = no diff (no staged changes), exit code 1 = diff exists (has staged changes)
        // run_command returns Ok on exit 0, Err on non-zero
        // So: Err means has changes
        Ok(self
            .run_command(&["diff", "--cached", "--quiet", "--exit-code"])
            .is_err())
    }

    /// Create a safety backup of current working tree state without affecting the working tree.
    ///
    /// This creates a backup commit containing all changes (staged, unstaged, and untracked files)
    /// and stores it in a custom ref (`refs/wt-backup/<branch>`). This creates a reflog entry
    /// for recovery without polluting the stash list. The working tree remains unchanged.
    ///
    /// Users can find safety backups with: `git reflog show refs/wt-backup/<branch>`
    ///
    /// Returns the short SHA of the backup commit.
    ///
    /// # Example
    /// ```no_run
    /// use worktrunk::git::Repository;
    ///
    /// let repo = Repository::current()?;
    /// let wt = repo.current_worktree();
    /// let sha = wt.create_safety_backup("feature → main (squash)")?;
    /// println!("Backup created: {}", sha);
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn create_safety_backup(&self, message: &str) -> anyhow::Result<String> {
        // Create a backup commit using git stash create (without storing it in the stash list)
        let backup_sha = self
            .run_command(&["stash", "create", "--include-untracked"])?
            .trim()
            .to_string();

        // Validate that we got a SHA back
        if backup_sha.is_empty() {
            return Err(GitError::Other {
                message: "git stash create returned empty SHA - no changes to backup".into(),
            }
            .into());
        }

        // Get current branch name to use in the ref name
        let branch = self
            .run_command(&["rev-parse", "--abbrev-ref", "HEAD"])?
            .trim()
            .to_string();

        // Sanitize branch name for use in ref path (replace / with -)
        let safe_branch = branch.replace('/', "-");

        // Update a custom ref to point to this commit
        // --create-reflog ensures the reflog is created for this custom ref
        // This creates a reflog entry but doesn't add to the stash list
        let ref_name = format!("refs/wt-backup/{}", safe_branch);
        self.run_command(&[
            "update-ref",
            "--create-reflog",
            "-m",
            message,
            &ref_name,
            &backup_sha,
        ])
        .context("Failed to create backup ref")?;

        Ok(backup_sha[..7].to_string())
    }
}
