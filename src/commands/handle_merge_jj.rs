//! Merge command handler for jj repositories.
//!
//! Simpler than git merge: no staging area, no pre-commit hooks, no branch
//! deletion. jj auto-snapshots the working copy.

use std::path::Path;

use anyhow::Context;
use color_print::cformat;
use worktrunk::config::UserConfig;
use worktrunk::styling::{eprintln, info_message, success_message};
use worktrunk::workspace::{JjWorkspace, Workspace};

use super::handle_remove_jj::remove_jj_workspace_and_cd;
use super::merge::MergeOptions;
use super::step_commands::{SquashResult, do_squash};

/// Handle `wt merge` for jj repositories.
///
/// Squashes (or rebases) the current workspace's changes into trunk,
/// updates the target bookmark, pushes if possible, and optionally
/// removes the workspace.
pub fn handle_merge_jj(opts: MergeOptions<'_>) -> anyhow::Result<()> {
    let workspace = JjWorkspace::from_current_dir()?;
    let cwd = std::env::current_dir()?;

    let current = workspace.current_workspace(&cwd)?;

    if current.is_default {
        anyhow::bail!("Cannot merge the default workspace");
    }

    let ws_name = current.name.clone();
    let ws_path = current.path.clone();

    // Load config for merge defaults
    let config = UserConfig::load().context("Failed to load config")?;
    let project_id = workspace.project_identifier().ok();
    let resolved = config.resolved(project_id.as_deref());

    // Target bookmark name — detect from trunk() or use explicit override
    let detected_target = workspace.trunk_bookmark()?;
    let target = opts.target.unwrap_or(detected_target.as_str());

    // Check if already integrated
    let feature_tip = workspace.feature_tip(&ws_path)?;
    if workspace.is_integrated(&feature_tip, target)?.is_some() {
        eprintln!(
            "{}",
            info_message(cformat!(
                "Workspace <bold>{ws_name}</> is already integrated into trunk"
            ))
        );
        return remove_if_requested(&workspace, &resolved, &opts, &ws_name, &ws_path);
    }

    // CLI flags override config values (jj always squashes by default)
    let squash = opts.squash.unwrap_or(resolved.merge.squash());

    if squash {
        let repo_name = project_id.as_deref().unwrap_or("repo");
        match do_squash(
            &workspace,
            target,
            &ws_path,
            &resolved.commit_generation,
            &ws_name,
            repo_name,
        )? {
            SquashResult::NoCommitsAhead(_) => {
                eprintln!(
                    "{}",
                    info_message(cformat!(
                        "Workspace <bold>{ws_name}</> is already integrated into trunk"
                    ))
                );
                return remove_if_requested(&workspace, &resolved, &opts, &ws_name, &ws_path);
            }
            SquashResult::AlreadySingleCommit | SquashResult::Squashed => {
                // Proceed to push
            }
            SquashResult::NoNetChanges => {
                // Feature commits canceled out — nothing to push, just remove
                return remove_if_requested(&workspace, &resolved, &opts, &ws_name, &ws_path);
            }
        }
    } else {
        rebase_onto_trunk(&workspace, &ws_path, target)?;
    }

    // Push (best-effort — may not have a git remote)
    match workspace.advance_and_push(target, &ws_path) {
        Ok(result) if result.commit_count > 0 => {
            eprintln!("{}", success_message(cformat!("Pushed <bold>{target}</>")));
        }
        _ => {}
    }

    let mode = if squash { "Squashed" } else { "Merged" };
    eprintln!(
        "{}",
        success_message(cformat!(
            "{mode} workspace <bold>{ws_name}</> into <bold>{target}</>"
        ))
    );

    remove_if_requested(&workspace, &resolved, &opts, &ws_name, &ws_path)
}

/// Rebase the feature branch onto trunk without squashing.
///
/// 1. `jj rebase -b @ -d {target}` — rebase entire branch
/// 2. Determine feature tip (@ if has content, @- if empty)
/// 3. `jj bookmark set {target} -r {tip}` — update bookmark
fn rebase_onto_trunk(workspace: &JjWorkspace, ws_path: &Path, target: &str) -> anyhow::Result<()> {
    workspace.run_in_dir(ws_path, &["rebase", "-b", "@", "-d", target])?;

    // After rebase, find the feature tip (same logic as squash path)
    let feature_tip = workspace.feature_tip(ws_path)?;
    workspace.run_in_dir(ws_path, &["bookmark", "set", target, "-r", &feature_tip])?;

    Ok(())
}

/// Remove the workspace if `--no-remove` wasn't specified.
fn remove_if_requested(
    workspace: &JjWorkspace,
    resolved: &worktrunk::config::ResolvedConfig,
    opts: &MergeOptions<'_>,
    ws_name: &str,
    ws_path: &Path,
) -> anyhow::Result<()> {
    let remove = opts.remove.unwrap_or(resolved.merge.remove());
    if !remove {
        eprintln!("{}", info_message("Workspace preserved (--no-remove)"));
        return Ok(());
    }

    // Merge handles its own hook flow — don't run remove hooks here
    remove_jj_workspace_and_cd(workspace, ws_name, ws_path, false, false)
}
