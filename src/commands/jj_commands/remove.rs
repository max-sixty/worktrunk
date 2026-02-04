//! Remove command for jj workspaces.
//!
//! Removes a jj workspace and optionally its bookmark.

use std::path::PathBuf;

use color_print::cformat;
use worktrunk::jj::{JjError, Repository, ResolvedWorkspace};
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{eprintln, hint_message, info_message, success_message, warning_message};

/// Options for the remove command.
pub struct RemoveOptions {
    /// Workspace/bookmark names to remove (defaults to current if empty)
    pub names: Vec<String>,
    /// Whether to delete the bookmark after removal
    pub delete_bookmark: bool,
    /// Force deletion of unmerged bookmarks
    pub force_delete: bool,
    /// Run removal in foreground (block until complete)
    pub foreground: bool,
    /// Skip approval prompts
    pub yes: bool,
    /// Force worktree removal (even with uncommitted changes)
    pub force: bool,
}

/// Handle the remove command for jj workspaces.
pub fn handle_remove_jj(options: RemoveOptions) -> anyhow::Result<Option<PathBuf>> {
    let repo = Repository::current()?;

    // Determine which workspaces to remove
    let names: Vec<String> = if options.names.is_empty() {
        // Default to current workspace
        match repo.current_workspace_info()? {
            Some(info) => vec![info.name.clone()],
            None => {
                return Err(JjError::NotInWorkspace {
                    action: Some("determine current workspace".into()),
                }
                .into());
            }
        }
    } else {
        options.names.clone()
    };

    let mut return_path = None;

    for name in names {
        let result = remove_single_workspace(&repo, &name, &options)?;
        if return_path.is_none() {
            return_path = result;
        }
    }

    Ok(return_path)
}

fn remove_single_workspace(
    repo: &Repository,
    name: &str,
    options: &RemoveOptions,
) -> anyhow::Result<Option<PathBuf>> {
    // Resolve the workspace
    let resolved = repo.resolve_workspace(name)?;

    let (workspace_path, workspace_name, bookmark) = match resolved {
        ResolvedWorkspace::Workspace {
            path,
            name,
            bookmark,
        } => (path, name, bookmark),
        ResolvedWorkspace::BookmarkOnly { bookmark } => {
            // No workspace, just a bookmark - nothing to remove
            eprintln!(
                "{}",
                warning_message(cformat!(
                    "No workspace found for bookmark <bold>{bookmark}</>. Nothing to remove."
                ))
            );

            if options.delete_bookmark && options.force_delete {
                repo.delete_bookmark(&bookmark)?;
                eprintln!(
                    "{}",
                    info_message(cformat!("Deleted bookmark <bold>{bookmark}</>"))
                );
            }

            return Ok(None);
        }
    };

    // Check if this is the current workspace
    let is_current = repo
        .current_workspace()
        .root()
        .map(|r| r == workspace_path)
        .unwrap_or(false);

    // If current workspace, we need to return to a different workspace
    let return_to = if is_current {
        // Try to find another workspace to return to
        let workspaces = repo.list_workspaces()?;
        workspaces
            .iter()
            .find(|ws| ws.path != workspace_path)
            .map(|ws| ws.path.clone())
            .or_else(|| Some(repo.repo_path().to_path_buf()))
    } else {
        None
    };

    // Check for uncommitted changes
    if !options.force {
        let ws = repo.workspace_at(&workspace_path);
        if ws.is_dirty()? {
            return Err(anyhow::anyhow!(
                "Workspace '{}' has uncommitted changes. Use --force to remove anyway.",
                workspace_name
            ));
        }
    }

    // Confirm removal
    if !options.yes && is_current {
        eprintln!(
            "{}",
            warning_message(cformat!(
                "About to remove current workspace <bold>{workspace_name}</>"
            ))
        );
        // In a real implementation, we'd prompt for confirmation here
        // For now, just proceed
    }

    // Remove the workspace
    repo.remove_workspace(&workspace_name, options.force)?;

    eprintln!(
        "{}",
        success_message(cformat!(
            "Removed workspace <bold>{workspace_name}</>"
        ))
    );

    // Delete bookmark if requested
    if options.delete_bookmark {
        if let Some(bookmark_name) = &bookmark {
            if options.force_delete || !is_bookmark_unmerged(repo, bookmark_name)? {
                repo.delete_bookmark(bookmark_name)?;
                eprintln!(
                    "{}",
                    info_message(cformat!("Deleted bookmark <bold>{bookmark_name}</>"))
                );
            } else {
                eprintln!(
                    "{}",
                    warning_message(cformat!(
                        "Bookmark <bold>{bookmark_name}</> has unmerged changes. Use -D to delete anyway."
                    ))
                );
            }
        }
    }

    Ok(return_to)
}

/// Check if a bookmark has unmerged changes.
///
/// Returns true if the bookmark's commit is not an ancestor of the default bookmark.
fn is_bookmark_unmerged(repo: &Repository, bookmark: &str) -> anyhow::Result<bool> {
    let Some(default_bookmark) = repo.default_bookmark() else {
        // No default bookmark - can't determine merge status
        return Ok(false);
    };

    if bookmark == default_bookmark {
        // The default bookmark is never "unmerged"
        return Ok(false);
    }

    // Check if bookmark's commit is an ancestor of default
    // jj revset: "bookmark ~ ancestors(default_bookmark)"
    // If the result is non-empty, bookmark has commits not in default
    let output = repo.run_command(&[
        "log",
        "-r",
        &format!("{} ~ ancestors({})", bookmark, default_bookmark),
        "--no-graph",
        "-T",
        "commit_id",
    ])?;

    // If output is non-empty, there are unmerged commits
    Ok(!output.trim().is_empty())
}
