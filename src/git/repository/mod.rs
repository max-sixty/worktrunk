use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

// Import types and functions from parent module (mod.rs)
use super::{DefaultBranchName, DiffStats, GitError, LineDiff, Worktree, WorktreeList};

/// Extension trait for Result types to simplify GitError conversions
///
/// This trait provides ergonomic methods to convert any Result into Result<T, GitError>.
///
/// # Examples
///
/// ```no_run
/// use worktrunk::git::{GitError, GitResultExt};
///
/// fn load_config() -> Result<String, std::io::Error> {
///     std::fs::read_to_string("config.toml")
/// }
///
/// // Without context:
/// let config = load_config().git_err()?;
///
/// // With context:
/// let config = load_config().git_context("Failed to load config")?;
/// # Ok::<(), GitError>(())
/// ```
pub trait GitResultExt<T> {
    /// Convert the error to GitError with additional context
    fn git_context(self, context: &str) -> Result<T, GitError>;

    /// Convert the error to GitError using its Display implementation
    fn git_err(self) -> Result<T, GitError>;
}

impl<T, E: std::fmt::Display> GitResultExt<T> for Result<T, E> {
    fn git_context(self, context: &str) -> Result<T, GitError> {
        use crate::styling::{ERROR, ERROR_EMOJI, format_with_gutter};
        self.map_err(|e| {
            let error_str = e.to_string();
            let header = format!("{ERROR_EMOJI} {ERROR}{context}{ERROR:#}");
            // External errors always use gutter formatting (canonical path)
            let formatted = format!("{}\n\n{}", header, format_with_gutter(&error_str, "", None));
            GitError::CommandFailed(formatted)
        })
    }

    fn git_err(self) -> Result<T, GitError> {
        use crate::styling::{ERROR, ERROR_EMOJI};
        self.map_err(|e| GitError::CommandFailed(format!("{ERROR_EMOJI} {ERROR}{e}{ERROR:#}")))
    }
}

/// Internal layout information for a repository.
///
/// Cached to avoid repeated git queries.
#[derive(Debug, Clone)]
struct RepositoryLayout {
    /// Base path for worktrees (repo root for normal repos, bare repo path for bare repos)
    worktree_base: PathBuf,
}

/// Repository context for git operations.
///
/// Provides a more ergonomic API than the `*_in(path, ...)` functions by
/// encapsulating the repository path.
///
/// # Examples
///
/// ```no_run
/// use worktrunk::git::Repository;
///
/// let repo = Repository::current();
/// let branch = repo.current_branch()?;
/// let is_dirty = repo.is_dirty()?;
/// # Ok::<(), worktrunk::git::GitError>(())
/// ```
#[derive(Debug)]
pub struct Repository {
    path: PathBuf,
    layout: OnceLock<RepositoryLayout>,
}

impl Repository {
    /// Create a repository context at the specified path.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            layout: OnceLock::new(),
        }
    }

    /// Create a repository context for the current directory.
    ///
    /// This is the most common usage pattern.
    pub fn current() -> Self {
        Self::at(".")
    }

    /// Get the path this repository context operates on.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Get the repository layout, initializing it if needed.
    fn layout(&self) -> Result<&RepositoryLayout, GitError> {
        if let Some(layout) = self.layout.get() {
            return Ok(layout);
        }

        let git_common_dir = self
            .git_common_dir()?
            .canonicalize()
            .map_err(|e| GitError::CommandFailed(format!("Failed to canonicalize path: {}", e)))?;

        let is_bare = self.is_bare_repo()?;

        let worktree_base = if is_bare {
            git_common_dir.clone()
        } else {
            git_common_dir
                .parent()
                .ok_or_else(|| GitError::message("Invalid git directory"))?
                .to_path_buf()
        };

        let layout = RepositoryLayout { worktree_base };

        // set() returns Err if already set, but we checked above, so ignore the result
        let _ = self.layout.set(layout);

        // Now get() will succeed
        Ok(self.layout.get().expect("just set"))
    }

    /// Check if this is a bare repository (no working tree).
    fn is_bare_repo(&self) -> Result<bool, GitError> {
        let output = self.run_command(&["config", "--bool", "core.bare"])?;
        Ok(output.trim() == "true")
    }

    /// Get the primary remote name for this repository.
    ///
    /// Uses the following strategy:
    /// 1. If the current branch has an upstream, use its remote
    ///    (Note: Detached HEAD falls through to step 2)
    /// 2. Otherwise, get the first remote from `git remote`
    /// 3. Fall back to "origin" if no remotes exist
    pub fn primary_remote(&self) -> Result<String, GitError> {
        // Try to get the remote from the current branch's upstream
        if let Ok(Some(branch)) = self.current_branch()
            && let Ok(Some(upstream)) = self.upstream_branch(&branch)
            && let Some((remote, _)) = upstream.split_once('/')
        {
            return Ok(remote.to_string());
        }

        // Fall back to first remote in the list
        let output = self.run_command(&["remote"])?;
        let first_remote = output.lines().next();

        Ok(first_remote.unwrap_or("origin").to_string())
    }

    /// Check if a local git branch exists.
    pub fn local_branch_exists(&self, branch: &str) -> Result<bool, GitError> {
        Ok(self
            .run_command(&["rev-parse", "--verify", &format!("refs/heads/{}", branch)])
            .is_ok())
    }

    /// Check if a git branch exists (local or remote).
    pub fn branch_exists(&self, branch: &str) -> Result<bool, GitError> {
        // Try local branch first
        if self.local_branch_exists(branch)? {
            return Ok(true);
        }

        // Try remote branch
        let remote = self.primary_remote()?;
        Ok(self
            .run_command(&[
                "rev-parse",
                "--verify",
                &format!("refs/remotes/{}/{}", remote, branch),
            ])
            .is_ok())
    }

    /// Get the current branch name, or None if in detached HEAD state.
    pub fn current_branch(&self) -> Result<Option<String>, GitError> {
        let stdout = self.run_command(&["branch", "--show-current"])?;
        let branch = stdout.trim();

        if branch.is_empty() {
            Ok(None) // Detached HEAD
        } else {
            Ok(Some(branch.to_string()))
        }
    }

    /// Read a user-defined status from `worktrunk.status.<branch>` in git config.
    pub fn branch_keyed_status(&self, branch: &str) -> Option<String> {
        let config_key = format!("worktrunk.status.{}", branch);
        self.run_command(&["config", "--get", &config_key])
            .ok()
            .map(|output| output.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// Read user-defined status, preferring worktree config then falling back to branch-keyed.
    pub fn user_status(&self, branch: Option<&str>) -> Option<String> {
        if let Ok(output) = self.run_command(&["config", "--worktree", "--get", "worktrunk.status"])
        {
            let status = output.trim().to_string();
            if !status.is_empty() {
                return Some(status);
            }
        }

        branch.and_then(|branch| self.branch_keyed_status(branch))
    }

    /// Record branch history in worktrunk.history for `wt switch -` support.
    ///
    /// Stores "current,previous" to enable ping-pong switching.
    /// This enables `wt switch -` to work after `wt switch` operations,
    /// which don't create git reflog entries.
    pub fn record_switch_history(
        &self,
        current: &str,
        previous: Option<&str>,
    ) -> Result<(), GitError> {
        let value = if let Some(prev) = previous {
            format!("{},{}", current, prev)
        } else {
            current.to_string()
        };
        self.run_command(&["config", "worktrunk.history", &value])?;
        Ok(())
    }

    /// Get the previous branch from worktrunk.history.
    ///
    /// Returns (current, previous) if history is recorded.
    /// For single-value history (legacy), returns (value, None).
    pub fn get_switch_history(&self) -> Option<(String, Option<String>)> {
        self.run_command(&["config", "--get", "worktrunk.history"])
            .ok()
            .and_then(|output| {
                let trimmed = output.trim();
                if trimmed.is_empty() {
                    return None;
                }
                let parts: Vec<&str> = trimmed.splitn(2, ',').collect();
                Some((parts[0].to_string(), parts.get(1).map(|s| s.to_string())))
            })
    }

    /// Resolve a worktree name, expanding "@" to the current branch and "-" to the previous branch.
    ///
    /// # Arguments
    /// * `name` - The worktree name to resolve:
    ///   - "@" for current HEAD
    ///   - "-" for previous branch (via worktrunk.history)
    ///   - any other string is returned as-is
    ///
    /// # Returns
    /// - `Ok(name)` if not "@" or "-"
    /// - `Ok(current_branch)` if "@" and on a branch
    /// - `Ok(previous_branch)` if "-" and worktrunk.history has a previous branch
    /// - `Err(DetachedHead)` if "@" and in detached HEAD state
    /// - `Err` if "-" but no previous branch in history
    pub fn resolve_worktree_name(&self, name: &str) -> Result<String, GitError> {
        match name {
            "@" => self.current_branch()?.ok_or(GitError::DetachedHead),
            "-" => {
                // Read from worktrunk.history (recorded by wt switch operations)
                // History stores (current, previous), we want previous
                self.get_switch_history()
                    .and_then(|(_, prev)| prev)
                    .ok_or_else(|| {
                        GitError::message(
                            "No previous branch found in history. Use 'wt list' to see available worktrees.",
                        )
                    })
            }
            _ => Ok(name.to_string()),
        }
    }

    /// Get the default branch name for the repository.
    ///
    /// **Performance note:** This method may trigger a network call on first invocation
    /// if the remote HEAD is not cached locally. The result is then cached in git's
    /// config for subsequent calls. To minimize latency:
    /// - Defer calling this until after fast, local checks (see e497f0f for example)
    /// - Consider passing the result as a parameter if needed multiple times
    /// - For optional operations, provide a fallback (e.g., `.unwrap_or("main")`)
    ///
    /// Uses a hybrid approach:
    /// 1. Try local cache (`git rev-parse origin/HEAD`) first - fast, no network
    /// 2. If not cached, query remote (`git ls-remote`) - may take 100ms-2s depending on network
    /// 3. Cache the result (`git remote set-head`) for future invocations
    /// 4. If no remote available, infer from local branches (no caching, shows hint)
    pub fn default_branch(&self) -> Result<String, GitError> {
        // Try to get default branch from remote
        if let Ok(remote) = self.primary_remote() {
            // Try local cache first (fast path)
            if let Ok(branch) = self.get_local_default_branch(&remote) {
                return Ok(branch);
            }

            // Query remote and cache it
            if let Ok(branch) = self.query_remote_default_branch(&remote) {
                let _ = self.cache_default_branch(&remote, &branch);
                return Ok(branch);
            }
        }

        // Fallback: No remote or remote query failed, try to infer locally
        // TODO: Show message to user when using inference fallback:
        //   "No remote configured. Using inferred default branch: {branch}"
        //   "To cache the default branch, set up a remote and run: wt config refresh-cache"
        // Problem: git.rs is in lib crate, output module is in binary.
        // Options: (1) Return info about whether fallback was used, let callers show message
        //          (2) Add messages in specific commands (merge.rs, worktree.rs)
        //          (3) Move output abstraction to lib crate
        self.infer_default_branch_locally()
    }

    /// Resolve a target branch from an optional override
    ///
    /// If target is Some, returns it as a String. Otherwise, queries the default branch.
    /// This is a common pattern used throughout commands that accept an optional --target flag.
    pub fn resolve_target_branch(&self, target: Option<&str>) -> Result<String, GitError> {
        target.map_or_else(|| self.default_branch(), |b| Ok(b.to_string()))
    }

    /// Infer the default branch locally (without remote).
    ///
    /// Uses local heuristics when no remote is available:
    /// 1. If only one local branch exists, use it
    /// 2. Check what HEAD points to (current branch in main worktree)
    /// 3. Check user's git config init.defaultBranch
    /// 4. Look for common branch names (main, master, develop)
    /// 5. Fail if none of the above work
    fn infer_default_branch_locally(&self) -> Result<String, GitError> {
        // 1. If there's only one local branch, use it
        let branches = self.local_branches()?;
        if branches.len() == 1 {
            return Ok(branches[0].clone());
        }

        // 2. Use the branch HEAD points to (from main worktree)
        // This is what the main worktree is currently on
        if let Ok(current) = self.run_command(&["symbolic-ref", "--short", "HEAD"]) {
            let branch = current.trim().to_string();
            if !branch.is_empty() {
                return Ok(branch);
            }
        }

        // 3. Check git config init.defaultBranch
        if let Ok(default) = self.run_command(&["config", "--get", "init.defaultBranch"]) {
            let branch = default.trim().to_string();
            if !branch.is_empty() && branches.contains(&branch) {
                return Ok(branch);
            }
        }

        // 4. Look for common branch names
        for name in ["main", "master", "develop", "trunk"] {
            if branches.contains(&name.to_string()) {
                return Ok(name.to_string());
            }
        }

        // 5. Give up - can't infer
        Err(GitError::CommandFailed(
            "Could not infer default branch. Please specify target branch explicitly or set up a remote.".to_string()
        ))
    }

    /// List all local branches.
    fn local_branches(&self) -> Result<Vec<String>, GitError> {
        let stdout = self.run_command(&["branch", "--format=%(refname:short)"])?;
        Ok(stdout.lines().map(|s| s.trim().to_string()).collect())
    }

    /// Get the git common directory (the actual .git directory for the repository).
    pub fn git_common_dir(&self) -> Result<PathBuf, GitError> {
        let stdout = self.run_command(&["rev-parse", "--git-common-dir"])?;
        Ok(PathBuf::from(stdout.trim()))
    }

    /// Get the git directory (may be different from common-dir in worktrees).
    pub fn git_dir(&self) -> Result<PathBuf, GitError> {
        let stdout = self.run_command(&["rev-parse", "--git-dir"])?;
        Ok(PathBuf::from(stdout.trim()))
    }

    /// Get the base directory where worktrees are created relative to.
    ///
    /// For normal repositories: the parent of .git (the repo root).
    /// For bare repositories: the bare repository directory itself.
    ///
    /// This is the path that should be used when constructing worktree paths.
    pub fn worktree_base(&self) -> Result<PathBuf, GitError> {
        Ok(self.layout()?.worktree_base.clone())
    }

    /// Check if the working tree has uncommitted changes.
    pub fn is_dirty(&self) -> Result<bool, GitError> {
        let stdout = self.run_command(&["status", "--porcelain"])?;
        Ok(!stdout.trim().is_empty())
    }

    /// Ensure the working tree is clean (no uncommitted changes).
    ///
    /// Returns an error if there are uncommitted changes.
    pub fn ensure_clean_working_tree(&self) -> Result<(), GitError> {
        if self.is_dirty()? {
            return Err(GitError::UncommittedChanges);
        }
        Ok(())
    }

    /// Get the worktree root directory (top-level of the working tree).
    ///
    /// Returns the canonicalized absolute path to the top-level directory of the
    /// current working tree. This could be the main worktree or a linked worktree.
    pub fn worktree_root(&self) -> Result<PathBuf, GitError> {
        let stdout = self.run_command(&["rev-parse", "--show-toplevel"])?;
        let path = PathBuf::from(stdout.trim());
        path.canonicalize().map_err(|e| {
            GitError::CommandFailed(format!("Failed to canonicalize worktree root: {}", e))
        })
    }

    /// Check if the path is in a worktree (vs the main repository).
    pub fn is_in_worktree(&self) -> Result<bool, GitError> {
        let git_dir = self.git_dir()?;
        let common_dir = self.git_common_dir()?;
        Ok(git_dir != common_dir)
    }

    /// Check if base is an ancestor of head (i.e., would be a fast-forward).
    pub fn is_ancestor(&self, base: &str, head: &str) -> Result<bool, GitError> {
        self.run_command_check(&["merge-base", "--is-ancestor", base, head])
    }

    /// Count commits between base and head.
    pub fn count_commits(&self, base: &str, head: &str) -> Result<usize, GitError> {
        let range = format!("{}..{}", base, head);
        let stdout = self.run_command(&["rev-list", "--count", &range])?;
        stdout
            .trim()
            .parse()
            .map_err(|e| GitError::ParseError(format!("Failed to parse commit count: {}", e)))
    }

    /// Check if there are merge commits in the range base..head.
    pub fn has_merge_commits(&self, base: &str, head: &str) -> Result<bool, GitError> {
        let range = format!("{}..{}", base, head);
        let stdout = self.run_command(&["rev-list", "--merges", &range])?;
        Ok(!stdout.trim().is_empty())
    }

    /// Get files changed between base and head.
    pub fn changed_files(&self, base: &str, head: &str) -> Result<Vec<String>, GitError> {
        let range = format!("{}..{}", base, head);
        let stdout = self.run_command(&["diff", "--name-only", &range])?;
        Ok(stdout.lines().map(String::from).collect())
    }

    /// Get commit timestamp in seconds since epoch.
    pub fn commit_timestamp(&self, commit: &str) -> Result<i64, GitError> {
        let stdout = self.run_command(&["show", "-s", "--format=%ct", commit])?;
        stdout
            .trim()
            .parse()
            .map_err(|e| GitError::ParseError(format!("Failed to parse timestamp: {}", e)))
    }

    /// Get commit message (subject line) for a commit.
    pub fn commit_message(&self, commit: &str) -> Result<String, GitError> {
        let stdout = self.run_command(&["show", "-s", "--format=%s", commit])?;
        Ok(stdout.trim().to_owned())
    }

    /// Get the upstream tracking branch for the given branch.
    pub fn upstream_branch(&self, branch: &str) -> Result<Option<String>, GitError> {
        let result = self.run_command(&["rev-parse", "--abbrev-ref", &format!("{}@{{u}}", branch)]);

        match result {
            Ok(upstream) => {
                let trimmed = upstream.trim();
                Ok((!trimmed.is_empty()).then(|| trimmed.to_string()))
            }
            Err(_) => Ok(None), // No upstream configured
        }
    }

    /// Get merge/rebase status for the worktree.
    pub fn worktree_state(&self) -> Result<Option<String>, GitError> {
        let git_dir = self.git_dir()?;

        // Check for merge
        if git_dir.join("MERGE_HEAD").exists() {
            return Ok(Some("MERGING".to_string()));
        }

        // Check for rebase
        if git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists() {
            let rebase_dir = if git_dir.join("rebase-merge").exists() {
                git_dir.join("rebase-merge")
            } else {
                git_dir.join("rebase-apply")
            };

            if let (Ok(msgnum), Ok(end)) = (
                std::fs::read_to_string(rebase_dir.join("msgnum")),
                std::fs::read_to_string(rebase_dir.join("end")),
            ) {
                let current = msgnum.trim();
                let total = end.trim();
                return Ok(Some(format!("REBASING {}/{}", current, total)));
            }

            return Ok(Some("REBASING".to_string()));
        }

        // Check for cherry-pick
        if git_dir.join("CHERRY_PICK_HEAD").exists() {
            return Ok(Some("CHERRY-PICKING".to_string()));
        }

        // Check for revert
        if git_dir.join("REVERT_HEAD").exists() {
            return Ok(Some("REVERTING".to_string()));
        }

        // Check for bisect
        if git_dir.join("BISECT_LOG").exists() {
            return Ok(Some("BISECTING".to_string()));
        }

        Ok(None)
    }

    /// Calculate commits ahead and behind between two refs.
    ///
    /// Returns (ahead, behind) where ahead is commits in head not in base,
    /// and behind is commits in base not in head.
    pub fn ahead_behind(&self, base: &str, head: &str) -> Result<(usize, usize), GitError> {
        let ahead = self.count_commits(base, head)?;
        let behind = self.count_commits(head, base)?;
        Ok((ahead, behind))
    }

    /// Get line diff statistics for working tree changes (unstaged + staged).
    pub fn working_tree_diff_stats(&self) -> Result<LineDiff, GitError> {
        let stdout = self.run_command(&["diff", "--numstat", "HEAD"])?;
        LineDiff::from_numstat(&stdout)
    }

    /// Get line diff statistics between working tree and a specific ref.
    ///
    /// This compares the current working tree contents (including uncommitted changes)
    /// against the specified ref, regardless of what HEAD points to.
    ///
    pub fn working_tree_diff_vs_ref(&self, ref_name: &str) -> Result<LineDiff, GitError> {
        let stdout = self.run_command(&["diff", "--numstat", ref_name])?;
        LineDiff::from_numstat(&stdout)
    }

    /// Return the working tree diff versus a base branch when their trees match.
    ///
    /// When `base_branch` is `None` (primary worktree), this always returns `Some(LineDiff::default())`.
    /// If the base branch tree matches HEAD and the working tree is dirty, the precise diff is
    /// computed; otherwise we return zero to indicate the trees (and working tree) match.
    /// When the trees differ, we return `None` so callers can skip expensive comparisons.
    pub fn working_tree_diff_with_base(
        &self,
        base_branch: Option<&str>,
        working_tree_dirty: bool,
    ) -> Result<Option<LineDiff>, GitError> {
        let Some(branch) = base_branch else {
            return Ok(Some(LineDiff::default()));
        };

        if !self.branch_tree_matches_head(branch)? {
            return Ok(None);
        }

        if working_tree_dirty {
            self.working_tree_diff_vs_ref(branch).map(Some)
        } else {
            Ok(Some(LineDiff::default()))
        }
    }

    /// Get line diff statistics between two refs (using three-dot diff for merge base).
    ///
    pub fn branch_diff_stats(&self, base: &str, head: &str) -> Result<LineDiff, GitError> {
        let range = format!("{}...{}", base, head);
        let stdout = self.run_command(&["diff", "--numstat", &range])?;
        LineDiff::from_numstat(&stdout)
    }

    /// Get formatted diff stats summary for display.
    ///
    /// Returns a vector of formatted strings like ["3 files", "+45", "-12"].
    /// Returns empty vector if diff command fails or produces no output.
    ///
    /// This method combines git diff --shortstat, parsing, and formatting into a single call.
    pub fn diff_stats_summary(&self, args: &[&str]) -> Vec<String> {
        self.run_command(args)
            .ok()
            .map(|output| DiffStats::from_shortstat(&output).format_summary())
            .unwrap_or_default()
    }

    /// Determine whether there are staged changes in the index.
    ///
    /// Returns `Ok(true)` when staged changes are present, `Ok(false)` otherwise.
    pub fn has_staged_changes(&self) -> Result<bool, GitError> {
        Ok(!self.run_command_check(&["diff", "--cached", "--quiet", "--exit-code"])?)
    }

    /// Get all branch names (local branches only).
    pub fn all_branches(&self) -> Result<Vec<String>, GitError> {
        let stdout = self.run_command(&["branch", "--format=%(refname:short)"])?;
        Ok(stdout
            .lines()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect())
    }

    /// Get the merge base between two commits.
    pub fn merge_base(&self, commit1: &str, commit2: &str) -> Result<String, GitError> {
        let output = self.run_command(&["merge-base", commit1, commit2])?;
        Ok(output.trim().to_owned())
    }

    /// Check if merging head into base would result in conflicts.
    ///
    /// Uses `git merge-tree` to simulate a merge without touching the working tree.
    /// Returns true if conflicts would occur, false for a clean merge.
    ///
    /// # Examples
    /// ```no_run
    /// use worktrunk::git::Repository;
    ///
    /// let repo = Repository::current();
    /// let has_conflicts = repo.has_merge_conflicts("main", "feature-branch")?;
    /// # Ok::<(), worktrunk::git::GitError>(())
    /// ```
    pub fn has_merge_conflicts(&self, base: &str, head: &str) -> Result<bool, GitError> {
        let merge_base = self.merge_base(base, head)?;

        // git merge-tree exits with 0 for clean merge, 1 for conflicts
        // run_command_check returns true for exit 0, false otherwise
        let clean_merge = self.run_command_check(&["merge-tree", &merge_base, base, head])?;
        Ok(!clean_merge)
    }

    /// Get commit subjects (first line of commit message) from a range.
    pub fn commit_subjects(&self, range: &str) -> Result<Vec<String>, GitError> {
        let output = self.run_command(&["log", "--format=%s", range])?;
        Ok(output.lines().map(String::from).collect())
    }

    /// List all worktrees for this repository.
    ///
    /// Returns a WorktreeList that automatically filters out bare repositories
    /// and provides access to the primary worktree.
    pub fn list_worktrees(&self) -> Result<WorktreeList, GitError> {
        let stdout = self.run_command(&["worktree", "list", "--porcelain"])?;
        let raw_worktrees = Worktree::parse_porcelain_list(&stdout)?;
        WorktreeList::from_raw(raw_worktrees)
    }

    /// Find the worktree path for a given branch, if one exists.
    pub fn worktree_for_branch(&self, branch: &str) -> Result<Option<PathBuf>, GitError> {
        let worktrees = self.list_worktrees()?;

        Ok(worktrees
            .worktrees
            .iter()
            .find(|wt| wt.branch.as_deref() == Some(branch))
            .map(|wt| wt.path.clone()))
    }

    /// Get branches that don't have worktrees (available for switch).
    pub fn available_branches(&self) -> Result<Vec<String>, GitError> {
        let all_branches = self.all_branches()?;
        let worktrees = self.list_worktrees()?;

        // Collect branches that have worktrees
        let branches_with_worktrees: std::collections::HashSet<String> = worktrees
            .worktrees
            .iter()
            .filter_map(|wt| wt.branch.clone())
            .collect();

        // Filter out branches with worktrees
        Ok(all_branches
            .into_iter()
            .filter(|branch| !branches_with_worktrees.contains(branch))
            .collect())
    }

    /// Get a git config value. Returns None if the key doesn't exist.
    pub fn get_config(&self, key: &str) -> Result<Option<String>, GitError> {
        match self.run_command(&["config", key]) {
            Ok(value) => Ok(Some(value.trim().to_string())),
            Err(_) => Ok(None), // Config key doesn't exist
        }
    }

    /// Set a git config value.
    pub fn set_config(&self, key: &str, value: &str) -> Result<(), GitError> {
        self.run_command(&["config", key, value])?;
        Ok(())
    }

    /// Remove a worktree at the specified path.
    pub fn remove_worktree(&self, path: &std::path::Path) -> Result<(), GitError> {
        let path_str = path
            .to_str()
            .ok_or_else(|| GitError::message("Invalid UTF-8 in worktree path"))?;
        self.run_command(&["worktree", "remove", path_str])?;
        Ok(())
    }

    /// Refresh the default branch cache by querying the remote.
    ///
    /// This forces a network call to `git ls-remote` to fetch the current default
    /// branch from the remote, then updates the local cache. Use this when you
    /// suspect the cached default branch is stale (e.g., after a repository's
    /// default branch has been changed on the remote).
    ///
    /// Returns the refreshed default branch name.
    pub fn refresh_default_branch(&self) -> Result<String, GitError> {
        let remote = self.primary_remote()?;
        let branch = self.query_remote_default_branch(&remote)?;
        self.cache_default_branch(&remote, &branch)?;
        Ok(branch)
    }

    fn branch_tree_matches_head(&self, branch: &str) -> Result<bool, GitError> {
        let head_tree = self.rev_parse_tree("HEAD^{tree}")?;
        let branch_tree = self.rev_parse_tree(&format!("{branch}^{{tree}}"))?;
        Ok(head_tree == branch_tree)
    }

    fn rev_parse_tree(&self, spec: &str) -> Result<String, GitError> {
        self.run_command(&["rev-parse", spec])
            .map(|output| output.trim().to_string())
    }

    // Private helper methods for default_branch()

    fn get_local_default_branch(&self, remote: &str) -> Result<String, GitError> {
        let stdout =
            self.run_command(&["rev-parse", "--abbrev-ref", &format!("{}/HEAD", remote)])?;
        DefaultBranchName::from_local(remote, &stdout).map(DefaultBranchName::into_string)
    }

    fn query_remote_default_branch(&self, remote: &str) -> Result<String, GitError> {
        let stdout = self.run_command(&["ls-remote", "--symref", remote, "HEAD"])?;
        DefaultBranchName::from_remote(&stdout).map(DefaultBranchName::into_string)
    }

    fn cache_default_branch(&self, remote: &str, branch: &str) -> Result<(), GitError> {
        self.run_command(&["remote", "set-head", remote, branch])?;
        Ok(())
    }

    /// Get a project identifier for approval tracking.
    ///
    /// Uses the git remote URL if available (e.g., "github.com/user/repo"),
    /// otherwise falls back to the repository directory name.
    ///
    /// This identifier is used to track which commands have been approved
    /// for execution in this project.
    pub fn project_identifier(&self) -> Result<String, GitError> {
        // Try to get the remote URL first
        let remote = self.primary_remote()?;

        if let Ok(url) = self.run_command(&["remote", "get-url", &remote]) {
            let url = url.trim();

            // Parse common git URL formats:
            // - https://github.com/user/repo.git
            // - git@github.com:user/repo.git
            // - ssh://git@github.com/user/repo.git

            // Remove .git suffix if present
            let url = url.strip_suffix(".git").unwrap_or(url);

            // Handle SSH format (git@host:path)
            if let Some(ssh_part) = url.strip_prefix("git@")
                && let Some((host, path)) = ssh_part.split_once(':')
            {
                return Ok(format!("{}/{}", host, path));
            }

            // Handle HTTPS/HTTP format
            if let Some(https_part) = url
                .strip_prefix("https://")
                .or_else(|| url.strip_prefix("http://"))
            {
                return Ok(https_part.to_string());
            }

            // Handle ssh:// format
            if let Some(ssh_part) = url.strip_prefix("ssh://") {
                // Remove git@ prefix if present
                let ssh_part = ssh_part.strip_prefix("git@").unwrap_or(ssh_part);
                // Replace first : with /
                if let Some(colon_pos) = ssh_part.find(':') {
                    let (host, path) = ssh_part.split_at(colon_pos);
                    return Ok(format!("{}{}", host, path.replacen(':', "/", 1)));
                }
                return Ok(ssh_part.to_string());
            }

            // If we can't parse it, use the URL as-is
            return Ok(url.to_string());
        }

        // Fall back to repository name (use worktree base for consistency across all worktrees)
        let repo_root = self.worktree_base()?;
        let repo_name = repo_root
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| GitError::message("Could not determine repository name"))?;

        Ok(repo_name.to_string())
    }

    /// Run a git command in this repository's context.
    ///
    /// Executes the git command with this repository's path as the working directory
    /// and returns the stdout output.
    ///
    /// # Examples
    /// ```no_run
    /// use worktrunk::git::Repository;
    ///
    /// let repo = Repository::current();
    /// let status = repo.run_command(&["status", "--porcelain"])?;
    /// # Ok::<(), worktrunk::git::GitError>(())
    /// ```
    pub fn run_command(&self, args: &[&str]) -> Result<String, GitError> {
        let mut cmd = Command::new("git");
        cmd.args(args);
        cmd.current_dir(&self.path);

        // Log: $ git <args> [worktree]
        // TODO: Guard with log::log_enabled! if args.join() overhead becomes measurable
        if self.path.to_str() == Some(".") {
            log::debug!("$ git {}", args.join(" "));
        } else {
            let worktree = self
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?");
            log::debug!("$ git {} [{}]", args.join(" "), worktree);
        }

        let output = cmd
            .output()
            .map_err(|e| GitError::CommandFailed(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Normalize carriage returns to newlines for consistent output
            // Git uses \r for progress updates; in non-TTY contexts this causes snapshot instability
            let stderr = stderr.replace('\r', "\n");
            // Log errors with ! prefix
            for line in stderr.trim().lines() {
                log::debug!("  ! {}", line);
            }
            return Err(GitError::CommandFailed(stderr));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        if !stdout.is_empty() {
            // Log output indented
            for line in stdout.trim().lines() {
                log::debug!("  {}", line);
            }
        }
        Ok(stdout)
    }

    /// Run a git command and return whether it succeeded (exit code 0).
    ///
    /// This is useful for commands that use exit codes for boolean results,
    /// like `git merge-base --is-ancestor` or `git diff --quiet`.
    ///
    /// # Examples
    /// ```no_run
    /// use worktrunk::git::Repository;
    ///
    /// let repo = Repository::current();
    /// let is_clean = repo.run_command_check(&["diff", "--quiet", "--exit-code"])?;
    /// # Ok::<(), worktrunk::git::GitError>(())
    /// ```
    pub fn run_command_check(&self, args: &[&str]) -> Result<bool, GitError> {
        let mut cmd = Command::new("git");
        cmd.args(args);
        cmd.current_dir(&self.path);

        // Log: $ git <args> [worktree]
        if self.path.to_str() == Some(".") {
            log::debug!("$ git {}", args.join(" "));
        } else {
            let worktree = self
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?");
            log::debug!("$ git {} [{}]", args.join(" "), worktree);
        }

        let output = cmd
            .output()
            .map_err(|e| GitError::CommandFailed(e.to_string()))?;

        let success = output.status.success();
        if !success {
            log::debug!("  → exit code: non-zero");
        }
        Ok(success)
    }

    /// Run git diff through a renderer (pager) if configured, otherwise return colored diff output.
    pub fn run_diff_with_pager(&self, args: &[&str]) -> Result<String, GitError> {
        let mut git_args = args.to_vec();
        git_args.push("--color=always");
        let git_output = self.run_command(&git_args)?;

        let result = match pager_config().as_ref() {
            Some(pager_cmd) => invoke_renderer(pager_cmd, &git_output).unwrap_or(git_output),
            None => {
                log::debug!("Using git output directly");
                git_output
            }
        };

        Ok(result)
    }
}

fn pager_config() -> &'static Option<String> {
    static PAGER_CONFIG: OnceLock<Option<String>> = OnceLock::new();
    PAGER_CONFIG.get_or_init(detect_pager)
}

fn detect_pager() -> Option<String> {
    let repo = Repository::current();

    let validate = |s: &str| -> Option<String> {
        let trimmed = s.trim();
        (!trimmed.is_empty() && trimmed != "cat").then(|| trimmed.to_string())
    };

    std::env::var("GIT_PAGER")
        .ok()
        .and_then(|s| validate(&s))
        .or_else(|| {
            repo.run_command(&["config", "--get", "pager.diff"])
                .ok()
                .and_then(|s| validate(&s))
        })
        .or_else(|| {
            repo.run_command(&["config", "--get", "core.pager"])
                .ok()
                .and_then(|s| validate(&s))
        })
        .or_else(|| std::env::var("PAGER").ok().and_then(|s| validate(&s)))
}

fn invoke_renderer(pager_cmd: &str, git_output: &str) -> Option<String> {
    log::debug!("Invoking renderer: {}", pager_cmd);

    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(pager_cmd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("PAGER", "cat")
        .env("DELTA_PAGER", "cat")
        .env("BAT_PAGER", "");

    let mut child = cmd.spawn().ok()?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(git_output.as_bytes());
        drop(stdin);
    }

    let output = child.wait_with_output().ok()?;
    if output.status.success() {
        log::debug!("Renderer succeeded, output len={}", output.stdout.len());
        String::from_utf8(output.stdout).ok()
    } else {
        log::debug!("Renderer failed with status={:?}", output.status);
        None
    }
}

#[cfg(test)]
mod tests;
