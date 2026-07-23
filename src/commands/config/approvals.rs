//! Approvals commands for `wt config approvals` subcommand.
//!
//! - `list_approvals` - Show approval status for all project commands
//! - `add_approvals` - Approve all project commands (hooks and aliases)
//! - `clear_approvals` - Clear approved commands

use std::fmt::Write;

use anyhow::Context;
use color_print::cformat;
use strum::IntoEnumIterator;
use worktrunk::HookType;
use worktrunk::config::{Approvals, ProjectConfig, require_approvals_path};
use worktrunk::git::{GitError, Repository};
use worktrunk::styling::{
    INFO_SYMBOL, PROMPT_SYMBOL, eprintln, format_bash_with_gutter, format_heading, hint_message,
    info_message, success_message, warning_message,
};

use crate::commands::command_approval::approve_command_batch;
use crate::commands::project_config::{
    ApprovableCommand, collect_commands_for_aliases, collect_commands_for_hooks,
};

/// Every approvable command a project config declares: hooks in lifecycle
/// order, then aliases (alphabetical), then any commit-message guidance.
/// The shared collection behind `wt config approvals {list,add}`.
fn collect_approvable_commands(project_config: &ProjectConfig) -> Vec<ApprovableCommand> {
    let all_hooks: Vec<_> = HookType::iter().collect();
    let mut commands = collect_commands_for_hooks(project_config, &all_hooks);
    commands.extend(collect_commands_for_aliases(project_config));
    if let Some(fragment) = project_config.commit_template_append() {
        commands.push(ApprovableCommand::commit_template_append(
            fragment.to_string(),
        ));
    }
    commands
}

/// The project config, erroring when none exists. For the operations whose
/// semantics need the config as their frame of reference (`add`, `clear
/// --stale`); the read-only `list` instead treats absence as zero commands.
fn require_project_config(repo: &Repository) -> anyhow::Result<ProjectConfig> {
    let config_path = repo
        .project_config_path()?
        .context("Cannot determine project config location — no worktree found")?;
    Ok(repo
        .load_project_config()?
        .ok_or(GitError::ProjectConfigNotFound { config_path })?)
}

/// Handle `wt config approvals list` - show approval status for all project commands
pub fn list_approvals() -> anyhow::Result<()> {
    let repo = Repository::current()?;
    let project_id = repo.project_identifier()?;
    let approvals = Approvals::load().context("Failed to load approvals")?;

    // A missing project config just means zero configured commands — recorded
    // approvals for the project still list (as stale), so this never errors.
    let commands = match repo.load_project_config()? {
        Some(cfg) => collect_approvable_commands(&cfg),
        None => Vec::new(),
    };

    let templates: Vec<&str> = commands
        .iter()
        .map(|cmd| cmd.command.template.as_str())
        .collect();
    let stale = approvals.stale_approvals(&project_id, &templates);

    if commands.is_empty() && stale.is_empty() {
        eprintln!("{}", info_message("No commands configured in project"));
        return Ok(());
    }

    let (approved, unapproved): (Vec<_>, Vec<_>) = commands
        .iter()
        .partition(|cmd| approvals.is_command_approved(&project_id, &cmd.command.template));

    let mut out = String::new();
    let render_section =
        |out: &mut String, title: &str, symbol: &str, section: &[&ApprovableCommand]| {
            writeln!(out, "{}", format_heading(title, None))?;
            if section.is_empty() {
                writeln!(out, "{}", hint_message("(none)"))?;
            }
            for cmd in section {
                writeln!(out, "{} {}", symbol, cmd.label())?;
                writeln!(out, "{}", cmd.format_template())?;
            }
            anyhow::Ok(())
        };
    // Symbols match `wt hook show`: ○ approved (a state, not a success),
    // ❯ awaiting approval.
    if !commands.is_empty() {
        render_section(&mut out, "APPROVED", INFO_SYMBOL, &approved)?;
        out.push('\n');
        render_section(&mut out, "UNAPPROVED", PROMPT_SYMBOL, &unapproved)?;
    }

    if !stale.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        writeln!(
            out,
            "{}",
            warning_message("Approved commands no longer in project config:")
        )?;
        // Stale approvals are bare strings — the phase isn't recorded in
        // approvals.toml — so bash formatting is the best default even though
        // a stale commit-template fragment is prose.
        for command in &stale {
            writeln!(out, "{}", format_bash_with_gutter(command))?;
        }
        writeln!(
            out,
            "{}",
            hint_message(cformat!(
                "To clear stale approvals, run <underline>wt config approvals clear --stale</>"
            ))
        )?;
    }

    // Human-oriented sectioned output, plausibly more than a screen — page it
    // like `wt hook show`. The helper TTY-detects, so piping stays plain.
    crate::help_pager::show_help_in_pager(&out, true);

    Ok(())
}

/// Handle `wt config approvals add` command - approve all hook and alias commands in the project
pub fn add_approvals(show_all: bool) -> anyhow::Result<()> {
    let repo = Repository::current()?;
    let project_id = repo.project_identifier()?;
    let approvals = Approvals::load().context("Failed to load approvals")?;

    let project_config = require_project_config(&repo)?;
    let commands = collect_approvable_commands(&project_config);

    if commands.is_empty() {
        eprintln!("{}", info_message("No commands configured in project"));
        return Ok(());
    }

    // Filter to only unapproved commands (unless --all is specified)
    let commands_to_approve = if !show_all {
        let unapproved: Vec<_> = commands
            .into_iter()
            .filter(|cmd| !approvals.is_command_approved(&project_id, &cmd.command.template))
            .collect();

        if unapproved.is_empty() {
            eprintln!("{}", info_message("All commands already approved"));
            return Ok(());
        }

        unapproved
    } else {
        commands
    };

    // Call the approval prompt (yes=false to require interactive approval and save)
    // When show_all=true, we've already included all commands in commands_to_approve
    // When show_all=false, we've already filtered to unapproved commands
    // So we pass skip_approval_filter=true to prevent double-filtering
    let approved =
        approve_command_batch(&commands_to_approve, &project_id, &approvals, false, true)?;

    // Show result
    if approved {
        eprintln!("{}", success_message("Commands approved & saved to config"));
    } else {
        eprintln!("{}", info_message("Commands declined"));
    }

    Ok(())
}

/// Handle `wt config approvals clear` command - clear approved commands
pub fn clear_approvals(global: bool, stale: bool) -> anyhow::Result<()> {
    let mut approvals = Approvals::load().context("Failed to load approvals")?;

    if stale {
        // Clear only approvals whose commands left the project config. A
        // missing config is an error (matching `add`), not "everything is
        // stale": approvals are keyed repo-wide while the config is resolved
        // per-worktree, so a branch that merely lacks the file must not wipe
        // the whole repo's approvals. Clearing everything is `clear`'s job.
        let repo = Repository::current()?;
        let project_id = repo.project_identifier()?;
        let project_config = require_project_config(&repo)?;
        let commands = collect_approvable_commands(&project_config);
        let templates: Vec<&str> = commands
            .iter()
            .map(|cmd| cmd.command.template.as_str())
            .collect();

        let removed = approvals
            .revoke_stale(&project_id, &templates, &require_approvals_path()?)
            .context("Failed to clear stale approvals")?;

        if removed.is_empty() {
            eprintln!(
                "{}",
                info_message("No stale approvals to clear for this project")
            );
            return Ok(());
        }

        eprintln!(
            "{}",
            success_message(format!(
                "Cleared {} stale approval{} for this project:",
                removed.len(),
                if removed.len() == 1 { "" } else { "s" }
            ))
        );
        for command in &removed {
            eprintln!("{}", format_bash_with_gutter(command));
        }
    } else if global {
        // Count projects with approvals before clearing
        let project_count = approvals
            .projects()
            .filter(|(_, cmds)| !cmds.is_empty())
            .count();

        if project_count == 0 {
            eprintln!("{}", info_message("No approvals to clear"));
            return Ok(());
        }

        approvals
            .clear_all(&require_approvals_path()?)
            .context("Failed to clear approvals")?;

        eprintln!(
            "{}",
            success_message(format!(
                "Cleared approvals for {project_count} project{}",
                if project_count == 1 { "" } else { "s" }
            ))
        );
    } else {
        // Clear approvals for current project (default)
        let repo = Repository::current()?;
        let project_id = repo.project_identifier()?;

        // Count approvals before clearing
        let approval_count = approvals
            .projects()
            .find(|(id, _)| *id == project_id)
            .map(|(_, cmds)| cmds.len())
            .unwrap_or(0);

        if approval_count == 0 {
            eprintln!("{}", info_message("No approvals to clear for this project"));
            return Ok(());
        }

        approvals
            .revoke_project(&project_id, &require_approvals_path()?)
            .context("Failed to clear project approvals")?;

        eprintln!(
            "{}",
            success_message(format!(
                "Cleared {approval_count} approval{} for this project",
                if approval_count == 1 { "" } else { "s" }
            ))
        );
    }

    Ok(())
}
