//! Switch command handler.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use worktrunk::HookType;
use worktrunk::config::{UserConfig, expand_template};
use worktrunk::git::{GitError, Repository, SwitchSuggestionCtx};
use worktrunk::styling::{eprintln, info_message};

use super::command_approval::approve_hooks;
use super::command_executor::{CommandContext, build_hook_context};
use super::worktree::{
    SwitchBranchInfo, SwitchPlan, SwitchResult, execute_switch, get_path_mismatch, plan_switch,
};
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
    /// Whether to change directory after switching (default: true)
    pub change_dir: bool,
    pub verify: bool,
}

/// Approve switch hooks upfront and show "Commands declined" if needed.
///
/// Returns `true` if hooks are approved to run.
/// Returns `false` if hooks should be skipped (`!verify` or user declined).
pub(crate) fn approve_switch_hooks(
    repo: &Repository,
    config: &UserConfig,
    plan: &SwitchPlan,
    yes: bool,
    verify: bool,
) -> anyhow::Result<bool> {
    if !verify {
        return Ok(false);
    }

    let ctx = CommandContext::new(repo, config, Some(plan.branch()), plan.worktree_path(), yes);
    let approved = if plan.is_create() {
        approve_hooks(
            &ctx,
            &[
                HookType::PostCreate,
                HookType::PostStart,
                HookType::PostSwitch,
            ],
        )?
    } else {
        approve_hooks(&ctx, &[HookType::PostSwitch])?
    };

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

    Ok(approved)
}

/// Compute extra template variables from a switch result.
///
/// Returns base branch context (`base`, `base_worktree_path`) for hooks and template expansion.
pub(crate) fn switch_extra_vars(result: &SwitchResult) -> Vec<(&str, &str)> {
    match result {
        SwitchResult::Created {
            base_branch,
            base_worktree_path,
            ..
        } => [
            base_branch.as_deref().map(|b| ("base", b)),
            base_worktree_path
                .as_deref()
                .map(|p| ("base_worktree_path", p)),
        ]
        .into_iter()
        .flatten()
        .collect(),
        SwitchResult::Existing { .. } | SwitchResult::AlreadyAt(_) => Vec::new(),
    }
}

/// Spawn post-switch (and post-start for creates) background hooks.
pub(crate) fn spawn_switch_background_hooks(
    repo: &Repository,
    config: &UserConfig,
    result: &SwitchResult,
    branch: &str,
    yes: bool,
    extra_vars: &[(&str, &str)],
    hooks_display_path: Option<&Path>,
) -> anyhow::Result<()> {
    let ctx = CommandContext::new(repo, config, Some(branch), result.path(), yes);

    let mut hooks = super::hooks::prepare_background_hooks(
        &ctx,
        HookType::PostSwitch,
        extra_vars,
        hooks_display_path,
    )?;
    if matches!(result, SwitchResult::Created { .. }) {
        hooks.extend(super::hooks::prepare_background_hooks(
            &ctx,
            HookType::PostStart,
            extra_vars,
            hooks_display_path,
        )?);
    }
    super::hooks::spawn_background_hooks(&ctx, hooks)
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
        change_dir,
        verify,
    } = opts;

    let repo = Repository::current().context("Failed to switch worktree")?;

    // Build switch suggestion context for enriching error hints with --execute/trailing args.
    // Without this, errors like "branch already exists" would suggest `wt switch <branch>`
    // instead of the full `wt switch <branch> --execute=<cmd> -- <args>`.
    let suggestion_ctx = execute.map(|exec| {
        let escaped = shlex::try_quote(exec).unwrap_or(exec.into());
        SwitchSuggestionCtx {
            extra_flags: vec![format!("--execute={escaped}")],
            trailing_args: execute_args.to_vec(),
        }
    });

    // Validate FIRST (before approval) - fails fast if branch doesn't exist, etc.
    let plan = plan_switch(&repo, branch, create, base, clobber, config).map_err(|err| {
        match suggestion_ctx {
            Some(ref ctx) => match err.downcast::<GitError>() {
                Ok(git_err) => GitError::WithSwitchSuggestion {
                    source: Box::new(git_err),
                    ctx: ctx.clone(),
                }
                .into(),
                Err(err) => err,
            },
            None => err,
        }
    })?;

    // "Approve at the Gate": collect and approve hooks upfront
    // This ensures approval happens once at the command entry point
    // If user declines, skip hooks but continue with worktree operation
    let skip_hooks = !approve_switch_hooks(&repo, config, &plan, yes, verify)?;

    // Execute the validated plan
    let (result, branch_info) = execute_switch(&repo, plan, config, yes, skip_hooks)?;

    // Early exit for benchmarking time-to-first-output
    if std::env::var_os("WORKTRUNK_FIRST_OUTPUT").is_some() {
        return Ok(());
    }

    // Compute path mismatch lazily (deferred from plan_switch for existing worktrees)
    let branch_info = match &result {
        SwitchResult::Existing { path } | SwitchResult::AlreadyAt(path) => {
            let expected_path = get_path_mismatch(&repo, &branch_info.branch, path, config);
            SwitchBranchInfo {
                expected_path,
                ..branch_info
            }
        }
        _ => branch_info,
    };

    // Show success message (temporal locality: immediately after worktree operation)
    // Returns path to display in hooks when user's shell won't be in the worktree
    // Also shows worktree-path hint on first --create (before shell integration warning)
    let cwd = std::env::current_dir().context("Failed to get current directory")?;
    let source_root = repo.current_worktree().root()?;
    let hooks_display_path =
        handle_switch_output(&result, &branch_info, change_dir, Some(&source_root), &cwd)?;

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
    let extra_vars = switch_extra_vars(&result);

    // Spawn background hooks after success message
    // - post-switch: runs on ALL switches (shows "@ path" when shell won't be there)
    // - post-start: runs only when creating a NEW worktree
    // Batch hooks into a single message when both types are present
    if !skip_hooks {
        spawn_switch_background_hooks(
            &repo,
            config,
            &result,
            &branch_info.branch,
            yes,
            &extra_vars,
            hooks_display_path.as_deref(),
        )?;
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
        let expanded_cmd = expand_template(cmd, &vars, true, &repo, "--execute command")?;

        // Append any trailing args (after --) to the execute command
        // Each arg is also expanded, then shell-escaped
        let full_cmd = if execute_args.is_empty() {
            expanded_cmd
        } else {
            let expanded_args: Result<Vec<_>, _> = execute_args
                .iter()
                .map(|arg| expand_template(arg, &vars, false, &repo, "--execute argument"))
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
