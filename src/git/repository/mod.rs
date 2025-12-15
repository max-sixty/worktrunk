use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use anyhow::{Context, bail};
use normalize_path::NormalizePath;

use dunce::canonicalize;

// Import types and functions from parent module (mod.rs)
use super::{
    BranchCategory, CompletionBranch, DefaultBranchName, DiffStats, GitError, LineDiff, Worktree,
    WorktreeList,
};

/// Result of resolving a worktree name.
///
/// Used by `resolve_worktree` to handle different resolution outcomes:
/// - A worktree exists (with optional branch for detached HEAD)
/// - Only a branch exists (no worktree)
#[derive(Debug, Clone)]
pub enum ResolvedWorktree {
    /// A worktree was found
    Worktree {
        /// The filesystem path to the worktree
        path: PathBuf,
        /// The branch name, if known (None for detached HEAD)
        branch: Option<String>,
    },
    /// Only a branch exists (no worktree)
    BranchOnly {
        /// The branch name
        branch: String,
    },
}

/// Global base path for repository operations, set by -C flag
static BASE_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Initialize the global base path for repository operations.
///
/// This should be called once at program startup from main().
/// If not called, defaults to "." (current directory).
pub fn set_base_path(path: PathBuf) {
    BASE_PATH.set(path).ok();
}

/// Get the base path for repository operations.
fn base_path() -> &'static PathBuf {
    static DEFAULT: OnceLock<PathBuf> = OnceLock::new();
    BASE_PATH
        .get()
        .unwrap_or_else(|| DEFAULT.get_or_init(|| PathBuf::from(".")))
}

/// Internal layout information for a repository.
///
/// Cached to avoid repeated git queries.
#[derive(Debug, Clone)]
struct RepositoryLayout {
    /// Base path for worktrees (repo root for normal repos, bare repo path for bare repos)
    worktree_base: PathBuf,
    /// Whether this is a bare repository
    is_bare: bool,
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
/// # Ok::<(), anyhow::Error>(())
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
    /// This is the most common usage pattern. If the -C flag was used,
    /// this uses that path instead of the actual current directory.
    pub fn current() -> Self {
        Self::at(base_path().clone())
    }

    /// Get the path this repository context operates on.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Get the repository layout, initializing it if needed.
    fn layout(&self) -> anyhow::Result<&RepositoryLayout> {
        if let Some(layout) = self.layout.get() {
            return Ok(layout);
        }

        let git_common_dir =
            canonicalize(&self.git_common_dir()?).context("Failed to canonicalize path")?;

        let is_bare = self.is_bare_repo()?;

        let worktree_base = if is_bare {
            git_common_dir.clone()
        } else {
            git_common_dir
                .parent()
                .ok_or_else(|| {
                    anyhow::Error::from(GitError::Other {
                        message: format!(
                            "Git directory has no parent: {}",
                            git_common_dir.display()
                        ),
                    })
                })?
                .to_path_buf()
        };

        let layout = RepositoryLayout {
            worktree_base,
            is_bare,
        };

        // set() returns Err if already set, but we checked above, so ignore the result
        let _ = self.layout.set(layout);

        // Now get() will succeed
        Ok(self.layout.get().unwrap())
    }

    /// Check if this is a bare repository (no working tree).
    fn is_bare_repo(&self) -> anyhow::Result<bool> {
        let output = self.run_command(&["config", "--bool", "core.bare"])?;
        Ok(output.trim() == "true")
    }

    /// Get the primary remote name for this repository.
    ///
    /// Uses the following strategy:
    /// 1. If the current branch has an upstream, use its remote
    ///    (Note: Detached HEAD falls through to step 2)
    /// 2. Use git's `checkout.defaultRemote` config if set and has a URL
    /// 3. Otherwise, get the first remote with a configured URL
    /// 4. Fall back to "origin" if no remotes exist
    pub fn primary_remote(&self) -> anyhow::Result<String> {
        // Try to get the remote from HEAD's upstream (single command)
        // @{u} refers to the upstream of the current branch
        if let Ok(upstream) = self.run_command(&["rev-parse", "--abbrev-ref", "@{u}"])
            && let Some((remote, _)) = upstream.trim().split_once('/')
        {
            return Ok(remote.to_string());
        }

        // Check git's checkout.defaultRemote config
        if let Ok(default_remote) = self.run_command(&["config", "checkout.defaultRemote"]) {
            let default_remote = default_remote.trim();
            if !default_remote.is_empty() && self.remote_has_url(default_remote) {
                return Ok(default_remote.to_string());
            }
        }

        // Fall back to first remote with a configured URL
        // Use git config to find remotes with URLs, filtering out phantom remotes
        // from global config (e.g., `remote.origin.prunetags=true` without a URL)
        let output = self
            .run_command(&["config", "--get-regexp", r"remote\..+\.url"])
            .unwrap_or_default();
        let first_remote = output.lines().next().and_then(|line| {
            // Parse "remote.<name>.url <value>" format
            // Use ".url " as delimiter to handle remote names with dots (e.g., "my.remote")
            line.strip_prefix("remote.")
                .and_then(|s| s.split_once(".url "))
                .map(|(name, _)| name)
        });

        Ok(first_remote.unwrap_or("origin").to_string())
    }

    /// Check if a remote has a URL configured.
    fn remote_has_url(&self, remote: &str) -> bool {
        self.run_command(&["config", &format!("remote.{}.url", remote)])
            .map(|url| !url.trim().is_empty())
            .unwrap_or(false)
    }

    /// Check if a local git branch exists.
    pub fn local_branch_exists(&self, branch: &str) -> anyhow::Result<bool> {
        Ok(self
            .run_command(&["rev-parse", "--verify", &format!("refs/heads/{}", branch)])
            .is_ok())
    }

    /// Check if a git branch exists (local or remote).
    pub fn branch_exists(&self, branch: &str) -> anyhow::Result<bool> {
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

    /// Find which remotes have a branch with the given name.
    ///
    /// Returns a list of remote names that have this branch (e.g., `["origin"]`).
    /// Returns an empty list if no remotes have this branch.
    pub fn remotes_with_branch(&self, branch: &str) -> anyhow::Result<Vec<String>> {
        // Get all remote tracking branches matching this name
        // Format: refs/remotes/<remote>/<branch>
        let output = self.run_command(&[
            "for-each-ref",
            "--format=%(refname:strip=2)",
            &format!("refs/remotes/*/{}", branch),
        ])?;

        // Parse output: each line is "<remote>/<branch>"
        // Extract the remote name (everything before the last /<branch>)
        let remotes: Vec<String> = output
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                // Strip the branch suffix to get the remote name
                line.strip_suffix(&format!("/{}", branch)).map(String::from)
            })
            .collect();

        Ok(remotes)
    }

    /// Get the current branch name, or None if in detached HEAD state.
    pub fn current_branch(&self) -> anyhow::Result<Option<String>> {
        let stdout = self.run_command(&["branch", "--show-current"])?;
        let branch = stdout.trim();

        if branch.is_empty() {
            Ok(None) // Detached HEAD
        } else {
            Ok(Some(branch.to_string()))
        }
    }

    /// Get the current branch name, or error if in detached HEAD state.
    ///
    /// `action` describes what requires being on a branch (e.g., "merge").
    pub fn require_current_branch(&self, action: &str) -> anyhow::Result<String> {
        self.current_branch()?.ok_or_else(|| {
            GitError::DetachedHead {
                action: Some(action.into()),
            }
            .into()
        })
    }

    /// Read a user-defined marker from `worktrunk.marker.<branch>` in git config.
    ///
    /// Markers are stored as JSON: `{"marker": "text", "set_at": unix_timestamp}`.
    /// Falls back to plain text for legacy markers without timestamps.
    pub fn branch_keyed_marker(&self, branch: &str) -> Option<String> {
        let escaped = super::escape_branch_for_config(branch);
        let config_key = format!("worktrunk.marker.{}", escaped);
        let raw = self
            .run_command(&["config", "--get", &config_key])
            .ok()
            .map(|output| output.trim().to_string())
            .filter(|s| !s.is_empty())?;

        // Try to parse as JSON, fall back to raw string for legacy markers
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&raw) {
            parsed
                .get("marker")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        } else {
            Some(raw) // Legacy plain-text marker
        }
    }

    /// Read user-defined branch-keyed marker.
    pub fn user_marker(&self, branch: Option<&str>) -> Option<String> {
        branch.and_then(|branch| self.branch_keyed_marker(branch))
    }

    /// Record the previous branch in worktrunk.history for `wt switch -` support.
    ///
    /// Stores the branch we're switching FROM, so `wt switch -` can return to it.
    pub fn record_switch_previous(&self, previous: Option<&str>) -> anyhow::Result<()> {
        if let Some(prev) = previous {
            self.run_command(&["config", "worktrunk.history", prev])?;
        }
        // If previous is None (detached HEAD), don't update history
        Ok(())
    }

    /// Get the previous branch from worktrunk.history for `wt switch -`.
    ///
    /// Returns the branch we came from, enabling ping-pong switching.
    pub fn get_switch_previous(&self) -> Option<String> {
        self.run_command(&["config", "--get", "worktrunk.history"])
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
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
            "@" => self.current_branch()?.ok_or_else(|| {
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
            "^" => self.default_branch(),
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
                let path = self.worktree_root()?;
                let worktrees = self.list_worktrees()?;
                let branch = worktrees
                    .worktrees
                    .iter()
                    .find(|wt| wt.path == path)
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
    pub fn default_branch(&self) -> anyhow::Result<String> {
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
        //   "To cache the default branch, set up a remote and run: wt config state get default-branch --refresh"
        // Problem: git.rs is in lib crate, output module is in binary.
        // Options: (1) Return info about whether fallback was used, let callers show message
        //          (2) Add messages in specific commands (merge.rs, worktree.rs)
        //          (3) Move output abstraction to lib crate
        self.infer_default_branch_locally()
    }

    /// Resolve a target branch from an optional override
    ///
    /// If target is Some, expands special symbols ("@", "-", "^") via `resolve_worktree_name`.
    /// Otherwise, queries the default branch.
    /// This is a common pattern used throughout commands that accept an optional --target flag.
    pub fn resolve_target_branch(&self, target: Option<&str>) -> anyhow::Result<String> {
        target.map_or_else(|| self.default_branch(), |b| self.resolve_worktree_name(b))
    }

    /// Infer the default branch locally (without remote).
    ///
    /// Uses local heuristics when no remote is available:
    /// 1. If only one local branch exists, use it
    /// 2. Check what HEAD points to (current branch in main worktree)
    /// 3. Check user's git config init.defaultBranch
    /// 4. Look for common branch names (main, master, develop)
    /// 5. Fail if none of the above work
    fn infer_default_branch_locally(&self) -> anyhow::Result<String> {
        // 1. If there's only one local branch, use it
        let branches = self.local_branches()?;
        if branches.len() == 1 {
            return Ok(branches[0].clone());
        }

        // 2. Check git config init.defaultBranch
        if let Ok(default) = self.run_command(&["config", "--get", "init.defaultBranch"]) {
            let branch = default.trim().to_string();
            if !branch.is_empty() && branches.contains(&branch) {
                return Ok(branch);
            }
        }

        // 3. Look for common branch names
        for name in ["main", "master", "develop", "trunk"] {
            if branches.contains(&name.to_string()) {
                return Ok(name.to_string());
            }
        }

        // 4. Give up - can't infer
        Err(GitError::Other {
            message:
                "Could not infer default branch. Please specify target branch explicitly or set up a remote."
                    .into(),
        }
        .into())
    }

    /// List all local branches.
    fn local_branches(&self) -> anyhow::Result<Vec<String>> {
        let stdout = self.run_command(&["branch", "--format=%(refname:short)"])?;
        Ok(stdout.lines().map(|s| s.trim().to_string()).collect())
    }

    /// Get the git common directory (the actual .git directory for the repository).
    ///
    /// Always returns an absolute path, resolving any relative paths returned by git.
    pub fn git_common_dir(&self) -> anyhow::Result<PathBuf> {
        let stdout = self.run_command(&["rev-parse", "--git-common-dir"])?;
        let path = PathBuf::from(stdout.trim());

        // Resolve relative paths against the repo's directory
        if path.is_relative() {
            canonicalize(self.path.join(&path)).context("Failed to resolve git common directory")
        } else {
            Ok(path)
        }
    }

    /// Get the git directory (may be different from common-dir in worktrees).
    ///
    /// Always returns an absolute path, resolving any relative paths returned by git.
    pub fn git_dir(&self) -> anyhow::Result<PathBuf> {
        let stdout = self.run_command(&["rev-parse", "--git-dir"])?;
        let path = PathBuf::from(stdout.trim());

        // Resolve relative paths against the repo's directory
        if path.is_relative() {
            canonicalize(self.path.join(&path)).context("Failed to resolve git directory")
        } else {
            Ok(path)
        }
    }

    /// Get the base directory where worktrees are created relative to.
    ///
    /// For normal repositories: the parent of .git (the repo root).
    /// For bare repositories: the bare repository directory itself.
    ///
    /// This is the path that should be used when constructing worktree paths.
    pub fn worktree_base(&self) -> anyhow::Result<PathBuf> {
        Ok(self.layout()?.worktree_base.clone())
    }

    /// Check if this is a bare repository (no working tree).
    ///
    /// Bare repositories have no main worktree — all worktrees are linked
    /// worktrees at templated paths, including the default branch.
    pub fn is_bare(&self) -> anyhow::Result<bool> {
        Ok(self.layout()?.is_bare)
    }

    /// Check if the working tree has uncommitted changes.
    pub fn is_dirty(&self) -> anyhow::Result<bool> {
        let stdout = self.run_command(&["status", "--porcelain"])?;
        Ok(!stdout.trim().is_empty())
    }

    /// Ensure the working tree is clean (no uncommitted changes).
    ///
    /// Returns an error if there are uncommitted changes.
    /// - `action` describes what was blocked (e.g., "remove worktree").
    /// - `worktree` identifies which worktree (e.g., branch name) for multi-worktree operations.
    pub fn ensure_clean_working_tree(
        &self,
        action: Option<&str>,
        worktree: Option<&str>,
    ) -> anyhow::Result<()> {
        if self.is_dirty()? {
            return Err(GitError::UncommittedChanges {
                action: action.map(String::from),
                worktree: worktree.map(String::from),
            }
            .into());
        }
        Ok(())
    }

    /// Get the worktree root directory (top-level of the working tree).
    ///
    /// Returns the canonicalized absolute path to the top-level directory of the
    /// current working tree. This could be the main worktree or a linked worktree.
    pub fn worktree_root(&self) -> anyhow::Result<PathBuf> {
        let stdout = self.run_command(&["rev-parse", "--show-toplevel"])?;
        let path = PathBuf::from(stdout.trim());
        canonicalize(&path).context("Failed to canonicalize worktree root")
    }

    /// Check if the path is in a worktree (vs the main repository).
    pub fn is_in_worktree(&self) -> anyhow::Result<bool> {
        let git_dir = self.git_dir()?;
        let common_dir = self.git_common_dir()?;
        Ok(git_dir != common_dir)
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

    /// Check if git's builtin fsmonitor daemon is enabled.
    ///
    /// Returns true only for `core.fsmonitor=true` (the builtin daemon).
    /// Returns false for Watchman hooks, disabled, or unset.
    pub fn is_builtin_fsmonitor_enabled(&self) -> bool {
        self.run_command(&["config", "--get", "core.fsmonitor"])
            .ok()
            .map(|s| s.trim() == "true")
            .unwrap_or(false)
    }

    /// Start the fsmonitor daemon for this worktree.
    ///
    /// This is idempotent - if the daemon is already running, this is a no-op.
    /// Used to avoid auto-start races when running many parallel git commands.
    pub fn start_fsmonitor_daemon(&self) {
        // Best effort - log errors at debug level for troubleshooting
        if let Err(e) = self.run_command(&["fsmonitor--daemon", "start"]) {
            log::debug!("fsmonitor daemon start failed (usually fine): {e}");
        }
    }

    /// Check if base is an ancestor of head (i.e., would be a fast-forward).
    pub fn is_ancestor(&self, base: &str, head: &str) -> anyhow::Result<bool> {
        self.run_command_check(&["merge-base", "--is-ancestor", base, head])
    }

    /// Check if two refs point to the same commit.
    pub fn same_commit(&self, ref1: &str, ref2: &str) -> anyhow::Result<bool> {
        // Parse both refs in a single git command
        let output = self.run_command(&["rev-parse", ref1, ref2])?;
        let mut lines = output.lines();
        let sha1 = lines.next().unwrap_or_default().trim();
        let sha2 = lines.next().unwrap_or_default().trim();
        Ok(sha1 == sha2)
    }

    /// Check if a branch has file changes beyond the merge-base with target.
    ///
    /// Uses three-dot diff (`target...branch`) which shows files changed from merge-base
    /// to branch. Returns false when the diff is empty (no added changes).
    pub fn has_added_changes(&self, branch: &str, target: &str) -> anyhow::Result<bool> {
        // git diff --name-only target...branch shows files changed from merge-base to branch
        let range = format!("{target}...{branch}");
        let stdout = self.run_command(&["diff", "--name-only", &range])?;
        Ok(!stdout.trim().is_empty())
    }

    /// Count commits between base and head.
    pub fn count_commits(&self, base: &str, head: &str) -> anyhow::Result<usize> {
        // Limit concurrent rev-list operations to reduce mmap thrash on commit-graph
        let _guard = super::HEAVY_OPS_SEMAPHORE.acquire();

        let range = format!("{}..{}", base, head);
        let stdout = self.run_command(&["rev-list", "--count", &range])?;

        stdout.trim().parse().map_err(|e| {
            GitError::ParseError {
                message: format!("Failed to parse commit count: {}", e),
            }
            .into()
        })
    }

    /// Check if there are merge commits in the range base..head.
    pub fn has_merge_commits(&self, base: &str, head: &str) -> anyhow::Result<bool> {
        let range = format!("{}..{}", base, head);
        let stdout = self.run_command(&["rev-list", "--merges", &range])?;
        Ok(!stdout.trim().is_empty())
    }

    /// Get files changed between base and head.
    ///
    /// For renames and copies, both old and new paths are included to ensure
    /// overlap detection works correctly (e.g., detecting conflicts when a file
    /// is renamed in one branch but has uncommitted changes under the old name).
    pub fn changed_files(&self, base: &str, head: &str) -> anyhow::Result<Vec<String>> {
        let range = format!("{}..{}", base, head);
        let stdout = self.run_command(&["diff", "--name-status", "-z", &range])?;

        // Format: STATUS\0PATH\0 or STATUS\0NEW_PATH\0OLD_PATH\0 for renames/copies
        let mut files = Vec::new();
        let mut parts = stdout.split('\0').filter(|s| !s.is_empty());

        while let Some(status) = parts.next() {
            let path = parts
                .next()
                .ok_or_else(|| anyhow::anyhow!("Malformed git diff output: status without path"))?;
            files.push(path.to_string());

            // For renames (R) and copies (C), the old path follows
            if status.starts_with('R') || status.starts_with('C') {
                let old_path = parts.next().ok_or_else(|| {
                    anyhow::anyhow!("Malformed git diff output: rename/copy without old path")
                })?;
                files.push(old_path.to_string());
            }
        }

        Ok(files)
    }

    /// Get commit timestamp in seconds since epoch.
    pub fn commit_timestamp(&self, commit: &str) -> anyhow::Result<i64> {
        let stdout = self.run_command(&["show", "-s", "--format=%ct", commit])?;
        stdout.trim().parse().map_err(|e| {
            GitError::ParseError {
                message: format!("Failed to parse timestamp: {}", e),
            }
            .into()
        })
    }

    /// Get commit message (subject line) for a commit.
    pub fn commit_message(&self, commit: &str) -> anyhow::Result<String> {
        let stdout = self.run_command(&["show", "-s", "--format=%s", commit])?;
        Ok(stdout.trim().to_owned())
    }

    /// Get the upstream tracking branch for the given branch.
    pub fn upstream_branch(&self, branch: &str) -> anyhow::Result<Option<String>> {
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
    pub fn worktree_state(&self) -> anyhow::Result<Option<String>> {
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
    pub fn ahead_behind(&self, base: &str, head: &str) -> anyhow::Result<(usize, usize)> {
        // Use single git call with --left-right --count for better performance
        let range = format!("{}...{}", base, head);
        let output = self.run_command(&["rev-list", "--left-right", "--count", &range])?;

        // Parse output: "<behind>\t<ahead>" format
        // Example: "5\t3" means 5 commits behind, 3 commits ahead
        // git rev-list --left-right outputs left (base) first, then right (head)
        let parts: Vec<&str> = output.trim().split('\t').collect();
        if parts.len() != 2 {
            return Err(crate::git::GitError::ParseError {
                message: format!("Unexpected rev-list output format: {}", output),
            }
            .into());
        }

        let behind: usize = parts[0].parse().context("Failed to parse behind count")?;
        let ahead: usize = parts[1].parse().context("Failed to parse ahead count")?;

        Ok((ahead, behind))
    }

    /// List all local branches with their HEAD commit SHA.
    /// Returns a vector of (branch_name, commit_sha) tuples.
    pub fn list_local_branches(&self) -> anyhow::Result<Vec<(String, String)>> {
        let output = self.run_command(&[
            "for-each-ref",
            "--format=%(refname:short) %(objectname)",
            "refs/heads/",
        ])?;

        let branches: Vec<(String, String)> = output
            .lines()
            .filter_map(|line| {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() == 2 {
                    Some((parts[0].to_string(), parts[1].to_string()))
                } else {
                    None
                }
            })
            .collect();

        Ok(branches)
    }

    /// List remote branches from all remotes, excluding HEAD refs.
    ///
    /// Returns (branch_name, commit_sha) pairs for remote branches.
    /// Branch names are in the form "origin/feature", not "feature".
    pub fn list_remote_branches(&self) -> anyhow::Result<Vec<(String, String)>> {
        let output = self.run_command(&[
            "for-each-ref",
            "--format=%(refname:short) %(objectname)",
            "refs/remotes/",
        ])?;

        let branches: Vec<(String, String)> = output
            .lines()
            .filter_map(|line| {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() == 2 {
                    let branch_name = parts[0];
                    // Skip <remote>/HEAD (symref)
                    if branch_name.ends_with("/HEAD") {
                        None
                    } else {
                        Some((branch_name.to_string(), parts[1].to_string()))
                    }
                } else {
                    None
                }
            })
            .collect();

        Ok(branches)
    }

    /// Get line diff statistics for working tree changes (unstaged + staged).
    pub fn working_tree_diff_stats(&self) -> anyhow::Result<LineDiff> {
        // Limit concurrent diff operations to reduce mmap thrash on pack files
        let _guard = super::HEAVY_OPS_SEMAPHORE.acquire();

        let stdout = self.run_command(&["diff", "--numstat", "HEAD"])?;
        LineDiff::from_numstat(&stdout)
    }

    /// Get line diff statistics between working tree and a specific ref.
    ///
    /// This compares the current working tree contents (including uncommitted changes)
    /// against the specified ref, regardless of what HEAD points to.
    ///
    pub fn working_tree_diff_vs_ref(&self, ref_name: &str) -> anyhow::Result<LineDiff> {
        // Limit concurrent diff operations to reduce mmap thrash on pack files
        let _guard = super::HEAVY_OPS_SEMAPHORE.acquire();

        let stdout = self.run_command(&["diff", "--numstat", ref_name])?;
        LineDiff::from_numstat(&stdout)
    }

    /// Return the working tree diff versus a base branch when their trees match.
    ///
    /// When `base_branch` is `None` (main worktree), this always returns `Some(LineDiff::default())`.
    /// If the base branch tree matches HEAD and the working tree is dirty, the precise diff is
    /// computed; otherwise we return zero to indicate the trees (and working tree) match.
    /// When the trees differ, we return `None` so callers can skip expensive comparisons.
    pub fn working_tree_diff_with_base(
        &self,
        base_branch: Option<&str>,
        working_tree_dirty: bool,
    ) -> anyhow::Result<Option<LineDiff>> {
        let Some(branch) = base_branch else {
            return Ok(Some(LineDiff::default()));
        };

        if !self.head_tree_matches_branch(branch)? {
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
    pub fn branch_diff_stats(&self, base: &str, head: &str) -> anyhow::Result<LineDiff> {
        // Limit concurrent diff operations to reduce mmap thrash on pack files
        let _guard = super::HEAVY_OPS_SEMAPHORE.acquire();

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
    pub fn has_staged_changes(&self) -> anyhow::Result<bool> {
        Ok(!self.run_command_check(&["diff", "--cached", "--quiet", "--exit-code"])?)
    }

    /// Create a safety backup of current working tree state without affecting the working tree.
    ///
    /// This creates a backup commit containing all changes (staged, unstaged, and untracked files)
    /// and stores it in a custom ref (`refs/wt-backup/<branch>`). This creates a reflog entry
    /// for recovery without polluting the stash list. The working tree remains unchanged.
    ///
    /// Users can find safety backups with: `git reflog show refs/wt-backup/<branch>`
    ///
    /// Returns the SHA of the backup commit and a restore command.
    ///
    /// # Example
    /// ```no_run
    /// use worktrunk::git::Repository;
    ///
    /// let repo = Repository::current();
    /// let (sha, restore_cmd) = repo.create_safety_backup("feature → main (squash)")?;
    /// println!("Backup created: {} - Restore with: {}", sha, restore_cmd);
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn create_safety_backup(&self, message: &str) -> anyhow::Result<(String, String)> {
        // Create a backup commit using git stash create (without storing it in the stash list)
        let backup_sha = self
            .run_command(&["stash", "create", "--include-untracked"])?
            .trim()
            .to_string();

        // Validate that we got a SHA back
        if backup_sha.is_empty() {
            return Err(crate::git::GitError::Other {
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

        // Return short SHA and restore command
        // Use git stash apply because the backup is a merge commit (created by git stash create)
        let short_sha = &backup_sha[..7];
        let restore_cmd = format!("git stash apply --index {}", short_sha);

        Ok((short_sha.to_string(), restore_cmd))
    }

    /// Get all branch names (local branches only).
    pub fn all_branches(&self) -> anyhow::Result<Vec<String>> {
        let stdout = self.run_command(&[
            "branch",
            "--sort=-committerdate",
            "--format=%(refname:short)",
        ])?;
        Ok(stdout
            .lines()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect())
    }

    /// Get branches with metadata for shell completions.
    ///
    /// Returns branches in completion order: worktrees first, then local branches,
    /// then remote-only branches. Each category is sorted by recency.
    ///
    /// For remote branches, returns the local name (e.g., "fix" not "origin/fix")
    /// since `git worktree add path fix` auto-creates a tracking branch.
    pub fn branches_for_completion(&self) -> anyhow::Result<Vec<CompletionBranch>> {
        use std::collections::HashSet;

        // Get worktree branches
        let worktrees = self.list_worktrees()?;
        let worktree_branches: HashSet<String> = worktrees
            .worktrees
            .iter()
            .filter_map(|wt| wt.branch.clone())
            .collect();

        // Get local branches with timestamps
        let local_output = self.run_command(&[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname:short)\t%(committerdate:unix)",
            "refs/heads/",
        ])?;

        let local_branches: Vec<(String, i64)> = local_output
            .lines()
            .filter_map(|line| {
                let parts: Vec<&str> = line.split('\t').collect();
                if parts.len() == 2 {
                    let timestamp = parts[1].parse().unwrap_or(0);
                    Some((parts[0].to_string(), timestamp))
                } else {
                    None
                }
            })
            .collect();

        let local_branch_names: HashSet<String> =
            local_branches.iter().map(|(n, _)| n.clone()).collect();

        // Get remote branches with timestamps
        let remote = self
            .primary_remote()
            .unwrap_or_else(|_| "origin".to_string());
        let remote_ref_path = format!("refs/remotes/{}/", remote);
        let remote_prefix = format!("{}/", remote);

        let remote_output = self.run_command(&[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname:short)\t%(committerdate:unix)",
            &remote_ref_path,
        ])?;

        let remote_head = format!("{}/HEAD", remote);
        let remote_branches: Vec<(String, String, i64)> = remote_output
            .lines()
            .filter_map(|line| {
                let parts: Vec<&str> = line.split('\t').collect();
                if parts.len() == 2 {
                    let full_name = parts[0];
                    // Skip <remote>/HEAD
                    if full_name == remote_head {
                        return None;
                    }
                    // Strip remote prefix to get local name
                    let local_name = full_name.strip_prefix(&remote_prefix)?;
                    // Skip if local branch exists (user should use local)
                    if local_branch_names.contains(local_name) {
                        return None;
                    }
                    let timestamp = parts[1].parse().unwrap_or(0);
                    Some((local_name.to_string(), remote.clone(), timestamp))
                } else {
                    None
                }
            })
            .collect();

        // Build result: worktrees first, then local, then remote
        let mut result = Vec::new();

        // Worktree branches (sorted by recency from local_branches order)
        for (name, timestamp) in &local_branches {
            if worktree_branches.contains(name) {
                result.push(CompletionBranch {
                    name: name.clone(),
                    timestamp: *timestamp,
                    category: BranchCategory::Worktree,
                });
            }
        }

        // Local branches without worktrees
        for (name, timestamp) in &local_branches {
            if !worktree_branches.contains(name) {
                result.push(CompletionBranch {
                    name: name.clone(),
                    timestamp: *timestamp,
                    category: BranchCategory::Local,
                });
            }
        }

        // Remote-only branches
        for (local_name, remote_name, timestamp) in remote_branches {
            result.push(CompletionBranch {
                name: local_name,
                timestamp,
                category: BranchCategory::Remote(remote_name),
            });
        }

        Ok(result)
    }

    /// Get the merge base between two commits.
    pub fn merge_base(&self, commit1: &str, commit2: &str) -> anyhow::Result<String> {
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
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn has_merge_conflicts(&self, base: &str, head: &str) -> anyhow::Result<bool> {
        // Use modern merge-tree --write-tree mode which exits with 1 when conflicts exist
        // (the old 3-argument deprecated mode always exits with 0)
        // run_command_check returns true for exit 0, false otherwise
        let clean_merge = self.run_command_check(&["merge-tree", "--write-tree", base, head])?;
        Ok(!clean_merge)
    }

    /// Check if merging a branch into target would add anything (not already integrated).
    ///
    /// Uses `git merge-tree` to simulate merging the branch into the target. If the
    /// resulting tree matches the target's tree, then merging would add nothing,
    /// meaning the branch's content is already integrated.
    ///
    /// This handles cases that simple tree comparison misses:
    /// - Squash-merged branches where main has advanced with additional commits
    /// - Rebased branches where the base has moved forward
    ///
    /// Returns:
    /// - `Ok(true)` if merging would change the target (branch has unintegrated changes)
    /// - `Ok(false)` if merging would NOT change target (branch is already integrated)
    /// - `Ok(true)` if merge would have conflicts (conservative: treat as not integrated)
    /// - `Err` if git commands fail
    pub fn would_merge_add_to_target(&self, branch: &str, target: &str) -> anyhow::Result<bool> {
        // Simulate merging branch into target
        // On conflict, merge-tree exits non-zero and we can't get a clean tree
        let merge_result = self.run_command(&["merge-tree", "--write-tree", target, branch]);

        let Ok(merge_tree) = merge_result else {
            // merge-tree failed (likely conflicts) - conservatively treat as having changes
            return Ok(true);
        };

        let merge_tree = merge_tree.trim();
        if merge_tree.is_empty() {
            // Empty output is unexpected - treat as having changes
            return Ok(true);
        }

        // Get target's tree for comparison
        let target_tree = self.rev_parse_tree(&format!("{target}^{{tree}}"))?;

        // If merge result differs from target's tree, merging would add something
        Ok(merge_tree != target_tree)
    }

    /// Get commit subjects (first line of commit message) from a range.
    pub fn commit_subjects(&self, range: &str) -> anyhow::Result<Vec<String>> {
        let output = self.run_command(&["log", "--format=%s", range])?;
        Ok(output.lines().map(String::from).collect())
    }

    /// List all worktrees for this repository.
    ///
    /// Returns a WorktreeList that automatically filters out bare repositories
    /// and provides access to the main worktree.
    pub fn list_worktrees(&self) -> anyhow::Result<WorktreeList> {
        let stdout = self.run_command(&["worktree", "list", "--porcelain"])?;
        let raw_worktrees = Worktree::parse_porcelain_list(&stdout)?;
        WorktreeList::from_raw(raw_worktrees)
    }

    /// Find the worktree path for a given branch, if one exists.
    pub fn worktree_for_branch(&self, branch: &str) -> anyhow::Result<Option<PathBuf>> {
        let worktrees = self.list_worktrees()?;

        Ok(worktrees
            .worktrees
            .iter()
            .find(|wt| wt.branch.as_deref() == Some(branch))
            .map(|wt| wt.path.clone()))
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
            .worktrees
            .iter()
            .find(|wt| wt.path.normalize() == normalized_path)
            .map(|wt| (wt.path.clone(), wt.branch.clone())))
    }

    /// Get branches that don't have worktrees (available for switch).
    pub fn available_branches(&self) -> anyhow::Result<Vec<String>> {
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
    pub fn get_config(&self, key: &str) -> anyhow::Result<Option<String>> {
        match self.run_command(&["config", key]) {
            Ok(value) => Ok(Some(value.trim().to_string())),
            Err(_) => Ok(None), // Config key doesn't exist
        }
    }

    /// Set a git config value.
    pub fn set_config(&self, key: &str, value: &str) -> anyhow::Result<()> {
        self.run_command(&["config", key, value])?;
        Ok(())
    }

    /// Remove a worktree at the specified path.
    pub fn remove_worktree(&self, path: &std::path::Path) -> anyhow::Result<()> {
        let path_str = path.to_str().ok_or_else(|| {
            anyhow::Error::from(GitError::Other {
                message: format!("Worktree path contains invalid UTF-8: {}", path.display()),
            })
        })?;
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
    pub fn refresh_default_branch(&self) -> anyhow::Result<String> {
        let remote = self.primary_remote()?;
        let branch = self.query_remote_default_branch(&remote)?;
        self.cache_default_branch(&remote, &branch)?;
        Ok(branch)
    }

    /// Set the default branch manually.
    ///
    /// This sets the local cache (`refs/remotes/<remote>/HEAD`) to point to the
    /// specified branch. Use `--refresh` to re-query the remote and overwrite
    /// this value.
    pub fn set_default_branch(&self, branch: &str) -> anyhow::Result<()> {
        let remote = self.primary_remote()?;
        self.cache_default_branch(&remote, branch)
    }

    /// Clear the default branch cache.
    ///
    /// Removes `refs/remotes/<remote>/HEAD` so the next call to `default_branch()`
    /// will re-query the remote.
    ///
    /// Returns `true` if cache was cleared, `false` if no cache existed.
    pub fn clear_default_branch_cache(&self) -> anyhow::Result<bool> {
        let remote = self.primary_remote()?;
        if self.get_local_default_branch(&remote).is_ok() {
            self.run_command(&["remote", "set-head", "-d", &remote])?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Check if two refs have identical tree content (same files/directories).
    /// Returns true when content is identical even if commit history differs.
    ///
    /// Useful for detecting squash merges or rebases where the content has been
    /// integrated but commit ancestry doesn't show the relationship.
    pub fn trees_match(&self, ref1: &str, ref2: &str) -> anyhow::Result<bool> {
        // Parse both tree refs in a single git command
        let output = self.run_command(&[
            "rev-parse",
            &format!("{ref1}^{{tree}}"),
            &format!("{ref2}^{{tree}}"),
        ])?;
        let mut lines = output.lines();
        let tree1 = lines.next().unwrap_or_default().trim();
        let tree2 = lines.next().unwrap_or_default().trim();
        Ok(tree1 == tree2)
    }

    /// Check if HEAD's tree SHA matches a branch's tree SHA.
    /// Returns true when content is identical even if commit history differs.
    pub fn head_tree_matches_branch(&self, branch: &str) -> anyhow::Result<bool> {
        self.trees_match("HEAD", branch)
    }

    fn rev_parse_tree(&self, spec: &str) -> anyhow::Result<String> {
        self.run_command(&["rev-parse", spec])
            .map(|output| output.trim().to_string())
    }

    // Private helper methods for default_branch()

    fn get_local_default_branch(&self, remote: &str) -> anyhow::Result<String> {
        let stdout =
            self.run_command(&["rev-parse", "--abbrev-ref", &format!("{}/HEAD", remote)])?;
        DefaultBranchName::from_local(remote, &stdout).map(DefaultBranchName::into_string)
    }

    fn query_remote_default_branch(&self, remote: &str) -> anyhow::Result<String> {
        let stdout = self.run_command(&["ls-remote", "--symref", remote, "HEAD"])?;
        DefaultBranchName::from_remote(&stdout).map(DefaultBranchName::into_string)
    }

    fn cache_default_branch(&self, remote: &str, branch: &str) -> anyhow::Result<()> {
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
    pub fn project_identifier(&self) -> anyhow::Result<String> {
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
            .ok_or_else(|| {
                anyhow::Error::from(GitError::Other {
                    message: format!(
                        "Repository directory has no valid name: {}",
                        repo_root.display()
                    ),
                })
            })?;

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
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn run_command(&self, args: &[&str]) -> anyhow::Result<String> {
        use std::time::Instant;

        let mut cmd = Command::new("git");
        cmd.args(args);
        cmd.current_dir(&self.path);

        // Log: $ git <args> [worktree]
        // TODO: Guard with log::log_enabled! if args.join() overhead becomes measurable
        let worktree_name = if self.path.to_str() == Some(".") {
            log::debug!("$ git {}", args.join(" "));
            ".".to_string()
        } else {
            let worktree = self
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?");
            log::debug!("$ git {} [{}]", args.join(" "), worktree);
            worktree.to_string()
        };

        let t0 = Instant::now();
        let output = cmd
            .output()
            .with_context(|| format!("Failed to execute: git {}", args.join(" ")))?;
        let duration = t0.elapsed();

        // Performance tracing at debug level (enable with RUST_LOG=debug)
        log::debug!(
            "[wt-trace] worktree={} cmd=\"git {}\" dur={:.1}ms",
            worktree_name,
            args.join(" "),
            duration.as_secs_f64() * 1e3
        );

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Normalize carriage returns to newlines for consistent output
            // Git uses \r for progress updates; in non-TTY contexts this causes snapshot instability
            let stderr = stderr.replace('\r', "\n");
            // Log errors with ! prefix
            for line in stderr.trim().lines() {
                log::debug!("  ! {}", line);
            }
            // Some git commands print errors to stdout (e.g., `commit` with nothing to commit)
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
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn run_command_check(&self, args: &[&str]) -> anyhow::Result<bool> {
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
            .with_context(|| format!("Failed to execute: git {}", args.join(" ")))?;

        let success = output.status.success();
        if !success {
            log::debug!("  → exit code: non-zero");
        }
        Ok(success)
    }
}

#[cfg(test)]
mod tests;
