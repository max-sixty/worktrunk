//! Command approval and execution utilities
//!
//! Shared helpers for approving commands declared in project configuration.

use worktrunk::config::{Command, WorktrunkConfig};
use worktrunk::git::{GitError, GitResultExt};
use worktrunk::styling::{
    AnstyleStyle, HINT_EMOJI, WARNING, WARNING_EMOJI, eprint, format_bash_with_gutter, print,
    println, stderr,
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
    context: &str,
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
        prompt_for_batch_approval(&needs_approval, project_id, context)?
    };

    if !approved {
        let dim = AnstyleStyle::new().dimmed();
        println!("{dim}{context} declined{dim:#}");
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
            println!("You will be prompted again next time.");
        }
    }

    Ok(true)
}

fn log_approval_warning(message: &str, error: impl std::fmt::Display) {
    println!("{WARNING_EMOJI} {WARNING}{message}: {error}{WARNING:#}");
}

fn prompt_for_batch_approval(
    commands: &[&Command],
    project_id: &str,
    context: &str,
) -> std::io::Result<bool> {
    use std::io::{self, Write};

    let project_name = project_id.split('/').next_back().unwrap_or(project_id);
    let bold = AnstyleStyle::new().bold();
    let dim = AnstyleStyle::new().dimmed();
    let warning_bold = WARNING.bold();
    let count = commands.len();
    let plural = if count == 1 { "" } else { "s" };

    println!();
    println!(
        "{WARNING_EMOJI} {WARNING}Permission required to execute {warning_bold}{count}{warning_bold:#} command{plural}{WARNING:#}",
    );
    println!();
    println!("{bold}{project_name}{bold:#} ({dim}{project_id}{dim:#}) wants to execute:");
    println!();

    for cmd in commands {
        // Format as: {context} {bold}{name}{bold:#}:
        // context is provided by caller in lowercase (e.g., "post-create", "pre-merge")
        let label = match &cmd.name {
            Some(name) => format!("{context} {bold}{name}{bold:#}:"),
            None => format!("{context}:"),
        };
        println!("{label}");
        print!("{}", format_bash_with_gutter(&cmd.expanded, ""));
        println!();
    }

    eprint!("{HINT_EMOJI} Allow and remember? {bold}[y/N]{bold:#} ");
    stderr().flush()?;

    let mut response = String::new();
    io::stdin().read_line(&mut response)?;

    println!();

    Ok(response.trim().eq_ignore_ascii_case("y"))
}
