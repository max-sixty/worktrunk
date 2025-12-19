//! Command approval and execution utilities
//!
//! Shared helpers for approving commands declared in project configuration.
//!
//! # "Approve at the Gate" Pattern
//!
//! Commands that run hooks should approve all project commands **upfront** before any execution:
//!
//! ```text
//! User runs command
//!     ↓
//! approve_hooks() ← Single approval prompt
//!     ↓
//! Execute hooks (approval already done)
//! ```
//!
//! This ensures approval happens exactly once at the command entry point,
//! eliminating the need to thread `auto_trust` through execution layers.

use super::project_config::collect_commands_for_hooks;
use super::repository_ext::RepositoryCliExt;
use crate::output;
use anyhow::Context;
use color_print::cformat;
use worktrunk::config::{Command, WorktrunkConfig};
use worktrunk::git::{GitError, HookType};
use worktrunk::styling::{
    INFO_EMOJI, PROMPT_EMOJI, WARNING_EMOJI, eprint, eprintln, format_bash_with_gutter,
    hint_message, stderr, warning_message,
};

/// Batch approval helper used when multiple commands are queued for execution.
/// Returns `Ok(true)` when execution may continue, `Ok(false)` when the user
/// declined, and `Err` if config reload/save fails.
///
/// Shows command templates to the user (what gets saved to config), not expanded values.
/// This ensures users see exactly what they're approving.
///
/// # Parameters
/// - `commands_already_filtered`: If true, commands list is pre-filtered; skip filtering by approval status
pub fn approve_command_batch(
    commands: &[Command],
    project_id: &str,
    config: &WorktrunkConfig,
    force: bool,
    commands_already_filtered: bool,
) -> anyhow::Result<bool> {
    let needs_approval: Vec<&Command> = commands
        .iter()
        .filter(|cmd| {
            commands_already_filtered || !config.is_command_approved(project_id, &cmd.template)
        })
        .collect();

    if needs_approval.is_empty() {
        return Ok(true);
    }

    let approved = if force {
        true
    } else {
        prompt_for_batch_approval(&needs_approval, project_id)?
    };

    if !approved {
        return Ok(false);
    }

    // Only save approvals when interactively approved, not when using --force
    if !force {
        let mut fresh_config = WorktrunkConfig::load().context("Failed to reload config")?;

        let project_entry = fresh_config
            .projects
            .entry(project_id.to_string())
            .or_default();

        let mut updated = false;
        for cmd in &needs_approval {
            if !project_entry.approved_commands.contains(&cmd.template) {
                project_entry.approved_commands.push(cmd.template.clone());
                updated = true;
            }
        }

        if updated && let Err(e) = fresh_config.save() {
            let _ = output::print(warning_message(format!(
                "Failed to save command approval: {e}"
            )));
            let _ = output::print(hint_message("Approval will be requested again next time."));
        }
    }

    Ok(true)
}

fn prompt_for_batch_approval(commands: &[&Command], project_id: &str) -> anyhow::Result<bool> {
    use std::io::{self, IsTerminal, Write};

    let project_name = project_id.split('/').next_back().unwrap_or(project_id);
    let count = commands.len();
    let plural = if count == 1 { "" } else { "s" };

    // CRITICAL: Flush stdout before writing to stderr to prevent stream interleaving
    // In directive mode, flushes both stdout (directives) and stderr (messages)
    // In interactive mode, flushes both stdout and stderr
    crate::output::flush_for_stderr_prompt()?;

    eprintln!(
        "{}",
        cformat!(
            "{WARNING_EMOJI} <yellow><bold>{project_name}</> needs approval to execute <bold>{count}</> command{plural}:</>"
        )
    );
    eprintln!();

    for cmd in commands {
        // Format as: {phase} {bold}{name}{bold:#}:
        // Phase comes from the command itself (e.g., "pre-commit", "pre-merge")
        // Uses INFO_SYMBOL (○) since this is a preview, not active execution
        let phase = cmd.phase.to_string();
        let label = match &cmd.name {
            Some(name) => cformat!("{INFO_EMOJI} {phase} <bold>{name}</>:"),
            None => format!("{INFO_EMOJI} {phase}:"),
        };
        eprintln!("{label}");
        eprint!("{}", format_bash_with_gutter(&cmd.template, ""));
    }

    // Check if stdin is a TTY before attempting to prompt
    // This happens AFTER showing the commands so they appear in CI/CD logs
    // even when the prompt cannot be displayed (fail-fast principle)
    if !io::stdin().is_terminal() {
        return Err(GitError::NotInteractive.into());
    }

    // Flush stderr before showing prompt to ensure all output is visible
    stderr().flush()?;

    eprint!(
        "{}",
        cformat!("{PROMPT_EMOJI} Allow and remember? <bold>[y/N]</> ")
    );
    stderr().flush()?;

    let mut response = String::new();
    io::stdin().read_line(&mut response)?;

    eprintln!();

    Ok(response.trim().eq_ignore_ascii_case("y"))
}

/// Collect project commands for hooks and request batch approval.
///
/// This is the "gate" function that should be called at command entry points
/// (like `wt remove`, `wt switch --create`, `wt merge`) before any hooks execute.
/// Shows command templates (not expanded values) so users see exactly what
/// patterns they're approving.
///
/// # Parameters
/// - `ctx`: Command context (provides project identifier and config)
/// - `hook_types`: Which hook types to collect commands for
///
/// # Example
///
/// ```ignore
/// let ctx = CommandContext::new(&repo, &config, &branch, &worktree_path, &repo_root, force);
/// let approved = approve_hooks(&ctx, &[HookType::PostCreate, HookType::PostStart])?;
/// ```
pub fn approve_hooks(
    ctx: &super::command_executor::CommandContext<'_>,
    hook_types: &[HookType],
) -> anyhow::Result<bool> {
    approve_hooks_filtered(ctx, hook_types, None)
}

/// Like `approve_hooks` but with optional name filter for targeted hook approval.
///
/// When `name_filter` is provided, only commands matching that name are shown
/// in the approval prompt. This is used by `wt hook <type> --name <name>` to
/// approve only the targeted hook rather than all hooks of that type.
pub fn approve_hooks_filtered(
    ctx: &super::command_executor::CommandContext<'_>,
    hook_types: &[HookType],
    name_filter: Option<&str>,
) -> anyhow::Result<bool> {
    let project_config = match ctx.repo.load_project_config()? {
        Some(cfg) => cfg,
        None => return Ok(true), // No project config = no commands to approve
    };

    let mut commands = collect_commands_for_hooks(&project_config, hook_types);

    // Apply name filter before approval to only prompt for targeted commands
    if let Some(name) = name_filter {
        commands.retain(|cmd| cmd.name.as_deref() == Some(name));
    }

    if commands.is_empty() {
        return Ok(true);
    }

    let project_id = ctx.repo.project_identifier()?;
    approve_command_batch(&commands, &project_id, ctx.config, ctx.force, false)
}
