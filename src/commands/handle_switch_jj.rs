//! Switch command handler for jj repositories.
//!
//! Supports the same hook lifecycle as the git path: approval, post-create
//! (blocking), post-start/post-switch (background), `--execute`, switch-previous
//! tracking, and shell integration prompts.

use std::path::PathBuf;

use color_print::cformat;
use normalize_path::NormalizePath;
use worktrunk::HookType;
use worktrunk::config::{UserConfig, sanitize_branch_name};
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{eprintln, info_message, success_message};
use worktrunk::workspace::{JjWorkspace, Workspace};

use super::command_approval::approve_hooks;
use super::command_executor::CommandContext;
use super::handle_switch::SwitchOptions;
use crate::output;
use crate::output::{is_shell_integration_active, prompt_shell_integration};

/// Handle `wt switch` for jj repositories.
pub fn handle_switch_jj(
    opts: SwitchOptions<'_>,
    config: &mut UserConfig,
    binary_name: &str,
) -> anyhow::Result<()> {
    let workspace = JjWorkspace::from_current_dir()?;

    // Resolve `wt switch -` to the previous workspace
    let resolved_name;
    let name = if opts.branch == "-" {
        resolved_name = workspace
            .switch_previous()
            .ok_or_else(|| anyhow::anyhow!("No previous workspace to switch to"))?;
        &resolved_name
    } else {
        opts.branch
    };

    // Check if workspace already exists
    let existing_path = find_existing_workspace(&workspace, name)?;

    if let Some(path) = existing_path {
        return handle_existing_switch(&workspace, name, &path, &opts, config, binary_name);
    }

    // Workspace doesn't exist — need --create to make one
    if !opts.create {
        anyhow::bail!("Workspace '{}' not found. Use --create to create it.", name);
    }

    handle_create_switch(&workspace, name, &opts, config, binary_name)
}

/// Switch to an existing workspace.
fn handle_existing_switch(
    workspace: &JjWorkspace,
    name: &str,
    path: &std::path::Path,
    opts: &SwitchOptions<'_>,
    config: &mut UserConfig,
    binary_name: &str,
) -> anyhow::Result<()> {
    // Approve post-switch hooks upfront
    let skip_hooks = if opts.verify {
        let ctx = CommandContext::new(workspace, config, Some(name), path, opts.yes);
        let approved = approve_hooks(&ctx, &[HookType::PostSwitch])?;
        if !approved {
            eprintln!("{}", info_message("Commands declined"));
        }
        !approved
    } else {
        true
    };

    // Track switch-previous before switching
    record_switch_previous(workspace);

    // Show success message
    let path_display = format_path_for_display(path);
    eprintln!(
        "{}",
        success_message(cformat!(
            "Switched to workspace <bold>{name}</> @ <bold>{path_display}</>"
        ))
    );

    if opts.change_dir {
        output::change_directory(path)?;
    }

    // Shell integration prompt
    if !is_shell_integration_active() {
        let skip_prompt = opts.execute.is_some();
        let _ = prompt_shell_integration(config, binary_name, skip_prompt);
    }

    let hooks_display_path = output::post_hook_display_path(path).map(|p| p.to_path_buf());

    // Background hooks (post-switch only for existing)
    if !skip_hooks {
        let ctx = CommandContext::new(workspace, config, Some(name), path, opts.yes);
        let hooks = super::hooks::prepare_background_hooks(
            &ctx,
            HookType::PostSwitch,
            &[],
            hooks_display_path.as_deref(),
        )?;
        super::hooks::spawn_background_hooks(&ctx, hooks)?;
    }

    // Execute user command (--execute)
    if let Some(cmd) = opts.execute {
        let ctx = CommandContext::new(workspace, config, Some(name), path, opts.yes);
        super::handle_switch::expand_and_execute_command(
            &ctx,
            cmd,
            opts.execute_args,
            &[],
            hooks_display_path.as_deref(),
        )?;
    }

    Ok(())
}

/// Create a new workspace and switch to it.
fn handle_create_switch(
    workspace: &JjWorkspace,
    name: &str,
    opts: &SwitchOptions<'_>,
    config: &mut UserConfig,
    binary_name: &str,
) -> anyhow::Result<()> {
    // Compute path for new workspace
    let worktree_path = compute_jj_workspace_path(workspace, name)?;

    if worktree_path.exists() {
        anyhow::bail!(
            "Path already exists: {}",
            format_path_for_display(&worktree_path)
        );
    }

    // Approve hooks upfront (post-create, post-start, post-switch)
    let skip_hooks = if opts.verify {
        let ctx = CommandContext::new(workspace, config, Some(name), &worktree_path, opts.yes);
        let approved = approve_hooks(
            &ctx,
            &[
                HookType::PostCreate,
                HookType::PostStart,
                HookType::PostSwitch,
            ],
        )?;
        if !approved {
            eprintln!(
                "{}",
                info_message("Commands declined, continuing workspace creation")
            );
        }
        !approved
    } else {
        true
    };

    // Track switch-previous before creating
    record_switch_previous(workspace);

    // Create the workspace
    workspace.create_workspace(name, opts.base, &worktree_path)?;

    // Run post-create hooks (blocking) before success message
    let base_str = opts.base.unwrap_or_default().to_string();
    let extra_vars: Vec<(&str, &str)> = if opts.base.is_some() {
        vec![("base", &base_str)]
    } else {
        Vec::new()
    };

    if !skip_hooks {
        let ctx = CommandContext::new(workspace, config, Some(name), &worktree_path, opts.yes);
        ctx.execute_post_create_commands(&extra_vars)?;
    }

    // Show success message
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

    // Shell integration prompt
    if !is_shell_integration_active() {
        let skip_prompt = opts.execute.is_some();
        let _ = prompt_shell_integration(config, binary_name, skip_prompt);
    }

    let hooks_display_path =
        output::post_hook_display_path(&worktree_path).map(|p| p.to_path_buf());

    // Background hooks (post-switch + post-start for creates)
    if !skip_hooks {
        let ctx = CommandContext::new(workspace, config, Some(name), &worktree_path, opts.yes);
        let mut hooks = super::hooks::prepare_background_hooks(
            &ctx,
            HookType::PostSwitch,
            &extra_vars,
            hooks_display_path.as_deref(),
        )?;
        hooks.extend(super::hooks::prepare_background_hooks(
            &ctx,
            HookType::PostStart,
            &extra_vars,
            hooks_display_path.as_deref(),
        )?);
        super::hooks::spawn_background_hooks(&ctx, hooks)?;
    }

    // Execute user command (--execute)
    if let Some(cmd) = opts.execute {
        let ctx = CommandContext::new(workspace, config, Some(name), &worktree_path, opts.yes);
        super::handle_switch::expand_and_execute_command(
            &ctx,
            cmd,
            opts.execute_args,
            &extra_vars,
            hooks_display_path.as_deref(),
        )?;
    }

    Ok(())
}

/// Record the current workspace name as switch-previous.
fn record_switch_previous(workspace: &JjWorkspace) {
    if let Ok(current_path) = workspace.current_workspace_path()
        && let Ok(Some(current)) = workspace.current_name(&current_path)
    {
        let _ = workspace.set_switch_previous(Some(&current));
    }
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
