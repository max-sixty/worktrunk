//! Output handlers for worktree operations using the global output context

use color_print::cformat;
use std::path::Path;

use crate::commands::command_executor::CommandContext;
use crate::commands::execute_pre_remove_commands;
use crate::commands::process::spawn_detached;
use crate::commands::worktree::{BranchDeletionMode, RemoveResult, SwitchBranchInfo, SwitchResult};
use worktrunk::config::WorktrunkConfig;
use worktrunk::git::GitError;
use worktrunk::git::IntegrationReason;
use worktrunk::git::Repository;
use worktrunk::git::path_dir_name;
use worktrunk::path::format_path_for_display;
use worktrunk::shell::Shell;
use worktrunk::styling::{
    FormattedMessage, error_message, format_with_gutter, hint_message, info_message,
    progress_message, success_message, suggest_command, warning_message,
};

/// Format a switch message with a consistent location phrase
///
/// Both modes (with and without shell integration) use the human-friendly
/// `"Created new worktree for {branch} from {base} @ {path}"` wording so
/// users see the same message regardless of how worktrunk is invoked.
///
/// Returns unstyled text - callers wrap in success_message() for Created
/// or info_message() for Switched (existing worktree).
fn format_switch_message(
    branch: &str,
    path: &Path,
    created_branch: bool,
    base_branch: Option<&str>,
    from_remote: Option<&str>,
) -> String {
    // Determine action and source based on how the worktree was created
    // Priority: explicit --create > DWIM from remote > existing local branch
    let (action, source) = if created_branch {
        ("Created new worktree for", base_branch)
    } else if let Some(remote) = from_remote {
        ("Created worktree for", Some(remote))
    } else {
        ("Switched to worktree for", None)
    };

    match source {
        Some(src) => cformat!(
            "{action} <bold>{branch}</> from <bold>{src}</> @ <bold>{}</>",
            format_path_for_display(path)
        ),
        None => cformat!(
            "{action} <bold>{branch}</> @ <bold>{}</>",
            format_path_for_display(path)
        ),
    }
}

/// Result of an integration check, including which target was used.
struct IntegrationResult {
    reason: Option<IntegrationReason>,
    /// The target that was actually checked against (may be upstream if ahead of local)
    effective_target: String,
}

/// Check if a branch's content has been integrated into the target.
///
/// Returns the reason if the branch is safe to delete (ordered by check cost):
/// - `SameCommit`: Branch HEAD is literally the same commit as target
/// - `NoAddedChanges`: Branch has no file changes beyond merge-base (empty three-dot diff)
/// - `TreesMatch`: The branch's tree SHA matches the target's tree SHA (squash merge/rebase)
/// - `MergeAddsNothing`: Merge simulation shows branch would add nothing (squash + target advanced)
///
/// Also returns the effective target used (may be upstream if it's ahead of local).
///
/// Returns None reason if no condition is met, or if an error occurs (e.g., invalid refs).
/// This fail-safe default prevents accidental branch deletion when integration cannot
/// be determined.
fn get_integration_reason(repo: &Repository, branch_name: &str, target: &str) -> IntegrationResult {
    let effective_target = repo.effective_integration_target(target);

    let reason = check_integration_against(repo, branch_name, &effective_target);

    IntegrationResult {
        reason,
        effective_target,
    }
}

/// Check integration against a specific target ref.
fn check_integration_against(
    repo: &Repository,
    branch_name: &str,
    target: &str,
) -> Option<IntegrationReason> {
    // Use lazy provider for short-circuit evaluation.
    // Expensive checks (would_merge_add) are skipped if cheaper ones succeed.
    let mut provider = worktrunk::git::LazyGitIntegration::new(repo, branch_name, target);
    worktrunk::git::check_integration(&mut provider)
}

/// Outcome of a branch deletion attempt.
enum BranchDeletionOutcome {
    /// Branch was not deleted (not integrated and not forced)
    NotDeleted,
    /// Branch was force-deleted without integration check
    ForceDeleted,
    /// Branch was deleted because it was integrated
    Integrated(IntegrationReason),
}

/// Result of a branch deletion attempt.
struct BranchDeletionResult {
    outcome: BranchDeletionOutcome,
    /// The target that was actually checked against (may be upstream if ahead of local)
    effective_target: String,
}

/// Attempt to delete a branch if it's integrated or force_delete is set.
///
/// Returns `BranchDeletionResult` with:
/// - `outcome`: Whether/why deletion occurred
/// - `effective_target`: The ref checked against (may be upstream if ahead of local)
fn delete_branch_if_safe(
    repo: &Repository,
    branch_name: &str,
    target: &str,
    force_delete: bool,
) -> anyhow::Result<BranchDeletionResult> {
    let IntegrationResult {
        reason,
        effective_target,
    } = get_integration_reason(repo, branch_name, target);

    // Determine outcome based on integration and force flag
    let outcome = match (reason, force_delete) {
        (Some(r), _) => {
            repo.run_command(&["branch", "-D", branch_name])?;
            BranchDeletionOutcome::Integrated(r)
        }
        (None, true) => {
            repo.run_command(&["branch", "-D", branch_name])?;
            BranchDeletionOutcome::ForceDeleted
        }
        (None, false) => BranchDeletionOutcome::NotDeleted,
    };

    Ok(BranchDeletionResult {
        outcome,
        effective_target,
    })
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
            super::gutter(format_with_gutter(&e.to_string(), None))?;
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

/// Shell integration hint message
fn shell_integration_hint() -> String {
    cformat!("Run <bright-black>wt config shell install</> to enable automatic cd")
}

/// Show switch message when changing directory after worktree removal
fn print_switch_message_if_changed(
    changed_directory: bool,
    main_path: &Path,
) -> anyhow::Result<()> {
    if changed_directory && let Ok(Some(dest_branch)) = Repository::at(main_path).current_branch() {
        let path_display = format_path_for_display(main_path);
        super::print(info_message(cformat!(
            "Switched to worktree for <bold>{dest_branch}</> @ <bold>{path_display}</>"
        )))?;
    }
    Ok(())
}

/// Handle output for a switch operation
///
/// When shell integration is not active and no execute command is provided,
/// we show warnings for operations that can't complete without shell integration.
pub fn handle_switch_output(
    result: &SwitchResult,
    branch_info: &SwitchBranchInfo,
    has_execute_command: bool,
) -> anyhow::Result<()> {
    // Set target directory for command execution
    super::change_directory(result.path())?;

    let path = result.path();
    let path_display = format_path_for_display(path);
    let branch = branch_info.branch();

    // Check if shell integration is active (directive file set)
    let is_shell_integration_active = super::is_shell_integration_active();

    // Show path mismatch warning after the main message
    let path_mismatch_warning = branch_info.expected_path.as_ref().map(|expected| {
        let expected_display = format_path_for_display(expected);
        warning_message(cformat!(
            "Worktree path doesn't match branch name; expected <bold>{expected_display}</> <red>⚑</>"
        ))
    });

    match result {
        SwitchResult::AlreadyAt(_) => {
            super::print(info_message(cformat!(
                "Already on worktree for <bold>{branch}</> @ <bold>{path_display}</>"
            )))?;
            if let Some(warning) = path_mismatch_warning {
                super::print(warning)?;
            }
        }
        SwitchResult::Existing(_) => {
            if is_shell_integration_active || has_execute_command {
                super::print(info_message(format_switch_message(
                    branch, path, false, None, None,
                )))?;
                if let Some(warning) = path_mismatch_warning {
                    super::print(warning)?;
                }
            } else if Shell::is_integration_configured(&crate::binary_name())
                .ok()
                .flatten()
                .is_some()
            {
                // Shell wrapper is configured but user ran binary directly
                super::print(warning_message(cformat!(
                    "Worktree for <bold>{branch}</> @ <bold>{path_display}</>, but cannot change directory — shell integration not active"
                )))?;
                if let Some(warning) = path_mismatch_warning {
                    super::print(warning)?;
                }
            } else {
                super::print(warning_message(cformat!(
                    "Worktree for <bold>{branch}</> @ <bold>{path_display}</>, but cannot change directory — shell integration not installed"
                )))?;
                if let Some(warning) = path_mismatch_warning {
                    super::print(warning)?;
                }
                super::shell_integration_hint(shell_integration_hint())?;
            }
        }
        SwitchResult::Created {
            created_branch,
            base_branch,
            from_remote,
            ..
        } => {
            super::print(success_message(format_switch_message(
                branch,
                path,
                *created_branch,
                base_branch.as_deref(),
                from_remote.as_deref(),
            )))?;
            // Show setup hint if no execute command (hint suppressed when shell integration is active)
            if !has_execute_command {
                super::shell_integration_hint(shell_integration_hint())?;
            }
        }
    }

    super::flush()?;
    Ok(())
}

/// Execute the --execute command after hooks have run
pub fn execute_user_command(command: &str) -> anyhow::Result<()> {
    use worktrunk::styling::format_bash_with_gutter;

    // Show what command is being executed (section header + gutter content)
    super::print(progress_message("Executing (--execute):"))?;
    super::gutter(format_bash_with_gutter(command))?;

    super::execute(command)?;

    Ok(())
}

/// Build shell command for background worktree removal
///
/// `branch_to_delete` is the branch to delete after removing the worktree.
/// Pass `None` for detached HEAD or when branch should be retained.
/// This decision is computed upfront (checking if branch is merged) before spawning the background process.
fn build_remove_command(worktree_path: &std::path::Path, branch_to_delete: Option<&str>) -> String {
    use shell_escape::escape;

    let worktree_path_str = worktree_path.to_string_lossy();
    let worktree_escaped = escape(worktree_path_str.as_ref().into());

    // Stop fsmonitor daemon first (best effort - ignore errors)
    // This prevents zombie daemons from accumulating when using builtin fsmonitor
    let stop_fsmonitor = format!(
        "git -C {} fsmonitor--daemon stop 2>/dev/null || true",
        worktree_escaped
    );

    match branch_to_delete {
        Some(branch_name) => {
            let branch_escaped = escape(branch_name.into());
            format!(
                "{} && git worktree remove {} && git branch -D {}",
                stop_fsmonitor, worktree_escaped, branch_escaped
            )
        }
        None => {
            format!(
                "{} && git worktree remove {}",
                stop_fsmonitor, worktree_escaped
            )
        }
    }
}

/// Handle output for a remove operation
///
/// Approval is handled at the gate (command entry point), not here.
pub fn handle_remove_output(
    result: &RemoveResult,
    branch: Option<&str>,
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
        } => handle_removed_worktree_output(
            main_path,
            worktree_path,
            *changed_directory,
            branch_name.as_deref(),
            *deletion_mode,
            target_branch.as_deref(),
            *integration_reason,
            branch,
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
    branch: Option<&str>,
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
        execute_pre_remove_commands(&ctx, None)?;
    }

    // Handle detached HEAD case (no branch known)
    let Some(branch_name) = branch_name else {
        // No branch associated - just remove the worktree
        if background {
            super::print(progress_message(
                "Removing worktree in background (detached HEAD, no branch to delete)",
            ))?;
            let remove_command = build_remove_command(worktree_path, None);
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
            if let Err(err) = repo.remove_worktree(worktree_path) {
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
        let remove_command =
            build_remove_command(worktree_path, should_delete_branch.then_some(branch_name));

        // Spawn the removal in background - runs from main_path (where we cd'd to)
        spawn_detached(
            &repo,
            main_path,
            &remove_command,
            branch_name,
            "remove",
            None,
        )?;

        super::flush()?;
        Ok(())
    } else {
        // Synchronous mode: remove immediately and report actual results

        // Stop fsmonitor daemon first (best effort - ignore errors)
        // This prevents zombie daemons from accumulating when using builtin fsmonitor
        let target_repo = worktrunk::git::Repository::at(worktree_path);
        let _ = target_repo.run_command(&["fsmonitor--daemon", "stop"]);

        // Track whether branch was actually deleted (will be computed based on deletion attempt)
        if let Err(err) = repo.remove_worktree(worktree_path) {
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
        // Use branch if provided, otherwise fall back to branch_name (which is always
        // Some here since we returned early for detached HEAD case on line 565)
        let branch_display = branch.unwrap_or(branch_name);

        // Message structure parallel to background mode:
        // - Branch deleted (integrated/force): "worktree & branch (reason)"
        // - Branch kept (any reason): "worktree" + hint (if relevant)
        let msg = if branch_deleted {
            let flag_note = get_flag_note(deletion_mode, &outcome, effective_target.as_deref());
            let flag_text = &flag_note.text;
            let flag_after = flag_note.after_green();
            cformat!(
                "<green>✓ Removed <bold>{branch_display}</> worktree & branch{flag_text}</>{flag_after}"
            )
        } else {
            // Branch kept: hint will explain why (integrated+flag, unmerged, or unmerged+flag)
            cformat!("<green>✓ Removed <bold>{branch_display}</> worktree</>")
        };
        super::print(FormattedMessage::new(msg))?;

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

        super::flush()?;
        Ok(())
    }
}

#[cfg(unix)]
fn process_group_alive(pgid: i32) -> bool {
    match nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pgid), None) {
        Ok(_) => true,
        Err(nix::errno::Errno::ESRCH) => false,
        Err(_) => true,
    }
}

#[cfg(unix)]
fn wait_for_exit(pgid: i32, grace: std::time::Duration) -> bool {
    std::thread::sleep(grace);
    !process_group_alive(pgid)
}

#[cfg(unix)]
fn forward_signal_with_escalation(pgid: i32, sig: i32) {
    let pgid = nix::unistd::Pid::from_raw(pgid);
    let initial_signal = match sig {
        signal_hook::consts::SIGINT => nix::sys::signal::Signal::SIGINT,
        signal_hook::consts::SIGTERM => nix::sys::signal::Signal::SIGTERM,
        _ => return,
    };

    let _ = nix::sys::signal::killpg(pgid, initial_signal);

    let grace = std::time::Duration::from_millis(200);
    match sig {
        signal_hook::consts::SIGINT => {
            if !wait_for_exit(pgid.as_raw(), grace) {
                let _ = nix::sys::signal::killpg(pgid, nix::sys::signal::Signal::SIGTERM);
                if !wait_for_exit(pgid.as_raw(), grace) {
                    let _ = nix::sys::signal::killpg(pgid, nix::sys::signal::Signal::SIGKILL);
                }
            }
        }
        signal_hook::consts::SIGTERM => {
            if !wait_for_exit(pgid.as_raw(), grace) {
                let _ = nix::sys::signal::killpg(pgid, nix::sys::signal::Signal::SIGKILL);
            }
        }
        _ => {}
    }
}

/// Execute a command with streaming output
///
/// Uses Stdio::inherit for stderr to preserve TTY behavior - this ensures commands like cargo
/// detect they're connected to a terminal and don't buffer their output.
///
/// If `redirect_stdout_to_stderr` is true, redirects child stdout to our stderr at the OS level
/// (via `Stdio::from(io::stderr())`). This ensures deterministic output ordering (all child output
/// flows through stderr). Per CLAUDE.md: child process output goes to stderr, worktrunk output
/// goes to stdout.
///
/// If `stdin_content` is provided, it will be piped to the command's stdin (used for hook context JSON).
///
/// If `inherit_stdin` is true and `stdin_content` is None, stdin is inherited from the parent process,
/// enabling interactive programs (like `claude`, `vim`, or `python -i`) to read user input.
/// If false and `stdin_content` is None, stdin is set to null (appropriate for non-interactive hooks).
///
/// Returns error if command exits with non-zero status.
///
/// ## Cross-Platform Shell Execution
///
/// Uses the platform's preferred shell via `shell_exec::ShellConfig`:
/// - Unix: `/bin/sh -c`
/// - Windows: Git Bash if available, PowerShell fallback
///
/// ## Signal Handling (Unix)
///
/// When `forward_signals` is true, the child is spawned in its own process group and
/// SIGINT/SIGTERM received by the parent are forwarded to that group so we can abort
/// the entire command tree without shell-wrapping. If the process group does not exit
/// promptly, we escalate to SIGTERM/SIGKILL (SIGINT path) or SIGKILL (SIGTERM path).
/// We still return exit code 128 + signal number (e.g., 130 for SIGINT) to match Unix conventions.
pub(crate) fn execute_streaming(
    command: &str,
    working_dir: &std::path::Path,
    redirect_stdout_to_stderr: bool,
    stdin_content: Option<&str>,
    inherit_stdin: bool,
    forward_signals: bool,
) -> anyhow::Result<()> {
    use std::io::Write;
    use worktrunk::git::WorktrunkError;
    use worktrunk::shell_exec::ShellConfig;
    #[cfg(unix)]
    use {
        signal_hook::consts::{SIGINT, SIGTERM},
        signal_hook::iterator::Signals,
        std::os::unix::process::CommandExt,
    };

    let shell = ShellConfig::get();
    #[cfg(not(unix))]
    let _ = forward_signals;

    // Determine stdout handling based on redirect flag
    // When redirecting, use Stdio::from(stderr) to redirect child stdout to our stderr at OS level.
    // This keeps stdout reserved for data output while hook output goes to stderr.
    // Previously used shell-level `{ cmd } 1>&2` wrapping, but OS-level redirect is simpler
    // and may improve signal handling by removing an extra shell process layer.
    let stdout_mode = if redirect_stdout_to_stderr {
        std::process::Stdio::from(std::io::stderr())
    } else {
        std::process::Stdio::inherit()
    };

    let stdin_mode = if stdin_content.is_some() {
        std::process::Stdio::piped()
    } else if inherit_stdin {
        std::process::Stdio::inherit()
    } else {
        std::process::Stdio::null()
    };

    #[cfg(unix)]
    let mut signals = if forward_signals {
        Some(Signals::new([SIGINT, SIGTERM])?)
    } else {
        None
    };

    let mut cmd = shell.command(command);
    #[cfg(unix)]
    if forward_signals {
        // Isolate the child in its own process group so we can signal the whole tree.
        cmd.process_group(0);
    }
    let mut child = cmd
        .current_dir(working_dir)
        .stdin(stdin_mode)
        .stdout(stdout_mode)
        .stderr(std::process::Stdio::inherit()) // Preserve TTY for errors
        // Prevent vergen "overridden" warning in nested cargo builds when run via `cargo run`.
        // Add more VERGEN_* variables here if we expand build.rs and hit similar issues.
        .env_remove("VERGEN_GIT_DESCRIBE")
        // Prevent hooks from writing to the directive file
        .env_remove(worktrunk::shell_exec::DIRECTIVE_FILE_ENV_VAR)
        .spawn()
        .map_err(|e| {
            anyhow::Error::from(worktrunk::git::GitError::Other {
                message: format!("Failed to execute command with {}: {}", shell.name, e),
            })
        })?;

    // Write stdin content if provided (used for hook context JSON)
    // We ignore write errors here because:
    // 1. The child may have already exited (broken pipe)
    // 2. Hooks that don't read stdin will still work
    // 3. Hooks that need stdin will fail with their own error message
    if let Some(content) = stdin_content
        && let Some(mut stdin) = child.stdin.take()
    {
        // Write and close stdin immediately so the child doesn't block waiting for more input
        let _ = stdin.write_all(content.as_bytes());
        // stdin is dropped here, closing the pipe
    }

    #[cfg(unix)]
    let (status, seen_signal) = if forward_signals {
        let child_pgid = child.id() as i32;
        let mut seen_signal: Option<i32> = None;
        loop {
            if let Some(status) = child.try_wait().map_err(|e| {
                anyhow::Error::from(worktrunk::git::GitError::Other {
                    message: format!("Failed to wait for command: {}", e),
                })
            })? {
                break (status, seen_signal);
            }
            if let Some(signals) = signals.as_mut() {
                for sig in signals.pending() {
                    if seen_signal.is_none() {
                        seen_signal = Some(sig);
                        forward_signal_with_escalation(child_pgid, sig);
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    } else {
        let status = child.wait().map_err(|e| {
            anyhow::Error::from(worktrunk::git::GitError::Other {
                message: format!("Failed to wait for command: {}", e),
            })
        })?;
        (status, None)
    };

    #[cfg(not(unix))]
    let status = child.wait().map_err(|e| {
        anyhow::Error::from(worktrunk::git::GitError::Other {
            message: format!("Failed to wait for command: {}", e),
        })
    })?;

    #[cfg(unix)]
    if let Some(sig) = seen_signal {
        return Err(WorktrunkError::ChildProcessExited {
            code: 128 + sig,
            message: format!("terminated by signal {}", sig),
        }
        .into());
    }

    // Check if child was killed by a signal (Unix only)
    // This handles Ctrl-C: when SIGINT is sent, the child receives it and terminates,
    // and we propagate the signal exit code (128 + signal number, e.g., 130 for SIGINT)
    #[cfg(unix)]
    if let Some(sig) = std::os::unix::process::ExitStatusExt::signal(&status) {
        return Err(WorktrunkError::ChildProcessExited {
            code: 128 + sig,
            message: format!("terminated by signal {}", sig),
        }
        .into());
    }

    if !status.success() {
        // Get the exit code if available (None means terminated by signal on some platforms)
        let code = status.code().unwrap_or(1);
        return Err(WorktrunkError::ChildProcessExited {
            code,
            message: format!("exit status: {}", code),
        }
        .into());
    }

    Ok(())
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

        // Switched to existing worktree (no creation, no remote)
        let msg = format_switch_message("feature", &path, false, None, None);
        assert!(msg.contains("Switched to worktree for"));
        assert!(msg.contains("feature"));

        // Created new worktree from base branch
        let msg = format_switch_message("feature", &path, true, Some("main"), None);
        assert!(msg.contains("Created new worktree for"));
        assert!(msg.contains("from"));
        assert!(msg.contains("main"));

        // Created worktree from remote (DWIM)
        let msg = format_switch_message("feature", &path, false, None, Some("origin/feature"));
        assert!(msg.contains("Created worktree for"));
        assert!(msg.contains("origin/feature"));
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
    fn test_build_remove_command() {
        let path = PathBuf::from("/tmp/test-worktree");

        // Without branch deletion
        let cmd = build_remove_command(&path, None);
        assert!(cmd.contains("git worktree remove"));
        assert!(cmd.contains("/tmp/test-worktree"));
        assert!(!cmd.contains("branch -D"));

        // With branch deletion
        let cmd = build_remove_command(&path, Some("feature-branch"));
        assert!(cmd.contains("git worktree remove"));
        assert!(cmd.contains("git branch -D"));
        assert!(cmd.contains("feature-branch"));

        // Shell escaping for special characters
        let special_path = PathBuf::from("/tmp/test worktree");
        let cmd = build_remove_command(&special_path, Some("feature/branch"));
        assert!(cmd.contains("worktree remove"));
    }

    #[test]
    fn test_branch_deletion_outcome_matching() {
        // Ensure the match patterns work correctly
        let outcomes = [
            (BranchDeletionOutcome::NotDeleted, false),
            (BranchDeletionOutcome::ForceDeleted, true),
            (
                BranchDeletionOutcome::Integrated(IntegrationReason::SameCommit),
                true,
            ),
        ];
        for (outcome, expected_deleted) in outcomes {
            let deleted = matches!(
                outcome,
                BranchDeletionOutcome::ForceDeleted | BranchDeletionOutcome::Integrated(_)
            );
            assert_eq!(deleted, expected_deleted);
        }
    }
}
