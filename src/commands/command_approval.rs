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

use std::io::{self, IsTerminal, Write};
use std::path::Path;

use anyhow::Context;
use color_print::cformat;
use worktrunk::config::Approvals;
use worktrunk::git::{GitError, HookType};
use worktrunk::styling::{
    INFO_SYMBOL, WARNING_SYMBOL, eprint, eprintln, format_bash_with_gutter, format_with_gutter,
    hint_message, prompt_message, stderr, warning_message,
};

use super::hook_filter::{HookSource, ParsedFilter};
use super::project_config::{ApprovableCommand, Phase, collect_commands_for_hooks};

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
    commands: &[ApprovableCommand],
    project_id: &str,
    approvals: &Approvals,
    yes: bool,
    commands_already_filtered: bool,
) -> anyhow::Result<bool> {
    let needs_approval: Vec<&ApprovableCommand> = commands
        .iter()
        .filter(|cmd| {
            commands_already_filtered
                || !approvals.is_command_approved(project_id, &cmd.command.template)
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
        let mut fresh_approvals = Approvals::load().context("Failed to load approvals")?;
        let commands: Vec<String> = needs_approval
            .iter()
            .map(|cmd| cmd.command.template.clone())
            .collect();
        if let Err(e) = fresh_approvals.approve_commands(project_id.to_string(), commands, None) {
            eprintln!(
                "{}",
                warning_message(format!("Failed to save command approval: {e}"))
            );
            eprintln!(
                "{}",
                hint_message("Approval will be requested again next time.")
            );
        }
    }

    Ok(true)
}

fn prompt_for_batch_approval(
    commands: &[&ApprovableCommand],
    project_id: &str,
) -> anyhow::Result<bool> {
    // Extract just the directory name for display
    let project_name = Path::new(project_id)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(project_id);
    let count = commands.len();
    let plural = if count == 1 { "" } else { "s" };

    eprintln!(
        "{}",
        cformat!(
            "{WARNING_SYMBOL} <yellow><bold>{project_name}</> needs approval to execute <bold>{count}</> command{plural}:</>"
        )
    );

    for cmd in commands {
        // Format as: {phase} {bold}{name}{bold:#}:
        // Uses INFO_SYMBOL (○) since this is a preview, not active execution
        let phase = cmd.phase.to_string();
        let label = match &cmd.command.name {
            Some(name) => cformat!("{INFO_SYMBOL} {phase} <bold>{name}</>:"),
            None => format!("{INFO_SYMBOL} {phase}:"),
        };
        eprintln!("{}", label);
        // Shell commands get bash syntax highlighting; the commit template
        // fragment is plain text (markdown-ish) and shouldn't be tokenized as
        // bash.
        let body = match cmd.phase {
            Phase::CommitTemplateAppend => format_with_gutter(&cmd.command.template, None),
            _ => format_bash_with_gutter(&cmd.command.template),
        };
        eprintln!("{}", body);
    }

    // Check if stdin is a TTY before attempting to prompt
    // This happens AFTER showing the commands so they appear in CI/CD logs
    // even when the prompt cannot be displayed (fail-fast principle)
    if !io::stdin().is_terminal() {
        return Err(GitError::NotInteractive.into());
    }

    // Blank line before prompt for visual separation
    worktrunk::styling::eprintln!();
    stderr().flush()?;

    eprint!(
        "{} ",
        prompt_message(cformat!("Allow and remember? <bold>[y/N]</>"))
    );
    stderr().flush()?;

    let mut response = String::new();
    io::stdin().read_line(&mut response)?;

    Ok(response.trim().eq_ignore_ascii_case("y"))
}

/// Approve a project-config alias before execution.
///
/// Returns `Ok(true)` if approved (or already approved), `Ok(false)` if declined.
pub fn approve_alias_commands(
    commands: &worktrunk::config::CommandConfig,
    alias_name: &str,
    project_id: &str,
    yes: bool,
) -> anyhow::Result<bool> {
    let approvals = Approvals::load().context("Failed to load approvals")?;

    let cmds: Vec<_> = commands
        .commands()
        .map(|cmd| ApprovableCommand {
            phase: Phase::Alias,
            command: worktrunk::config::Command::new(
                Some(cmd.name.clone().unwrap_or_else(|| alias_name.to_string())),
                cmd.template.clone(),
            ),
        })
        .collect();

    approve_command_batch(&cmds, project_id, &approvals, yes, false)
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
/// let ctx = CommandContext::new(&repo, &config, &branch, &worktree_path, yes);
/// let approved = approve_hooks(&ctx, &[HookType::PreStart, HookType::PostStart])?;
/// ```
pub fn approve_hooks(
    ctx: &super::command_executor::CommandContext<'_>,
    hook_types: &[HookType],
) -> anyhow::Result<bool> {
    approve_hooks_filtered(ctx, hook_types, &[])
}

/// Like `approve_hooks` but with name filters for targeted hook approval.
///
/// When `name_filters` is non-empty, only commands matching those names are shown
/// in the approval prompt. This is used by `wt hook <type> <name...>` to
/// approve only the targeted hooks rather than all hooks of that type.
///
/// Supports filter syntax per name:
/// - `"foo"` — approves commands named "foo" from project config
/// - `"project:foo"` — approves commands named "foo" from project config
/// - `"project:"` — approves all commands from project config
/// - `"user:"` or `"user:foo"` — skips approval (user hooks don't need approval)
pub fn approve_hooks_filtered(
    ctx: &super::command_executor::CommandContext<'_>,
    hook_types: &[HookType],
    name_filters: &[String],
) -> anyhow::Result<bool> {
    // Parse filters to understand source and name separately
    // Uses the same ParsedFilter as hooks.rs for consistent behavior
    let parsed_filters: Vec<ParsedFilter<'_>> = name_filters
        .iter()
        .map(|f| ParsedFilter::parse(f))
        .collect();

    // If all filters explicitly target user hooks only, skip project approval entirely
    if !parsed_filters.is_empty()
        && parsed_filters
            .iter()
            .all(|f| f.source == Some(HookSource::User))
    {
        return Ok(true);
    }

    let project_config = match ctx.repo.load_project_config()? {
        Some(cfg) => cfg,
        None => return Ok(true), // No project config = no commands to approve
    };

    let mut commands = collect_commands_for_hooks(&project_config, hook_types);

    // Apply name filters before approval to only prompt for targeted commands
    // Collect non-empty name parts from filters
    // Empty names (e.g., "project:") mean match all project commands - no filtering
    let filter_names: Vec<&str> = parsed_filters
        .iter()
        .map(|f| f.name)
        .filter(|n| !n.is_empty())
        .collect();
    if !filter_names.is_empty() {
        commands.retain(|cmd| {
            cmd.command
                .name
                .as_deref()
                .is_some_and(|n| filter_names.contains(&n))
        });
    }

    if commands.is_empty() {
        return Ok(true);
    }

    let project_id = ctx.repo.project_identifier()?;
    let approvals = Approvals::load().context("Failed to load approvals")?;
    approve_command_batch(&commands, &project_id, &approvals, ctx.yes, false)
}

/// Approve `hook_types` and centralize the "decline → continue without hooks" message.
///
/// Returns `true` when approval succeeded (hooks should run) and `false` when the
/// user declined (caller should fall through without hook execution). Emits
/// `on_decline` as an info message on the decline path.
pub fn approve_or_skip(
    ctx: &super::command_executor::CommandContext<'_>,
    hook_types: &[HookType],
    on_decline: &str,
) -> anyhow::Result<bool> {
    let approved = approve_hooks(ctx, hook_types)?;
    if !approved {
        worktrunk::styling::eprintln!("{}", worktrunk::styling::info_message(on_decline));
    }
    Ok(approved)
}

/// Resolve the project commit template for a preview path (`--show-prompt` /
/// `--dry-run`).
///
/// `--show-prompt` doesn't invoke the LLM, so it always returns the configured
/// fragment verbatim. `--dry-run` does invoke the LLM, so when an LLM is
/// configured it goes through the same approval gate as a real commit.
pub fn resolve_template_for_preview(
    ctx: &super::command_executor::CommandContext<'_>,
    commit_config: &worktrunk::config::CommitGenerationConfig,
    dry_run: bool,
) -> anyhow::Result<Option<String>> {
    if dry_run && commit_config.is_configured() {
        approve_commit_template_append(ctx)
    } else {
        Ok(ctx
            .repo
            .load_project_config()?
            .as_ref()
            .and_then(|cfg| cfg.commit_template_append().map(str::to_string)))
    }
}

/// Approve the project-level commit append fragment before sending it to the LLM.
///
/// Returns `Ok(Some(fragment))` when approved (or already approved), `Ok(None)`
/// when no fragment is configured or the user declined. Declining is non-fatal:
/// the LLM still runs, just without the project append, mirroring the
/// "commands declined, continuing without hooks" pattern. The user-level
/// `template-append` is not gated here — it's the developer's own config.
///
/// Callers should invoke this on every LLM-bearing path (`wt commit`, `wt step
/// commit`, `wt step squash`, `wt merge`, `wt step commit --dry-run`, etc.).
/// `--show-prompt` paths skip the gate — they don't execute the LLM, so showing
/// the fragment is a preview, not a send.
pub fn approve_commit_template_append(
    ctx: &super::command_executor::CommandContext<'_>,
) -> anyhow::Result<Option<String>> {
    let Some(project_config) = ctx.repo.load_project_config()? else {
        return Ok(None);
    };
    let Some(fragment) = project_config.commit_template_append() else {
        return Ok(None);
    };

    let project_id = ctx.repo.project_identifier()?;
    let approvals = Approvals::load().context("Failed to load approvals")?;
    let owned = fragment.to_string();
    if approvals.is_command_approved(&project_id, &owned) {
        return Ok(Some(owned));
    }

    let batch = vec![ApprovableCommand::commit_template_append(owned.clone())];
    let approved = approve_command_batch(&batch, &project_id, &approvals, ctx.yes, true)?;
    if !approved {
        worktrunk::styling::eprintln!(
            "{}",
            worktrunk::styling::info_message(
                "Project commit guidance declined; generating without it",
            )
        );
        return Ok(None);
    }
    Ok(Some(owned))
}
