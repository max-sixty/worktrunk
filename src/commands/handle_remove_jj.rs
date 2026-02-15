//! Remove command handler for jj repositories.
//!
//! Simpler than git removal: no branch deletion, no merge status checks.
//! Just forget the workspace and remove the directory.

use std::path::Path;

use color_print::cformat;
use worktrunk::HookType;
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{eprintln, info_message, success_message, warning_message};
use worktrunk::workspace::{JjWorkspace, Workspace};

use super::command_approval::approve_hooks;
use super::context::CommandEnv;
use super::hooks::{HookFailureStrategy, run_hook_with_filter};
use crate::output;

/// Handle `wt remove` for jj repositories.
///
/// Removes one or more workspaces by name. If no names given, removes the
/// current workspace. Cannot remove the default workspace.
pub fn handle_remove_jj(names: &[String], verify: bool, yes: bool) -> anyhow::Result<()> {
    let workspace = JjWorkspace::from_current_dir()?;
    let cwd = std::env::current_dir()?;

    let targets = if names.is_empty() {
        let current = workspace.current_workspace(&cwd)?;
        vec![current.name]
    } else {
        names.to_vec()
    };

    // "Approve at the Gate": approve remove hooks upfront
    let run_hooks = if verify {
        let env = CommandEnv::for_action_branchless()?;
        let ctx = env.context(yes);
        let approved = approve_hooks(
            &ctx,
            &[
                HookType::PreRemove,
                HookType::PostRemove,
                HookType::PostSwitch,
            ],
        )?;
        if !approved {
            eprintln!("{}", info_message("Commands declined, continuing removal"));
        }
        approved
    } else {
        false
    };

    for name in &targets {
        remove_jj_workspace_and_cd(
            &workspace,
            name,
            &workspace.workspace_path(name)?,
            run_hooks,
            yes,
        )?;
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
    run_hooks: bool,
    yes: bool,
) -> anyhow::Result<()> {
    if name == "default" {
        anyhow::bail!("Cannot remove the default workspace");
    }

    let path_display = format_path_for_display(ws_path);

    // Check if we're inside the workspace being removed
    let cwd = dunce::canonicalize(std::env::current_dir()?)?;
    let canonical_ws = dunce::canonicalize(ws_path).unwrap_or_else(|_| ws_path.to_path_buf());
    let removing_current = cwd.starts_with(&canonical_ws);

    // Build hook context BEFORE deletion — CommandEnv needs the current directory to exist
    let hook_ctx = if run_hooks {
        let env = CommandEnv::for_action_branchless()?;
        let project_config = workspace.load_project_config()?;
        Some((env, project_config))
    } else {
        None
    };

    // Run pre-remove hooks
    if let Some((ref env, ref project_config)) = hook_ctx {
        let ctx = env.context(yes);
        let user_hooks = ctx.config.hooks(ctx.project_id().as_deref());
        run_hook_with_filter(
            &ctx,
            user_hooks.pre_remove.as_ref(),
            project_config
                .as_ref()
                .and_then(|c| c.hooks.pre_remove.as_ref()),
            HookType::PreRemove,
            &[],
            HookFailureStrategy::FailFast,
            None,
            None,
        )?;
    }

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

    // Run post-remove hooks (using pre-built context from before deletion)
    if let Some((ref env, ref project_config)) = hook_ctx {
        let ctx = env.context(yes);
        let user_hooks = ctx.config.hooks(ctx.project_id().as_deref());
        run_hook_with_filter(
            &ctx,
            user_hooks.post_remove.as_ref(),
            project_config
                .as_ref()
                .and_then(|c| c.hooks.post_remove.as_ref()),
            HookType::PostRemove,
            &[],
            HookFailureStrategy::Warn,
            None,
            None,
        )?;

        // Run post-switch hooks when removing the current workspace
        // (we've changed directory to the default workspace)
        if removing_current {
            run_hook_with_filter(
                &ctx,
                user_hooks.post_switch.as_ref(),
                project_config
                    .as_ref()
                    .and_then(|c| c.hooks.post_switch.as_ref()),
                HookType::PostSwitch,
                &[],
                HookFailureStrategy::Warn,
                None,
                None,
            )?;
        }
    }

    Ok(())
}
