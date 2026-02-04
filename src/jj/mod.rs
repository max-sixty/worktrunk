//! Jujutsu (jj) operations and repository management
//!
//! This module provides the [`Repository`] type for interacting with jj repositories,
//! [`WorkingCopy`] for workspace-specific operations, and workspace management.

use std::path::PathBuf;

// Submodules
mod error;
mod parse;
mod repository;

#[cfg(test)]
mod test;

// Re-exports from submodules
pub use error::JjError;
pub use repository::{Repository, ResolvedWorkspace, WorkingCopy, set_base_path};

/// Hook types for jj operations
///
/// Note: jj doesn't natively support hooks yet, but we keep this for future compatibility
/// and to maintain API parity during the transition from git.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    clap::ValueEnum,
    strum::Display,
    strum::EnumString,
    strum::EnumIter,
)]
#[strum(serialize_all = "kebab-case")]
pub enum HookType {
    PostCreate,
    PostStart,
    PostSwitch,
    PreCommit,
    PreMerge,
    PostMerge,
    PreRemove,
    PostRemove,
}

/// Parsed workspace data from `jj workspace list`.
///
/// This is a data record containing metadata about a workspace.
/// For running commands in a workspace, use [`WorkingCopy`] via
/// [`Repository::workspace_at()`] or [`WorkspaceRef::working_copy()`].
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct WorkspaceInfo {
    /// Workspace name (e.g., "default", "feature-auth")
    pub name: String,
    /// Filesystem path to the workspace
    pub path: PathBuf,
    /// Current working copy commit ID (change ID)
    pub working_copy_commit: String,
    /// Bookmark name if the working copy is on a bookmark (jj's equivalent of branch)
    pub bookmark: Option<String>,
    /// Whether this is the current workspace
    pub is_current: bool,
}

impl WorkspaceInfo {
    /// Returns the workspace directory name.
    ///
    /// This is the filesystem directory name (e.g., "repo.feature" from "/path/to/repo.feature").
    pub fn dir_name(&self) -> &str {
        path_dir_name(&self.path)
    }

    /// Returns true if this workspace has a bookmark.
    pub fn has_bookmark(&self) -> bool {
        self.bookmark.is_some()
    }
}

/// Extract the directory name from a path for display purposes.
///
/// Returns the last component of the path as a string, or "(unknown)" if
/// the path has no filename or contains invalid UTF-8.
pub fn path_dir_name(path: &std::path::Path) -> &str {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("(unknown)")
}

/// Reference to a workspace for parallel task execution.
///
/// Works for both workspace items (has path) and bookmark-only items (no workspace).
#[derive(Debug, Clone)]
pub struct WorkspaceRef {
    /// Bookmark name (e.g., "main", "feature/auth").
    /// None if no bookmark is associated.
    pub bookmark: Option<String>,
    /// Working copy commit ID.
    pub commit_id: String,
    /// Path to workspace, if this bookmark has one.
    /// None for bookmark-only items.
    pub workspace_path: Option<PathBuf>,
    /// Workspace name, if this has a workspace.
    pub workspace_name: Option<String>,
}

impl WorkspaceRef {
    /// Create a WorkspaceRef for a bookmark without a workspace.
    pub fn bookmark_only(bookmark: &str, commit_id: &str) -> Self {
        Self {
            bookmark: Some(bookmark.to_string()),
            commit_id: commit_id.to_string(),
            workspace_path: None,
            workspace_name: None,
        }
    }

    /// Get a working copy handle for this workspace.
    ///
    /// Returns `Some(WorkingCopy)` if this ref has a workspace path,
    /// `None` for bookmark-only items.
    pub fn working_copy<'a>(&self, repo: &'a Repository) -> Option<WorkingCopy<'a>> {
        self.workspace_path
            .as_ref()
            .map(|p| repo.workspace_at(p.clone()))
    }

    /// Returns true if this ref has a workspace.
    pub fn has_workspace(&self) -> bool {
        self.workspace_path.is_some()
    }
}

impl From<&WorkspaceInfo> for WorkspaceRef {
    fn from(ws: &WorkspaceInfo) -> Self {
        Self {
            bookmark: ws.bookmark.clone(),
            commit_id: ws.working_copy_commit.clone(),
            workspace_path: Some(ws.path.clone()),
            workspace_name: Some(ws.name.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_dir_name() {
        assert_eq!(
            path_dir_name(&PathBuf::from("/home/user/repo.feature")),
            "repo.feature"
        );
        assert_eq!(path_dir_name(&PathBuf::from("/")), "(unknown)");
        assert!(!path_dir_name(&PathBuf::from("/home/user/repo/")).is_empty());

        // WorkspaceInfo::dir_name
        let ws = WorkspaceInfo {
            name: "feature".into(),
            path: PathBuf::from("/repos/myrepo.feature"),
            working_copy_commit: "abc123".into(),
            bookmark: Some("feature".into()),
            is_current: false,
        };
        assert_eq!(ws.dir_name(), "myrepo.feature");
    }

    #[test]
    fn test_hook_type_display() {
        use strum::IntoEnumIterator;

        // Verify all hook types serialize to kebab-case
        for hook in HookType::iter() {
            let display = format!("{hook}");
            assert!(
                display.chars().all(|c| c.is_lowercase() || c == '-'),
                "Hook {hook:?} should be kebab-case, got: {display}"
            );
        }
    }

    #[test]
    fn test_workspace_ref_from_workspace_info() {
        let ws = WorkspaceInfo {
            name: "feature".into(),
            path: PathBuf::from("/repo.feature"),
            working_copy_commit: "abc123".into(),
            bookmark: Some("feature".into()),
            is_current: false,
        };

        let workspace_ref = WorkspaceRef::from(&ws);

        assert_eq!(workspace_ref.bookmark, Some("feature".to_string()));
        assert_eq!(workspace_ref.commit_id, "abc123");
        assert_eq!(
            workspace_ref.workspace_path,
            Some(PathBuf::from("/repo.feature"))
        );
        assert!(workspace_ref.has_workspace());
    }

    #[test]
    fn test_workspace_ref_bookmark_only() {
        let workspace_ref = WorkspaceRef::bookmark_only("feature", "abc123");

        assert_eq!(workspace_ref.bookmark, Some("feature".to_string()));
        assert_eq!(workspace_ref.commit_id, "abc123");
        assert_eq!(workspace_ref.workspace_path, None);
        assert!(!workspace_ref.has_workspace());
    }
}
