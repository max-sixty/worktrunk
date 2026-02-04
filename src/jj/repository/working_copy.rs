//! WorkingCopy - workspace-specific jj operations.
//!
//! A borrowed handle for running jj commands in a specific workspace directory.

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use dunce::canonicalize;

use super::Repository;
use crate::shell_exec::Cmd;

/// Get a short display name from a path, used in logging context.
///
/// Returns "." for the current directory, or the directory name otherwise.
pub fn path_to_logging_context(path: &Path) -> String {
    if path == Path::new(".") {
        ".".to_string()
    } else {
        path.file_name()
            .and_then(|n| n.to_str())
            .map(String::from)
            .unwrap_or_else(|| ".".to_string())
    }
}

/// A borrowed handle for workspace-specific jj operations.
///
/// Created via [`Repository::current_workspace()`] or [`Repository::workspace_at()`].
/// Holds a reference to the repository and the path to operate in.
///
/// # Examples
///
/// ```no_run
/// use worktrunk::jj::Repository;
///
/// let repo = Repository::current()?;
/// let ws = repo.workspace_at("/path/to/workspace");
///
/// // Workspace-specific operations
/// let bookmark = ws.bookmark()?;
/// let dirty = ws.is_dirty()?;
/// # Ok::<(), anyhow::Error>(())
/// ```
#[derive(Debug)]
pub struct WorkingCopy<'a> {
    pub(super) repo: &'a Repository,
    pub(super) path: PathBuf,
}

impl<'a> WorkingCopy<'a> {
    /// Get the workspace root path (canonicalized).
    ///
    /// This resolves symlinks and returns the absolute path.
    /// Result is cached per workspace path within the repository.
    pub fn root(&self) -> anyhow::Result<PathBuf> {
        // Check cache first
        if let Some(cached) = self.repo.cache.workspace_roots.get(&self.path) {
            return Ok(cached.clone());
        }

        // Run jj workspace root to get the actual workspace root
        let output = Cmd::new("jj")
            .args(["workspace", "root"])
            .current_dir(&self.path)
            .context(self.logging_context())
            .run()
            .context("Failed to execute: jj workspace root")?;

        if !output.status.success() {
            bail!(
                "Not in a jj workspace: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }

        let root_str = String::from_utf8_lossy(&output.stdout);
        let root = canonicalize(PathBuf::from(root_str.trim()))
            .context("Failed to canonicalize workspace root")?;

        // Cache the result
        self.repo
            .cache
            .workspace_roots
            .insert(self.path.clone(), root.clone());

        Ok(root)
    }

    /// Get the current bookmark (jj's equivalent of git branch).
    ///
    /// Returns `None` if the working copy is not on a bookmark.
    /// Result is cached per workspace path.
    pub fn bookmark(&self) -> anyhow::Result<Option<String>> {
        // Check cache first
        if let Some(cached) = self.repo.cache.current_bookmarks.get(&self.path) {
            return Ok(cached.clone());
        }

        // Get the working copy commit, then check what bookmarks point to it
        let output = self.run_command(&[
            "log",
            "-r",
            "@",
            "--no-graph",
            "-T",
            r#"bookmarks.map(|b| b.name()).join("\n")"#,
        ])?;

        let bookmark = output.lines().next().map(|s| s.trim().to_string());

        // Filter out empty strings
        let bookmark = bookmark.filter(|s| !s.is_empty());

        // Cache the result
        self.repo
            .cache
            .current_bookmarks
            .insert(self.path.clone(), bookmark.clone());

        Ok(bookmark)
    }

    /// Get the current working copy commit ID.
    pub fn working_copy_commit(&self) -> anyhow::Result<String> {
        let output = self.run_command(&["log", "-r", "@", "--no-graph", "-T", "commit_id"])?;
        Ok(output.trim().to_string())
    }

    /// Get the current change ID.
    pub fn change_id(&self) -> anyhow::Result<String> {
        let output = self.run_command(&["log", "-r", "@", "--no-graph", "-T", "change_id"])?;
        Ok(output.trim().to_string())
    }

    /// Check if the working copy has uncommitted changes.
    ///
    /// In jj, this checks if there are any uncommitted changes in the working copy.
    pub fn is_dirty(&self) -> anyhow::Result<bool> {
        // jj status outputs changes; if empty, the workspace is clean
        let output = self.run_command(&["status"])?;
        // If there's no "Working copy changes" section, it's clean
        Ok(output.contains("Working copy changes:"))
    }

    /// Check for merge conflicts in the working copy.
    pub fn has_conflicts(&self) -> anyhow::Result<bool> {
        let output = self.run_command(&["status"])?;
        Ok(output.contains("Conflicted"))
    }

    /// Get the logging context for this workspace.
    fn logging_context(&self) -> String {
        path_to_logging_context(&self.path)
    }

    /// Run a jj command in this workspace's directory.
    ///
    /// # Examples
    /// ```no_run
    /// use worktrunk::jj::Repository;
    ///
    /// let repo = Repository::current()?;
    /// let ws = repo.current_workspace();
    /// let status = ws.run_command(&["status"])?;
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn run_command(&self, args: &[&str]) -> anyhow::Result<String> {
        let output = Cmd::new("jj")
            .args(args.iter().copied())
            .current_dir(&self.path)
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
    pub fn run_command_check(&self, args: &[&str]) -> anyhow::Result<bool> {
        let output = Cmd::new("jj")
            .args(args.iter().copied())
            .current_dir(&self.path)
            .context(self.logging_context())
            .run()
            .with_context(|| format!("Failed to execute: jj {}", args.join(" ")))?;

        Ok(output.status.success())
    }
}
