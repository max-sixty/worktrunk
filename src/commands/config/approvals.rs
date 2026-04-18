//! Approvals commands for `wt config approvals` subcommand.
//!
//! - `add_approvals` - Approve all project commands (hooks and aliases)
//! - `clear_approvals` - Clear approved commands

use anyhow::Context;
use strum::IntoEnumIterator;
use worktrunk::HookType;
use worktrunk::config::Approvals;
use worktrunk::git::{GitError, Repository};
use worktrunk::styling::{eprintln, info_message, success_message};

use crate::commands::command_approval::approve_command_batch;
use crate::commands::project_config::{collect_commands_for_aliases, collect_commands_for_hooks};

/// Handle `wt config approvals add` command - approve all hook and alias commands in the project
pub fn add_approvals(show_all: bool) -> anyhow::Result<()> {
    let repo = Repository::current()?;
    let project_id = repo.project_identifier()?;
    let approvals = Approvals::load().context("Failed to load approvals")?;

    // Load project config (error if missing - this command requires it)
    let config_path = repo
        .project_config_path()?
        .context("Cannot determine project config location — no worktree found")?;
    let project_config = repo
        .load_project_config()?
        .ok_or(GitError::ProjectConfigNotFound { config_path })?;

    // Collect all commands from the project config: hooks first (lifecycle order),
    // then aliases (alphabetical via BTreeMap).
    let all_hooks: Vec<_> = HookType::iter().collect();
    let mut commands = collect_commands_for_hooks(&project_config, &all_hooks);
    commands.extend(collect_commands_for_aliases(&project_config));

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
pub fn clear_approvals(global: bool) -> anyhow::Result<()> {
    let mut approvals = Approvals::load().context("Failed to load approvals")?;

    if global {
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
            .clear_all(None)
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
            .revoke_project(&project_id, None)
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
