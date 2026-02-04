//! Workspace management operations for Repository.

use std::path::{Path, PathBuf};

use color_print::cformat;
use dunce::canonicalize;
use normalize_path::NormalizePath;

use super::{JjError, Repository, ResolvedWorkspace, WorkspaceInfo};
use crate::path::format_path_for_display;

impl Repository {
    /// List all workspaces for this repository.
    ///
    /// Returns a list of workspaces with their metadata.
    ///
    /// **Ordering:** The default workspace is listed first.
    pub fn list_workspaces(&self) -> anyhow::Result<Vec<WorkspaceInfo>> {
        // Use jj workspace list to get workspace names and paths
        let output = self.run_command(&["workspace", "list"])?;

        let mut workspaces = Vec::new();

        for line in output.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            // Parse format: "name: /path/to/workspace (current)"
            // or "name: /path/to/workspace"
            if let Some((name, rest)) = line.split_once(": ") {
                let is_current = rest.ends_with("(current)");
                let path_str = if is_current {
                    rest.trim_end_matches("(current)").trim()
                } else {
                    rest.trim()
                };

                let path = PathBuf::from(path_str);

                // Get the working copy commit and bookmark for this workspace
                let (commit_id, bookmark) = self.get_workspace_commit_info(&path)?;

                workspaces.push(WorkspaceInfo {
                    name: name.to_string(),
                    path,
                    working_copy_commit: commit_id,
                    bookmark,
                    is_current,
                });
            }
        }

        Ok(workspaces)
    }

    /// Get commit info (commit_id, bookmark) for a workspace.
    fn get_workspace_commit_info(
        &self,
        workspace_path: &Path,
    ) -> anyhow::Result<(String, Option<String>)> {
        // Run jj in the workspace to get working copy info
        let ws = self.workspace_at(workspace_path);

        let commit_id = ws.working_copy_commit().unwrap_or_default();
        let bookmark = ws.bookmark().ok().flatten();

        Ok((commit_id, bookmark))
    }

    /// Get the current workspace info, if we're inside one.
    ///
    /// Returns `None` if not in a workspace.
    pub fn current_workspace_info(&self) -> anyhow::Result<Option<WorkspaceInfo>> {
        let current_path = match self.current_workspace().root() {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };

        let workspaces = self.list_workspaces()?;
        Ok(workspaces.into_iter().find(|ws| {
            canonicalize(&ws.path)
                .map(|p| p == current_path)
                .unwrap_or(false)
        }))
    }

    /// Find the workspace path for a given bookmark, if one exists.
    pub fn workspace_for_bookmark(&self, bookmark: &str) -> anyhow::Result<Option<PathBuf>> {
        let workspaces = self.list_workspaces()?;

        Ok(workspaces
            .iter()
            .find(|ws| ws.bookmark.as_deref() == Some(bookmark))
            .map(|ws| ws.path.clone()))
    }

    /// Find the workspace for a given workspace name, if one exists.
    pub fn workspace_by_name(&self, name: &str) -> anyhow::Result<Option<WorkspaceInfo>> {
        let workspaces = self.list_workspaces()?;
        Ok(workspaces.into_iter().find(|ws| ws.name == name))
    }

    /// The "home" workspace — the default workspace.
    ///
    /// Used as the default source for `copy-ignored` and the `{{ primary_workspace_path }}` template.
    /// Returns `None` if no default workspace exists.
    pub fn primary_workspace(&self) -> anyhow::Result<Option<PathBuf>> {
        self.workspace_by_name("default")
            .map(|ws| ws.map(|w| w.path))
    }

    /// Find the workspace at a given path, returning its info.
    pub fn workspace_at_path(&self, path: &Path) -> anyhow::Result<Option<WorkspaceInfo>> {
        let workspaces = self.list_workspaces()?;
        let normalized_path = path.normalize();

        Ok(workspaces
            .into_iter()
            .find(|ws| ws.path.normalize() == normalized_path))
    }

    /// Add a new workspace.
    ///
    /// Creates a new workspace at the specified path with the given name.
    /// If `bookmark` is provided, creates or moves a bookmark to the new workspace's
    /// working copy.
    pub fn add_workspace(
        &self,
        name: &str,
        path: &Path,
        bookmark: Option<&str>,
    ) -> anyhow::Result<()> {
        let path_str = path.to_str().ok_or_else(|| {
            anyhow::Error::from(JjError::Other {
                message: format!(
                    "Workspace path contains invalid UTF-8: {}",
                    format_path_for_display(path)
                ),
            })
        })?;

        // Create the workspace
        self.run_command(&["workspace", "add", "--name", name, path_str])
            .map_err(|e| JjError::WorkspaceCreationFailed {
                name: name.to_string(),
                error: e.to_string(),
            })?;

        // If a bookmark was specified, create/move it to the new workspace
        if let Some(bookmark_name) = bookmark {
            let ws = self.workspace_at(path);
            // Create or move the bookmark to point to this workspace's working copy
            let _ = ws.run_command(&["bookmark", "set", bookmark_name]);
        }

        Ok(())
    }

    /// Forget (remove) a workspace.
    ///
    /// This removes the workspace from jj's tracking but does NOT delete the directory.
    /// Use `remove_workspace` if you also want to delete the directory.
    pub fn forget_workspace(&self, name: &str) -> anyhow::Result<()> {
        self.run_command(&["workspace", "forget", name])?;
        Ok(())
    }

    /// Remove a workspace completely (forget + delete directory).
    ///
    /// This removes the workspace from jj's tracking AND deletes the directory.
    /// The `force` flag allows removal even with uncommitted changes.
    pub fn remove_workspace(&self, name: &str, force: bool) -> anyhow::Result<()> {
        // First, get the workspace path before forgetting
        let workspace = self.workspace_by_name(name)?;
        let workspace_path = workspace
            .as_ref()
            .map(|ws| ws.path.clone())
            .ok_or_else(|| JjError::WorkspaceNotFound {
                name: name.to_string(),
            })?;

        // Check if the directory exists
        if !workspace_path.exists() {
            // Directory already gone, just forget the workspace
            self.forget_workspace(name)?;
            return Ok(());
        }

        // If not forcing, check for uncommitted changes
        if !force {
            let ws = self.workspace_at(&workspace_path);
            if ws.is_dirty()? {
                anyhow::bail!(
                    "Workspace '{}' has uncommitted changes. Use --force to remove anyway.",
                    name
                );
            }
        }

        // Forget the workspace first (so jj stops tracking it)
        self.forget_workspace(name)?;

        // Then remove the directory
        std::fs::remove_dir_all(&workspace_path).map_err(|e| {
            anyhow::anyhow!(
                "Failed to remove workspace directory {}: {}",
                format_path_for_display(&workspace_path),
                e
            )
        })?;

        Ok(())
    }

    /// Resolve a workspace name, expanding "@" to current, "-" to previous, and "^" to main.
    ///
    /// # Arguments
    /// * `name` - The workspace name to resolve:
    ///   - "@" for current workspace's bookmark
    ///   - "-" for previous workspace (via worktrunk.history)
    ///   - "^" for default bookmark
    ///   - any other string is returned as-is (treated as bookmark name)
    ///
    /// # Returns
    /// - `Ok(name)` if not a special symbol
    /// - `Ok(current_bookmark)` if "@" and on a bookmark
    /// - `Ok(previous_bookmark)` if "-" and worktrunk.history has a previous bookmark
    /// - `Ok(default_bookmark)` if "^"
    /// - `Err` if "@" and not on a bookmark
    /// - `Err` if "-" but no previous bookmark in history
    pub fn resolve_workspace_name(&self, name: &str) -> anyhow::Result<String> {
        match name {
            "@" => self.current_workspace().bookmark()?.ok_or_else(|| {
                JjError::Other {
                    message: cformat!(
                        "Current workspace is not on a bookmark. Cannot resolve '@'."
                    ),
                }
                .into()
            }),
            "-" => {
                // Read from worktrunk.previous (recorded by wt switch operations)
                self.switch_previous().ok_or_else(|| {
                    JjError::Other {
                        message: cformat!(
                            "No previous workspace found in history. Run <bright-black>wt list</> to see available workspaces."
                        ),
                    }
                    .into()
                })
            }
            "^" => self.default_bookmark().ok_or_else(|| {
                JjError::Other {
                    message: cformat!(
                        "Cannot determine default bookmark. Specify target explicitly or set a default."
                    ),
                }
                .into()
            }),
            _ => Ok(name.to_string()),
        }
    }

    /// Resolve a workspace by name, returning its path and info.
    ///
    /// Unlike `resolve_workspace_name` which returns a bookmark name, this returns
    /// the workspace path directly. Useful for commands like `wt remove` that
    /// operate on workspaces, not bookmarks.
    ///
    /// # Arguments
    /// * `name` - The workspace name to resolve:
    ///   - "@" for current workspace
    ///   - "-" for previous workspace
    ///   - "^" for default bookmark's workspace
    ///   - any other string is treated as a bookmark name
    ///
    /// # Returns
    /// - `Workspace { path, name, bookmark }` if a workspace exists
    /// - `BookmarkOnly { bookmark }` if only the bookmark exists (no workspace)
    /// - `Err` if neither workspace nor bookmark exists
    pub fn resolve_workspace(&self, name: &str) -> anyhow::Result<ResolvedWorkspace> {
        match name {
            "@" => {
                // Current workspace by path
                let path = self.current_workspace().root().map_err(|_| JjError::NotInWorkspace {
                    action: Some("resolve '@'".into()),
                })?;
                let workspaces = self.list_workspaces()?;
                let ws_info = workspaces
                    .into_iter()
                    .find(|ws| canonicalize(&ws.path).map(|p| p == path).unwrap_or(false));

                if let Some(info) = ws_info {
                    Ok(ResolvedWorkspace::Workspace {
                        path: info.path,
                        name: info.name,
                        bookmark: info.bookmark,
                    })
                } else {
                    Ok(ResolvedWorkspace::Workspace {
                        path,
                        name: "unknown".to_string(),
                        bookmark: None,
                    })
                }
            }
            _ => {
                // Resolve to bookmark name first, then find its workspace
                let bookmark = self.resolve_workspace_name(name)?;
                match self.workspace_for_bookmark(&bookmark)? {
                    Some(path) => {
                        let ws_info = self.workspace_at_path(&path)?;
                        Ok(ResolvedWorkspace::Workspace {
                            path,
                            name: ws_info.map(|w| w.name).unwrap_or_else(|| bookmark.clone()),
                            bookmark: Some(bookmark),
                        })
                    }
                    None => Ok(ResolvedWorkspace::BookmarkOnly { bookmark }),
                }
            }
        }
    }

    /// Find the "home" path - where to cd when leaving a workspace.
    ///
    /// Returns the primary workspace if it exists, otherwise the repo root.
    pub fn home_path(&self) -> anyhow::Result<PathBuf> {
        Ok(self
            .primary_workspace()?
            .unwrap_or_else(|| self.repo_path().to_path_buf()))
    }
}
