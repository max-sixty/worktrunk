//! Command approval and execution utilities
//!
//! Shared helpers for approving commands declared in project configuration.

use worktrunk::config::WorktrunkConfig;
use worktrunk::git::{GitError, GitResultExt};
use worktrunk::styling::{
    AnstyleStyle, HINT_EMOJI, WARNING, WARNING_EMOJI, eprint, eprintln, format_bash_with_gutter,
};

/// Batch approval helper used when multiple commands are queued for execution.
/// Returns `Ok(true)` when execution may continue, `Ok(false)` when the user
/// declined, and `Err` if config reload/save fails.
pub fn approve_command_batch(
    commands: &[(Option<String>, String)],
    project_id: &str,
    config: &WorktrunkConfig,
    force: bool,
    context: &str,
) -> Result<bool, GitError> {
    let needs_approval: Vec<(&Option<String>, &String)> = commands
        .iter()
        .filter(|(_, command)| !config.is_command_approved(project_id, command))
        .map(|(name, command)| (name, command))
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
        eprintln!("{dim}{context} declined{dim:#}");
        return Ok(false);
    }

    // Only save approvals when interactively approved, not when using --force
    if !force {
        let mut fresh_config = WorktrunkConfig::load().git_context("Failed to reload config")?;

        // Get or create the project entry once, outside the loop
        let project_entry = fresh_config
            .projects
            .entry(project_id.to_string())
            .or_default();

        let mut updated = false;
        for (_, command) in needs_approval {
            if !project_entry.approved_commands.iter().any(|c| c == command) {
                project_entry.approved_commands.push(command.to_string());
                updated = true;
            }
        }

        if updated && let Err(e) = fresh_config.save() {
            log_approval_warning("Failed to save command approval", e);
            eprintln!("You will be prompted again next time.");
        }
    }

    Ok(true)
}

fn log_approval_warning(message: &str, error: impl std::fmt::Display) {
    eprintln!("{WARNING_EMOJI} {WARNING}{message}: {error}{WARNING:#}");
}

fn prompt_for_batch_approval(
    commands: &[(&Option<String>, &String)],
    project_id: &str,
) -> std::io::Result<bool> {
    use std::io::{self, Write};

    let project_name = project_id.split('/').next_back().unwrap_or(project_id);
    let bold = AnstyleStyle::new().bold();
    let dim = AnstyleStyle::new().dimmed();
    let warning_bold = WARNING.bold();
    let count = commands.len();
    let plural = if count == 1 { "" } else { "s" };

    eprintln!();
    eprintln!(
        "{WARNING_EMOJI} {WARNING}Permission required to execute {warning_bold}{count}{warning_bold:#} command{plural}{WARNING:#}",
    );
    eprintln!();
    eprintln!("{bold}{project_name}{bold:#} ({dim}{project_id}{dim:#}) wants to execute:");
    eprintln!();

    for (name, command) in commands {
        let label = match name {
            Some(n) => format!("{n}: {command}"),
            None => (*command).clone(),
        };
        eprint!("{}", format_bash_with_gutter(&label, ""));
    }

    eprintln!();
    eprint!("{HINT_EMOJI} Allow and remember? {bold}[y/N]{bold:#} ");
    io::stderr().flush()?;

    let mut response = String::new();
    io::stdin().read_line(&mut response)?;

    // Add newline after user input to prevent output interleaving.
    // Without this, success messages (stdout) can appear on the same line as the prompt (stderr)
    // when the user types 'y' + Enter, since the Enter only adds a newline to stderr.
    eprintln!();

    Ok(response.trim().eq_ignore_ascii_case("y"))
}
