//! Output handlers for worktree operations using the global output context

use color_print::cformat;
use std::path::{Path, PathBuf};

use crate::commands::branch_deletion::{
    BranchDeletionOutcome, BranchDeletionResult, delete_branch_if_safe,
};
use crate::commands::command_executor::CommandContext;
use crate::commands::execute_pre_remove_commands;
use crate::commands::process::{build_remove_command, spawn_detached};
use crate::commands::worktree::{BranchDeletionMode, RemoveResult, SwitchBranchInfo, SwitchResult};
use worktrunk::config::WorktrunkConfig;
use worktrunk::git::GitError;
use worktrunk::git::IntegrationReason;
use worktrunk::git::Repository;
use worktrunk::git::path_dir_name;
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{
    FormattedMessage, error_message, format_with_gutter, hint_message, info_message,
    progress_message, success_message, suggest_command, warning_message,
};

use super::shell_integration::{
    compute_shell_warning_reason, git_subcommand_warning, shell_integration_hint,
};

/// Format a switch message based on what was created
///
/// # Message formats
/// - Branch + worktree created (`--create`): "Created branch X and worktree from Y @ path"
/// - Branch from remote + worktree (DWIM): "Created branch X (tracking remote) and worktree @ path"
/// - Worktree only created: "Created worktree for X @ path"
/// - Switched to existing: "Switched to worktree for X @ path"
fn format_switch_message(
    branch: &str,
    path: &Path,
    worktree_created: bool,
    created_branch: bool,
    base_branch: Option<&str>,
    from_remote: Option<&str>,
) -> String {
    let path_display = format_path_for_display(path);

    if created_branch {
        // --create flag: created branch and worktree
        match base_branch {
            Some(base) => cformat!(
                "Created branch <bold>{branch}</> and worktree from <bold>{base}</> @ <bold>{path_display}</>"
            ),
            None => {
                cformat!("Created branch <bold>{branch}</> and worktree @ <bold>{path_display}</>")
            }
        }
    } else if let Some(remote) = from_remote {
        // DWIM from remote: created local tracking branch and worktree
        cformat!(
            "Created branch <bold>{branch}</> (tracking <bold>{remote}</>) and worktree @ <bold>{path_display}</>"
        )
    } else if worktree_created {
        // Local branch existed, created worktree only
        cformat!("Created worktree for <bold>{branch}</> @ <bold>{path_display}</>")
    } else {
        // Switched to existing worktree
        cformat!("Switched to worktree for <bold>{branch}</> @ <bold>{path_display}</>")
    }
}

/// Format a branch-worktree mismatch warning message.
///
/// Shows when a worktree is at a path that doesn't match the config template.
fn format_path_mismatch_warning(branch: &str, expected_path: &Path) -> FormattedMessage {
    let expected_display = format_path_for_display(expected_path);
    warning_message(cformat!(
        "Branch-worktree mismatch; expected <bold>{branch}</> @ <bold>{expected_display}</> <red>⚑</>"
    ))
}

/// Handle the result of a branch deletion attempt.
///
/// Shows appropriate messages for non-deleted branches:
/// - `NotDeleted`: We checked and chose not to delete (not integrated) - show info
/// - `Err(e)`: Git command failed - show warning with actual error
///
/// Returns (result, needs_hint) where needs_hint indicates the caller should print
/// the unmerged branch hint after any success message.
///
/// When `defer_output` is true, info and hint are suppressed (caller will handle).
fn handle_branch_deletion_result(
    result: anyhow::Result<BranchDeletionResult>,
    branch_name: &str,
    defer_output: bool,
) -> anyhow::Result<(BranchDeletionResult, bool)> {
    match result {
        Ok(r) if !matches!(r.outcome, BranchDeletionOutcome::NotDeleted) => Ok((r, false)),
        Ok(r) => {
            // Branch not integrated - we chose not to delete (not a failure)
            if !defer_output {
                super::print(info_message(cformat!(
                    "Branch <bold>{branch_name}</> retained; has unmerged changes"
                )))?;
                let cmd = suggest_command("remove", &[branch_name], &["-D"]);
                super::print(hint_message(cformat!(
                    "To delete the unmerged branch, run <bright-black>{cmd}</>"
                )))?;
            }
            Ok((r, defer_output))
        }
        Err(e) => {
            // Git command failed - this is an error (we decided to delete but couldn't)
            super::print(error_message(cformat!(
                "Failed to delete branch <bold>{branch_name}</>"
            )))?;
            super::print(format_with_gutter(&e.to_string(), None))?;
            Err(e)
        }
    }
}

// ============================================================================
// FlagNote: Workaround for cformat! being compile-time only
// ============================================================================
//
// We want to parameterize the color (cyan/green) but can't because cformat!
// parses color tags at compile time before generic substitution. So we have
// duplicate methods (after_cyan, after_green) instead of after(color).
//
// This is ugly but unavoidable. Keep it encapsulated here.
// ============================================================================

struct FlagNote {
    text: String,
    symbol: Option<String>,
    suffix: String,
}

impl FlagNote {
    fn empty() -> Self {
        Self {
            text: String::new(),
            symbol: None,
            suffix: String::new(),
        }
    }

    fn text_only(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            symbol: None,
            suffix: String::new(),
        }
    }

    fn with_symbol(
        text: impl Into<String>,
        symbol: impl Into<String>,
        suffix: impl Into<String>,
    ) -> Self {
        Self {
            text: text.into(),
            symbol: Some(symbol.into()),
            suffix: suffix.into(),
        }
    }

    fn after_cyan(&self) -> String {
        match &self.symbol {
            Some(s) => cformat!("{}<cyan>{}</>", s, self.suffix),
            None => String::new(),
        }
    }

    fn after_green(&self) -> String {
        match &self.symbol {
            Some(s) => cformat!("{}<green>{}</>", s, self.suffix),
            None => String::new(),
        }
    }
}

// ============================================================================

/// Get flag acknowledgment note for remove messages
///
/// `target_branch`: The branch we checked integration against (shown in reason)
fn get_flag_note(
    deletion_mode: BranchDeletionMode,
    outcome: &BranchDeletionOutcome,
    target_branch: Option<&str>,
) -> FlagNote {
    if deletion_mode.should_keep() {
        return FlagNote::text_only(" (--no-delete-branch)");
    }

    match outcome {
        BranchDeletionOutcome::NotDeleted => FlagNote::empty(),
        BranchDeletionOutcome::ForceDeleted => FlagNote::text_only(" (--force-delete)"),
        BranchDeletionOutcome::Integrated(reason) => {
            let Some(target) = target_branch else {
                return FlagNote::empty();
            };
            let symbol = reason.symbol();
            let desc = reason.description();
            FlagNote::with_symbol(
                cformat!(" ({desc} <bold>{target}</>,"),
                cformat!(" <dim>{symbol}</>"),
                ")",
            )
        }
    }
}

/// Show switch message when changing directory after worktree removal.
///
/// When shell integration is not active, warns that cd cannot happen.
/// This is important for remove/merge since the user would be left in a deleted directory.
///
/// # Warning Message Format
///
/// Uses the standard "Cannot change directory — {reason}" pattern.
/// See [`compute_shell_warning_reason`] for the full list of reasons.
fn print_switch_message_if_changed(
    changed_directory: bool,
    main_path: &Path,
) -> anyhow::Result<()> {
    if !changed_directory {
        return Ok(());
    }

    let repo = Repository::at(main_path);
    let Ok(Some(dest_branch)) = repo.current_branch() else {
        return Ok(());
    };

    let path_display = format_path_for_display(main_path);

    if super::is_shell_integration_active() {
        // Shell integration active - cd will work
        super::print(info_message(cformat!(
            "Switched to worktree for <bold>{dest_branch}</> @ <bold>{path_display}</>"
        )))?;
    } else if crate::is_git_subcommand() {
        // Running as `git wt` - explain why cd can't work
        super::print(warning_message(
            "Cannot change directory — ran git wt; running through git prevents cd",
        ))?;
        super::print(hint_message(git_subcommand_warning()))?;
    } else {
        // Shell integration not active - compute specific reason
        let reason = compute_shell_warning_reason();
        super::print(warning_message(cformat!(
            "Cannot change directory — {reason}"
        )))?;
        super::print(hint_message(shell_integration_hint()))?;
    }
    Ok(())
}

/// Handle output for a switch operation
///
/// # Shell Integration Warnings
///
/// Always warn when the shell's directory won't change. Users expect to be in
/// the target worktree after switching.
///
/// **When to warn:** Shell integration is not active (`WORKTRUNK_DIRECTIVE_FILE`
/// not set). This applies to both `Existing` and `Created` results.
///
/// **When NOT to warn:**
/// - `AlreadyAt` — user is already in the target directory
/// - Shell integration IS active — cd will happen automatically
///
/// **Warning format:** `Cannot change directory — {reason}`
///
/// See [`compute_shell_warning_reason`] for the full list of reasons.
///
/// **Message order for Created:** Success message first, then warning. Creation
/// is a real accomplishment, but users still need to know they won't cd there.
///
/// # Return Value
///
/// Returns `Some(path)` when post-switch hooks should show "@ path" in their
/// announcements (because the user's shell won't be in that directory). This happens when:
/// - Shell integration is not active (user's shell stays in original directory)
///
/// Returns `None` when the user will be in the worktree directory (shell integration
/// active or already at the worktree), so no path annotation needed.
pub fn handle_switch_output(
    result: &SwitchResult,
    branch_info: &SwitchBranchInfo,
    execute_command: Option<&str>,
) -> anyhow::Result<Option<std::path::PathBuf>> {
    // Set target directory for command execution
    super::change_directory(result.path())?;

    let path = result.path();
    let path_display = format_path_for_display(path);
    let branch = &branch_info.branch;

    // Check if shell integration is active (directive file set)
    let is_shell_integration_active = super::is_shell_integration_active();

    // Compute shell warning reason once (only if we'll need it)
    // Git subcommand case is special — needs a hint after the warning
    let is_git_subcommand = crate::is_git_subcommand();
    let shell_warning_reason: Option<String> = if is_shell_integration_active {
        None
    } else if is_git_subcommand {
        Some("ran git wt; running through git prevents cd".to_string())
    } else {
        Some(compute_shell_warning_reason())
    };

    // Show branch-worktree mismatch warning after the main message
    let branch_worktree_mismatch_warning = branch_info
        .expected_path
        .as_ref()
        .map(|expected| format_path_mismatch_warning(&branch_info.branch, expected));

    let display_path_for_hooks = match result {
        SwitchResult::AlreadyAt(_) => {
            // Already in target directory — no shell warning needed
            super::print(info_message(cformat!(
                "Already on worktree for <bold>{branch}</> @ <bold>{path_display}</>"
            )))?;
            if let Some(warning) = branch_worktree_mismatch_warning {
                super::print(warning)?;
            }
            // User is already there - no path annotation needed
            None
        }
        SwitchResult::Existing(_) => {
            if let Some(reason) = &shell_warning_reason {
                // Shell integration not active — single warning with context
                if let Some(warning) = branch_worktree_mismatch_warning {
                    super::print(warning)?;
                }
                if let Some(cmd) = execute_command {
                    // --execute: command runs in target dir, but shell stays put
                    super::print(warning_message(cformat!(
                        "Executing <bold>{cmd}</> @ <bold>{path_display}</>, but shell directory unchanged — {reason}"
                    )))?;
                } else {
                    // No --execute: what exists + why cd won't happen
                    super::print(warning_message(cformat!(
                        "Worktree for <bold>{branch}</> @ <bold>{path_display}</>, but cannot change directory — {reason}"
                    )))?;
                }
                // Show git subcommand hint if running as git wt
                if is_git_subcommand {
                    super::print(hint_message(git_subcommand_warning()))?;
                }
                // User won't be there - show path in hook announcements
                Some(path.clone())
            } else {
                // Shell integration active — user actually switched
                super::print(info_message(format_switch_message(
                    branch, path, false, // worktree_created
                    false, // created_branch
                    None, None,
                )))?;
                if let Some(warning) = branch_worktree_mismatch_warning {
                    super::print(warning)?;
                }
                // cd will happen - no path annotation needed
                None
            }
        }
        SwitchResult::Created {
            created_branch,
            base_branch,
            from_remote,
            ..
        } => {
            // Always show success for creation
            super::print(success_message(format_switch_message(
                branch,
                path,
                true, // worktree_created
                *created_branch,
                base_branch.as_deref(),
                from_remote.as_deref(),
            )))?;

            // Show worktree-path config hint on first --create in this repo,
            // unless user already has a custom worktree-path config
            if *created_branch {
                let repo = worktrunk::git::Repository::current();
                let has_custom_config = WorktrunkConfig::load()
                    .map(|c| c.has_custom_worktree_path())
                    .unwrap_or(false);
                if !has_custom_config && !repo.has_shown_hint("worktree-path") {
                    let hint = hint_message(cformat!(
                        "Customize worktree locations: <bright-black>wt config create</>"
                    ));
                    super::print(hint)?;
                    let _ = repo.mark_hint_shown("worktree-path");
                }
            }

            // Warn if shell won't cd to the new worktree
            if let Some(reason) = shell_warning_reason {
                if let Some(cmd) = execute_command {
                    super::print(warning_message(cformat!(
                        "Executing <bold>{cmd}</> @ <bold>{path_display}</>, but shell directory unchanged — {reason}"
                    )))?;
                } else {
                    // Don't repeat "Created worktree" — success message above already said that
                    super::print(warning_message(cformat!(
                        "Cannot change directory — {reason}"
                    )))?;
                }
                // Show git subcommand hint if running as git wt
                if is_git_subcommand {
                    super::print(hint_message(git_subcommand_warning()))?;
                }
                // User won't be there - show path in hook announcements
                Some(path.clone())
            } else {
                // cd will happen - no path annotation needed
                None
            }
            // Note: No branch_worktree_mismatch_warning — created worktrees are always at
            // the expected path (SwitchBranchInfo::expected_path is None)
        }
    };

    super::flush()?;
    Ok(display_path_for_hooks)
}

/// Execute the --execute command after hooks have run
pub fn execute_user_command(command: &str) -> anyhow::Result<()> {
    use worktrunk::styling::format_bash_with_gutter;

    // Show what command is being executed (section header + gutter content)
    super::print(progress_message("Executing (--execute):"))?;
    super::print(format_bash_with_gutter(command))?;

    super::execute(command)?;

    Ok(())
}

/// Handle output for a remove operation
///
/// Approval is handled at the gate (command entry point), not here.
pub fn handle_remove_output(
    result: &RemoveResult,
    background: bool,
    verify: bool,
) -> anyhow::Result<()> {
    match result {
        RemoveResult::RemovedWorktree {
            main_path,
            worktree_path,
            changed_directory,
            branch_name,
            deletion_mode,
            target_branch,
            integration_reason,
            force_worktree,
            expected_path,
        } => handle_removed_worktree_output(
            main_path,
            worktree_path,
            *changed_directory,
            branch_name.as_deref(),
            *deletion_mode,
            target_branch.as_deref(),
            *integration_reason,
            *force_worktree,
            expected_path.as_ref(),
            background,
            verify,
        ),
        RemoveResult::BranchOnly {
            branch_name,
            deletion_mode,
        } => handle_branch_only_output(branch_name, *deletion_mode),
    }
}

/// Handle output for BranchOnly removal (branch exists but no worktree)
fn handle_branch_only_output(
    branch_name: &str,
    deletion_mode: BranchDeletionMode,
) -> anyhow::Result<()> {
    // Warn that no worktree was found (user asked to remove it)
    super::print(warning_message(cformat!(
        "No worktree found for branch <bold>{branch_name}</>"
    )))?;

    // Attempt branch deletion (unless --no-delete-branch was specified)
    if deletion_mode.should_keep() {
        // User explicitly requested no branch deletion - nothing more to do
        super::flush()?;
        return Ok(());
    }

    let repo = worktrunk::git::Repository::current();

    // Get default branch for integration check and reason display
    // Falls back to HEAD if default branch can't be determined
    let default_branch = repo.default_branch().ok();
    let check_target = default_branch.as_deref().unwrap_or("HEAD");

    let result = delete_branch_if_safe(&repo, branch_name, check_target, deletion_mode.is_force());
    let (deletion, _) = handle_branch_deletion_result(result, branch_name, false)?;

    if !matches!(deletion.outcome, BranchDeletionOutcome::NotDeleted) {
        let flag_note = get_flag_note(
            deletion_mode,
            &deletion.outcome,
            Some(&deletion.effective_target),
        );
        let flag_text = &flag_note.text;
        let flag_after = flag_note.after_green();
        super::print(FormattedMessage::new(cformat!(
            "<green>✓ Removed branch <bold>{branch_name}</>{flag_text}</>{flag_after}"
        )))?;
    }

    super::flush()?;
    Ok(())
}

/// Spawn post-switch hooks in the destination worktree after a directory change.
///
/// Called when removing a worktree causes a cd to the main worktree.
/// Only runs if `verify` is true (hooks approved) and `changed_directory` is true.
fn spawn_post_switch_after_remove(
    main_path: &std::path::Path,
    verify: bool,
    changed_directory: bool,
) -> anyhow::Result<()> {
    if !verify || !changed_directory {
        return Ok(());
    }
    let Ok(config) = WorktrunkConfig::load() else {
        return Ok(());
    };
    let dest_repo = Repository::at(main_path);
    let dest_branch = dest_repo.current_branch()?;
    let repo_root = dest_repo.worktree_base()?;
    let ctx = CommandContext::new(
        &dest_repo,
        &config,
        dest_branch,
        main_path,
        &repo_root,
        false, // force=false for CommandContext
    );
    // No base context for remove-triggered switch (we're returning to main, not creating)
    ctx.spawn_post_switch_commands(&[], super::post_hook_display_path(main_path))
}

/// Handle output for RemovedWorktree removal
#[allow(clippy::too_many_arguments)]
fn handle_removed_worktree_output(
    main_path: &std::path::Path,
    worktree_path: &std::path::Path,
    changed_directory: bool,
    branch_name: Option<&str>,
    deletion_mode: BranchDeletionMode,
    target_branch: Option<&str>,
    pre_computed_integration: Option<IntegrationReason>,
    force_worktree: bool,
    expected_path: Option<&PathBuf>,
    background: bool,
    verify: bool,
) -> anyhow::Result<()> {
    // 1. Emit cd directive if needed - shell will execute this immediately
    if changed_directory {
        super::change_directory(main_path)?;
        super::flush()?; // Force flush to ensure shell processes the cd
    }

    let repo = worktrunk::git::Repository::current();

    // Execute pre-remove hooks in the worktree being removed
    // Non-zero exit aborts removal (FailFast strategy)
    // For detached HEAD, {{ branch }} expands to "HEAD" in templates
    if verify && let Ok(config) = WorktrunkConfig::load() {
        let target_repo = Repository::at(worktree_path);
        let ctx = CommandContext::new(
            &target_repo,
            &config,
            branch_name,
            worktree_path,
            main_path,
            false, // force=false for CommandContext (not approval-related)
        );
        // Show path when removing a different worktree (user is elsewhere)
        let display_path = if changed_directory {
            None // User was already here
        } else {
            Some(worktree_path) // Show path when user is elsewhere
        };
        execute_pre_remove_commands(&ctx, None, display_path)?;
    }

    // Handle detached HEAD case (no branch known)
    let Some(branch_name) = branch_name else {
        // No branch associated - just remove the worktree
        if background {
            super::print(progress_message(
                "Removing worktree in background (detached HEAD, no branch to delete)",
            ))?;
            let remove_command = build_remove_command(worktree_path, None, force_worktree);
            spawn_detached(
                &repo,
                main_path,
                &remove_command,
                "detached",
                "remove",
                None,
            )?;
        } else {
            let target_repo = worktrunk::git::Repository::at(worktree_path);
            let _ = target_repo.run_command(&["fsmonitor--daemon", "stop"]);
            if let Err(err) = repo.remove_worktree(worktree_path, force_worktree) {
                return Err(GitError::WorktreeRemovalFailed {
                    branch: path_dir_name(worktree_path).to_string(),
                    path: worktree_path.to_path_buf(),
                    error: err.to_string(),
                }
                .into());
            }
            super::print(success_message(
                "Removed worktree (detached HEAD, no branch to delete)",
            ))?;
        }
        spawn_post_switch_after_remove(main_path, verify, changed_directory)?;
        super::flush()?;
        return Ok(());
    };

    if background {
        // Background mode: spawn detached process

        // Use pre-computed integration reason to avoid race conditions when removing
        // multiple worktrees (background processes can hold git locks)
        let (outcome, effective_target) = if deletion_mode.should_keep() {
            (
                BranchDeletionOutcome::NotDeleted,
                target_branch.map(String::from),
            )
        } else if deletion_mode.is_force() {
            (
                BranchDeletionOutcome::ForceDeleted,
                target_branch.map(String::from),
            )
        } else {
            // Use pre-computed integration reason
            let outcome = match pre_computed_integration {
                Some(r) => BranchDeletionOutcome::Integrated(r),
                None => BranchDeletionOutcome::NotDeleted,
            };
            (outcome, target_branch.map(String::from))
        };

        let should_delete_branch = matches!(
            outcome,
            BranchDeletionOutcome::ForceDeleted | BranchDeletionOutcome::Integrated(_)
        );

        let flag_note = get_flag_note(deletion_mode, &outcome, effective_target.as_deref());
        let flag_text = &flag_note.text;
        let flag_after = flag_note.after_cyan();

        // Reason in parentheses: user flags shown explicitly, integration reason for automatic cleanup
        // Note: We use FormattedMessage directly instead of progress_message() to control
        // where cyan styling ends. Symbol must be inside the <cyan> block to get proper coloring.
        //
        // Message structure by case:
        // - Branch deleted (integrated/force): "worktree & branch in background (reason)"
        // - Branch kept (any reason): "worktree in background" + hint (if relevant)
        let branch_was_integrated = pre_computed_integration.is_some();

        let action = if should_delete_branch {
            // Branch will be deleted (integrated or force-deleted)
            cformat!(
                "<cyan>◎ Removing <bold>{branch_name}</> worktree & branch in background{flag_text}</>{flag_after}"
            )
        } else {
            // Branch kept: hint will explain why (integrated+flag, unmerged, or unmerged+flag)
            cformat!("<cyan>◎ Removing <bold>{branch_name}</> worktree in background</>")
        };
        super::print(FormattedMessage::new(action))?;

        // Show path mismatch warning if the worktree is at an unexpected location
        if let Some(expected) = expected_path {
            super::print(format_path_mismatch_warning(branch_name, expected))?;
        }

        // Show hints for branch status
        if !should_delete_branch {
            if deletion_mode.should_keep() && branch_was_integrated {
                // User kept an integrated branch - show integration info
                let reason = pre_computed_integration.as_ref().unwrap();
                let target = effective_target.as_deref().unwrap_or("target");
                let desc = reason.description();
                let symbol = reason.symbol();
                super::print(hint_message(cformat!(
                    "Branch integrated ({desc} <bold>{target}</>, <dim>{symbol}</>); retained with <bright-black>--no-delete-branch</>"
                )))?;
            } else if !deletion_mode.should_keep() {
                // Unmerged, no flag - show how to force delete
                let cmd = suggest_command("remove", &[branch_name], &["-D"]);
                super::print(hint_message(cformat!(
                    "Branch unmerged; to delete, run <bright-black>{cmd}</>"
                )))?;
            }
            // else: Unmerged + flag - no hint (flag had no effect)
        }

        print_switch_message_if_changed(changed_directory, main_path)?;

        // Build command with the decision we already made
        let remove_command = build_remove_command(
            worktree_path,
            should_delete_branch.then_some(branch_name),
            force_worktree,
        );

        // Spawn the removal in background - runs from main_path (where we cd'd to)
        spawn_detached(
            &repo,
            main_path,
            &remove_command,
            branch_name,
            "remove",
            None,
        )?;

        spawn_post_switch_after_remove(main_path, verify, changed_directory)?;
        super::flush()?;
        Ok(())
    } else {
        // Synchronous mode: remove immediately and report actual results

        // Stop fsmonitor daemon first (best effort - ignore errors)
        // This prevents zombie daemons from accumulating when using builtin fsmonitor
        let target_repo = worktrunk::git::Repository::at(worktree_path);
        let _ = target_repo.run_command(&["fsmonitor--daemon", "stop"]);

        // Track whether branch was actually deleted (will be computed based on deletion attempt)
        if let Err(err) = repo.remove_worktree(worktree_path, force_worktree) {
            return Err(GitError::WorktreeRemovalFailed {
                branch: branch_name.into(),
                path: worktree_path.to_path_buf(),
                error: err.to_string(),
            }
            .into());
        }

        // Delete the branch (unless --no-delete-branch was specified)
        // Only show effective_target in message if we had a meaningful target (not tautological "HEAD" fallback)
        let branch_was_integrated = pre_computed_integration.is_some();

        let (outcome, effective_target, show_unmerged_hint) = if !deletion_mode.should_keep() {
            let deletion_repo = worktrunk::git::Repository::at(main_path);
            let check_target = target_branch.unwrap_or("HEAD");
            let result = delete_branch_if_safe(
                &deletion_repo,
                branch_name,
                check_target,
                deletion_mode.is_force(),
            );
            let (deletion, needs_hint) = handle_branch_deletion_result(result, branch_name, true)?;
            // Only use effective_target for display if we had a real target (not "HEAD" fallback)
            let display_target = target_branch.map(|_| deletion.effective_target);
            (deletion.outcome, display_target, needs_hint)
        } else {
            (
                BranchDeletionOutcome::NotDeleted,
                target_branch.map(String::from),
                false,
            )
        };

        let branch_deleted = matches!(
            outcome,
            BranchDeletionOutcome::ForceDeleted | BranchDeletionOutcome::Integrated(_)
        );
        // Message structure parallel to background mode:
        // - Branch deleted (integrated/force): "worktree & branch (reason)"
        // - Branch kept (any reason): "worktree" + hint (if relevant)
        let msg = if branch_deleted {
            let flag_note = get_flag_note(deletion_mode, &outcome, effective_target.as_deref());
            let flag_text = &flag_note.text;
            let flag_after = flag_note.after_green();
            cformat!(
                "<green>✓ Removed <bold>{branch_name}</> worktree & branch{flag_text}</>{flag_after}"
            )
        } else {
            // Branch kept: hint will explain why (integrated+flag, unmerged, or unmerged+flag)
            cformat!("<green>✓ Removed <bold>{branch_name}</> worktree</>")
        };
        super::print(FormattedMessage::new(msg))?;

        // Show path mismatch warning if the worktree was at an unexpected location
        if let Some(expected) = expected_path {
            super::print(format_path_mismatch_warning(branch_name, expected))?;
        }

        // Show hints for branch status
        if !branch_deleted {
            if deletion_mode.should_keep() && branch_was_integrated {
                // User kept an integrated branch - show integration info
                let reason = pre_computed_integration.as_ref().unwrap();
                let target = effective_target.as_deref().unwrap_or("target");
                let desc = reason.description();
                let symbol = reason.symbol();
                super::print(hint_message(cformat!(
                    "Branch integrated ({desc} <bold>{target}</>, <dim>{symbol}</>); retained with <bright-black>--no-delete-branch</>"
                )))?;
            } else if show_unmerged_hint {
                // Unmerged, no flag - show how to force delete
                let cmd = suggest_command("remove", &[branch_name], &["-D"]);
                super::print(hint_message(cformat!(
                    "Branch unmerged; to delete, run <bright-black>{cmd}</>"
                )))?;
            }
            // else: Unmerged + flag - no hint (flag had no effect)
        }

        print_switch_message_if_changed(changed_directory, main_path)?;

        spawn_post_switch_after_remove(main_path, verify, changed_directory)?;
        super::flush()?;
        Ok(())
    }
}

/// Execute a command in a worktree directory
///
/// Merges stdout into stderr using shell redirection (1>&2) to ensure deterministic output ordering.
/// Per CLAUDE.md guidelines: child process output goes to stderr, worktrunk output goes to stdout.
///
/// If `stdin_content` is provided, it will be piped to the command's stdin. This is used to pass
/// hook context as JSON to hook commands.
///
/// ## Color Bleeding Prevention
///
/// This function explicitly resets ANSI codes on stderr before executing child commands.
///
/// Root cause: Terminal emulators maintain a single rendering state machine. When stdout
/// and stderr both connect to the same TTY, output from both streams passes through this
/// state machine in arrival order. If stdout writes color codes but stderr's output arrives
/// next, the terminal applies stdout's color state to stderr's text. The flush ensures stdout
/// completes, but doesn't reset the terminal state - hence this explicit reset to stderr.
///
/// We write the reset to stderr (not stdout) because:
/// 1. Child process output goes to stderr (per CLAUDE.md guidelines)
/// 2. The reset must reach the terminal before child output
/// 3. Writing to stdout could arrive after stderr due to buffering
///
pub fn execute_command_in_worktree(
    worktree_path: &std::path::Path,
    command: &str,
    stdin_content: Option<&str>,
) -> anyhow::Result<()> {
    use std::io::Write;
    use worktrunk::shell_exec::execute_streaming;
    use worktrunk::styling::{eprint, stderr};

    // Flush stdout before executing command to ensure all our messages appear
    // before the child process output
    super::flush()?;

    // Reset ANSI codes on stderr to prevent color bleeding (see function docs for details)
    // This fixes color bleeding observed when worktrunk prints colored output to stdout
    // followed immediately by child process output to stderr (e.g., pre-commit run output).
    eprint!("{}", anstyle::Reset);
    stderr().flush().ok(); // Ignore flush errors - reset is best-effort, command execution should proceed

    // Execute with stdout→stderr redirect for deterministic ordering
    // Hooks don't need stdin inheritance (inherit_stdin=false)
    execute_streaming(command, worktree_path, true, stdin_content, false, true)?;

    // Flush to ensure all output appears before we continue
    super::flush()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use worktrunk::git::IntegrationReason;

    #[test]
    fn test_format_switch_message() {
        let path = PathBuf::from("/tmp/test");

        // Switched to existing worktree (no creation)
        let msg = format_switch_message("feature", &path, false, false, None, None);
        assert!(msg.contains("Switched to worktree for"));
        assert!(msg.contains("feature"));

        // Created branch and worktree with --create
        let msg = format_switch_message("feature", &path, true, true, Some("main"), None);
        assert!(msg.contains("Created branch"));
        assert!(msg.contains("and worktree"));
        assert!(msg.contains("from"));
        assert!(msg.contains("main"));

        // Created worktree from remote (DWIM) - also creates local tracking branch
        let msg =
            format_switch_message("feature", &path, true, false, None, Some("origin/feature"));
        assert!(msg.contains("Created branch"));
        assert!(msg.contains("tracking"));
        assert!(msg.contains("origin/feature"));
        assert!(msg.contains("and worktree"));

        // Created worktree only (local branch already existed)
        let msg = format_switch_message("feature", &path, true, false, None, None);
        assert!(msg.contains("Created worktree for"));
        assert!(msg.contains("feature"));
        assert!(!msg.contains("branch")); // Should NOT mention branch creation
    }

    #[test]
    fn test_get_flag_note() {
        // --no-delete-branch flag (text only, no symbol, no suffix)
        let note = get_flag_note(
            BranchDeletionMode::Keep,
            &BranchDeletionOutcome::NotDeleted,
            None,
        );
        assert_eq!(note.text, " (--no-delete-branch)");
        assert!(note.symbol.is_none());
        assert!(note.suffix.is_empty());

        // NotDeleted without flag (empty)
        let note = get_flag_note(
            BranchDeletionMode::SafeDelete,
            &BranchDeletionOutcome::NotDeleted,
            None,
        );
        assert!(note.text.is_empty());
        assert!(note.symbol.is_none());
        assert!(note.suffix.is_empty());

        // Force deleted (text only, no symbol, no suffix)
        let note = get_flag_note(
            BranchDeletionMode::ForceDelete,
            &BranchDeletionOutcome::ForceDeleted,
            None,
        );
        assert_eq!(note.text, " (--force-delete)");
        assert!(note.symbol.is_none());
        assert!(note.suffix.is_empty());

        // Integration reasons - text includes description and target, symbol is separate, suffix is closing paren
        let cases = [
            (IntegrationReason::SameCommit, "same commit as"),
            (IntegrationReason::Ancestor, "ancestor of"),
            (IntegrationReason::NoAddedChanges, "no added changes on"),
            (IntegrationReason::TreesMatch, "tree matches"),
            (IntegrationReason::MergeAddsNothing, "all changes in"),
        ];
        for (reason, expected_desc) in cases {
            let note = get_flag_note(
                BranchDeletionMode::SafeDelete,
                &BranchDeletionOutcome::Integrated(reason),
                Some("main"),
            );
            assert!(
                note.text.contains(expected_desc),
                "reason {:?} text should contain '{}'",
                reason,
                expected_desc
            );
            assert!(
                note.text.contains("main"),
                "reason {:?} text should contain target 'main'",
                reason
            );
            assert!(
                note.symbol.is_some(),
                "reason {:?} should have a symbol",
                reason
            );
            let symbol = note.symbol.as_ref().unwrap();
            assert!(
                symbol.contains(reason.symbol()),
                "reason {:?} symbol part should contain the symbol",
                reason
            );
            assert_eq!(
                note.suffix, ")",
                "reason {:?} suffix should be closing paren",
                reason
            );
        }
    }

    #[test]
    fn test_shell_integration_hint() {
        let hint = shell_integration_hint();
        assert!(hint.contains("wt config shell install"));
    }

    #[test]
    fn test_git_subcommand_warning() {
        let warning = git_subcommand_warning();
        assert!(warning.contains("git-wt"));
        assert!(warning.contains("shell function"));
    }
}
