//! Switch command handler for jj repositories.
//!
//! Simpler than the git switch path: no PR/MR resolution, no hooks, no DWIM
//! branch lookup. jj workspaces are identified by name, not branch.

use std::path::PathBuf;

use color_print::cformat;
use normalize_path::NormalizePath;
use worktrunk::config::sanitize_branch_name;
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{eprintln, success_message};
use worktrunk::workspace::{JjWorkspace, Workspace};

use super::handle_switch::SwitchOptions;
use crate::output;

/// Handle `wt switch` for jj repositories.
pub fn handle_switch_jj(opts: SwitchOptions<'_>) -> anyhow::Result<()> {
    let workspace = JjWorkspace::from_current_dir()?;
    let name = opts.branch;

    // Check if workspace already exists
    let existing_path = find_existing_workspace(&workspace, name)?;

    if let Some(path) = existing_path {
        if !opts.change_dir {
            return Ok(());
        }
        // Switch to existing workspace
        let path_display = format_path_for_display(&path);
        eprintln!(
            "{}",
            success_message(cformat!(
                "Switched to workspace <bold>{name}</> @ <bold>{path_display}</>"
            ))
        );
        output::change_directory(&path)?;
        return Ok(());
    }

    // Workspace doesn't exist — need --create to make one
    if !opts.create {
        anyhow::bail!("Workspace '{}' not found. Use --create to create it.", name);
    }

    // Compute path for new workspace
    let worktree_path = compute_jj_workspace_path(&workspace, name)?;

    if worktree_path.exists() {
        anyhow::bail!(
            "Path already exists: {}",
            format_path_for_display(&worktree_path)
        );
    }

    // Create the workspace
    workspace.create_workspace(name, opts.base, &worktree_path)?;

    let path_display = format_path_for_display(&worktree_path);
    eprintln!(
        "{}",
        success_message(cformat!(
            "Created workspace <bold>{name}</> @ <bold>{path_display}</>"
        ))
    );

    if opts.change_dir {
        output::change_directory(&worktree_path)?;
    }

    Ok(())
}

/// Find an existing workspace by name, returning its path if it exists.
fn find_existing_workspace(workspace: &JjWorkspace, name: &str) -> anyhow::Result<Option<PathBuf>> {
    let workspaces = workspace.list_workspaces()?;
    for ws in &workspaces {
        if ws.name == name {
            return Ok(Some(ws.path.clone()));
        }
    }
    Ok(None)
}

/// Compute the filesystem path for a new jj workspace.
///
/// Uses the same sibling-directory convention as git worktrees:
/// `{repo_root}/../{repo_name}.{workspace_name}`
fn compute_jj_workspace_path(workspace: &JjWorkspace, name: &str) -> anyhow::Result<PathBuf> {
    let root = workspace.root();
    let repo_name = root
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Repository path has no filename"))?
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Repository path contains invalid UTF-8"))?;

    let sanitized = sanitize_branch_name(name);
    let path = root
        .join(format!("../{}.{}", repo_name, sanitized))
        .normalize();
    Ok(path)
}
