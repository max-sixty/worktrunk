//! Repository - git repository operations.
//!
//! This module provides the [`Repository`] type for interacting with git repositories,
//! and [`WorkingTree`] for worktree-specific operations.
//!
//! # Module organization
//!
//! - `mod.rs` - Core types and construction
//! - `working_tree.rs` - WorkingTree struct and worktree-specific operations
//! - `branches.rs` - Branch listing, existence checks, completions
//! - `worktrees.rs` - Worktree management (list, resolve, remove)
//! - `remotes.rs` - Remote and URL operations
//! - `diff.rs` - Diff, history, and commit operations
//! - `config.rs` - Git config, hints, markers, and default branch detection
//! - `integration.rs` - Integration detection (same commit, ancestor, trees match)

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};

use dashmap::DashMap;
use once_cell::sync::OnceCell;

use anyhow::{Context, bail};

use dunce::canonicalize;

use crate::config::ProjectConfig;

// Import types from parent module
use super::{DefaultBranchName, GitError, LineDiff, WorktreeInfo};

// Re-export types needed by submodules
pub(super) use super::{BranchCategory, CompletionBranch, DiffStats, GitRemoteUrl};

// Submodules with impl blocks
mod branches;
mod config;
mod diff;
mod integration;
mod remotes;
mod working_tree;
mod worktrees;

// Re-export WorkingTree
pub(super) use working_tree::path_to_logging_context;
pub use working_tree::{TreesMatchResult, WorkingTree};

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
pub(super) struct RepoCache {
    // ========== Repo-wide values (same for all worktrees) ==========
    /// Whether this is a bare repository
    pub(super) is_bare: OnceCell<bool>,
    /// Default branch (main, master, etc.)
    pub(super) default_branch: OnceCell<Option<String>>,
    /// Invalid default branch config (set when user configured a non-existent branch).
    /// Populated by `default_branch()` as a side effect, read by `invalid_default_branch_config()`.
    pub(super) invalid_default_branch: OnceCell<Option<String>>,
    /// Effective integration target (local default branch or upstream if ahead)
    pub(super) integration_target: OnceCell<Option<String>>,
    /// Primary remote name (None if no remotes configured)
    pub(super) primary_remote: OnceCell<Option<String>>,
    /// Primary remote URL (None if no remotes configured or no URL)
    pub(super) primary_remote_url: OnceCell<Option<String>>,
    /// Project identifier derived from remote URL
    pub(super) project_identifier: OnceCell<String>,
    /// Base path for worktrees (repo root for normal repos, bare repo path for bare)
    pub(super) worktree_base: OnceCell<PathBuf>,
    /// Project config (loaded from .config/wt.toml in main worktree)
    pub(super) project_config: OnceCell<Option<ProjectConfig>>,
    /// Merge-base cache: (commit1, commit2) -> merge_base_sha
    pub(super) merge_base: DashMap<(String, String), String>,
    /// Batch ahead/behind cache: (base_ref, branch_name) -> (ahead, behind)
    /// Populated by batch_ahead_behind(), used by get_cached_ahead_behind()
    pub(super) ahead_behind: DashMap<(String, String), (usize, usize)>,

    // ========== Per-worktree values (keyed by path) ==========
    /// Worktree root paths: worktree_path -> canonicalized root
    pub(super) worktree_roots: DashMap<PathBuf, PathBuf>,
    /// Current branch per worktree: worktree_path -> branch name (None = detached HEAD)
    pub(super) current_branches: DashMap<PathBuf, Option<String>>,
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
/// if let Some(default) = repo.default_branch() {
///     println!("Default branch: {}", default);
/// }
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
    pub(super) cache: Arc<RepoCache>,
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

    // =========================================================================
    // Core repository properties
    // =========================================================================

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

    // =========================================================================
    // Command execution
    // =========================================================================

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
