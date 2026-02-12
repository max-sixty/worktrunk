//! Remove command handler for jj repositories.
//!
//! Simpler than git removal: no branch deletion, no merge status checks.
//! Just forget the workspace and remove the directory.

use std::path::Path;

use color_print::cformat;
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{eprintln, success_message, warning_message};
use worktrunk::workspace::{JjWorkspace, Workspace};

use crate::output;

/// Handle `wt remove` for jj repositories.
///
/// Removes one or more workspaces by name. If no names given, removes the
/// current workspace. Cannot remove the default workspace.
pub fn handle_remove_jj(names: &[String]) -> anyhow::Result<()> {
    let workspace = JjWorkspace::from_current_dir()?;
    let cwd = std::env::current_dir()?;

    let targets = if names.is_empty() {
        let current = workspace.current_workspace(&cwd)?;
        vec![current.name]
    } else {
        names.to_vec()
    };

    for name in &targets {
        remove_jj_workspace_and_cd(&workspace, name, &workspace.workspace_path(name)?)?;
    }

    Ok(())
}

/// Forget a jj workspace, remove its directory, and cd to default if needed.
///
/// Shared between `wt remove` and `wt merge` for jj repositories.
pub fn remove_jj_workspace_and_cd(
    workspace: &JjWorkspace,
    name: &str,
    ws_path: &Path,
) -> anyhow::Result<()> {
    if name == "default" {
        anyhow::bail!("Cannot remove the default workspace");
    }

    let path_display = format_path_for_display(ws_path);

    // Check if we're inside the workspace being removed
    let cwd = dunce::canonicalize(std::env::current_dir()?)?;
    let canonical_ws = dunce::canonicalize(ws_path).unwrap_or_else(|_| ws_path.to_path_buf());
    let removing_current = cwd.starts_with(&canonical_ws);

    // Forget the workspace in jj
    workspace.remove_workspace(name)?;

    // Remove the directory
    if ws_path.exists() {
        std::fs::remove_dir_all(ws_path).map_err(|e| {
            anyhow::anyhow!(
                "Workspace forgotten but failed to remove {}: {}",
                path_display,
                e
            )
        })?;
    } else {
        eprintln!(
            "{}",
            warning_message(cformat!(
                "Workspace directory already removed: <bold>{path_display}</>"
            ))
        );
    }
    eprintln!(
        "{}",
        success_message(cformat!(
            "Removed workspace <bold>{name}</> @ <bold>{path_display}</>"
        ))
    );

    // If removing current workspace, cd to default workspace
    if removing_current {
        let default_path = workspace
            .default_workspace_path()?
            .unwrap_or_else(|| workspace.root().to_path_buf());
        output::change_directory(&default_path)?;
    }

    Ok(())
}
