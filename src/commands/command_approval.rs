//! Command approval and execution utilities
//!
//! Shared helpers for approving commands declared in project configuration.

use worktrunk::config::{Command, WorktrunkConfig};
use worktrunk::git::{GitError, GitResultExt};
use worktrunk::styling::{
    AnstyleStyle, HINT, HINT_EMOJI, INFO_EMOJI, PROGRESS_EMOJI, WARNING, WARNING_EMOJI, eprint,
    eprintln, format_bash_with_gutter, println, stderr,
};

/// Batch approval helper used when multiple commands are queued for execution.
/// Returns `Ok(true)` when execution may continue, `Ok(false)` when the user
/// declined, and `Err` if config reload/save fails.
///
/// Shows expanded commands to the user. Templates are saved to config for future approval checks.
pub fn approve_command_batch(
    commands: &[Command],
    project_id: &str,
    config: &WorktrunkConfig,
    force: bool,
) -> Result<bool, GitError> {
    let needs_approval: Vec<&Command> = commands
        .iter()
        .filter(|cmd| !config.is_command_approved(project_id, &cmd.template))
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
        let dim = AnstyleStyle::new().dimmed();
        // Derive phase from commands - if all same phase, show it; otherwise show generic message
        let phase_str = if commands.len() == 1 {
            commands[0].phase.to_string()
        } else {
            let first_phase = commands[0].phase;
            if commands.iter().all(|cmd| cmd.phase == first_phase) {
                first_phase.to_string()
            } else {
                "commands".to_string()
            }
        };
        println!("{INFO_EMOJI} {dim}{phase_str} declined{dim:#}");
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
            log_approval_warning("Failed to save command approval", e);
            println!("{HINT_EMOJI} {HINT}You will be prompted again next time.{HINT:#}");
        }
    }

    Ok(true)
}

fn log_approval_warning(message: &str, error: impl std::fmt::Display) {
    println!("{WARNING_EMOJI} {WARNING}{message}: {error}{WARNING:#}");
}

fn prompt_for_batch_approval(commands: &[&Command], project_id: &str) -> std::io::Result<bool> {
    use std::io::{self, Write};

    let project_name = project_id.split('/').next_back().unwrap_or(project_id);
    let bold = AnstyleStyle::new().bold();
    let warning_bold = WARNING.bold();
    let count = commands.len();
    let plural = if count == 1 { "" } else { "s" };

    eprintln!();
    eprintln!(
        "{WARNING_EMOJI} {WARNING}{warning_bold}{project_name}{warning_bold:#}{WARNING:#} ({project_id}) {WARNING}wants to execute {warning_bold}{count}{warning_bold:#} command{plural}:{WARNING:#}"
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

    // Flush stderr before showing prompt to ensure all output is visible
    stderr().flush()?;

    eprint!("{HINT_EMOJI} Allow and remember? {bold}[y/N]{bold:#} ");
    stderr().flush()?;

    let mut response = String::new();
    io::stdin().read_line(&mut response)?;

    eprintln!();

    Ok(response.trim().eq_ignore_ascii_case("y"))
}
