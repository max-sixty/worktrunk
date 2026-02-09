//! Repository - git repository operations.
//!
//! This module provides the [`Repository`] type for interacting with git repositories,
//! [`WorkingTree`] for worktree-specific operations, and [`Branch`] for branch-specific
//! operations.
//!
//! # Module organization
//!
//! - `mod.rs` - Core types and construction
//! - `working_tree.rs` - WorkingTree struct and worktree-specific operations
//! - `branch.rs` - Branch struct and single-branch operations (exists, upstream, remotes)
//! - `branches.rs` - Multi-branch operations (listing, filtering, completions)
//! - `worktrees.rs` - Worktree management (list, resolve, remove)
//! - `remotes.rs` - Remote and URL operations
//! - `diff.rs` - Diff, history, and commit operations
//! - `config.rs` - Git config, hints, markers, and default branch detection
//! - `integration.rs` - Integration detection (same commit, ancestor, trees match)

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::shell_exec::Cmd;

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
mod branch;
mod branches;
mod config;
mod diff;
mod integration;
mod remotes;
mod working_tree;
mod worktrees;

// Re-export WorkingTree and Branch
pub use branch::Branch;
pub use working_tree::WorkingTree;
pub(super) use working_tree::path_to_logging_context;

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
    /// Repository root path (main worktree for normal repos, bare directory for bare repos)
    pub(super) repo_path: OnceCell<PathBuf>,
    /// Default branch (main, master, etc.)
    pub(super) default_branch: OnceCell<Option<String>>,
    /// Invalid default branch config (user configured a branch that doesn't exist).
    /// Populated by `default_branch()` during config validation.
    pub(super) invalid_default_branch: OnceCell<Option<String>>,
    /// Effective integration target (local default branch or upstream if ahead)
    pub(super) integration_target: OnceCell<Option<String>>,
    /// Primary remote name (None if no remotes configured)
    pub(super) primary_remote: OnceCell<Option<String>>,
    /// Primary remote URL (None if no remotes configured or no URL)
    pub(super) primary_remote_url: OnceCell<Option<String>>,
    /// Project identifier derived from remote URL
    pub(super) project_identifier: OnceCell<String>,
    /// Project config (loaded from .config/wt.toml in main worktree)
    pub(super) project_config: OnceCell<Option<ProjectConfig>>,
    /// Merge-base cache: (commit1, commit2) -> merge_base_sha (None = no common ancestor)
    pub(super) merge_base: DashMap<(String, String), Option<String>>,
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

/// Global base path for repository operations, set by -C flag.
static BASE_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Default base path when -C flag is not provided.
static DEFAULT_BASE_PATH: LazyLock<PathBuf> = LazyLock::new(|| PathBuf::from("."));

/// Initialize the global base path for repository operations.
///
/// This should be called once at program startup from main().
/// If not called, defaults to "." (current directory).
pub fn set_base_path(path: PathBuf) {
    BASE_PATH.set(path).ok();
}

/// Get the base path for repository operations.
fn base_path() -> &'static PathBuf {
    BASE_PATH.get().unwrap_or(&DEFAULT_BASE_PATH)
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
    ///
    /// Always returns a canonicalized absolute path to ensure consistent
    /// comparison with `WorkingTree::git_dir()`.
    fn resolve_git_common_dir(discovery_path: &Path) -> anyhow::Result<PathBuf> {
        let output = Cmd::new("git")
            .args(["rev-parse", "--git-common-dir"])
            .current_dir(discovery_path)
            .context(path_to_logging_context(discovery_path))
            .run()
            .context("Failed to execute: git rev-parse --git-common-dir")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("{}", stderr.trim());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let path = PathBuf::from(stdout.trim());
        // Always canonicalize to resolve symlinks (e.g., /var -> /private/var on macOS)
        let absolute_path = if path.is_relative() {
            discovery_path.join(&path)
        } else {
            path
        };
        canonicalize(&absolute_path).context("Failed to resolve git common directory")
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

    /// Get a branch handle for branch-specific operations.
    ///
    /// Use this when you need to query properties of a specific branch.
    pub fn branch(&self, name: &str) -> Branch<'_> {
        Branch {
            repo: self,
            name: name.to_string(),
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
    /// Returns `<git-common-dir>/wt-logs/` (typically `.git/wt-logs/`).
    pub fn wt_logs_dir(&self) -> PathBuf {
        self.git_common_dir().join("wt-logs")
    }

    /// The repository root path (the main worktree directory).
    ///
    /// - Normal repositories: the main worktree directory (parent of .git)
    /// - Bare repositories: the bare repository directory itself
    /// - Submodules: the submodule's worktree (e.g., `/parent/sub`, not `/parent/.git/modules/sub`)
    ///
    /// This is the base for template expansion (`{{ repo }}`, `{{ repo_path }}`).
    /// NOT necessarily where established files live — use `primary_worktree()` for that.
    ///
    /// Result is cached in the repository's shared cache (same for all clones).
    ///
    /// # Why we run from `git_common_dir`
    ///
    /// We need to return the *main* worktree regardless of which worktree we were discovered
    /// from. For linked worktrees, `git_common_dir` is the stable reference that's shared
    /// across all worktrees (e.g., `/myapp/.git` whether you're in `/myapp` or `/myapp.feature`).
    ///
    /// # Why the try-fallback approach
    ///
    /// `--show-toplevel` behavior depends on whether git has explicit worktree metadata:
    ///
    /// | git_common_dir location    | Has `core.worktree`? | `--show-toplevel` works? |
    /// |----------------------------|----------------------|--------------------------|
    /// | Normal `.git`              | No (implicit)        | No — "not a work tree"   |
    /// | Submodule `.git/modules/X` | Yes (explicit)       | Yes — reads config       |
    ///
    /// Normal repos don't need `core.worktree` because the worktree is implicitly `parent(.git)`.
    /// Submodules need it because their git data lives in the parent's `.git/modules/`.
    ///
    /// So we try `--show-toplevel` first (handles submodules), fall back to `parent()` (handles
    /// normal repos). This avoids fragile path-based detection of submodules.
    pub fn repo_path(&self) -> &Path {
        self.cache.repo_path.get_or_init(|| {
            // Bare repos have no worktree — the git directory IS the repo
            if self.is_bare() {
                return self.git_common_dir.clone();
            }

            // Submodules: --show-toplevel succeeds (git has explicit core.worktree config)
            if let Ok(out) = Cmd::new("git")
                .args(["rev-parse", "--show-toplevel"])
                .current_dir(&self.git_common_dir)
                .context(path_to_logging_context(&self.git_common_dir))
                .run()
                && out.status.success()
            {
                return PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
            }

            // Normal repos: --show-toplevel fails from inside .git, use implicit relationship
            self.git_common_dir
                .parent()
                .expect("Git directory has no parent")
                .to_path_buf()
        })
    }

    /// Check if this is a bare repository (no working tree).
    ///
    /// Bare repositories have no main worktree — all worktrees are linked
    /// worktrees at templated paths, including the default branch.
    ///
    /// Result is cached in the repository's shared cache (same for all clones).
    /// Runs `git rev-parse --is-bare-repository` from git_common_dir to correctly
    /// detect bare repos even when called from a linked worktree.
    pub fn is_bare(&self) -> bool {
        *self.cache.is_bare.get_or_init(|| {
            // Run from git_common_dir, not discovery_path. This is important for
            // worktrees of bare repos: running from the worktree returns false,
            // but running from the bare repo returns true.
            let output = Cmd::new("git")
                .args(["rev-parse", "--is-bare-repository"])
                .current_dir(&self.git_common_dir)
                .context(path_to_logging_context(&self.git_common_dir))
                .run()
                .expect("git rev-parse failed on valid repo");
            output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "true"
        })
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
        let output = Cmd::new("git")
            .args(args.iter().copied())
            .current_dir(&self.discovery_path)
            .context(self.logging_context())
            .run()
            .with_context(|| format!("Failed to execute: git {}", args.join(" ")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Normalize carriage returns to newlines for consistent output
            // Git uses \r for progress updates; in non-TTY contexts this causes snapshot instability
            let stderr = stderr.replace('\r', "\n");
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
        Ok(self.run_command_output(args)?.status.success())
    }

    /// Delay before showing progress output for slow operations.
    /// See .claude/rules/cli-output-formatting.md: "Progress messages apply only to slow operations (>400ms)"
    pub const SLOW_OPERATION_DELAY_MS: i64 = 400;

    /// Run a git command with delayed output streaming.
    ///
    /// Buffers output initially, then streams if the command takes longer than
    /// `delay_ms`. This provides a quiet experience for fast operations while
    /// still showing progress for slow ones (like `worktree add` on large repos).
    /// Pass `-1` to never switch to streaming (always buffer).
    ///
    /// If `progress_message` is provided, it will be printed to stderr when
    /// streaming starts (i.e., when the delay threshold is exceeded).
    ///
    /// All output (both stdout and stderr from the child) is sent to stderr
    /// to keep stdout clean for commands like `wt switch`.
    pub fn run_command_delayed_stream(
        &self,
        args: &[&str],
        delay_ms: i64,
        progress_message: Option<String>,
    ) -> anyhow::Result<()> {
        // Allow tests to override delay threshold (-1 to disable, 0 for immediate)
        let delay_ms = std::env::var("WT_TEST_DELAYED_STREAM_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(delay_ms);

        let cmd_str = format!("git {}", args.join(" "));
        log::debug!(
            "$ {} [{}] (delayed stream, {}ms)",
            cmd_str,
            self.logging_context(),
            delay_ms
        );

        let mut child = std::process::Command::new("git")
            .args(args)
            .current_dir(&self.discovery_path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_remove(crate::shell_exec::DIRECTIVE_FILE_ENV_VAR)
            .spawn()
            .with_context(|| format!("Failed to spawn: {}", cmd_str))?;

        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");

        // Shared state: when true, output streams directly; when false, buffers
        let streaming = Arc::new(AtomicBool::new(false));
        let buffer = Arc::new(Mutex::new(Vec::new()));

        // Reader threads for stdout and stderr (both go to stderr)
        let stdout_handle = {
            let streaming = streaming.clone();
            let buffer = buffer.clone();
            thread::spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines().map_while(Result::ok) {
                    if streaming.load(Ordering::Relaxed) {
                        let _ = writeln!(std::io::stderr(), "{}", line);
                        let _ = std::io::stderr().flush();
                    } else {
                        buffer.lock().unwrap().push(line);
                    }
                }
            })
        };

        let stderr_handle = {
            let streaming = streaming.clone();
            let buffer = buffer.clone();
            thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    if streaming.load(Ordering::Relaxed) {
                        let _ = writeln!(std::io::stderr(), "{}", line);
                        let _ = std::io::stderr().flush();
                    } else {
                        buffer.lock().unwrap().push(line);
                    }
                }
            })
        };

        let start = Instant::now();

        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let _ = stdout_handle.join();
                    let _ = stderr_handle.join();

                    if status.success() {
                        return Ok(());
                    }
                    // Failed - return buffered output as error
                    let lines = buffer.lock().unwrap();
                    let exit_info = status
                        .code()
                        .map(|c| format!("exit code {c}"))
                        .unwrap_or_else(|| "killed by signal".to_string());
                    if lines.is_empty() {
                        bail!("{cmd_str} failed ({exit_info})");
                    } else {
                        bail!("{}\n({cmd_str} failed, {exit_info})", lines.join("\n"));
                    }
                }
                Ok(None) => {
                    // Still running - check if we should switch to streaming (skip if delay_ms < 0)
                    if delay_ms >= 0
                        && !streaming.load(Ordering::Relaxed)
                        && start.elapsed() >= Duration::from_millis(delay_ms as u64)
                    {
                        streaming.store(true, Ordering::Relaxed);

                        if let Some(ref msg) = progress_message {
                            let _ = writeln!(std::io::stderr(), "{}", msg);
                        }
                        for line in buffer.lock().unwrap().drain(..) {
                            let _ = writeln!(std::io::stderr(), "{}", line);
                        }
                        let _ = std::io::stderr().flush();
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(e) => bail!("Failed to wait for command: {}", e),
            }
        }
    }

    /// Run a git command and return the raw Output (for inspecting exit codes).
    ///
    /// Use this when exit codes have semantic meaning beyond success/failure.
    /// For most cases, prefer `run_command` (returns stdout) or `run_command_check` (returns bool).
    pub(super) fn run_command_output(&self, args: &[&str]) -> anyhow::Result<std::process::Output> {
        Cmd::new("git")
            .args(args.iter().copied())
            .current_dir(&self.discovery_path)
            .context(self.logging_context())
            .run()
            .with_context(|| format!("Failed to execute: git {}", args.join(" ")))
    }
}

#[cfg(test)]
mod tests;
