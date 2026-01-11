use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};

use dashmap::DashMap;
use once_cell::sync::OnceCell;

use anyhow::{Context, bail};
use normalize_path::NormalizePath;

use dunce::canonicalize;

use crate::config::ProjectConfig;

// Import types and functions from parent module (mod.rs)
use super::{
    BranchCategory, CompletionBranch, DefaultBranchName, DiffStats, GitError, GitRemoteUrl,
    LineDiff, WorktreeInfo,
};

// ============================================================================
// Repository Cache
// ============================================================================

/// Cached data for a single repository.
///
/// Contains:
/// - Repo-wide values (same for all worktrees): is_bare, default_branch, etc.
/// - Per-worktree values keyed by path: worktree_root, current_branch
///
/// Wrapped in Arc to allow releasing the outer HashMap lock before accessing
/// cached values, avoiding deadlocks when cached methods call each other.
#[derive(Debug, Default)]
struct RepoCache {
    // ========== Repo-wide values (same for all worktrees) ==========
    /// Whether this is a bare repository
    is_bare: OnceCell<bool>,
    /// Default branch (main, master, etc.)
    default_branch: OnceCell<String>,
    /// Effective integration target (local default branch or upstream if ahead)
    integration_target: OnceCell<String>,
    /// Primary remote name (None if no remotes configured)
    primary_remote: OnceCell<Option<String>>,
    /// Primary remote URL (None if no remotes configured or no URL)
    primary_remote_url: OnceCell<Option<String>>,
    /// Project identifier derived from remote URL
    project_identifier: OnceCell<String>,
    /// Base path for worktrees (repo root for normal repos, bare repo path for bare)
    worktree_base: OnceCell<PathBuf>,
    /// Project config (loaded from .config/wt.toml in main worktree)
    project_config: OnceCell<Option<ProjectConfig>>,
    /// Merge-base cache: (commit1, commit2) -> merge_base_sha
    merge_base: DashMap<(String, String), String>,
    /// Batch ahead/behind cache: (base_ref, branch_name) -> (ahead, behind)
    /// Populated by batch_ahead_behind(), used by get_cached_ahead_behind()
    ahead_behind: DashMap<(String, String), (usize, usize)>,

    // ========== Per-worktree values (keyed by path) ==========
    /// Worktree root paths: worktree_path -> canonicalized root
    worktree_roots: DashMap<PathBuf, PathBuf>,
    /// Current branch per worktree: worktree_path -> branch name (None = detached HEAD)
    current_branches: DashMap<PathBuf, Option<String>>,
}

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

/// Repository state for git operations.
///
/// Represents the shared state of a git repository (the `.git` directory).
/// For worktree-specific operations, use [`WorkingTree`] obtained via
/// [`current_worktree()`](Self::current_worktree) or [`worktree_at()`](Self::worktree_at).
///
/// # Examples
///
/// ```no_run
/// use worktrunk::git::Repository;
///
/// let repo = Repository::current()?;
/// let wt = repo.current_worktree();
///
/// // Repo-wide operations
/// let default = repo.default_branch()?;
///
/// // Worktree-specific operations
/// let branch = wt.branch()?;
/// let dirty = wt.is_dirty()?;
/// # Ok::<(), anyhow::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct Repository {
    /// Path used for discovering the repository and running git commands.
    /// For repo-wide operations, any path within the repo works.
    discovery_path: PathBuf,
    /// The shared .git directory, computed at construction time.
    git_common_dir: PathBuf,
    /// Cached data for this repository. Shared across clones via Arc.
    cache: Arc<RepoCache>,
}

/// A borrowed handle for running git commands in a specific worktree.
///
/// This type borrows a [`Repository`] and holds a path to a specific worktree.
/// All worktree-specific operations (like `branch`, `is_dirty`) are on this type.
///
/// For an owned equivalent that can be cloned across threads, see [`super::BranchRef`].
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
    repo: &'a Repository,
    path: PathBuf,
}

/// Get a short display name for a path, used in logging context.
fn path_to_logging_context(path: &Path) -> String {
    if path.to_str() == Some(".") {
        ".".to_string()
    } else {
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(".")
            .to_string()
    }
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

    /// Get working tree diff stats vs a base branch, if trees differ.
    ///
    /// When `base_branch` is `None` (main worktree), returns `Some(LineDiff::default())`.
    /// If the base branch tree matches HEAD and the working tree is dirty, computes
    /// the precise diff; otherwise returns zero to indicate trees match.
    /// When trees differ, returns `None` so callers can skip expensive comparisons.
    pub fn working_tree_diff_with_base(
        &self,
        base_branch: Option<&str>,
        working_tree_dirty: bool,
    ) -> anyhow::Result<Option<LineDiff>> {
        let Some(branch) = base_branch else {
            // Main worktree has no base to compare against
            return Ok(Some(LineDiff::default()));
        };

        // Check if branch exists
        if self
            .run_command(&["rev-parse", "--verify", branch])
            .is_err()
        {
            return Ok(None);
        }

        // Check if trees match
        let trees_match = self
            .run_command(&["diff-tree", "--quiet", "HEAD", branch])
            .is_ok();

        if trees_match {
            // Trees identical - if working tree is dirty, compute the diff
            if working_tree_dirty {
                Ok(Some(self.working_tree_diff_vs_ref(branch)?))
            } else {
                Ok(Some(LineDiff::default()))
            }
        } else {
            Ok(None)
        }
    }
}

impl Repository {
    /// Discover the repository from the current directory.
    ///
    /// This is the primary way to create a Repository. If the -C flag was used,
    /// this uses that path instead of the actual current directory.
    ///
    /// For worktree-specific operations on paths other than cwd, use
    /// `repo.worktree_at(path)` to get a [`WorkingTree`].
    pub fn current() -> anyhow::Result<Self> {
        Self::at(base_path().clone())
    }

    /// Discover the repository from the specified path.
    ///
    /// Creates a new Repository with its own cache. For sharing cache across
    /// operations (e.g., parallel tasks in `wt list`), clone an existing
    /// Repository instead of calling `at()` multiple times.
    ///
    /// Use cases:
    /// - **Command entry points**: Starting a new command that needs a Repository
    /// - **Tests**: Tests that need to operate on test repositories
    ///
    /// For worktree-specific operations within an existing Repository context,
    /// use [`Repository::worktree_at()`] instead.
    pub fn at(path: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let discovery_path = path.into();
        let git_common_dir = Self::resolve_git_common_dir(&discovery_path)?;
        Ok(Self {
            discovery_path,
            git_common_dir,
            cache: Arc::new(RepoCache::default()),
        })
    }

    /// Check if this repository shares its cache with another.
    ///
    /// Returns true if both repositories point to the same underlying cache.
    /// This is primarily useful for testing that cloned repositories share
    /// cached data.
    #[doc(hidden)]
    pub fn shares_cache_with(&self, other: &Repository) -> bool {
        Arc::ptr_eq(&self.cache, &other.cache)
    }

    /// Resolve the git common directory for a path.
    // TODO: Consolidate the "Failed to execute: git ..." context pattern.
    // Currently duplicated in WorkingTree::run_command, Repository::run_command, and here.
    // Consider extracting a helper that handles the context message consistently.
    fn resolve_git_common_dir(discovery_path: &Path) -> anyhow::Result<PathBuf> {
        use crate::shell_exec::run;

        let mut cmd = Command::new("git");
        cmd.args(["rev-parse", "--git-common-dir"]);
        cmd.current_dir(discovery_path);

        let output = run(&mut cmd, Some(&path_to_logging_context(discovery_path)))
            .context("Failed to execute: git rev-parse --git-common-dir")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("{}", stderr.trim());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let path = PathBuf::from(stdout.trim());
        if path.is_relative() {
            canonicalize(discovery_path.join(&path))
                .context("Failed to resolve git common directory")
        } else {
            Ok(path)
        }
    }

    /// Get the path this repository was discovered from.
    ///
    /// This is primarily for internal use. For worktree operations,
    /// use [`current_worktree()`](Self::current_worktree) or [`worktree_at()`](Self::worktree_at).
    pub fn discovery_path(&self) -> &Path {
        &self.discovery_path
    }

    /// Get a worktree view at the current directory.
    ///
    /// This is the primary way to get a [`WorkingTree`] for worktree-specific operations.
    pub fn current_worktree(&self) -> WorkingTree<'_> {
        self.worktree_at(base_path().clone())
    }

    /// Get a worktree view at a specific path.
    ///
    /// Use this when you need to operate on a worktree other than the current one.
    pub fn worktree_at(&self, path: impl Into<PathBuf>) -> WorkingTree<'_> {
        WorkingTree {
            repo: self,
            path: path.into(),
        }
    }

    /// Get the primary remote name for this repository.
    ///
    /// Returns a consistent value across all worktrees (not branch-specific).
    ///
    /// Uses the following strategy:
    /// 1. Use git's [`checkout.defaultRemote`][1] config if set and has a URL
    /// 2. Otherwise, get the first remote with a configured URL
    /// 3. Return error if no remotes exist
    ///
    /// Result is cached in the shared repo cache (shared across all worktrees).
    ///
    /// [1]: https://git-scm.com/docs/git-config#Documentation/git-config.txt-checkoutdefaultRemote
    pub fn primary_remote(&self) -> anyhow::Result<String> {
        self.cache
            .primary_remote
            .get_or_init(|| {
                // Check git's checkout.defaultRemote config
                if let Ok(default_remote) = self.run_command(&["config", "checkout.defaultRemote"])
                {
                    let default_remote = default_remote.trim();
                    if !default_remote.is_empty() && self.remote_has_url(default_remote) {
                        return Some(default_remote.to_string());
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

                first_remote.map(|s| s.to_string())
            })
            .clone()
            .ok_or_else(|| anyhow::anyhow!("No remotes configured"))
    }

    /// Check if a remote has a URL configured.
    fn remote_has_url(&self, remote: &str) -> bool {
        self.run_command(&["config", &format!("remote.{}.url", remote)])
            .map(|url| !url.trim().is_empty())
            .unwrap_or(false)
    }

    /// Get the URL for a remote, if configured.
    pub fn remote_url(&self, remote: &str) -> Option<String> {
        self.run_command(&["remote", "get-url", remote])
            .ok()
            .map(|url| url.trim().to_string())
            .filter(|url| !url.is_empty())
    }

    /// Get the URL for the primary remote, if configured.
    ///
    /// Result is cached in the repository's shared cache (same for all clones).
    pub fn primary_remote_url(&self) -> Option<String> {
        self.cache
            .primary_remote_url
            .get_or_init(|| {
                self.primary_remote()
                    .ok()
                    .and_then(|remote| self.remote_url(&remote))
            })
            .clone()
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

        // Try remote branch (if remotes exist)
        let Ok(remote) = self.primary_remote() else {
            return Ok(false);
        };
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

    /// Get the current branch name, or error if in detached HEAD state.
    ///
    /// `action` describes what requires being on a branch (e.g., "merge").
    pub fn require_current_branch(&self, action: &str) -> anyhow::Result<String> {
        self.current_worktree().branch()?.ok_or_else(|| {
            GitError::DetachedHead {
                action: Some(action.into()),
            }
            .into()
        })
    }

    /// Read a user-defined marker from `worktrunk.state.<branch>.marker` in git config.
    ///
    /// Markers are stored as JSON: `{"marker": "text", "set_at": unix_timestamp}`.
    pub fn branch_keyed_marker(&self, branch: &str) -> Option<String> {
        #[derive(serde::Deserialize)]
        struct MarkerValue {
            marker: Option<String>,
        }

        let config_key = format!("worktrunk.state.{branch}.marker");
        let raw = self
            .run_command(&["config", "--get", &config_key])
            .ok()
            .map(|output| output.trim().to_string())
            .filter(|s| !s.is_empty())?;

        let parsed: MarkerValue = serde_json::from_str(&raw).ok()?;
        parsed.marker
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

    /// Check if a hint has been shown in this repo.
    ///
    /// Hints are stored as `worktrunk.hints.<name> = true`.
    /// TODO: Could move to global git config if we accumulate more global hints.
    pub fn has_shown_hint(&self, name: &str) -> bool {
        self.run_command(&["config", "--get", &format!("worktrunk.hints.{name}")])
            .is_ok()
    }

    /// Mark a hint as shown in this repo.
    pub fn mark_hint_shown(&self, name: &str) -> anyhow::Result<()> {
        self.run_command(&["config", &format!("worktrunk.hints.{name}"), "true"])?;
        Ok(())
    }

    /// Clear a hint so it will show again.
    pub fn clear_hint(&self, name: &str) -> anyhow::Result<bool> {
        match self.run_command(&["config", "--unset", &format!("worktrunk.hints.{name}")]) {
            Ok(_) => Ok(true),
            Err(_) => Ok(false), // Key didn't exist
        }
    }

    /// List all hints that have been shown in this repo.
    pub fn list_shown_hints(&self) -> Vec<String> {
        self.run_command(&["config", "--get-regexp", r"^worktrunk\.hints\."])
            .unwrap_or_default()
            .lines()
            .filter_map(|line| {
                // Format: "worktrunk.hints.worktree-path true"
                line.split_whitespace()
                    .next()
                    .and_then(|key| key.strip_prefix("worktrunk.hints."))
                    .map(String::from)
            })
            .collect()
    }

    /// Clear all hints so they will show again.
    pub fn clear_all_hints(&self) -> anyhow::Result<usize> {
        let hints = self.list_shown_hints();
        let count = hints.len();
        for hint in hints {
            self.clear_hint(&hint)?;
        }
        Ok(count)
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
    /// 1. Check worktrunk cache (`git config worktrunk.default-branch`) — single command
    /// 2. Detect primary remote, try its cache (e.g., `origin/HEAD`)
    /// 3. Query remote (`git ls-remote`) — may take 100ms-2s
    /// 4. Infer from local branches if no remote
    ///
    /// Detection results are cached to `worktrunk.default-branch` for future calls.
    /// Result is cached in the shared repo cache (shared across all worktrees).
    pub fn default_branch(&self) -> anyhow::Result<String> {
        self.cache
            .default_branch
            .get_or_try_init(|| {
                // Fast path: check worktrunk's persistent cache (git config)
                if let Ok(branch) =
                    self.run_command(&["config", "--get", "worktrunk.default-branch"])
                {
                    let branch = branch.trim();
                    if !branch.is_empty() {
                        return Ok(branch.to_string());
                    }
                }

                // Detect and persist to git config for future processes
                let branch = self.detect_default_branch()?;
                let _ = self.run_command(&["config", "worktrunk.default-branch", &branch]);
                Ok(branch)
            })
            .cloned()
    }

    /// Detect the default branch without using worktrunk's cache.
    ///
    /// Used by `default_branch()` to populate the cache, and by
    /// `wt config state get default-branch --refresh` to force re-detection.
    pub fn detect_default_branch(&self) -> anyhow::Result<String> {
        // Try to get from the primary remote
        if let Ok(remote) = self.primary_remote() {
            // Try git's cache for this remote (e.g., origin/HEAD)
            if let Ok(branch) = self.get_local_default_branch(&remote) {
                return Ok(branch);
            }

            // Query remote (no caching to git's remote HEAD - we only manage worktrunk's cache)
            if let Ok(branch) = self.query_remote_default_branch(&remote) {
                return Ok(branch);
            }
        }

        // Fallback: No remote or remote query failed, try to infer locally
        // TODO: Show message to user when using inference fallback:
        //   "No remote configured. Using inferred default branch: {branch}"
        //   "To set explicitly, run: wt config state default-branch set <branch>"
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
    /// 2. Check symbolic-ref HEAD (authoritative for bare repos, works before first commit)
    /// 3. Check user's git config init.defaultBranch (if branch exists)
    /// 4. Look for common branch names (main, master, develop, trunk)
    /// 5. Fail if none of the above work
    fn infer_default_branch_locally(&self) -> anyhow::Result<String> {
        // 1. If there's only one local branch, use it
        let branches = self.local_branches()?;
        if branches.len() == 1 {
            return Ok(branches[0].clone());
        }

        // 2. Check symbolic-ref HEAD - authoritative for bare repos and empty repos
        // - Bare repo directory: HEAD always points to the default branch
        // - Empty repos: No branches exist yet, but HEAD tells us the intended default
        // - Linked worktrees: HEAD points to CURRENT branch, so skip this heuristic
        // - Normal repos: HEAD points to CURRENT branch, so skip this heuristic
        let is_bare = self.is_bare().unwrap_or(false);
        let in_linked_worktree = self.current_worktree().is_linked().unwrap_or(false);
        if ((is_bare && !in_linked_worktree) || branches.is_empty())
            && let Ok(head_ref) = self.run_command(&["symbolic-ref", "HEAD"])
            && let Some(branch) = head_ref.trim().strip_prefix("refs/heads/")
        {
            return Ok(branch.to_string());
        }

        // 3. Check git config init.defaultBranch (if branch exists)
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

        // 5. Give up — can't infer
        Err(GitError::Other {
            message:
                "Could not infer default branch. Please specify target branch explicitly or set up a remote."
                    .into(),
        }
        .into())
    }

    /// List all local branches.
    fn local_branches(&self) -> anyhow::Result<Vec<String>> {
        // Use lstrip=2 instead of refname:short - git adds "heads/" prefix to short
        // names when disambiguation is needed (e.g., branch "foo" + remote "foo").
        let stdout = self.run_command(&["branch", "--format=%(refname:lstrip=2)"])?;
        Ok(stdout.lines().map(|s| s.trim().to_string()).collect())
    }

    /// Get the git common directory (the actual .git directory for the repository).
    ///
    /// For linked worktrees, this returns the shared `.git` directory in the main
    /// worktree, not the per-worktree `.git/worktrees/<name>` directory.
    /// See [`--git-common-dir`][1] for details.
    ///
    /// Always returns an absolute path, resolving any relative paths returned by git.
    /// Result is cached per Repository instance (also used as key for global cache).
    ///
    /// [1]: https://git-scm.com/docs/git-rev-parse#Documentation/git-rev-parse.txt---git-common-dir
    pub fn git_common_dir(&self) -> &Path {
        &self.git_common_dir
    }

    /// Get the directory where worktrunk background logs are stored.
    ///
    /// Logs are centralized under the main worktree's git directory:
    /// `.git/wt-logs/`.
    pub fn wt_logs_dir(&self) -> PathBuf {
        self.git_common_dir().join("wt-logs")
    }

    /// Get the base directory where worktrees are created relative to.
    ///
    /// For normal repositories: the parent of .git (the repo root).
    /// For bare repositories: the bare repository directory itself.
    ///
    /// This is the path that should be used when constructing worktree paths.
    /// Result is cached in the repository's shared cache (same for all clones).
    pub fn worktree_base(&self) -> anyhow::Result<PathBuf> {
        self.cache
            .worktree_base
            .get_or_try_init(|| {
                let git_common_dir =
                    canonicalize(self.git_common_dir()).context("Failed to canonicalize path")?;

                if self.is_bare()? {
                    Ok(git_common_dir)
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
                        })
                        .map(Path::to_path_buf)
                }
            })
            .cloned()
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
        self.worktree_base()
    }

    /// Check if this is a bare repository (no working tree).
    ///
    /// Bare repositories have no main worktree — all worktrees are linked
    /// worktrees at templated paths, including the default branch.
    /// Result is cached in the repository's shared cache (same for all clones).
    pub fn is_bare(&self) -> anyhow::Result<bool> {
        self.cache
            .is_bare
            .get_or_try_init(|| {
                let output = self.run_command(&["config", "--bool", "core.bare"])?;
                Ok(output.trim() == "true")
            })
            .copied()
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

    /// Start fsmonitor daemon at a specific worktree path.
    ///
    /// Like `start_fsmonitor_daemon` but runs the command in the specified worktree.
    pub fn start_fsmonitor_daemon_at(&self, path: &Path) {
        if let Err(e) = self
            .worktree_at(path)
            .run_command(&["fsmonitor--daemon", "start"])
        {
            log::debug!("fsmonitor daemon start failed (usually fine): {e}");
        }
    }

    /// Check if base is an ancestor of head (i.e., would be a fast-forward).
    ///
    /// See [`--is-ancestor`][1] for details.
    ///
    /// [1]: https://git-scm.com/docs/git-merge-base#Documentation/git-merge-base.txt---is-ancestor
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
    /// Uses merge-base (cached) to find common ancestor, then two-dot diff to
    /// check for file changes. Returns false when the diff is empty (no added changes).
    ///
    /// For orphan branches (no common ancestor with target), returns true since all
    /// their changes are unique.
    pub fn has_added_changes(&self, branch: &str, target: &str) -> anyhow::Result<bool> {
        // Try to get merge-base (cached). Orphan branches will fail here.
        let merge_base = match self.merge_base(target, branch) {
            Ok(base) => base,
            Err(e) => {
                // Check if it's an orphan branch (no common ancestor)
                let msg = e.to_string();
                if msg.contains("no merge base") || msg.contains("Not a valid commit") {
                    return Ok(true); // Orphan branches have unique changes
                }
                return Err(e);
            }
        };

        // git diff --name-only merge_base..branch shows files changed from merge-base to branch
        let range = format!("{merge_base}..{branch}");
        let output = self.run_command(&["diff", "--name-only", &range])?;
        Ok(!output.trim().is_empty())
    }

    /// Count commits between base and head.
    pub fn count_commits(&self, base: &str, head: &str) -> anyhow::Result<usize> {
        // Limit concurrent rev-list operations to reduce mmap thrash on commit-graph
        let _guard = super::HEAVY_OPS_SEMAPHORE.acquire();

        let range = format!("{}..{}", base, head);
        let stdout = self.run_command(&["rev-list", "--count", &range])?;

        stdout
            .trim()
            .parse()
            .context("Failed to parse commit count")
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
        stdout.trim().parse().context("Failed to parse timestamp")
    }

    /// Get commit timestamps for multiple commits in a single git command.
    ///
    /// Returns a map from commit SHA to timestamp. More efficient than calling
    /// `commit_timestamp` multiple times when you have many commits.
    pub fn commit_timestamps(
        &self,
        commits: &[&str],
    ) -> anyhow::Result<std::collections::HashMap<String, i64>> {
        use std::collections::HashMap;

        if commits.is_empty() {
            return Ok(HashMap::new());
        }

        // Build command: git show -s --format='%H %ct' sha1 sha2 sha3 ...
        let mut args = vec!["show", "-s", "--format=%H %ct"];
        args.extend(commits);

        let stdout = self.run_command(&args)?;

        let mut result = HashMap::with_capacity(commits.len());
        for line in stdout.lines() {
            if let Some((sha, timestamp_str)) = line.split_once(' ')
                && let Ok(timestamp) = timestamp_str.parse::<i64>()
            {
                result.insert(sha.to_string(), timestamp);
            }
        }

        Ok(result)
    }

    /// Get commit message (subject line) for a commit.
    pub fn commit_message(&self, commit: &str) -> anyhow::Result<String> {
        let stdout = self.run_command(&["show", "-s", "--format=%s", commit])?;
        Ok(stdout.trim().to_owned())
    }

    /// Get the upstream tracking branch for the given branch.
    ///
    /// Uses [`@{upstream}` syntax][1] to resolve the tracking branch.
    ///
    /// [1]: https://git-scm.com/docs/gitrevisions#Documentation/gitrevisions.txt-emltaboranchgtemuaboranchgtupaboranchgtupstream
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

    /// Determine the effective target for integration checks.
    ///
    /// If the upstream of the local target (e.g., `origin/main`) is strictly ahead of
    /// the local target (i.e., local is an ancestor of upstream but not the same commit),
    /// uses the upstream. This handles the common case where a branch was merged remotely
    /// but the user hasn't pulled yet.
    ///
    /// When local and upstream are the same commit, prefers local for clearer messaging.
    ///
    /// Returns the effective target ref to check against.
    ///
    /// Used by both `wt list` and `wt remove` to ensure consistent integration detection.
    ///
    /// TODO(future): When local and remote have diverged (neither is ancestor),
    /// check integration against both and delete only if integrated into both.
    /// Current behavior: uses only local in diverged state, may miss remote-merged branches.
    pub fn effective_integration_target(&self, local_target: &str) -> String {
        // Get the upstream ref for the local target (e.g., origin/main for main)
        let upstream = match self.upstream_branch(local_target) {
            Ok(Some(upstream)) => upstream,
            _ => return local_target.to_string(),
        };

        // If local and upstream are the same commit, prefer local for clearer messaging
        if self.same_commit(local_target, &upstream).unwrap_or(false) {
            return local_target.to_string();
        }

        // Check if local is strictly behind upstream (local is ancestor of upstream)
        // This means upstream has commits that local doesn't have
        // On error, fall back to local target (defensive: don't fail due to git errors)
        if self.is_ancestor(local_target, &upstream).unwrap_or(false) {
            return upstream;
        }

        local_target.to_string()
    }

    /// Get the cached integration target for this repository.
    ///
    /// This is the effective target for integration checks (status symbols, safe deletion).
    /// May be upstream (e.g., "origin/main") if it's ahead of local, catching remotely-merged branches.
    ///
    /// Result is cached in the shared repo cache (shared across all worktrees).
    pub fn integration_target(&self) -> anyhow::Result<String> {
        self.cache
            .integration_target
            .get_or_try_init(|| {
                let default_branch = self.default_branch()?;
                Ok(self.effective_integration_target(&default_branch))
            })
            .cloned()
    }

    /// Get merge/rebase status for the worktree at this repository's discovery path.
    pub fn worktree_state(&self) -> anyhow::Result<Option<String>> {
        let git_dir = self.worktree_at(self.discovery_path()).git_dir()?;

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
    ///
    /// Uses `merge_base()` internally (which is cached) to compute the common
    /// ancestor, then counts commits using two-dot syntax. This allows the
    /// merge-base result to be reused across multiple operations.
    pub fn ahead_behind(&self, base: &str, head: &str) -> anyhow::Result<(usize, usize)> {
        // Get merge-base (cached in shared repo cache)
        let merge_base = self.merge_base(base, head)?;

        // Count commits using two-dot syntax (faster when merge-base is cached)
        // ahead = commits in head but not in merge_base
        // behind = commits in base but not in merge_base
        let ahead_output =
            self.run_command(&["rev-list", "--count", &format!("{}..{}", merge_base, head)])?;
        let behind_output =
            self.run_command(&["rev-list", "--count", &format!("{}..{}", merge_base, base)])?;

        let ahead: usize = ahead_output
            .trim()
            .parse()
            .context("Failed to parse ahead count")?;
        let behind: usize = behind_output
            .trim()
            .parse()
            .context("Failed to parse behind count")?;

        Ok((ahead, behind))
    }

    /// Batch-fetch ahead/behind counts for all local branches vs a base ref.
    ///
    /// Uses `git for-each-ref --format='%(ahead-behind:BASE)'` (git 2.36+) to get
    /// all counts in a single command. Returns a map from branch name to (ahead, behind).
    ///
    /// Results are cached so subsequent lookups via `get_cached_ahead_behind()` avoid
    /// running individual git commands (though cache access still has minor overhead).
    ///
    /// On git < 2.36 or if the command fails, returns an empty map.
    pub fn batch_ahead_behind(
        &self,
        base: &str,
    ) -> std::collections::HashMap<String, (usize, usize)> {
        let format = format!("%(refname:lstrip=2) %(ahead-behind:{})", base);
        let output = match self.run_command(&[
            "for-each-ref",
            &format!("--format={}", format),
            "refs/heads/",
        ]) {
            Ok(output) => output,
            Err(e) => {
                // Fails on git < 2.36 (no %(ahead-behind:) support), invalid base ref, etc.
                log::debug!("batch_ahead_behind({base}): git for-each-ref failed: {e}");
                return std::collections::HashMap::new();
            }
        };

        let results: std::collections::HashMap<String, (usize, usize)> = output
            .lines()
            .filter_map(|line| {
                // Format: "branch-name ahead behind"
                let mut parts = line.rsplitn(3, ' ');
                let behind: usize = parts.next()?.parse().ok()?;
                let ahead: usize = parts.next()?.parse().ok()?;
                let branch = parts.next()?.to_string();
                // Cache each result for later lookup
                self.cache
                    .ahead_behind
                    .insert((base.to_string(), branch.clone()), (ahead, behind));
                Some((branch, (ahead, behind)))
            })
            .collect();

        results
    }

    /// Get cached ahead/behind counts for a branch.
    ///
    /// Returns cached results from a prior `batch_ahead_behind()` call, or None
    /// if the branch wasn't in the batch or batch wasn't run.
    pub fn get_cached_ahead_behind(&self, base: &str, branch: &str) -> Option<(usize, usize)> {
        self.cache
            .ahead_behind
            .get(&(base.to_string(), branch.to_string()))
            .map(|r| *r)
    }

    /// List all local branches with their HEAD commit SHA.
    /// Returns a vector of (branch_name, commit_sha) tuples.
    pub fn list_local_branches(&self) -> anyhow::Result<Vec<(String, String)>> {
        let output = self.run_command(&[
            "for-each-ref",
            "--format=%(refname:lstrip=2) %(objectname)",
            "refs/heads/",
        ])?;

        let branches: Vec<(String, String)> = output
            .lines()
            .filter_map(|line| {
                let (branch, sha) = line.split_once(' ')?;
                Some((branch.to_string(), sha.to_string()))
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
            "--format=%(refname:lstrip=2) %(objectname)",
            "refs/remotes/",
        ])?;

        let branches: Vec<(String, String)> = output
            .lines()
            .filter_map(|line| {
                let (branch_name, sha) = line.split_once(' ')?;
                // Skip <remote>/HEAD (symref)
                if branch_name.ends_with("/HEAD") {
                    None
                } else {
                    Some((branch_name.to_string(), sha.to_string()))
                }
            })
            .collect();

        Ok(branches)
    }

    /// List all upstream tracking refs that local branches are tracking.
    ///
    /// Returns a set of upstream refs like "origin/main", "origin/feature".
    /// Useful for filtering remote branches to only show those not tracked locally.
    pub fn list_tracked_upstreams(&self) -> anyhow::Result<std::collections::HashSet<String>> {
        let output =
            self.run_command(&["for-each-ref", "--format=%(upstream:short)", "refs/heads/"])?;

        let upstreams: std::collections::HashSet<String> = output
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| line.to_string())
            .collect();

        Ok(upstreams)
    }

    /// List remote branches that aren't tracked by any local branch.
    ///
    /// Returns (branch_name, commit_sha) pairs for remote branches that have no
    /// corresponding local tracking branch.
    pub fn list_untracked_remote_branches(&self) -> anyhow::Result<Vec<(String, String)>> {
        let all_remote_branches = self.list_remote_branches()?;
        let tracked_upstreams = self.list_tracked_upstreams()?;

        let remote_branches: Vec<_> = all_remote_branches
            .into_iter()
            .filter(|(remote_branch_name, _)| !tracked_upstreams.contains(remote_branch_name))
            .collect();

        Ok(remote_branches)
    }

    /// Get recent commit subjects for style reference.
    ///
    /// Returns up to `count` commit subjects (first line of message), excluding merges.
    /// If `start_ref` is provided, gets commits starting from that ref.
    /// Returns `None` if no commits are found or the command fails.
    pub fn recent_commit_subjects(
        &self,
        start_ref: Option<&str>,
        count: usize,
    ) -> Option<Vec<String>> {
        let count_str = count.to_string();
        let mut args = vec!["log", "--pretty=format:%s", "-n", &count_str, "--no-merges"];
        if let Some(ref_name) = start_ref {
            args.push(ref_name);
        }
        self.run_command(&args).ok().and_then(|output| {
            if output.trim().is_empty() {
                None
            } else {
                Some(output.lines().map(String::from).collect())
            }
        })
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

    /// Get line diff statistics between two refs.
    ///
    /// Uses merge-base (cached) to find common ancestor, then two-dot diff
    /// to get the stats. This allows the merge-base result to be reused
    /// across multiple operations.
    pub fn branch_diff_stats(&self, base: &str, head: &str) -> anyhow::Result<LineDiff> {
        // Limit concurrent diff operations to reduce mmap thrash on pack files
        let _guard = super::HEAVY_OPS_SEMAPHORE.acquire();

        // Get merge-base (cached in shared repo cache)
        let merge_base = self.merge_base(base, head)?;

        // Use two-dot syntax with the cached merge-base
        let range = format!("{}..{}", merge_base, head);
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
    /// Returns the short SHA of the backup commit.
    ///
    /// # Example
    /// ```no_run
    /// use worktrunk::git::Repository;
    ///
    /// let repo = Repository::current()?;
    /// let sha = repo.create_safety_backup("feature → main (squash)")?;
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

        Ok(backup_sha[..7].to_string())
    }

    /// Get all branch names (local branches only).
    pub fn all_branches(&self) -> anyhow::Result<Vec<String>> {
        let stdout = self.run_command(&[
            "branch",
            "--sort=-committerdate",
            "--format=%(refname:lstrip=2)",
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
            .iter()
            .filter_map(|wt| wt.branch.clone())
            .collect();

        // Get local branches with timestamps
        let local_output = self.run_command(&[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname:lstrip=2)\t%(committerdate:unix)",
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

        // Get remote branches with timestamps (if remotes exist)
        let remote_branches: Vec<(String, String, i64)> = if let Ok(remote) = self.primary_remote()
        {
            let remote_ref_path = format!("refs/remotes/{}/", remote);
            let remote_prefix = format!("{}/", remote);

            let remote_output = self.run_command(&[
                "for-each-ref",
                "--sort=-committerdate",
                "--format=%(refname:lstrip=2)\t%(committerdate:unix)",
                &remote_ref_path,
            ])?;

            let remote_head = format!("{}/HEAD", remote);
            remote_output
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
                        Some((local_name.to_string(), remote.to_string(), timestamp))
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

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
    ///
    /// Results are cached in the shared repo cache to avoid redundant git commands
    /// when multiple tasks need the same merge-base (e.g., parallel `wt list` tasks).
    /// The cache key is normalized (sorted) since merge-base(A, B) == merge-base(B, A).
    pub fn merge_base(&self, commit1: &str, commit2: &str) -> anyhow::Result<String> {
        // Normalize key order since merge-base is symmetric: merge-base(A, B) == merge-base(B, A)
        let key = if commit1 <= commit2 {
            (commit1.to_string(), commit2.to_string())
        } else {
            (commit2.to_string(), commit1.to_string())
        };

        Ok(self
            .cache
            .merge_base
            .entry(key)
            .or_insert_with(|| {
                self.run_command(&["merge-base", commit1, commit2])
                    .map(|output| output.trim().to_owned())
                    .unwrap_or_default()
            })
            .clone())
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
    /// let repo = Repository::current()?;
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
    /// to get a [`WorkingTree`] instead.
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

    /// Get branches that don't have worktrees (available for switch).
    pub fn available_branches(&self) -> anyhow::Result<Vec<String>> {
        let all_branches = self.all_branches()?;
        let worktrees = self.list_worktrees()?;

        // Collect branches that have worktrees
        let branches_with_worktrees: std::collections::HashSet<String> = worktrees
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
        // Update worktrunk's cache
        let _ = self.run_command(&["config", "worktrunk.default-branch", &branch]);
        Ok(branch)
    }

    /// Set the default branch manually.
    ///
    /// This sets worktrunk's cache (`worktrunk.default-branch`). Use `--refresh`
    /// to re-query the remote and update git's cache.
    pub fn set_default_branch(&self, branch: &str) -> anyhow::Result<()> {
        self.run_command(&["config", "worktrunk.default-branch", branch])?;
        Ok(())
    }

    /// Clear the default branch cache.
    ///
    /// Clears worktrunk's cache (`worktrunk.default-branch`). The next call to
    /// `default_branch()` will re-detect (using git's cache or querying remote).
    ///
    /// Returns `true` if cache was cleared, `false` if no cache existed.
    pub fn clear_default_branch_cache(&self) -> anyhow::Result<bool> {
        Ok(self
            .run_command(&["config", "--unset", "worktrunk.default-branch"])
            .is_ok())
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

    /// Get a project identifier for approval tracking.
    ///
    /// Uses the git remote URL if available (e.g., "github.com/user/repo"),
    /// otherwise falls back to the repository directory name.
    ///
    /// This identifier is used to track which commands have been approved
    /// for execution in this project.
    ///
    /// Result is cached in the repository's shared cache (same for all clones).
    pub fn project_identifier(&self) -> anyhow::Result<String> {
        self.cache
            .project_identifier
            .get_or_try_init(|| {
                // Try to get the remote URL first (cached)
                if let Some(url) = self.primary_remote_url() {
                    if let Some(parsed) = GitRemoteUrl::parse(url.trim()) {
                        return Ok(parsed.project_identifier());
                    }
                    // Fallback for URLs that don't fit host/owner/repo model (e.g., with ports)
                    let url = url.strip_suffix(".git").unwrap_or(url.as_str());
                    // Handle ssh:// format with port: ssh://git@host:port/path -> host/port/path
                    if let Some(ssh_part) = url.strip_prefix("ssh://") {
                        let ssh_part = ssh_part.strip_prefix("git@").unwrap_or(ssh_part);
                        if let Some(colon_pos) = ssh_part.find(':') {
                            let (host, rest) = ssh_part.split_at(colon_pos);
                            return Ok(format!("{}{}", host, rest.replacen(':', "/", 1)));
                        }
                        return Ok(ssh_part.to_string());
                    }
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
            })
            .cloned()
    }

    /// Load the project configuration (.config/wt.toml) if it exists.
    ///
    /// Result is cached in the repository's shared cache (same for all clones).
    /// Returns `None` if not in a worktree or if no config file exists.
    pub fn load_project_config(&self) -> anyhow::Result<Option<ProjectConfig>> {
        self.cache
            .project_config
            .get_or_try_init(|| {
                match self.current_worktree().root() {
                    Ok(_) => {
                        ProjectConfig::load(self, true).context("Failed to load project config")
                    }
                    Err(_) => Ok(None), // Not in a worktree, no project config
                }
            })
            .cloned()
    }

    /// Get the URL template from project config, if configured.
    ///
    /// Convenience method that extracts `list.url` from the project config.
    /// Returns `None` if no config exists or no URL template is configured.
    pub fn url_template(&self) -> Option<String> {
        self.load_project_config()
            .ok()
            .flatten()
            .and_then(|config| config.list)
            .and_then(|list| list.url)
    }

    /// Get a short display name for this repository, used in logging context.
    ///
    /// Returns "." for the current directory, or the directory name otherwise.
    fn logging_context(&self) -> String {
        path_to_logging_context(&self.discovery_path)
    }

    /// Run a git command in this repository's context.
    ///
    /// Executes the git command with this repository's discovery path as the working directory.
    /// For repo-wide operations, any path within the repo works.
    ///
    /// # Examples
    /// ```no_run
    /// use worktrunk::git::Repository;
    ///
    /// let repo = Repository::current()?;
    /// let branches = repo.run_command(&["branch", "--list"])?;
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn run_command(&self, args: &[&str]) -> anyhow::Result<String> {
        use crate::shell_exec::run;

        let mut cmd = Command::new("git");
        cmd.args(args);
        cmd.current_dir(&self.discovery_path);

        let output = run(&mut cmd, Some(&self.logging_context()))
            .with_context(|| format!("Failed to execute: git {}", args.join(" ")))?;

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
    /// let repo = Repository::current()?;
    /// let is_clean = repo.run_command_check(&["diff", "--quiet", "--exit-code"])?;
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn run_command_check(&self, args: &[&str]) -> anyhow::Result<bool> {
        use crate::shell_exec::run;

        let mut cmd = Command::new("git");
        cmd.args(args);
        cmd.current_dir(&self.discovery_path);

        let output = run(&mut cmd, Some(&self.logging_context()))
            .with_context(|| format!("Failed to execute: git {}", args.join(" ")))?;

        Ok(output.status.success())
    }
}

#[cfg(test)]
mod tests;
