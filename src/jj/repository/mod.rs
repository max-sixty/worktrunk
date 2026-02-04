//! Repository - jj repository operations.
//!
//! This module provides the [`Repository`] type for interacting with jj repositories,
//! [`WorkingCopy`] for workspace-specific operations.
//!
//! # Module organization
//!
//! - `mod.rs` - Core types and construction
//! - `working_copy.rs` - WorkingCopy struct and workspace-specific operations
//! - `workspaces.rs` - Workspace management (list, resolve, add, forget)
//! - `bookmarks.rs` - Bookmark (branch) operations

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, OnceLock};

use crate::shell_exec::Cmd;

use anyhow::{Context, bail};
use dashmap::DashMap;
use dunce::canonicalize;
use once_cell::sync::OnceCell;

use crate::config::ProjectConfig;

// Import types from parent module
use super::{JjError, WorkspaceInfo};

// Submodules with impl blocks
mod bookmarks;
mod working_copy;
mod workspaces;

// Re-export WorkingCopy
pub use working_copy::WorkingCopy;
pub(super) use working_copy::path_to_logging_context;

// ============================================================================
// Repository Cache
// ============================================================================

/// Cached data for a single repository.
///
/// Contains:
/// - Repo-wide values (same for all workspaces): repo_path, default_bookmark, etc.
/// - Per-workspace values keyed by path: working copy commit, etc.
///
/// Wrapped in Arc to allow releasing the outer HashMap lock before accessing
/// cached values, avoiding deadlocks when cached methods call each other.
#[derive(Debug, Default)]
pub(super) struct RepoCache {
    // ========== Repo-wide values (same for all workspaces) ==========
    /// Repository root path
    pub(super) repo_path: OnceCell<PathBuf>,
    /// The .jj directory path
    pub(super) jj_dir: OnceCell<PathBuf>,
    /// Default bookmark (main, master, trunk, etc.)
    pub(super) default_bookmark: OnceCell<Option<String>>,
    /// Project config (loaded from .config/wt.toml in repo root)
    pub(super) project_config: OnceCell<Option<ProjectConfig>>,

    // ========== Per-workspace values (keyed by path) ==========
    /// Workspace root paths: workspace_path -> canonicalized root
    pub(super) workspace_roots: DashMap<PathBuf, PathBuf>,
    /// Current bookmark per workspace: workspace_path -> bookmark name (None = no bookmark)
    pub(super) current_bookmarks: DashMap<PathBuf, Option<String>>,
}

/// Result of resolving a workspace name.
///
/// Used by `resolve_workspace` to handle different resolution outcomes:
/// - A workspace exists (with optional bookmark)
/// - Only a bookmark exists (no workspace)
#[derive(Debug, Clone)]
pub enum ResolvedWorkspace {
    /// A workspace was found
    Workspace {
        /// The filesystem path to the workspace
        path: PathBuf,
        /// The workspace name
        name: String,
        /// The bookmark name, if any
        bookmark: Option<String>,
    },
    /// Only a bookmark exists (no workspace)
    BookmarkOnly {
        /// The bookmark name
        bookmark: String,
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

/// Repository state for jj operations.
///
/// Represents the shared state of a jj repository (the `.jj` directory).
/// For workspace-specific operations, use [`WorkingCopy`] obtained via
/// [`current_workspace()`](Self::current_workspace) or [`workspace_at()`](Self::workspace_at).
///
/// # Examples
///
/// ```no_run
/// use worktrunk::jj::Repository;
///
/// let repo = Repository::current()?;
/// let ws = repo.current_workspace();
///
/// // Repo-wide operations
/// if let Some(default) = repo.default_bookmark() {
///     println!("Default bookmark: {}", default);
/// }
///
/// // Workspace-specific operations
/// let bookmark = ws.bookmark()?;
/// let dirty = ws.is_dirty()?;
/// # Ok::<(), anyhow::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct Repository {
    /// Path used for discovering the repository and running jj commands.
    /// For repo-wide operations, any path within the repo works.
    discovery_path: PathBuf,
    /// The .jj directory, computed at construction time.
    jj_dir: PathBuf,
    /// Cached data for this repository. Shared across clones via Arc.
    pub(super) cache: Arc<RepoCache>,
}

impl Repository {
    /// Discover the repository from the current directory.
    ///
    /// This is the primary way to create a Repository. If the -C flag was used,
    /// this uses that path instead of the actual current directory.
    ///
    /// For workspace-specific operations on paths other than cwd, use
    /// `repo.workspace_at(path)` to get a [`WorkingCopy`].
    pub fn current() -> anyhow::Result<Self> {
        Self::at(base_path().clone())
    }

    /// Discover the repository from the specified path.
    ///
    /// Creates a new Repository with its own cache. For sharing cache across
    /// operations (e.g., parallel tasks in `wt list`), clone an existing
    /// Repository instead of calling `at()` multiple times.
    pub fn at(path: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let discovery_path = path.into();
        let jj_dir = Self::resolve_jj_dir(&discovery_path)?;

        Ok(Self {
            discovery_path,
            jj_dir,
            cache: Arc::new(RepoCache::default()),
        })
    }

    /// Check if this repository shares its cache with another.
    ///
    /// Returns true if both repositories point to the same underlying cache.
    #[doc(hidden)]
    pub fn shares_cache_with(&self, other: &Repository) -> bool {
        Arc::ptr_eq(&self.cache, &other.cache)
    }

    /// Resolve the .jj directory for a path.
    ///
    /// Always returns a canonicalized absolute path.
    fn resolve_jj_dir(discovery_path: &Path) -> anyhow::Result<PathBuf> {
        let output = Cmd::new("jj")
            .args(["root"])
            .current_dir(discovery_path)
            .context(path_to_logging_context(discovery_path))
            .run()
            .context("Failed to execute: jj root")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("There is no jj repo") {
                return Err(JjError::NotInRepository {
                    path: discovery_path.to_path_buf(),
                }
                .into());
            }
            bail!("{}", stderr.trim());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let repo_root = PathBuf::from(stdout.trim());
        let jj_dir = repo_root.join(".jj");

        canonicalize(&jj_dir).context("Failed to resolve .jj directory")
    }

    /// Get the path this repository was discovered from.
    pub fn discovery_path(&self) -> &Path {
        &self.discovery_path
    }

    /// Get a workspace view at the current directory.
    ///
    /// This is the primary way to get a [`WorkingCopy`] for workspace-specific operations.
    pub fn current_workspace(&self) -> WorkingCopy<'_> {
        self.workspace_at(base_path().clone())
    }

    /// Get a workspace view at a specific path.
    ///
    /// Use this when you need to operate on a workspace other than the current one.
    pub fn workspace_at(&self, path: impl Into<PathBuf>) -> WorkingCopy<'_> {
        WorkingCopy {
            repo: self,
            path: path.into(),
        }
    }

    // =========================================================================
    // Core repository properties
    // =========================================================================

    /// Get the .jj directory.
    ///
    /// Always returns an absolute path.
    pub fn jj_dir(&self) -> &Path {
        &self.jj_dir
    }

    /// Get the directory where worktrunk background logs are stored.
    ///
    /// Returns `<jj-dir>/wt-logs/` (typically `.jj/wt-logs/`).
    pub fn wt_logs_dir(&self) -> PathBuf {
        self.jj_dir().join("wt-logs")
    }

    /// The repository root path.
    ///
    /// This is the parent of the .jj directory.
    pub fn repo_path(&self) -> &Path {
        self.cache.repo_path.get_or_init(|| {
            self.jj_dir
                .parent()
                .expect(".jj directory has no parent")
                .to_path_buf()
        })
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

    /// Run a jj command in this repository's context.
    ///
    /// Executes the jj command with this repository's discovery path as the working directory.
    /// For repo-wide operations, any path within the repo works.
    ///
    /// # Examples
    /// ```no_run
    /// use worktrunk::jj::Repository;
    ///
    /// let repo = Repository::current()?;
    /// let bookmarks = repo.run_command(&["bookmark", "list"])?;
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn run_command(&self, args: &[&str]) -> anyhow::Result<String> {
        let output = Cmd::new("jj")
            .args(args.iter().copied())
            .current_dir(&self.discovery_path)
            .context(self.logging_context())
            .run()
            .with_context(|| format!("Failed to execute: jj {}", args.join(" ")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
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

    /// Run a jj command and return whether it succeeded (exit code 0).
    ///
    /// This is useful for commands that use exit codes for boolean results.
    pub fn run_command_check(&self, args: &[&str]) -> anyhow::Result<bool> {
        Ok(self.run_command_output(args)?.status.success())
    }

    /// Delay before showing progress output for slow operations.
    pub const SLOW_OPERATION_DELAY_MS: i64 = 400;

    /// Run a jj command and return the raw Output (for inspecting exit codes).
    pub(super) fn run_command_output(&self, args: &[&str]) -> anyhow::Result<std::process::Output> {
        Cmd::new("jj")
            .args(args.iter().copied())
            .current_dir(&self.discovery_path)
            .context(self.logging_context())
            .run()
            .with_context(|| format!("Failed to execute: jj {}", args.join(" ")))
    }
}

#[cfg(test)]
mod tests;
