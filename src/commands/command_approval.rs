//! Command approval and execution utilities
//!
//! Shared helpers for approving commands declared in project configuration.

use crate::output;
use anyhow::Context;
use color_print::cformat;
use worktrunk::config::{Command, WorktrunkConfig};
use worktrunk::git::GitError;
use worktrunk::styling::{
    INFO_EMOJI, PROMPT_EMOJI, WARNING_EMOJI, eprint, eprintln, format_bash_with_gutter,
    hint_message, stderr, warning_message,
};

/// Batch approval helper used when multiple commands are queued for execution.
/// Returns `Ok(true)` when execution may continue, `Ok(false)` when the user
/// declined, and `Err` if config reload/save fails.
///
/// Shows expanded commands to the user. Templates are saved to config for future approval checks.
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
        // Uses INFO_EMOJI (⚪) since this is a preview, not active execution
        let phase = cmd.phase.to_string();
        let label = match &cmd.name {
            Some(name) => cformat!("{INFO_EMOJI} {phase} <bold>{name}</>:"),
            None => format!("{INFO_EMOJI} {phase}:"),
        };
        eprintln!("{label}");
        eprint!("{}", format_bash_with_gutter(&cmd.expanded, ""));
        eprintln!();
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
