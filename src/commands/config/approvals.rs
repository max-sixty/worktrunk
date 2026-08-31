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
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{
    INFO_SYMBOL, PROMPT_SYMBOL, eprintln, format_bash_with_gutter, format_heading, hint_message,
    info_message, success_message, warning_message,
};

use crate::cli::SwitchFormat;
use crate::commands::command_approval::{announce_batch_approval, prompt_for_batch_review};
use crate::commands::project_config::{
    ApprovableCommand, collect_commands_for_aliases, collect_commands_for_hooks,
};
use crate::output::print_json;

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

/// One project command and whether its template is currently approved.
#[derive(serde::Serialize)]
struct JsonApprovalCommand<'a> {
    /// `post-start`, `pre-merge`, `alias`, `commit-template-append`, …
    phase: String,
    /// The command's name within its phase; absent for an unnamed command
    /// and for the commit-template fragment.
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    template: &'a str,
    approved: bool,
}

/// Structured form of `wt config approvals list`.
///
/// `state` is the single field a caller branches on before scheduling a
/// non-interactive run; `commands` and `stale` say which commands produced it.
/// Stale approvals are their own list rather than a `state`, because they
/// co-occur with any of the three — and they are what `--yes` would silently
/// re-approve, so an orchestrator preserving the approval model has to see
/// them separately.
#[derive(serde::Serialize)]
struct JsonApprovals<'a> {
    state: &'static str,
    commands: Vec<JsonApprovalCommand<'a>>,
    /// Templates approved earlier but since edited or removed from the
    /// project config.
    stale: Vec<&'a str>,
}

/// Handle `wt config approvals list` - show approval status for all project commands
pub fn list_approvals(format: SwitchFormat) -> anyhow::Result<()> {
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

    if format == SwitchFormat::Json {
        let json_commands: Vec<_> = commands
            .iter()
            .map(|cmd| JsonApprovalCommand {
                phase: cmd.phase.to_string(),
                name: cmd.command.name.as_deref(),
                template: &cmd.command.template,
                approved: approvals.is_command_approved(&project_id, &cmd.command.template),
            })
            .collect();
        // An empty command set is not the same answer as an approved one, so
        // the three states aren't derivable from `commands` alone.
        let state = if json_commands.is_empty() {
            "no_commands"
        } else if json_commands.iter().any(|cmd| !cmd.approved) {
            "approval_required"
        } else {
            "approved"
        };
        return print_json(&JsonApprovals {
            state,
            commands: json_commands,
            stale,
        });
    }

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
///
/// `yes` skips the review prompt, which is what makes the command usable
/// unattended: the approvals are still written, so an orchestrator can
/// pre-approve a project's commands before any `wt` run that would execute
/// them. Templates edited since an earlier approval are re-approved without
/// comment — read `wt config approvals list --format=json`'s `stale` first to
/// see them.
pub fn add_approvals(show_all: bool, yes: bool) -> anyhow::Result<()> {
    let repo = Repository::current()?;
    let project_id = repo.project_identifier()?;
    let mut approvals = Approvals::load().context("Failed to load approvals")?;

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

    // Unlike the execution gate (`approve_command_batch`), whose `--yes`
    // consents to one run and records nothing, the record is this command's
    // product: it saves on either path, and a failed save fails the command —
    // an orchestrator that pre-approves and reads only the exit code would
    // otherwise walk into the prompt it just paid to avoid.
    let batch: Vec<&_> = commands_to_approve.iter().collect();
    let approved = if yes {
        announce_batch_approval(&batch, &project_id);
        true
    } else {
        prompt_for_batch_review(&batch, &project_id)?
    };

    if !approved {
        eprintln!("{}", info_message("Commands declined"));
        return Ok(());
    }

    let templates: Vec<String> = commands_to_approve
        .iter()
        .map(|cmd| cmd.command.template.clone())
        .collect();
    approvals
        .approve_commands(project_id, templates, &require_approvals_path()?)
        .context("Failed to save command approval")?;

    eprintln!("{}", success_message("Commands approved & saved to config"));
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
            emit_pattern_entries_hint(&approvals, &project_id)?;
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
        emit_pattern_entries_hint(&approvals, &project_id)?;
    }

    Ok(())
}

/// After a per-project clear — or one with nothing exact to remove — name the
/// hand-written pattern entries still approving commands for this project.
/// `clear` only ever touches the exact entry, so without this the approval
/// survives with nothing pointing at the entry supplying it.
fn emit_pattern_entries_hint(approvals: &Approvals, project_id: &str) -> anyhow::Result<()> {
    let keys = approvals.matching_pattern_keys(project_id);
    if keys.is_empty() {
        return Ok(());
    }
    let label = if keys.len() == 1 { "entry" } else { "entries" };
    let keys = keys.join(", ");
    let path = format_path_for_display(&require_approvals_path()?);
    eprintln!(
        "{}",
        hint_message(cformat!(
            "Commands approved by pattern {label} <underline>{keys}</> still apply; to change them, edit <underline>{path}</>"
        ))
    );
    Ok(())
}
