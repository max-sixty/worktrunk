//! Remove helper for jj repositories.
//!
//! Simpler than git removal: no branch deletion, no merge status checks.
//! Just forget the workspace and remove the directory.

use std::path::Path;

use color_print::cformat;
use worktrunk::HookType;
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{eprintln, success_message, warning_message};
use worktrunk::workspace::Workspace;
use worktrunk::workspace::types::IntegrationReason;

use super::context::CommandEnv;
use super::hooks::{HookFailureStrategy, run_hook_with_filter};
use crate::output;

/// Integration context for display in the removal success message.
pub struct IntegrationInfo<'a> {
    pub reason: IntegrationReason,
    pub target: &'a str,
}

/// Forget a jj workspace, remove its directory, and cd to default if needed.
///
/// When `integration` is provided (e.g., from `wt merge`), the success message
/// includes the integration reason, matching git's removal output style.
///
/// Shared between `wt remove` and `wt merge` for jj repositories.
pub fn remove_jj_workspace_and_cd(
    workspace: &dyn Workspace,
    name: &str,
    ws_path: &Path,
    run_hooks: bool,
    yes: bool,
    integration: Option<IntegrationInfo<'_>>,
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
    let integration_note = match &integration {
        Some(info) => {
            let desc = info.reason.description();
            let target = info.target;
            let symbol = info.reason.symbol();
            cformat!(" ({desc} <bold>{target}</>, <dim>{symbol}</>)")
        }
        None => String::new(),
    };
    eprintln!(
        "{}",
        success_message(cformat!(
            "Removed workspace <bold>{name}</> @ <bold>{path_display}</>{integration_note}"
        ))
    );

    // If removing current workspace, cd to default workspace
    if removing_current {
        let default_path = match workspace.default_workspace_path()? {
            Some(p) => p,
            None => workspace.root_path()?,
        };
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
