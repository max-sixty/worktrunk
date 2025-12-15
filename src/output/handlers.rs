//! Output handlers for worktree operations using the global output context

use color_print::cformat;
use std::path::Path;

use crate::commands::command_executor::CommandContext;
use crate::commands::execute_pre_remove_commands;
use crate::commands::process::spawn_detached;
use crate::commands::worktree::{RemoveResult, SwitchBranchInfo, SwitchResult};
use worktrunk::config::WorktrunkConfig;
use worktrunk::git::GitError;
use worktrunk::git::IntegrationReason;
use worktrunk::git::Repository;
use worktrunk::git::path_dir_name;
use worktrunk::path::format_path_for_display;
use worktrunk::shell::Shell;
use worktrunk::styling::{
    error_message, format_with_gutter, hint_message, info_message, progress_message,
    success_message, warning_message,
};

/// Format a switch success message with a consistent location phrase
///
/// Both interactive and directive modes now use the human-friendly
/// `"Created new worktree for {branch} from {base} @ {path}"` wording so
/// users see the same message regardless of how worktrunk is invoked.
fn format_switch_success_message(
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
            "<green>{action} <bold>{branch}</> from <bold>{src}</> @ <bold>{}</></>",
            format_path_for_display(path)
        ),
        None => cformat!(
            "<green>{action} <bold>{branch}</> @ <bold>{}</></>",
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
                super::print(hint_message(cformat!(
                    "<bright-black>wt remove -D</> deletes unmerged branches"
                )))?;
            }
            Ok((r, defer_output))
        }
        Err(e) => {
            // Git command failed - this is an error (we decided to delete but couldn't)
            super::print(error_message(cformat!(
                "Failed to delete branch <bold>{branch_name}</>"
            )))?;
            super::gutter(format_with_gutter(&e.to_string(), "", None))?;
            Err(e)
        }
    }
}

/// Get flag acknowledgment note for remove messages
///
/// `target_branch`: The branch we checked integration against (shown in reason)
fn get_flag_note(
    no_delete_branch: bool,
    outcome: &BranchDeletionOutcome,
    target_branch: Option<&str>,
) -> String {
    if no_delete_branch {
        return " (--no-delete-branch)".to_string();
    }

    match outcome {
        BranchDeletionOutcome::NotDeleted => String::new(),
        BranchDeletionOutcome::ForceDeleted => " (--force-delete)".to_string(),
        BranchDeletionOutcome::Integrated(reason) => {
            let Some(target) = target_branch else {
                return String::new();
            };
            let desc = reason.description();
            match reason {
                IntegrationReason::NoAddedChanges => format!(" ({desc})"),
                _ => format!(" ({desc} {target})"),
            }
        }
    }
}

/// Format message for remove worktree operation (includes emoji and color for consistency)
///
/// `target_branch`: The branch we checked integration against (Some = merge context, None = explicit remove)
#[allow(clippy::too_many_arguments)]
fn format_remove_worktree_message(
    main_path: &std::path::Path,
    changed_directory: bool,
    branch_name: &str,
    branch: Option<&str>,
    no_delete_branch: bool,
    outcome: &BranchDeletionOutcome,
    target_branch: Option<&str>,
) -> String {
    let flag_note = get_flag_note(no_delete_branch, outcome, target_branch);

    let branch_display = branch.or(Some(branch_name));
    let path_display = format_path_for_display(main_path);

    // Build message parallel to background format: "Removed {branch} worktree & branch{flag_note}"
    let branch_deleted = matches!(
        outcome,
        BranchDeletionOutcome::ForceDeleted | BranchDeletionOutcome::Integrated(_)
    );

    // Determine action suffix based on what happened to the branch
    let action_suffix = if branch_deleted {
        "worktree & branch"
    } else if no_delete_branch {
        // User explicitly kept the branch via --no-delete-branch
        "worktree"
    } else if matches!(outcome, BranchDeletionOutcome::NotDeleted) {
        // Branch retained because it has unmerged changes
        "worktree; retaining unmerged branch"
    } else {
        "worktree"
    };

    if changed_directory {
        if let Some(b) = branch_display {
            cformat!(
                "Removed <bold>{b}</> {action_suffix}; changed directory to <bold>{path_display}</>{flag_note}"
            )
        } else {
            cformat!(
                "Removed {action_suffix}; changed directory to <bold>{path_display}</>{flag_note}"
            )
        }
    } else if let Some(b) = branch_display {
        cformat!("Removed <bold>{b}</> {action_suffix}{flag_note}")
    } else {
        format!("Removed {action_suffix}{flag_note}")
    }
}

/// Shell integration hint message
fn shell_integration_hint() -> String {
    cformat!("Run <bright-black>wt config shell install</> to enable automatic cd")
}

/// Handle output for a switch operation
///
/// `is_directive_mode` indicates whether shell integration is active (via --internal flag).
/// When false, we show warnings for operations that can't complete without shell integration.
pub fn handle_switch_output(
    result: &SwitchResult,
    branch_info: &SwitchBranchInfo,
    has_execute_command: bool,
    is_directive_mode: bool,
) -> anyhow::Result<()> {
    // Set target directory for command execution
    super::change_directory(result.path())?;

    let path = result.path();
    let path_display = format_path_for_display(path);
    let expected_branch = &branch_info.expected;

    // Handle branch mismatch cases (worktree checked out to wrong branch or detached)
    if branch_info.is_branch_mismatch() {
        let mismatch_info = match &branch_info.current {
            Some(current) => {
                cformat!("expecting <bold>{expected_branch}</>, got <bold>{current}</>")
            }
            None => cformat!("expecting <bold>{expected_branch}</>, got detached HEAD"),
        };

        match result {
            SwitchResult::AlreadyAt(_) => {
                super::print(warning_message(cformat!(
                    "Already at <bold>{path_display}</>. Branch doesn't match path: {mismatch_info}"
                )))?;
            }
            SwitchResult::Existing(_) => {
                if is_directive_mode || has_execute_command {
                    super::print(warning_message(cformat!(
                        "Switched to <bold>{path_display}</>. Branch doesn't match path: {mismatch_info}"
                    )))?;
                } else if Shell::is_integration_configured().ok().flatten().is_some() {
                    super::print(warning_message(cformat!(
                        "Worktree at <bold>{path_display}</>. Branch doesn't match path: {mismatch_info} (cannot cd, binary invoked directly)"
                    )))?;
                } else {
                    super::print(warning_message(cformat!(
                        "Worktree at <bold>{path_display}</>. Branch doesn't match path: {mismatch_info} (cannot cd, no shell integration)"
                    )))?;
                    super::shell_integration_hint(shell_integration_hint())?;
                }
            }
            SwitchResult::Created { .. } => {
                // Created case should never have mismatch - we just created it
                unreachable!("Created worktree should not have branch mismatch");
            }
        }

        super::flush()?;
        return Ok(());
    }

    // Normal case - branch matches
    let branch = branch_info.branch();

    // Show path mismatch warning after the main message
    let path_mismatch_warning = branch_info.expected_path.as_ref().map(|expected| {
        let expected_display = format_path_for_display(expected);
        warning_message(format!(
            "Worktree path doesn't match naming template (expected {expected_display})"
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
            if is_directive_mode || has_execute_command {
                super::print(success_message(format_switch_success_message(
                    branch, path, false, None, None,
                )))?;
                if let Some(warning) = path_mismatch_warning {
                    super::print(warning)?;
                }
            } else if Shell::is_integration_configured().ok().flatten().is_some() {
                super::print(warning_message(cformat!(
                    "Worktree for <bold>{branch}</> @ <bold>{path_display}</>; cannot cd (binary invoked directly)"
                )))?;
                if let Some(warning) = path_mismatch_warning {
                    super::print(warning)?;
                }
            } else {
                super::print(warning_message(cformat!(
                    "Worktree for <bold>{branch}</> @ <bold>{path_display}</>; cannot cd (no shell integration)"
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
            super::print(success_message(format_switch_success_message(
                branch,
                path,
                *created_branch,
                base_branch.as_deref(),
                from_remote.as_deref(),
            )))?;
            // Show setup hint if no execute command (hint suppressed in directive mode)
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
    super::gutter(format_bash_with_gutter(command, ""))?;

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
            no_delete_branch,
            force_delete,
            target_branch,
        } => handle_removed_worktree_output(
            main_path,
            worktree_path,
            *changed_directory,
            branch_name.as_deref(),
            *no_delete_branch,
            *force_delete,
            target_branch.as_deref(),
            branch,
            background,
            verify,
        ),
        RemoveResult::BranchOnly {
            branch_name,
            no_delete_branch,
            force_delete,
        } => handle_branch_only_output(branch_name, *no_delete_branch, *force_delete),
    }
}

/// Handle output for BranchOnly removal (branch exists but no worktree)
fn handle_branch_only_output(
    branch_name: &str,
    no_delete_branch: bool,
    force_delete: bool,
) -> anyhow::Result<()> {
    // Warn that no worktree was found (user asked to remove it)
    super::print(warning_message(cformat!(
        "No worktree found for branch <bold>{branch_name}</>"
    )))?;

    // Attempt branch deletion (unless --no-delete-branch was specified)
    if no_delete_branch {
        // User explicitly requested no branch deletion - nothing more to do
        super::flush()?;
        return Ok(());
    }

    let repo = worktrunk::git::Repository::current();

    // Get default branch for integration check and reason display
    // Falls back to HEAD if default branch can't be determined
    let default_branch = repo.default_branch().ok();
    let check_target = default_branch.as_deref().unwrap_or("HEAD");

    let result = delete_branch_if_safe(&repo, branch_name, check_target, force_delete);
    let (deletion, _) = handle_branch_deletion_result(result, branch_name, false)?;

    if !matches!(deletion.outcome, BranchDeletionOutcome::NotDeleted) {
        let flag_note = get_flag_note(
            no_delete_branch,
            &deletion.outcome,
            Some(&deletion.effective_target),
        );
        super::print(success_message(cformat!(
            "Removed branch <bold>{branch_name}</>{flag_note}"
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
    no_delete_branch: bool,
    force_delete: bool,
    target_branch: Option<&str>,
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

        // Determine outcome upfront (check once, not in background script)
        // Only show effective_target in message if we had a meaningful target (not tautological "HEAD" fallback)
        let (outcome, effective_target) = if no_delete_branch {
            (
                BranchDeletionOutcome::NotDeleted,
                target_branch.map(String::from),
            )
        } else if force_delete {
            (
                BranchDeletionOutcome::ForceDeleted,
                target_branch.map(String::from),
            )
        } else {
            // Check if branch is integrated
            let check_target = target_branch.unwrap_or("HEAD");
            let deletion_repo = worktrunk::git::Repository::at(main_path);
            let IntegrationResult {
                reason,
                effective_target,
            } = get_integration_reason(&deletion_repo, branch_name, check_target);
            // Only use effective_target for display if we had a real target (not "HEAD" fallback)
            let display_target = target_branch.map(|_| effective_target);
            let outcome = match reason {
                Some(r) => BranchDeletionOutcome::Integrated(r),
                None => BranchDeletionOutcome::NotDeleted,
            };
            (outcome, display_target)
        };

        let should_delete_branch = matches!(
            outcome,
            BranchDeletionOutcome::ForceDeleted | BranchDeletionOutcome::Integrated(_)
        );

        let flag_note = get_flag_note(no_delete_branch, &outcome, effective_target.as_deref());

        // Reason in parentheses: user flags shown explicitly, integration reason for automatic cleanup
        let action = if no_delete_branch {
            cformat!(
                "<cyan>Removing <bold>{branch_name}</> worktree in background; retaining branch{flag_note}</>"
            )
        } else if should_delete_branch {
            cformat!(
                "<cyan>Removing <bold>{branch_name}</> worktree & branch in background{flag_note}</>"
            )
        } else {
            cformat!(
                "<cyan>Removing <bold>{branch_name}</> worktree in background; retaining unmerged branch</>"
            )
        };
        super::print(progress_message(action))?;

        // Show hint for unmerged branches (same as synchronous path)
        if !no_delete_branch && !should_delete_branch {
            super::print(hint_message(cformat!(
                "<bright-black>wt remove -D</> deletes unmerged branches"
            )))?;
        }

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
        let (outcome, effective_target, show_hint) = if !no_delete_branch {
            let deletion_repo = worktrunk::git::Repository::at(main_path);
            let check_target = target_branch.unwrap_or("HEAD");
            let result =
                delete_branch_if_safe(&deletion_repo, branch_name, check_target, force_delete);
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

        // Show success message (includes emoji and color)
        super::print(success_message(format_remove_worktree_message(
            main_path,
            changed_directory,
            branch_name,
            branch,
            no_delete_branch,
            &outcome,
            effective_target.as_deref(),
        )))?;

        // Show hint for unmerged branches (after success message)
        if show_hint {
            super::print(hint_message(cformat!(
                "<bright-black>wt remove -D</> deletes unmerged branches"
            )))?;
        }

        super::flush()?;
        Ok(())
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
/// SIGINT (Ctrl-C) is handled by checking the child's exit status:
/// - If the child was killed by a signal, we return exit code 128 + signal number
/// - This follows Unix conventions (e.g., exit code 130 for SIGINT)
///
/// The child process receives SIGINT directly from the terminal (via Stdio::inherit for stderr).
pub(crate) fn execute_streaming(
    command: &str,
    working_dir: &std::path::Path,
    redirect_stdout_to_stderr: bool,
    stdin_content: Option<&str>,
    inherit_stdin: bool,
) -> anyhow::Result<()> {
    use std::io::Write;
    use worktrunk::git::WorktrunkError;
    use worktrunk::shell_exec::ShellConfig;

    let shell = ShellConfig::get();

    // Determine stdout handling based on redirect flag
    // When redirecting, use Stdio::from(stderr) to redirect child stdout to our stderr at OS level.
    // This keeps stdout clean for directive scripts while hook output goes to stderr.
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

    let mut cmd = shell.command(command);
    let mut child = cmd
        .current_dir(working_dir)
        .stdin(stdin_mode)
        .stdout(stdout_mode)
        .stderr(std::process::Stdio::inherit()) // Preserve TTY for errors
        // Prevent vergen "overridden" warning in nested cargo builds when run via `cargo run`.
        // Add more VERGEN_* variables here if we expand build.rs and hit similar issues.
        .env_remove("VERGEN_GIT_DESCRIBE")
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

    // Wait for command to complete
    let status = child.wait().map_err(|e| {
        anyhow::Error::from(worktrunk::git::GitError::Other {
            message: format!("Failed to wait for command: {}", e),
        })
    })?;

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
    execute_streaming(command, worktree_path, true, stdin_content, false)?;

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
    fn test_format_switch_success_message() {
        let path = PathBuf::from("/tmp/test");

        // Switched to existing worktree (no creation, no remote)
        let msg = format_switch_success_message("feature", &path, false, None, None);
        assert!(msg.contains("Switched to worktree for"));
        assert!(msg.contains("feature"));

        // Created new worktree from base branch
        let msg = format_switch_success_message("feature", &path, true, Some("main"), None);
        assert!(msg.contains("Created new worktree for"));
        assert!(msg.contains("from"));
        assert!(msg.contains("main"));

        // Created worktree from remote (DWIM)
        let msg =
            format_switch_success_message("feature", &path, false, None, Some("origin/feature"));
        assert!(msg.contains("Created worktree for"));
        assert!(msg.contains("origin/feature"));
    }

    #[test]
    fn test_get_flag_note() {
        // --no-delete-branch flag
        assert_eq!(
            get_flag_note(true, &BranchDeletionOutcome::NotDeleted, None),
            " (--no-delete-branch)"
        );

        // NotDeleted without flag
        assert_eq!(
            get_flag_note(false, &BranchDeletionOutcome::NotDeleted, None),
            ""
        );

        // Force deleted
        assert_eq!(
            get_flag_note(false, &BranchDeletionOutcome::ForceDeleted, None),
            " (--force-delete)"
        );

        // Integration reasons
        let cases = [
            (IntegrationReason::SameCommit, "same commit as main"),
            (IntegrationReason::Ancestor, "ancestor of main"),
            (IntegrationReason::TreesMatch, "tree matches main"),
            (IntegrationReason::MergeAddsNothing, "all changes in main"),
        ];
        for (reason, expected_contains) in cases {
            let note = get_flag_note(
                false,
                &BranchDeletionOutcome::Integrated(reason),
                Some("main"),
            );
            assert!(
                note.contains(expected_contains),
                "reason {:?} should contain '{}'",
                reason,
                expected_contains
            );
        }

        // NoAddedChanges is special - no target shown
        let note = get_flag_note(
            false,
            &BranchDeletionOutcome::Integrated(IntegrationReason::NoAddedChanges),
            Some("main"),
        );
        assert!(note.contains("no added changes"));
        assert!(!note.contains("main"));
    }

    #[test]
    fn test_format_remove_worktree_message() {
        let main_path = PathBuf::from("/home/user/project");

        // Removed worktree and branch (integrated)
        let msg = format_remove_worktree_message(
            &main_path,
            false,
            "feature",
            Some("feature"),
            false,
            &BranchDeletionOutcome::Integrated(IntegrationReason::SameCommit),
            Some("main"),
        );
        assert!(msg.contains("feature"));
        assert!(msg.contains("worktree & branch"));
        assert!(msg.contains("same commit as main"));

        // Removed worktree only (--no-delete-branch)
        let msg = format_remove_worktree_message(
            &main_path,
            false,
            "feature",
            Some("feature"),
            true,
            &BranchDeletionOutcome::NotDeleted,
            None,
        );
        assert!(msg.contains("worktree"));
        assert!(!msg.contains("& branch"));
        assert!(msg.contains("--no-delete-branch"));

        // Removed with directory change
        let msg = format_remove_worktree_message(
            &main_path,
            true,
            "feature",
            Some("feature"),
            false,
            &BranchDeletionOutcome::ForceDeleted,
            None,
        );
        assert!(msg.contains("changed directory"));
        assert!(msg.contains("--force-delete"));

        // Unmerged branch retained
        let msg = format_remove_worktree_message(
            &main_path,
            false,
            "feature",
            Some("feature"),
            false,
            &BranchDeletionOutcome::NotDeleted,
            None,
        );
        assert!(msg.contains("retaining unmerged branch"));
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
