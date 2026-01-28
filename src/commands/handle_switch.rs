//! Switch command handler.

use std::collections::HashMap;

use anyhow::Context;
use worktrunk::HookType;
use worktrunk::config::{UserConfig, expand_template};
use worktrunk::git::Repository;
use worktrunk::styling::{eprintln, info_message};

use super::command_approval::approve_hooks;
use super::command_executor::{CommandContext, build_hook_context};
use super::worktree::{SwitchResult, execute_switch, plan_switch};
use crate::output::{
    execute_user_command, handle_switch_output, is_shell_integration_active,
    prompt_shell_integration,
};

/// Options for the switch command
pub struct SwitchOptions<'a> {
    pub branch: &'a str,
    pub create: bool,
    pub base: Option<&'a str>,
    pub execute: Option<&'a str>,
    pub execute_args: &'a [String],
    pub yes: bool,
    pub clobber: bool,
    pub verify: bool,
}

/// Handle the switch command.
pub fn handle_switch(
    opts: SwitchOptions<'_>,
    config: &mut UserConfig,
    binary_name: &str,
) -> anyhow::Result<()> {
    let SwitchOptions {
        branch,
        create,
        base,
        execute,
        execute_args,
        yes,
        clobber,
        verify,
    } = opts;

    let repo = Repository::current().context("Failed to switch worktree")?;

    // Validate FIRST (before approval) - fails fast if branch doesn't exist, etc.
    let plan = plan_switch(&repo, branch, create, base, clobber, config)?;

    // "Approve at the Gate": collect and approve hooks upfront
    // This ensures approval happens once at the command entry point
    // If user declines, skip hooks but continue with worktree operation
    let approved = if verify {
        let ctx = CommandContext::new(
            &repo,
            config,
            Some(plan.branch()),
            plan.worktree_path(),
            yes,
        );
        // Approve different hooks based on whether we're creating or switching
        if plan.is_create() {
            approve_hooks(
                &ctx,
                &[
                    HookType::PostCreate,
                    HookType::PostStart,
                    HookType::PostSwitch,
                ],
            )?
        } else {
            // When switching to existing, only post-switch needs approval
            approve_hooks(&ctx, &[HookType::PostSwitch])?
        }
    } else {
        true // --no-verify: skip all hooks
    };

    // Skip hooks if --no-verify or user declined approval
    let skip_hooks = !verify || !approved;

    // Show message if user declined approval
    if !approved {
        eprintln!(
            "{}",
            info_message(if plan.is_create() {
                "Commands declined, continuing worktree creation"
            } else {
                "Commands declined"
            })
        );
    }

    // Execute the validated plan
    let (result, branch_info) = execute_switch(&repo, plan, config, yes, skip_hooks)?;

    // Show success message (temporal locality: immediately after worktree operation)
    // Returns path to display in hooks when user's shell won't be in the worktree
    // Also shows worktree-path hint on first --create (before shell integration warning)
    let hooks_display_path = handle_switch_output(&result, &branch_info)?;

    // Offer shell integration if not already installed/active
    // (only shows prompt/hint when shell integration isn't working)
    // With --execute: show hints only (don't interrupt with prompt)
    // Best-effort: don't fail switch if offer fails
    if !is_shell_integration_active() {
        let skip_prompt = execute.is_some();
        let _ = prompt_shell_integration(config, binary_name, skip_prompt);
    }

    // Build extra vars for base branch context (used by both hooks and --execute)
    // "base" is the branch we branched from when creating a new worktree.
    // For existing worktrees, there's no base concept.
    let (base_branch, base_worktree_path): (Option<&str>, Option<&str>) = match &result {
        SwitchResult::Created {
            base_branch,
            base_worktree_path,
            ..
        } => (base_branch.as_deref(), base_worktree_path.as_deref()),
        SwitchResult::Existing { .. } | SwitchResult::AlreadyAt(_) => (None, None),
    };
    let extra_vars: Vec<(&str, &str)> = [
        base_branch.map(|b| ("base", b)),
        base_worktree_path.map(|p| ("base_worktree_path", p)),
    ]
    .into_iter()
    .flatten()
    .collect();

    // Spawn background hooks after success message
    // - post-switch: runs on ALL switches (shows "@ path" when shell won't be there)
    // - post-start: runs only when creating a NEW worktree
    if !skip_hooks {
        let ctx = CommandContext::new(&repo, config, Some(&branch_info.branch), result.path(), yes);

        // Post-switch runs first (immediate "I'm here" signal)
        ctx.spawn_post_switch_commands(&extra_vars, hooks_display_path.as_deref())?;

        // Post-start runs only on creation (setup tasks)
        if matches!(&result, SwitchResult::Created { .. }) {
            ctx.spawn_post_start_commands(&extra_vars, hooks_display_path.as_deref())?;
        }
    }

    // Execute user command after post-start hooks have been spawned
    // Note: execute_args requires execute via clap's `requires` attribute
    if let Some(cmd) = execute {
        // Build template context for expansion (includes base vars when creating)
        let ctx = CommandContext::new(&repo, config, Some(&branch_info.branch), result.path(), yes);
        let template_vars = build_hook_context(&ctx, &extra_vars);
        let vars: HashMap<&str, &str> = template_vars
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        // Expand template variables in command (shell_escape: true for safety)
        let expanded_cmd = expand_template(cmd, &vars, true, &repo, "--execute command")
            .map_err(|e| anyhow::anyhow!("Failed to expand --execute template: {}", e))?;

        // Append any trailing args (after --) to the execute command
        // Each arg is also expanded, then shell-escaped
        let full_cmd = if execute_args.is_empty() {
            expanded_cmd
        } else {
            let expanded_args: Result<Vec<_>, _> = execute_args
                .iter()
                .map(|arg| {
                    expand_template(arg, &vars, false, &repo, "--execute argument")
                        .map_err(|e| anyhow::anyhow!("Failed to expand argument template: {}", e))
                })
                .collect();
            let escaped_args: Vec<_> = expanded_args?
                .iter()
                .map(|arg| shlex::try_quote(arg).unwrap_or(arg.into()).into_owned())
                .collect();
            format!("{} {}", expanded_cmd, escaped_args.join(" "))
        };
        execute_user_command(&full_cmd, hooks_display_path.as_deref())?;
    }

    Ok(())
}
