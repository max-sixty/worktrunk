//! Merge command handler for jj repositories.
//!
//! Simpler than git merge: no staging area, no pre-commit hooks, no branch
//! deletion. jj auto-snapshots the working copy.

use std::path::Path;

use color_print::cformat;
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{eprintln, info_message, success_message};
use worktrunk::workspace::{JjWorkspace, Workspace};

use super::merge::MergeOptions;
use crate::output;

/// Handle `wt merge` for jj repositories.
///
/// Squashes (or rebases) the current workspace's changes into trunk,
/// updates the target bookmark, pushes if possible, and optionally
/// removes the workspace.
pub fn handle_merge_jj(opts: MergeOptions<'_>) -> anyhow::Result<()> {
    let workspace = JjWorkspace::from_current_dir()?;
    let cwd = dunce::canonicalize(std::env::current_dir()?)?;

    // Find current workspace
    let workspaces = workspace.list_workspaces()?;
    let current = workspaces
        .iter()
        .find(|ws| {
            dunce::canonicalize(&ws.path)
                .map(|p| cwd.starts_with(&p))
                .unwrap_or(false)
        })
        .ok_or_else(|| anyhow::anyhow!("Not inside a jj workspace"))?;

    if current.is_default {
        anyhow::bail!("Cannot merge the default workspace");
    }

    let ws_name = current.name.clone();
    let ws_path = current.path.clone();

    // Target bookmark name (default: "main")
    let target = opts.target.unwrap_or("main");

    // Get the feature tip change ID. The workspace's working copy (@) is often
    // an empty auto-snapshot; the real feature commits are its parents. Use @-
    // when @ is empty so we don't reference a commit that jj may abandon.
    let feature_tip = get_feature_tip(&workspace, &ws_path)?;

    // Check if already integrated
    if workspace.is_integrated(&feature_tip, "trunk()")?.is_some() {
        eprintln!(
            "{}",
            info_message(cformat!(
                "Workspace <bold>{ws_name}</> is already integrated into trunk"
            ))
        );
        return remove_workspace_if_requested(&workspace, &opts, &ws_name, &ws_path);
    }

    // Squash by default for jj (combine all feature commits into one on trunk)
    let squash = opts.squash.unwrap_or(true);

    if squash {
        squash_into_trunk(&workspace, &ws_path, &feature_tip, &ws_name, target)?;
    } else {
        rebase_onto_trunk(&workspace, &ws_path, target)?;
    }

    // Push (best-effort — may not have a git remote)
    push_bookmark(&workspace, &ws_path, target);

    let mode = if squash { "Squashed" } else { "Merged" };
    eprintln!(
        "{}",
        success_message(cformat!(
            "{mode} workspace <bold>{ws_name}</> into <bold>{target}</>"
        ))
    );

    remove_workspace_if_requested(&workspace, &opts, &ws_name, &ws_path)
}

/// Determine the feature tip change ID.
///
/// In jj, the working copy (@) is often an empty auto-snapshot commit.
/// When @ is empty, the real feature tip is @- (the parent). We use @-
/// in that case because empty commits get abandoned by `jj new`.
fn get_feature_tip(workspace: &JjWorkspace, ws_path: &Path) -> anyhow::Result<String> {
    let empty_check = workspace.run_in_dir(
        ws_path,
        &[
            "log",
            "-r",
            "@",
            "--no-graph",
            "-T",
            r#"if(self.empty(), "empty", "content")"#,
        ],
    )?;

    let revset = if empty_check.trim() == "empty" {
        "@-"
    } else {
        "@"
    };

    let output = workspace.run_in_dir(
        ws_path,
        &[
            "log",
            "-r",
            revset,
            "--no-graph",
            "-T",
            r#"self.change_id().short(12)"#,
        ],
    )?;

    Ok(output.trim().to_string())
}

/// Squash all feature changes into a single commit on trunk.
///
/// 1. `jj new trunk()` — create empty commit on trunk
/// 2. `jj squash --from 'trunk()..{tip}' --into @` — combine feature into it
/// 3. `jj bookmark set {target} -r @` — update bookmark
fn squash_into_trunk(
    workspace: &JjWorkspace,
    ws_path: &Path,
    feature_tip: &str,
    ws_name: &str,
    target: &str,
) -> anyhow::Result<()> {
    workspace.run_in_dir(ws_path, &["new", "trunk()"])?;

    // Collect the descriptions from feature commits for the squash message
    let descriptions = workspace.run_in_dir(
        ws_path,
        &[
            "log",
            "-r",
            &format!("trunk()..{feature_tip}"),
            "--no-graph",
            "-T",
            r#"self.description() ++ "\n""#,
        ],
    )?;

    let message = descriptions.trim();
    let message = if message.is_empty() {
        format!("Merge workspace {ws_name}")
    } else {
        message.to_string()
    };

    let from_revset = format!("trunk()..{feature_tip}");
    workspace.run_in_dir(
        ws_path,
        &[
            "squash",
            "--from",
            &from_revset,
            "--into",
            "@",
            "-m",
            &message,
        ],
    )?;

    workspace.run_in_dir(ws_path, &["bookmark", "set", target, "-r", "@"])?;

    Ok(())
}

/// Rebase the feature branch onto trunk without squashing.
///
/// 1. `jj rebase -b @ -d trunk()` — rebase entire branch
/// 2. Determine feature tip (@ if has content, @- if empty)
/// 3. `jj bookmark set {target} -r {tip}` — update bookmark
fn rebase_onto_trunk(workspace: &JjWorkspace, ws_path: &Path, target: &str) -> anyhow::Result<()> {
    workspace.run_in_dir(ws_path, &["rebase", "-b", "@", "-d", "trunk()"])?;

    // After rebase, find the feature tip (same logic as squash path)
    let feature_tip = get_feature_tip(workspace, ws_path)?;
    workspace.run_in_dir(ws_path, &["bookmark", "set", target, "-r", &feature_tip])?;

    Ok(())
}

/// Push the bookmark to remote (best-effort).
fn push_bookmark(workspace: &JjWorkspace, ws_path: &Path, target: &str) {
    match workspace.run_in_dir(ws_path, &["git", "push", "--bookmark", target]) {
        Ok(_) => {
            eprintln!("{}", success_message(cformat!("Pushed <bold>{target}</>")));
        }
        Err(e) => {
            log::debug!("Push failed (may not have remote): {e}");
        }
    }
}

/// Remove the workspace if `--no-remove` wasn't specified.
fn remove_workspace_if_requested(
    workspace: &JjWorkspace,
    opts: &MergeOptions<'_>,
    ws_name: &str,
    ws_path: &Path,
) -> anyhow::Result<()> {
    let remove = opts.remove.unwrap_or(true);
    if !remove {
        eprintln!("{}", info_message("Workspace preserved (--no-remove)"));
        return Ok(());
    }

    let default_path = workspace
        .default_workspace_path()?
        .unwrap_or_else(|| workspace.root().to_path_buf());

    workspace.remove_workspace(ws_name)?;
    if ws_path.exists() {
        std::fs::remove_dir_all(ws_path).map_err(|e| {
            anyhow::anyhow!(
                "Workspace forgotten but failed to remove {}: {}",
                format_path_for_display(ws_path),
                e
            )
        })?;
    }

    let path_display = format_path_for_display(ws_path);
    eprintln!(
        "{}",
        success_message(cformat!(
            "Removed workspace <bold>{ws_name}</> @ <bold>{path_display}</>"
        ))
    );

    output::change_directory(&default_path)?;
    Ok(())
}
