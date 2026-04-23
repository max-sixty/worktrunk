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
//!
//! # Caching
//!
//! Most repository data — remote URLs, config, default branch, merge-bases — is stable
//! for the duration of a single CLI command. [`RepoCache`] exploits this by caching
//! read-only values so repeated queries hit memory instead of spawning git processes.
//!
//! **Lifetime.** A cache is created once per `Repository::at()` call and never
//! invalidated. There is no expiry, no dirty-tracking, no
//! manual flush — the cache lives exactly as long as the command.
//!
//! **Sharing.** `Repository` holds an `Arc<RepoCache>`, so cloning a `Repository`
//! (e.g., to pass into parallel worktree operations in `wt list`) shares the same
//! cache. Callers that need a *separate* cache must call `Repository::at()` again.
//!
//! **What is NOT cached.** Values that change during command execution are intentionally
//! excluded:
//! - `WorkingTree::is_dirty()` — changes as we stage and commit
//!
//! [`Repository::list_worktrees`] is cached despite mutating commands adding or
//! removing worktrees. The cache is safe because no caller reads the list
//! through the same `Repository` after its own mutation — mutating paths
//! either read once up front and thread the slice through, or rebuild a
//! fresh `Repository::at(...)` before any post-mutation probe. This mirrors
//! the invariant the branch inventories already rely on.
//!
//! **Access patterns.** See the [`RepoCache`] doc comment for the two storage patterns
//! (repo-wide `OnceCell` vs per-key `DashMap`) and their infallible/fallible variants.
//!
//! **Invariants:**
//! - A cached value, once written, is never updated within the same command.
//! - All cache access is lock-free at the call site — `OnceCell` and `DashMap` handle
//!   synchronization internally.
//! - Code that mutates repository state (committing, creating worktrees) must not read
//!   its own mutations through the cache. Use direct git commands for post-mutation
//!   state.
//!
//! **Process-level singletons.** Outside `RepoCache`, several modules use `OnceLock`/`LazyLock`
//! for process-global singletons that are computed once and never change:
//! - Resource limiters: `CMD_SEMAPHORE` (shell_exec), `HEAVY_OPS_SEMAPHORE` (git),
//!   `LLM_SEMAPHORE` (summary), `COPY_POOL` (copy)
//! - Global state: `OUTPUT_STATE` (output), `TRACE` and `OUTPUT` (log_files), `COMMAND_LOG`
//! - Config: `CONFIG_PATH` (config/user/path), `SHELL_CONFIG`, `GIT_ENV_OVERRIDES` (shell_exec)
//! - Git discovery: `GIT_COMMON_DIR_CACHE` (below) — memoizes `git rev-parse --git-common-dir`
//!   across `Repository::at()` calls
//!
//! These are lazy initialization, not caches — they have no invalidation concerns
//! because the container is initialized once and never replaced — unlike `RepoCache`,
//! there is no risk of reading stale external state.
//!
//! The picker also maintains a `PreviewCache` (`Arc<DashMap>` in `commands/picker/items.rs`)
//! for rendered preview output, scoped to a single picker session.

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
use wait_timeout::ChildExt;

use anyhow::{Context, bail};

use dunce::canonicalize;

use crate::config::{LoadError, ProjectConfig, ResolvedConfig, UserConfig};

// Import types from parent module
use super::{DefaultBranchName, GitError, IntegrationReason, LineDiff, WorktreeInfo};

// Re-export types needed by submodules
pub(super) use super::{
    BranchCategory, CompletionBranch, DiffStats, GitRemoteUrl, LocalBranch, RemoteBranch,
};

// Submodules with impl blocks
mod branch;
mod branches;
mod config;
mod diff;
mod integration;
mod remotes;
mod sha_cache;
mod working_tree;
mod worktrees;

// Re-export WorkingTree and Branch
pub use branch::Branch;
pub use working_tree::WorkingTree;
pub(super) use working_tree::path_to_logging_context;

/// Structured error from [`Repository::run_command_delayed_stream`].
///
/// Separates command output from command identity so callers can format
/// each part with appropriate styling (e.g., bold command, gray exit code).
#[derive(Debug)]
pub(crate) struct StreamCommandError {
    /// Lines of output from the command (may be empty)
    pub output: String,
    /// The command string, e.g., "git worktree add /path -b fix main"
    pub command: String,
    /// Exit information, e.g., "exit code 255" or "killed by signal"
    pub exit_info: String,
}

impl std::fmt::Display for StreamCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Callers use Repository::extract_failed_command() to access fields directly.
        // This Display impl exists only to satisfy the Error trait bound.
        write!(f, "{}", self.output)
    }
}

impl std::error::Error for StreamCommandError {}

/// Convert a child exit status into `Ok(())` or a [`StreamCommandError`].
fn stream_exit_result(
    status: std::process::ExitStatus,
    buffer: &Arc<Mutex<Vec<String>>>,
    cmd_str: &str,
) -> anyhow::Result<()> {
    if status.success() {
        return Ok(());
    }
    let lines = buffer.lock().unwrap();
    let exit_info = status
        .code()
        .map(|c| format!("exit code {c}"))
        .unwrap_or_else(|| "killed by signal".to_string());
    Err(StreamCommandError {
        output: lines.join("\n"),
        command: cmd_str.to_string(),
        exit_info,
    }
    .into())
}

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
///
/// # Cache access patterns
///
/// Repo-wide values use `OnceCell::get_or_init` / `get_or_try_init` — single
/// initialization, no key.
///
/// Keyed values use `DashMap`. Both patterns hold the shard lock across
/// check-and-insert (no TOCTOU gap). Choose based on whether computation
/// is fallible:
///
/// **Infallible** — use `entry().or_insert_with()`:
///
/// ```rust,ignore
/// self.cache.some_map
///     .entry(key)
///     .or_insert_with(|| compute())
///     .clone()
/// ```
///
/// **Fallible** — use explicit `Entry` matching to propagate errors:
///
/// ```rust,ignore
/// match self.cache.some_map.entry(key) {
///     Entry::Occupied(e) => Ok(e.get().clone()),
///     Entry::Vacant(e) => {
///         let value = compute()?;
///         Ok(e.insert(value).clone())
///     }
/// }
/// ```
#[derive(Debug, Default)]
pub(super) struct RepoCache {
    // ========== Repo-wide values (same for all worktrees) ==========
    /// Every git config key in the merged config (system + global + repo),
    /// populated by one `git config --list -z` read on first access.
    ///
    /// All the single-key config accessors (`is_bare`, `primary_remote`,
    /// `remote_url`, `default_branch` fast path, hint/marker readers)
    /// consult this map instead of spawning their own subprocess. Multivars
    /// (e.g., multiple `remote.origin.url` entries) accumulate in the Vec.
    /// `RwLock` rather than `OnceCell` so in-process writes via
    /// [`Repository::set_config_value`] stay coherent after population.
    ///
    /// `IndexMap` preserves insertion order, matching git's own `--list -z`
    /// output order — so accessors that iterate the map (e.g.,
    /// `primary_remote` picking "first remote with a URL") follow config
    /// file order the same way the old `--get-regexp` calls did.
    pub(super) all_config: OnceCell<std::sync::RwLock<indexmap::IndexMap<String, Vec<String>>>>,
    /// Repository root path (main worktree for normal repos, bare directory for bare repos)
    pub(super) repo_path: OnceCell<PathBuf>,
    /// Default branch (main, master, etc.)
    pub(super) default_branch: OnceCell<Option<String>>,
    /// Effective integration target (local default branch or upstream if ahead)
    pub(super) integration_target: OnceCell<Option<String>>,
    /// Project identifier derived from remote URL
    pub(super) project_identifier: OnceCell<String>,
    /// Project config (loaded from .config/wt.toml in main worktree)
    pub(super) project_config: OnceCell<Option<ProjectConfig>>,
    /// User config (raw, as loaded from disk).
    /// Lazily loaded on first access.
    pub(super) user_config: OnceCell<UserConfig>,
    /// Resolved user config (global merged with per-project overrides, defaults applied).
    /// Lazily loaded on first access via `Repository::config()`.
    pub(super) resolved_config: OnceCell<ResolvedConfig>,
    /// Sparse checkout paths (empty if not a sparse checkout)
    pub(super) sparse_checkout_paths: OnceCell<Vec<String>>,
    /// Merge-base cache: (commit1, commit2) -> merge_base_sha (None = no common ancestor)
    pub(super) merge_base: DashMap<(String, String), Option<String>>,
    /// Ahead/behind cache: (base_ref, head) -> (ahead, behind).
    /// Primed in bulk by `batch_ahead_behind()`; populated on demand by
    /// `ahead_behind()` for keys the batch didn't cover (e.g., HEAD SHAs
    /// during rebase/merge, or git < 2.36 where the batch is a no-op).
    pub(super) ahead_behind: DashMap<(String, String), (usize, usize)>,
    /// Effective remote URLs: remote_name -> effective URL (with `url.insteadOf` applied).
    /// Separate from `all_config` because `git remote get-url` applies
    /// `url.insteadOf` rewrites that aren't visible in raw config.
    pub(super) effective_remote_urls: DashMap<String, Option<String>>,
    /// Resolved refs: unresolved ref (e.g., "main") -> resolved form (e.g., "refs/heads/main")
    /// or original if not a local branch. Populated by `resolve_preferring_branch()`.
    pub(super) resolved_refs: DashMap<String, String>,
    /// Effective integration targets: local_target -> effective ref (may be upstream).
    /// Cached because `integration_reason()` calls `effective_integration_target()` for
    /// every branch, but the result depends only on the target ref's relationship with
    /// its upstream — stable for the duration of a command.
    pub(super) effective_integration_targets: DashMap<String, String>,
    /// Integration reason cache: (branch, target) -> (effective_target, reason).
    /// Populated by `integration_reason()`, avoids redundant `compute_integration_lazy()`
    /// calls when the same branch is checked multiple times (e.g., step_prune Phase 1
    /// followed by prepare_worktree_removal).
    pub(super) integration_reasons: DashMap<(String, String), (String, Option<IntegrationReason>)>,

    /// Tree SHA cache: tree spec (e.g., "refs/heads/main^{tree}") -> SHA.
    /// The tree SHA for a given ref doesn't change during a command.
    pub(super) tree_shas: DashMap<String, String>,

    /// Commit SHA cache: ref (e.g., "main", "refs/heads/main") -> commit SHA.
    /// The commit SHA for a given ref doesn't change during a command.
    /// Used by `rev_parse_commit()` to key the persistent `sha_cache` by SHA.
    pub(super) commit_shas: DashMap<String, String>,

    /// Local branch inventory: one `git for-each-ref refs/heads/` scan, cached
    /// for the lifetime of the repository. Entries are sorted by most recent
    /// commit first; the inventory also holds a name → index map for O(1)
    /// single-branch lookups. Populated lazily via
    /// [`Repository::local_branches`] — the first call runs the scan and
    /// primes `resolved_refs`/`commit_shas` so subsequent ref resolution and
    /// SHA lookups hit memory.
    pub(super) local_branches: OnceCell<branches::LocalBranchInventory>,
    /// Remote-tracking branch inventory: one `git for-each-ref refs/remotes/`
    /// scan, cached for the lifetime of the repository. Sorted by most recent
    /// commit first. Populated lazily via [`Repository::remote_branches`].
    /// Excludes `<remote>/HEAD` symrefs.
    pub(super) remote_branches: OnceCell<Vec<RemoteBranch>>,
    /// Worktree inventory: one `git worktree list --porcelain` scan, cached
    /// for the lifetime of the repository. Populated lazily via
    /// [`Repository::list_worktrees`]. The picker warms this on the main
    /// thread (for its preview-window sizing estimate) so the background
    /// `collect::collect` pass hits memory instead of respawning the
    /// subprocess on the critical path to skeleton.
    pub(super) worktrees: OnceCell<Vec<WorktreeInfo>>,
    /// In-memory branch diff stats cache: (base_sha, head_sha) -> LineDiff.
    /// Sits in front of the persistent `sha_cache` to prevent parallel tasks
    /// from racing through the file-based cache for the same SHA pair.
    pub(super) diff_stats: DashMap<(String, String), LineDiff>,

    // ========== Per-worktree values (keyed by path) ==========
    /// Per-worktree git directory: worktree_path -> canonicalized git dir
    /// (e.g., `.git/worktrees/<name>` for linked worktrees, `.git` for main)
    pub(super) git_dirs: DashMap<PathBuf, PathBuf>,
    /// Worktree root paths: worktree_path -> canonicalized root
    pub(super) worktree_roots: DashMap<PathBuf, PathBuf>,
    /// Current branch per worktree: worktree_path -> branch name (None = detached HEAD)
    pub(super) current_branches: DashMap<PathBuf, Option<String>>,
    /// HEAD commit SHA per worktree: worktree_path -> SHA (None = unborn, HEAD unresolvable).
    /// Primed in bulk by `WorkingTree::prewarm_info()`; lazily resolved on miss via
    /// `WorkingTree::head_sha()`. Lets alias-context expansion consult the cache
    /// instead of spawning a fresh `git rev-parse HEAD`.
    pub(super) head_shas: DashMap<PathBuf, Option<String>>,
    /// Cached `git status --porcelain` output per worktree: worktree_path -> raw porcelain.
    /// Populated by `WorkingTree::status_porcelain_cached()` so parallel tasks
    /// (working-tree diff + conflict detection) share one subprocess per worktree
    /// instead of spawning `git status` twice.
    pub(super) status_porcelain: DashMap<PathBuf, String>,
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

/// Process-wide cache for `git rev-parse --git-common-dir` resolution,
/// keyed by the discovery path passed to [`Repository::at`].
///
/// Unlike per-Repository caches, this lives for the whole process so that
/// multiple Repository instances pointed at the same path (e.g.
/// `init_command_log` early in `main`, then a command handler later) skip
/// the duplicate `git rev-parse` subprocess. The value (a canonicalized
/// `.git` directory) is invariant for the lifetime of the process.
///
/// Keys are stored as-is (not canonicalized) — the goal is only to dedupe
/// repeated calls with the same path. The duplicate case we care about (both
/// callers go through `base_path()`) always passes the same `PathBuf`, so
/// equality on the raw path is sufficient.
static GIT_COMMON_DIR_CACHE: LazyLock<DashMap<PathBuf, PathBuf>> = LazyLock::new(DashMap::new);

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

    /// Resolved user config (global merged with per-project overrides, defaults applied).
    ///
    /// Lazily loads `UserConfig` and resolves it using this repository's project identifier.
    /// Cached for the lifetime of the repository (shared across clones via Arc).
    ///
    /// Falls back to default config if loading fails (e.g., no config file).
    pub fn config(&self) -> &ResolvedConfig {
        self.cache.resolved_config.get_or_init(|| {
            let project_id = self.project_identifier().ok();
            self.user_config().resolved(project_id.as_deref())
        })
    }

    /// Raw user config (as loaded from disk, before project-specific resolution).
    ///
    /// Prefer [`config()`](Self::config) for behavior settings. This is only needed
    /// for operations that require the full `UserConfig` (e.g., path template formatting,
    /// approval state, hook resolution).
    ///
    /// Each config layer (system file, user file, env vars) degrades
    /// independently — a failure in one preserves data from earlier layers.
    /// Issues are surfaced on stderr so they're visible without `RUST_LOG`.
    pub fn user_config(&self) -> &UserConfig {
        self.cache.user_config.get_or_init(|| {
            let (config, warnings) = UserConfig::load_with_warnings();
            for warning in &warnings {
                match warning {
                    LoadError::File { path, label, err } => {
                        crate::styling::eprintln!(
                            "{}",
                            crate::styling::warning_message(format!(
                                "{label} at {} failed to parse, skipping",
                                crate::path::format_path_for_display(path),
                            ))
                        );
                        crate::styling::eprintln!(
                            "{}",
                            crate::styling::format_with_gutter(&err.to_string(), None)
                        );
                    }
                    LoadError::Env { err, vars } => {
                        let var_list: Vec<_> = vars
                            .iter()
                            .map(|(name, value)| format!("{name}={value}"))
                            .collect();
                        crate::styling::eprintln!(
                            "{}",
                            crate::styling::warning_message(format!(
                                "Ignoring env var overrides: {}",
                                var_list.join(", ")
                            ))
                        );
                        crate::styling::eprintln!(
                            "{}",
                            crate::styling::format_with_gutter(err.trim(), None)
                        );
                    }
                    LoadError::Validation(err) => {
                        crate::styling::eprintln!(
                            "{}",
                            crate::styling::warning_message(format!(
                                "Config validation warning: {err}"
                            ))
                        );
                    }
                }
            }
            config
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
    ///
    /// Result is cached process-wide in [`GIT_COMMON_DIR_CACHE`] so multiple
    /// `Repository::at()` calls for the same discovery path don't each spawn
    /// `git rev-parse --git-common-dir`.
    fn resolve_git_common_dir(discovery_path: &Path) -> anyhow::Result<PathBuf> {
        if let Some(cached) = GIT_COMMON_DIR_CACHE.get(discovery_path) {
            return Ok(cached.clone());
        }

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
        let resolved =
            canonicalize(&absolute_path).context("Failed to resolve git common directory")?;
        GIT_COMMON_DIR_CACHE.insert(discovery_path.to_path_buf(), resolved.clone());
        Ok(resolved)
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
    ///
    /// The path is canonicalized when it exists so that callers passing
    /// equivalent forms (e.g., cwd from JSON vs path from `git worktree list
    /// --porcelain`) hit the same per-worktree cache entries in `RepoCache`.
    /// Falls back to the raw path if canonicalization fails (e.g., path does
    /// not yet exist for a worktree about to be created).
    pub fn worktree_at(&self, path: impl Into<PathBuf>) -> WorkingTree<'_> {
        let raw = path.into();
        let path = canonicalize(&raw).unwrap_or(raw);
        WorkingTree { repo: self, path }
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

    /// Get the epoch timestamp of the last `git fetch`, if available.
    ///
    /// Checks the modification time of `FETCH_HEAD` in the git common directory.
    /// Returns `None` if the file doesn't exist (never fetched) or on any I/O error.
    pub fn last_fetch_epoch(&self) -> Option<u64> {
        let fetch_head = self.git_common_dir().join("FETCH_HEAD");
        let metadata = std::fs::metadata(fetch_head).ok()?;
        let modified = metadata.modified().ok()?;
        modified
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs())
    }

    /// Get the worktrunk data directory inside the git directory.
    ///
    /// Returns `<git-common-dir>/wt/` (typically `.git/wt/`).
    /// All worktrunk-managed state lives under this single directory.
    pub fn wt_dir(&self) -> PathBuf {
        self.git_common_dir().join("wt")
    }

    /// Clear all cached git command results, returning the count removed.
    ///
    /// Propagates I/O errors so the user-initiated clear path cannot lie
    /// about success; see `sha_cache`'s module docs.
    pub fn clear_git_commands_cache(&self) -> anyhow::Result<usize> {
        sha_cache::clear_all(self)
    }

    /// Count all cached git command results without clearing.
    ///
    /// Surfaces the same state that `clear_git_commands_cache` would sweep,
    /// for the `wt config state get` parity view.
    pub fn git_commands_cache_count(&self) -> usize {
        sha_cache::count_all(self)
    }

    /// Get the directory where worktrunk background logs are stored.
    ///
    /// Returns `<git-common-dir>/wt/logs/` (typically `.git/wt/logs/`).
    pub fn wt_logs_dir(&self) -> PathBuf {
        self.wt_dir().join("logs")
    }

    /// Get the directory where worktrees are staged for background deletion.
    ///
    /// Returns `<git-common-dir>/wt/trash/` (typically `.git/wt/trash/`).
    /// Worktrees are renamed here (instant same-filesystem rename) before
    /// being deleted by a background process.
    pub fn wt_trash_dir(&self) -> PathBuf {
        self.wt_dir().join("trash")
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
    /// # Resolution strategy
    ///
    /// We anchor on `git_common_dir` so that linked worktrees return the
    /// *main* worktree regardless of which worktree we were discovered
    /// from — `git_common_dir` is the stable reference shared across all
    /// worktrees (e.g., `/myapp/.git` whether you're in `/myapp` or
    /// `/myapp.feature`).
    ///
    /// | git_common_dir location    | Signal                     | Resolution                    |
    /// |----------------------------|----------------------------|-------------------------------|
    /// | Bare `.git`                | `core.bare = true`         | `git_common_dir` is the repo  |
    /// | Submodule `.git/modules/X` | `core.worktree` set by git | `rev-parse --show-toplevel`   |
    /// | Normal `.git`              | neither set                | `parent(git_common_dir)`      |
    ///
    /// Submodules need `core.worktree` because their git data lives in the
    /// parent's `.git/modules/` — the `parent(.git)` rule would point at
    /// `.git/modules`, which is wrong. Git writes `core.worktree`
    /// explicitly to compensate.
    ///
    /// We can't read `core.worktree` straight from the bulk config map:
    /// `git config --list -z` merges system/global/local scope, but git
    /// only honors `core.worktree` from **local** config for worktree
    /// discovery. So when the bulk map reports it, we delegate to
    /// `rev-parse --show-toplevel` and let git apply its scope rules; if
    /// the probe fails (non-local value, git ignored it) we fall through
    /// to the normal-repo path. The common case — no `core.worktree`
    /// anywhere — skips the subprocess, which is the point.
    ///
    /// # Errors
    ///
    /// Returns an error if the bulk config read fails (e.g., git timeout). This
    /// surfaces the failure early rather than caching a potentially wrong path.
    pub fn repo_path(&self) -> anyhow::Result<&Path> {
        self.cache
            .repo_path
            .get_or_try_init(|| {
                if self.is_bare()? {
                    return Ok(self.git_common_dir.clone());
                }

                // `core.worktree` in the bulk map could come from any scope,
                // but git only honors the local one. Let git resolve scope
                // via `rev-parse --show-toplevel` rather than trusting the
                // merged value ourselves.
                if self.config_last("core.worktree")?.is_some()
                    && let Ok(out) = Cmd::new("git")
                        .args(["rev-parse", "--show-toplevel"])
                        .current_dir(&self.git_common_dir)
                        .context(path_to_logging_context(&self.git_common_dir))
                        .run()
                    && out.status.success()
                {
                    return Ok(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()));
                }

                Ok(self
                    .git_common_dir
                    .parent()
                    .expect("Git directory has no parent")
                    .to_path_buf())
            })
            .map(|p| p.as_path())
    }

    /// Access the bulk git config map, populating on first call.
    ///
    /// Reads every key from the merged git config (system + global + repo)
    /// via a single `git config --list -z` subprocess. The NUL-delimited
    /// `-z` format handles values containing newlines or `=`. Populated
    /// lazily on first access; every config-reading accessor consults this
    /// map rather than spawning its own subprocess.
    ///
    /// Run from `git_common_dir` so linked worktrees of bare repos correctly
    /// read the bare repo's config.
    pub(super) fn all_config(
        &self,
    ) -> anyhow::Result<&std::sync::RwLock<indexmap::IndexMap<String, Vec<String>>>> {
        self.cache.all_config.get_or_try_init(|| {
            let output = Cmd::new("git")
                .args(["config", "--list", "-z"])
                .current_dir(&self.git_common_dir)
                .context(path_to_logging_context(&self.git_common_dir))
                .run()
                .context("failed to read git config")?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                bail!("git config --list failed: {}", stderr.trim());
            }
            Ok(std::sync::RwLock::new(parse_config_list_z(&output.stdout)))
        })
    }

    /// Read the last value for a config key from the bulk map.
    ///
    /// Convenience wrapper for the common "single-value" accessor pattern.
    /// Git treats the last value as authoritative when a key is set multiple
    /// times; callers that need multivars read through `all_config()` directly.
    ///
    /// Keys are normalized to git's canonical form (section and variable
    /// names lowercased, subsection preserved) so callers may pass the
    /// mixed-case form from git docs (e.g., `init.defaultBranch`) without
    /// missing the lookup.
    pub(super) fn config_last(&self, key: &str) -> anyhow::Result<Option<String>> {
        let canonical = canonical_config_key(key);
        let guard = self.all_config()?.read().unwrap();
        Ok(guard.get(&canonical).and_then(|v| v.last().cloned()))
    }

    /// Read a git-bool config value, defaulting to `false` when the key is
    /// unset or absent.
    ///
    /// Returns `Err` only when the bulk config read itself fails. A missing
    /// key is `Ok(false)`, matching git's own behaviour for unset booleans.
    pub(super) fn config_bool(&self, key: &str) -> anyhow::Result<bool> {
        Ok(self
            .config_last(key)?
            .as_deref()
            .map(parse_git_bool)
            .unwrap_or(false))
    }

    /// Check if this is a bare repository (no working tree).
    ///
    /// Bare repositories have no main worktree — all worktrees are linked
    /// worktrees at templated paths, including the default branch.
    ///
    /// Reads `core.bare` from the bulk config map rather than using `git
    /// rev-parse --is-bare-repository`. The rev-parse approach is unreliable
    /// when run from inside a `.git` directory — when `core.bare` is unset,
    /// git infers based on directory context, and from inside `.git/` there's
    /// no working tree so it returns `true` even for normal repos. This
    /// affects repos where `core.bare` was never written (e.g., repos cloned
    /// by Eclipse/EGit). Reading the config value directly avoids this false
    /// positive.
    ///
    /// When `core.bare` is unset, defaults to non-bare — matching libgit2's
    /// behavior.
    ///
    /// See <https://github.com/max-sixty/worktrunk/issues/1939>.
    pub fn is_bare(&self) -> anyhow::Result<bool> {
        self.config_bool("core.bare")
    }

    /// Get the sparse checkout paths for this repository.
    ///
    /// Returns the list of paths from `git sparse-checkout list`. For non-sparse
    /// repos, returns an empty slice (the command exits with code 128).
    ///
    /// Assumes cone mode (the git default). Cached using `discovery_path` —
    /// scoped to the worktree the user is running from, not per-listed-worktree.
    pub fn sparse_checkout_paths(&self) -> &[String] {
        self.cache.sparse_checkout_paths.get_or_init(|| {
            let output = match self.run_command_output(&["sparse-checkout", "list"]) {
                Ok(out) => out,
                Err(_) => return Vec::new(),
            };

            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                stdout.lines().map(String::from).collect()
            } else {
                // Exit 128 = not a sparse checkout (expected, not an error)
                Vec::new()
            }
        })
    }

    /// Check if git's builtin fsmonitor daemon is enabled.
    ///
    /// Returns true for any git-bool truthy value (`true/1/yes/on`), which
    /// matches how git itself routes the bool-or-string `core.fsmonitor`
    /// config to the builtin daemon. Returns false for Watchman hook paths,
    /// disabled, or unset.
    pub fn is_builtin_fsmonitor_enabled(&self) -> bool {
        self.config_bool("core.fsmonitor").unwrap_or(false)
    }

    /// Start the fsmonitor daemon at a worktree path.
    ///
    /// Idempotent — if the daemon is already running, this is a no-op.
    /// Used to avoid auto-start races when running many parallel git commands.
    ///
    /// Uses `Command::status()` with null stdio instead of `Cmd::run()` to avoid
    /// pipe inheritance: the daemon process (`git fsmonitor--daemon run --detach`)
    /// inherits pipe file descriptors from its parent, keeping them open
    /// indefinitely. `read_to_end()` in `Command::output()` then blocks forever
    /// waiting for EOF that never comes.
    pub fn start_fsmonitor_daemon_at(&self, path: &Path) {
        log::debug!("$ git fsmonitor--daemon start [{}]", path.display());
        let mut cmd = std::process::Command::new("git");
        cmd.args(["fsmonitor--daemon", "start"])
            .current_dir(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        crate::shell_exec::scrub_directive_env_vars(&mut cmd);
        let result = cmd.status();
        match result {
            Ok(status) if !status.success() => {
                log::debug!("fsmonitor daemon start exited {status} (usually fine)");
            }
            Err(e) => {
                log::debug!("fsmonitor daemon start failed (usually fine): {e}");
            }
            _ => {}
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
        let delay_ms = std::env::var("WORKTRUNK_TEST_DELAYED_STREAM_MS")
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

        let mut cmd = std::process::Command::new("git");
        cmd.args(args)
            .current_dir(&self.discovery_path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::shell_exec::scrub_directive_env_vars(&mut cmd);
        let mut child = cmd
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

        // Phase 1: If delay threshold is enabled, wait that long for the child to
        // exit. If it finishes before the threshold, output stays buffered (quiet).
        if delay_ms >= 0 {
            let delay = Duration::from_millis(delay_ms as u64);
            let remaining = delay.saturating_sub(start.elapsed());

            // Zero delay means "stream immediately", not "try a zero-timeout reap".
            if !remaining.is_zero()
                && let Some(status) = child
                    .wait_timeout(remaining)
                    .context("Failed to wait for command")?
            {
                let _ = stdout_handle.join();
                let _ = stderr_handle.join();
                return stream_exit_result(status, &buffer, &cmd_str);
            }

            // Delay threshold exceeded — switch to streaming
            streaming.store(true, Ordering::Relaxed);
            if let Some(ref msg) = progress_message {
                let _ = writeln!(std::io::stderr(), "{}", msg);
            }
            for line in buffer.lock().unwrap().drain(..) {
                let _ = writeln!(std::io::stderr(), "{}", line);
            }
            let _ = std::io::stderr().flush();
        }

        // Phase 2: Block until the child exits (no polling).
        let status = child.wait().context("Failed to wait for command")?;
        let _ = stdout_handle.join();
        let _ = stderr_handle.join();
        stream_exit_result(status, &buffer, &cmd_str)
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

    /// Extract structured failure info from a [`Repository::run_command_delayed_stream`] error.
    ///
    /// Returns `(output, Some(FailedCommand))` if the error is a `StreamCommandError`,
    /// or `(error_string, None)` for other error types (e.g., spawn failures).
    pub fn extract_failed_command(
        err: &anyhow::Error,
    ) -> (String, Option<super::error::FailedCommand>) {
        match err.downcast_ref::<StreamCommandError>() {
            Some(e) => (
                e.output.clone(),
                Some(super::error::FailedCommand {
                    command: e.command.clone(),
                    exit_info: e.exit_info.clone(),
                }),
            ),
            None => (err.to_string(), None),
        }
    }
}

/// Normalize a git config key to its canonical form.
///
/// Git section and variable names are case-insensitive; subsection names
/// (the middle parts of 3+-part keys) preserve case. `git config --list`
/// emits the canonical form — so lookups against the parsed map must
/// normalize the same way.
///
/// - 1 or 2 parts (`section` or `section.variable`): lowercase the whole thing.
/// - 3+ parts (`section.subsection….variable`): lowercase first and last parts,
///   preserve the middle.
pub(super) fn canonical_config_key(key: &str) -> String {
    let parts: Vec<&str> = key.split('.').collect();
    match parts.len() {
        0 | 1 => key.to_ascii_lowercase(),
        2 => key.to_ascii_lowercase(),
        _ => {
            let (first, rest) = parts.split_first().unwrap();
            let (last, middle) = rest.split_last().unwrap();
            let mut out = String::with_capacity(key.len());
            out.push_str(&first.to_ascii_lowercase());
            for part in middle {
                out.push('.');
                out.push_str(part);
            }
            out.push('.');
            out.push_str(&last.to_ascii_lowercase());
            out
        }
    }
}

/// Parse the output of `git config --list -z`.
///
/// Format: each entry is `key\nvalue\0`. Values may be empty (no `\n`) for
/// keys set via `git config key ""` — handled as `key -> ""`.
///
/// Returns a map from canonical key (as git emits it) to the list of
/// values, preserving order (matches git's own multivar semantics where
/// the last value wins).
fn parse_config_list_z(stdout: &[u8]) -> indexmap::IndexMap<String, Vec<String>> {
    let mut map: indexmap::IndexMap<String, Vec<String>> = indexmap::IndexMap::new();
    for entry in stdout.split(|&b| b == 0) {
        if entry.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(entry);
        let (key, value) = match text.split_once('\n') {
            Some((k, v)) => (k, v),
            // `key` without any newline → no value set (shouldn't happen
            // with `--list -z`, but tolerate gracefully).
            None => (text.as_ref(), ""),
        };
        map.entry(key.to_string())
            .or_default()
            .push(value.to_string());
    }
    map
}

/// Parse a git boolean config value.
///
/// Accepts the forms `git config --type=bool` normalizes: `true/1/yes/on`
/// (case-insensitive) → `true`; anything else → `false`.
fn parse_git_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "true" | "1" | "yes" | "on"
    )
}

#[cfg(test)]
mod tests;
