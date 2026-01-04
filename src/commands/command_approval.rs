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

use super::hooks::{HookSource, ParsedFilter};
use super::project_config::{HookCommand, collect_commands_for_hooks};
use crate::output;
use anyhow::Context;
use color_print::cformat;
use worktrunk::config::WorktrunkConfig;
use worktrunk::git::{GitError, HookType};
use worktrunk::styling::{
    INFO_SYMBOL, PROMPT_SYMBOL, WARNING_SYMBOL, eprint, format_bash_with_gutter, hint_message,
    stderr, warning_message,
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
    commands: &[HookCommand],
    project_id: &str,
    config: &WorktrunkConfig,
    yes: bool,
    commands_already_filtered: bool,
) -> anyhow::Result<bool> {
    let needs_approval: Vec<&HookCommand> = commands
        .iter()
        .filter(|cmd| {
            commands_already_filtered
                || !config.is_command_approved(project_id, &cmd.command.template)
        })
        .collect();

    if needs_approval.is_empty() {
        return Ok(true);
    }

    let approved = if yes {
        true
    } else {
        prompt_for_batch_approval(&needs_approval, project_id)?
    };

    if !approved {
        return Ok(false);
    }

    // Only save approvals when interactively approved, not when using --yes
    if !yes {
        let mut fresh_config = WorktrunkConfig::load().context("Failed to reload config")?;

        let project_entry = fresh_config
            .projects
            .entry(project_id.to_string())
            .or_default();

        let mut updated = false;
        for cmd in &needs_approval {
            if !project_entry
                .approved_commands
                .contains(&cmd.command.template)
            {
                project_entry
                    .approved_commands
                    .push(cmd.command.template.clone());
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

fn prompt_for_batch_approval(commands: &[&HookCommand], project_id: &str) -> anyhow::Result<bool> {
    use std::io::{self, IsTerminal, Write};

    let project_name = project_id.split('/').next_back().unwrap_or(project_id);
    let count = commands.len();
    let plural = if count == 1 { "" } else { "s" };

    // CRITICAL: Flush stdout before writing to stderr to prevent stream interleaving
    // Flushes both stdout (for data output) and stderr (for messages)
    crate::output::flush()?;

    output::print(cformat!(
        "{WARNING_SYMBOL} <yellow><bold>{project_name}</> needs approval to execute <bold>{count}</> command{plural}:</>"
    ))?;
    output::blank()?;

    for cmd in commands {
        // Format as: {phase} {bold}{name}{bold:#}:
        // Phase comes from the hook type (e.g., "pre-commit", "pre-merge")
        // Uses INFO_SYMBOL (○) since this is a preview, not active execution
        let phase = cmd.hook_type.to_string();
        let label = match &cmd.command.name {
            Some(name) => cformat!("{INFO_SYMBOL} {phase} <bold>{name}</>:"),
            None => format!("{INFO_SYMBOL} {phase}:"),
        };
        output::print(label)?;
        output::print(format_bash_with_gutter(&cmd.command.template))?;
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
        cformat!("{PROMPT_SYMBOL} Allow and remember? <bold>[y/N]</> ")
    );
    stderr().flush()?;

    let mut response = String::new();
    io::stdin().read_line(&mut response)?;

    output::blank()?;

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
/// let ctx = CommandContext::new(&repo, &config, &branch, &worktree_path, &repo_root, yes);
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
///
/// Supports filter syntax:
/// - `"foo"` — approves commands named "foo" from project config
/// - `"project:foo"` — approves commands named "foo" from project config
/// - `"project:"` — approves all commands from project config
/// - `"user:"` or `"user:foo"` — skips approval (user hooks don't need approval)
pub fn approve_hooks_filtered(
    ctx: &super::command_executor::CommandContext<'_>,
    hook_types: &[HookType],
    name_filter: Option<&str>,
) -> anyhow::Result<bool> {
    // Parse filter to understand source and name separately
    // Uses the same ParsedFilter as hooks.rs for consistent behavior
    let parsed = name_filter.map(ParsedFilter::parse);

    // If filter explicitly targets user hooks only, skip project approval entirely
    if let Some(ref f) = parsed
        && f.source == Some(HookSource::User)
    {
        return Ok(true);
    }

    let project_config = match ctx.repo.load_project_config()? {
        Some(cfg) => cfg,
        None => return Ok(true), // No project config = no commands to approve
    };

    let mut commands = collect_commands_for_hooks(&project_config, hook_types);

    // Apply name filter before approval to only prompt for targeted commands
    // Use the parsed name (not raw filter) for matching
    // Empty name (e.g., "project:") means match all project commands - no filtering
    if let Some(ref f) = parsed
        && !f.name.is_empty()
    {
        commands.retain(|cmd| cmd.command.name.as_deref() == Some(f.name));
    }

    if commands.is_empty() {
        return Ok(true);
    }

    let project_id = ctx.repo.project_identifier()?;
    approve_command_batch(&commands, project_id, ctx.config, ctx.yes, false)
}
