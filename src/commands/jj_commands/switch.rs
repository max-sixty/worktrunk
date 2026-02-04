//! Switch command for jj workspaces.
//!
//! Creates or switches to a jj workspace.

use std::path::PathBuf;

use anyhow::Context;
use color_print::cformat;
use worktrunk::config::UserConfig;
use worktrunk::jj::{JjError, Repository, ResolvedWorkspace};
use worktrunk::styling::{eprintln, hint_message, info_message, success_message};

/// Options for the switch command.
pub struct SwitchOptions {
    pub bookmark: Option<String>,
    pub create: bool,
    pub base: Option<String>,
    pub yes: bool,
    pub clobber: bool,
}

/// Result of a switch operation.
pub enum SwitchResult {
    /// Already at the requested workspace
    AlreadyAt(PathBuf),
    /// Switched to an existing workspace
    Existing { path: PathBuf },
    /// Created a new workspace
    Created {
        path: PathBuf,
        name: String,
        created_bookmark: bool,
        base_bookmark: Option<String>,
    },
}

/// Handle the switch command for jj workspaces.
pub fn handle_switch_jj(
    options: SwitchOptions,
    config: &UserConfig,
) -> anyhow::Result<Option<PathBuf>> {
    let repo = Repository::current()?;

    // If no bookmark specified, list workspaces
    let Some(bookmark_arg) = options.bookmark else {
        eprintln!(
            "{}",
            hint_message("Use wt list to see available workspaces, or wt switch <bookmark> to switch")
        );
        return Ok(None);
    };

    // Resolve the bookmark name (handles @, -, ^)
    let bookmark = repo.resolve_workspace_name(&bookmark_arg)?;

    // Check if workspace already exists for this bookmark
    let existing_workspace = repo.workspace_for_bookmark(&bookmark)?;

    if let Some(existing_path) = existing_workspace {
        // Workspace exists - switch to it
        let current_bookmark = repo.current_workspace().bookmark().ok().flatten();

        // Record previous for "wt switch -"
        repo.set_switch_previous(current_bookmark.as_deref())?;

        // Check if we're already at this workspace
        if let Ok(current_root) = repo.current_workspace().root() {
            if current_root == existing_path {
                eprintln!(
                    "{}",
                    info_message(cformat!("Already at workspace for <bold>{bookmark}</>"))
                );
                return Ok(Some(existing_path));
            }
        }

        eprintln!(
            "{}",
            success_message(cformat!(
                "Switching to workspace for <bold>{bookmark}</>"
            ))
        );
        return Ok(Some(existing_path));
    }

    // No existing workspace - need to create one
    if !options.create && !repo.bookmark_exists(&bookmark)? {
        return Err(JjError::BookmarkNotFound {
            bookmark: bookmark.clone(),
            show_create_hint: true,
        }
        .into());
    }

    // Compute workspace path and name
    let workspace_path = compute_workspace_path(&repo, &bookmark, config)?;
    let workspace_name = sanitize_workspace_name(&bookmark);

    // Check if path is occupied
    if let Some(existing) = repo.workspace_at_path(&workspace_path)? {
        return Err(JjError::WorkspacePathOccupied {
            name: workspace_name,
            path: workspace_path,
            occupant: Some(existing.name),
        }
        .into());
    }

    // If --clobber, handle existing directory
    if workspace_path.exists() {
        if options.clobber {
            let backup_path = compute_backup_path(&workspace_path);
            eprintln!(
                "{}",
                info_message(cformat!(
                    "Moving existing directory to <bright-black>{}</>",
                    backup_path.display()
                ))
            );
            std::fs::rename(&workspace_path, &backup_path)
                .context("Failed to move existing directory")?;
        } else {
            return Err(anyhow::anyhow!(
                "Path {} already exists. Use --clobber to replace it.",
                workspace_path.display()
            ));
        }
    }

    // Create the workspace
    let created_bookmark = if options.create {
        // Create bookmark first if it doesn't exist
        if !repo.bookmark_exists(&bookmark)? {
            // Get base bookmark - either from options or default
            let default_bookmark = repo.default_bookmark();
            let base = options
                .base
                .as_deref()
                .or(default_bookmark.as_deref());
            if let Some(base_ref) = base {
                // Create bookmark at the base revision
                repo.create_bookmark(&bookmark, Some(base_ref))?;
            } else {
                // Create bookmark at current revision
                repo.create_bookmark(&bookmark, None)?;
            }
            true
        } else {
            false
        }
    } else {
        false
    };

    // Add the workspace
    repo.add_workspace(&workspace_name, &workspace_path, Some(&bookmark))?;

    // Record previous for "wt switch -"
    let current_bookmark = repo.current_workspace().bookmark().ok().flatten();
    repo.set_switch_previous(current_bookmark.as_deref())?;

    eprintln!(
        "{}",
        success_message(cformat!(
            "Created workspace <bold>{workspace_name}</> at <bright-black>{}</>",
            workspace_path.display()
        ))
    );

    if created_bookmark {
        eprintln!(
            "{}",
            info_message(cformat!(
                "Created bookmark <bold>{bookmark}</>"
            ))
        );
    }

    Ok(Some(workspace_path))
}

/// Compute the workspace path based on the bookmark name and config.
fn compute_workspace_path(
    repo: &Repository,
    bookmark: &str,
    config: &UserConfig,
) -> anyhow::Result<PathBuf> {
    // Use the configured template or default
    let template = config
        .configs
        .worktree_path
        .as_deref()
        .unwrap_or("{{ repo_path }}/../{{ repo }}.{{ branch | sanitize }}");

    // Simple template expansion (subset of the full git version)
    let repo_path = repo.repo_path().to_string_lossy();
    let repo_name = repo
        .repo_path()
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo");

    let sanitized_bookmark = bookmark.replace('/', "-").replace('\\', "-");

    let expanded = template
        .replace("{{ repo_path }}", &repo_path)
        .replace("{{ repo }}", repo_name)
        .replace("{{ branch | sanitize }}", &sanitized_bookmark)
        .replace("{{ branch }}", bookmark);

    // Expand ~ to home directory
    let expanded = shellexpand::tilde(&expanded).into_owned();

    // Resolve relative paths
    let path = PathBuf::from(expanded);
    if path.is_relative() {
        Ok(repo.repo_path().join(path))
    } else {
        Ok(path)
    }
}

/// Sanitize a bookmark name for use as a workspace name.
fn sanitize_workspace_name(bookmark: &str) -> String {
    bookmark
        .replace('/', "-")
        .replace('\\', "-")
        .replace(' ', "-")
        .to_lowercase()
}

/// Compute a backup path for an existing directory.
fn compute_backup_path(path: &PathBuf) -> PathBuf {
    let mut backup = path.clone();
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("backup");

    for i in 1.. {
        backup.set_file_name(format!("{}.backup{}", file_name, i));
        if !backup.exists() {
            break;
        }
    }

    backup
}
