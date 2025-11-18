//! Command approval and execution utilities
//!
//! Shared helpers for approving commands declared in project configuration.

use worktrunk::config::{Command, WorktrunkConfig};
use worktrunk::git::{GitError, GitResultExt};
use worktrunk::styling::{
    AnstyleStyle, HINT, HINT_EMOJI, PROGRESS_EMOJI, WARNING, WARNING_BOLD, WARNING_EMOJI, eprint,
    eprintln, format_bash_with_gutter, println, stderr,
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
) -> Result<bool, GitError> {
    let needs_approval: Vec<&Command> = if commands_already_filtered {
        // Commands already filtered by caller, use as-is
        commands.iter().collect()
    } else {
        // Filter to only unapproved commands
        commands
            .iter()
            .filter(|cmd| !config.is_command_approved(project_id, &cmd.template))
            .collect()
    };

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
        let mut fresh_config = WorktrunkConfig::load().git_context("Failed to reload config")?;

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
            println!("{WARNING_EMOJI} {WARNING}Failed to save command approval: {e}{WARNING:#}");
            println!("{HINT_EMOJI} {HINT}You will be prompted again next time.{HINT:#}");
        }
    }

    Ok(true)
}

fn prompt_for_batch_approval(commands: &[&Command], project_id: &str) -> Result<bool, GitError> {
    use std::io::{self, IsTerminal, Write};

    let project_name = project_id.split('/').next_back().unwrap_or(project_id);
    let bold = AnstyleStyle::new().bold();
    let count = commands.len();
    let plural = if count == 1 { "" } else { "s" };

    // CRITICAL: Flush stdout before writing to stderr to prevent stream interleaving
    // In directive mode, flushes both stdout (directives) and stderr (messages)
    // In interactive mode, flushes both stdout and stderr
    crate::output::flush_for_stderr_prompt()?;

    eprintln!(
        "{WARNING_EMOJI} {WARNING}{WARNING_BOLD}{project_name}{WARNING_BOLD:#}{WARNING} wants to execute {WARNING_BOLD}{count}{WARNING_BOLD:#}{WARNING} command{plural}:{WARNING:#}"
    );
    eprintln!();

    for cmd in commands {
        // Format as: {phase} {bold}{name}{bold:#}:
        // Phase comes from the command itself (e.g., "pre-commit", "pre-merge")
        let phase = cmd.phase.to_string();
        let label = match &cmd.name {
            Some(name) => format!("{PROGRESS_EMOJI} {phase} {bold}{name}{bold:#}:"),
            None => format!("{PROGRESS_EMOJI} {phase}:"),
        };
        eprintln!("{label}");
        eprint!("{}", format_bash_with_gutter(&cmd.expanded, ""));
        eprintln!();
    }

    // Check if stdin is a TTY before attempting to prompt
    // This happens AFTER showing the commands so they appear in CI/CD logs
    // even when the prompt cannot be displayed (fail-fast principle)
    if !io::stdin().is_terminal() {
        return Err(GitError::NotInteractive);
    }

    // Flush stderr before showing prompt to ensure all output is visible
    stderr().flush()?;

    eprint!("{HINT_EMOJI} Allow and remember? {bold}[y/N]{bold:#} ");
    stderr().flush()?;

    let mut response = String::new();
    io::stdin().read_line(&mut response)?;

    eprintln!();

    Ok(response.trim().eq_ignore_ascii_case("y"))
}
