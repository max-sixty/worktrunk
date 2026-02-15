//! Merge command handler for jj repositories.
//!
//! Handles squash/rebase, hook execution, push, and optional workspace removal.
//! jj auto-snapshots the working copy (no staging area or pre-commit hooks).

use std::path::Path;

use anyhow::Context;
use color_print::cformat;
use worktrunk::HookType;
use worktrunk::config::UserConfig;
use worktrunk::styling::{eprintln, info_message, success_message};
use worktrunk::workspace::{JjWorkspace, Workspace};

use super::command_approval::approve_hooks;
use super::context::CommandEnv;
use super::handle_remove_jj::remove_jj_workspace_and_cd;
use super::hooks::{HookFailureStrategy, run_hook_with_filter};
use super::merge::MergeOptions;
use super::step_commands::{SquashResult, do_squash};

/// Handle `wt merge` for jj repositories.
///
/// Squashes (or rebases) the current workspace's changes into trunk,
/// runs pre-merge/post-merge hooks, updates the target bookmark, pushes
/// if possible, and optionally removes the workspace.
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

    // CLI flags override config values
    let verify = opts.verify.unwrap_or(resolved.merge.verify());
    let yes = opts.yes;
    let remove = opts.remove.unwrap_or(resolved.merge.remove());

    // "Approve at the Gate": approve all hooks upfront
    let verify = if verify {
        let env = CommandEnv::for_action_branchless()?;
        let ctx = env.context(yes);

        let mut hook_types = vec![HookType::PreMerge, HookType::PostMerge];
        if remove {
            hook_types.extend_from_slice(&[
                HookType::PreRemove,
                HookType::PostRemove,
                HookType::PostSwitch,
            ]);
        }

        let approved = approve_hooks(&ctx, &hook_types)?;
        if !approved {
            eprintln!("{}", info_message("Commands declined, continuing merge"));
            false
        } else {
            true
        }
    } else {
        false
    };

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
        return remove_if_requested(&workspace, remove, yes, &ws_name, &ws_path, verify);
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
                return remove_if_requested(&workspace, remove, yes, &ws_name, &ws_path, verify);
            }
            SquashResult::AlreadySingleCommit | SquashResult::Squashed => {
                // Proceed to push
            }
            SquashResult::NoNetChanges => {
                // Feature commits canceled out — nothing to push, just remove
                return remove_if_requested(&workspace, remove, yes, &ws_name, &ws_path, verify);
            }
        }
    } else {
        rebase_onto_trunk(&workspace, &ws_path, target)?;
    }

    // Run pre-merge hooks (after squash/rebase, before push)
    if verify {
        let env = CommandEnv::for_action_branchless()?;
        let ctx = env.context(yes);
        let project_config = workspace.load_project_config()?;
        let user_hooks = ctx.config.hooks(ctx.project_id().as_deref());
        run_hook_with_filter(
            &ctx,
            user_hooks.pre_merge.as_ref(),
            project_config
                .as_ref()
                .and_then(|c| c.hooks.pre_merge.as_ref()),
            HookType::PreMerge,
            &[("target", target)],
            HookFailureStrategy::FailFast,
            None,
            None,
        )?;
    }

    let mode = if squash { "Squashed" } else { "Merged" };
    eprintln!(
        "{}",
        success_message(cformat!(
            "{mode} workspace <bold>{ws_name}</> into <bold>{target}</>"
        ))
    );

    // Run post-merge hooks before removal (cwd must still exist)
    if verify {
        let env = CommandEnv::for_action_branchless()?;
        let ctx = env.context(yes);
        let project_config = workspace.load_project_config()?;
        let user_hooks = ctx.config.hooks(ctx.project_id().as_deref());
        run_hook_with_filter(
            &ctx,
            user_hooks.post_merge.as_ref(),
            project_config
                .as_ref()
                .and_then(|c| c.hooks.post_merge.as_ref()),
            HookType::PostMerge,
            &[("target", target)],
            HookFailureStrategy::Warn,
            None,
            None,
        )?;
    }

    // Remove workspace if requested
    remove_if_requested(&workspace, remove, yes, &ws_name, &ws_path, verify)?;

    Ok(())
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
    remove: bool,
    yes: bool,
    ws_name: &str,
    ws_path: &Path,
    run_hooks: bool,
) -> anyhow::Result<()> {
    if !remove {
        eprintln!("{}", info_message("Workspace preserved (--no-remove)"));
        return Ok(());
    }

    // Pass through run_hooks so pre-remove/post-remove/post-switch hooks execute during merge
    remove_jj_workspace_and_cd(workspace, ws_name, ws_path, run_hooks, yes)
}
