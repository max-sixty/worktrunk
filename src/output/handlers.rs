//! Output handlers for worktree operations using the global output context

use color_print::cformat;
use std::path::Path;

use crate::commands::command_executor::CommandContext;
use crate::commands::execute_pre_remove_commands;
use crate::commands::process::spawn_detached;
use crate::commands::worktree::{RemoveResult, SwitchResult};
use worktrunk::config::WorktrunkConfig;
use worktrunk::git::GitError;
use worktrunk::git::Repository;
use worktrunk::path::format_path_for_display;
use worktrunk::shell::Shell;
use worktrunk::styling::{HINT_EMOJI, format_with_gutter};

/// Format a switch success message with a consistent location phrase
///
/// Both interactive and directive modes now use the human-friendly
/// `"Created new worktree for {branch} from {base} at {path}"` wording so
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
            "<green>{action} <bold>{branch}</> from <bold>{src}</> at <bold>{}</></>",
            format_path_for_display(path)
        ),
        None => cformat!(
            "<green>{action} <bold>{branch}</> at <bold>{}</></>",
            format_path_for_display(path)
        ),
    }
}

/// Why a branch is considered integrated into the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntegrationReason {
    /// Branch has no file changes beyond the merge-base with target
    NoAddedChanges,
    /// Branch's tree SHA matches target's tree SHA (squash merge/rebase)
    ContentsMatch,
    /// Merge simulation shows branch would add nothing to target
    /// (squash merge where target has since advanced)
    MergeAddsNothing,
}

/// Check if a branch's content has been integrated into the target.
///
/// Returns the reason if the branch is safe to delete:
/// - `NoAddedChanges`: Branch has no file changes beyond merge-base (empty three-dot diff)
/// - `ContentsMatch`: The branch's tree SHA matches the target's tree SHA (squash merge/rebase)
/// - `MergeAddsNothing`: Merge simulation shows branch would add nothing (squash + target advanced)
///
/// Returns None if neither condition is met, or if an error occurs (e.g., invalid refs).
/// This fail-safe default prevents accidental branch deletion when integration cannot
/// be determined.
fn get_integration_reason(
    repo: &Repository,
    branch_name: &str,
    target: &str,
) -> Option<IntegrationReason> {
    // Check if branch has no file changes beyond merge-base (empty three-dot diff)
    // On error, conservatively assume branch HAS changes (won't delete)
    if !repo.has_added_changes(branch_name, target).unwrap_or(true) {
        return Some(IntegrationReason::NoAddedChanges);
    }

    // Check if tree content matches (handles squash merge/rebase)
    if repo.trees_match(branch_name, target).unwrap_or(false) {
        return Some(IntegrationReason::ContentsMatch);
    }

    // Check via merge simulation: would merging this branch into target add anything?
    // This handles squash-merged branches where target has since advanced.
    // If merge would NOT add anything, the branch's content is already in target.
    if !repo
        .would_merge_add_to_target(branch_name, target)
        .unwrap_or(true)
    {
        return Some(IntegrationReason::MergeAddsNothing);
    }

    None
}

/// Attempt to delete a branch if it's integrated or force_delete is set.
///
/// Returns:
/// - `Ok(Some(reason))` if branch was deleted due to integration
/// - `Ok(Some(None))` if branch was force-deleted (no integration reason)
/// - `Ok(None)` if branch was not deleted (not integrated and not forced)
/// - `Err` if git command failed
///
/// The outer Option indicates whether deletion occurred; the inner Option
/// indicates the integration reason (None when force-deleted).
fn delete_branch_if_safe(
    repo: &Repository,
    branch_name: &str,
    target: &str,
    force_delete: bool,
) -> anyhow::Result<Option<Option<IntegrationReason>>> {
    let reason = get_integration_reason(repo, branch_name, target);

    // Delete if integrated or force-delete requested
    if reason.is_none() && !force_delete {
        return Ok(None); // Not integrated and not forced - don't delete
    }

    repo.run_command(&["branch", "-D", branch_name])?;
    Ok(Some(reason)) // Deleted: Some(Some(reason)) if integrated, Some(None) if forced
}

/// Handle the result of a branch deletion attempt.
///
/// Shows appropriate messages for non-deleted branches:
/// - `Ok(None)`: We checked and chose not to delete (not integrated) - show info
/// - `Err(e)`: Git command failed - show warning with actual error
///
/// Returns:
/// - `Ok(Some(reason))` if branch was deleted (reason is None if force-deleted)
/// - `Ok(None)` if branch was not deleted
fn handle_branch_deletion_result(
    result: anyhow::Result<Option<Option<IntegrationReason>>>,
    branch_name: &str,
) -> anyhow::Result<Option<Option<IntegrationReason>>> {
    match result {
        Ok(Some(reason)) => Ok(Some(reason)), // Deleted (with or without integration reason)
        Ok(None) => {
            // Branch not integrated - we chose not to delete (not a failure)
            super::info(cformat!(
                "Branch <bold>{branch_name}</> retained; has unmerged changes"
            ))?;
            super::print(cformat!(
                "{HINT_EMOJI} <dim>Use </>wt remove -D<dim> to delete unmerged branches</>"
            ))?;
            Ok(None)
        }
        Err(e) => {
            // Git command failed - this is an error (we decided to delete but couldn't)
            super::error(cformat!("Failed to delete branch <bold>{branch_name}</>"))?;
            super::gutter(format_with_gutter(&e.to_string(), "", None))?;
            Err(e)
        }
    }
}

/// Get flag acknowledgment note for remove messages
///
/// `deletion_result`: None = not deleted, Some(None) = force-deleted, Some(Some(reason)) = integrated
/// `target_branch`: The branch we checked integration against (shown in reason)
fn get_flag_note(
    no_delete_branch: bool,
    force_delete: bool,
    deletion_result: Option<Option<IntegrationReason>>,
    target_branch: Option<&str>,
) -> String {
    if no_delete_branch {
        " (--no-delete-branch)".to_string()
    } else if force_delete && deletion_result.is_some() {
        " (--force-delete)".to_string()
    } else if let Some(target) = target_branch {
        // Show integration reason when branch is deleted (both wt merge and wt remove)
        match deletion_result {
            Some(Some(IntegrationReason::NoAddedChanges)) => {
                format!(" (already in {target})")
            }
            Some(Some(IntegrationReason::ContentsMatch)) => format!(" (files match {target})"),
            Some(Some(IntegrationReason::MergeAddsNothing)) => {
                format!(" (all changes in {target})")
            }
            Some(None) | None => String::new(),
        }
    } else {
        // No target branch available (e.g., couldn't resolve default branch)
        String::new()
    }
}

/// Format message for remove worktree operation (includes emoji and color for consistency)
///
/// `deletion_result`: None = not deleted, Some(None) = force-deleted, Some(Some(reason)) = integrated
/// `target_branch`: The branch we checked integration against (Some = merge context, None = explicit remove)
#[allow(clippy::too_many_arguments)]
fn format_remove_worktree_message(
    main_path: &std::path::Path,
    changed_directory: bool,
    branch_name: &str,
    branch: Option<&str>,
    no_delete_branch: bool,
    force_delete: bool,
    deletion_result: Option<Option<IntegrationReason>>,
    target_branch: Option<&str>,
) -> String {
    // Show flag acknowledgment when applicable
    let flag_note = get_flag_note(
        no_delete_branch,
        force_delete,
        deletion_result,
        target_branch,
    );

    let branch_display = branch.or(Some(branch_name));
    let path_display = format_path_for_display(main_path);

    // Build message parallel to background format: "Removed {branch} worktree & branch{flag_note}"
    let action_suffix = if no_delete_branch || deletion_result.is_none() {
        "worktree"
    } else {
        "worktree & branch"
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
    branch: &str,
    has_execute_command: bool,
    is_directive_mode: bool,
) -> anyhow::Result<()> {
    // Set target directory for command execution
    super::change_directory(result.path())?;

    // Show message based on result type and mode
    match result {
        SwitchResult::AlreadyAt(path) => {
            // Already at target - show info, no hint needed
            super::info(cformat!(
                "Already on worktree for <bold>{branch}</> at <bold>{}</>",
                format_path_for_display(path)
            ))?;
        }
        SwitchResult::Existing(path) => {
            // Check if we can cd or if shell integration is at least configured
            let is_configured = Shell::is_integration_configured().ok().flatten().is_some();

            if is_directive_mode || has_execute_command || is_configured {
                // Shell integration active, --execute provided, or configured - show success
                super::success(format_switch_success_message(
                    branch, path, false, None, None,
                ))?;
            } else {
                // Shell integration not configured - show warning and setup hint
                let path_display = format_path_for_display(path);
                super::warning(cformat!(
                    "Worktree for <bold>{branch}</> at <bold>{path_display}</>; cannot cd (no shell integration)"
                ))?;
                super::shell_integration_hint(shell_integration_hint())?;
            }
        }
        SwitchResult::Created {
            path,
            created_branch,
            base_branch,
            from_remote,
        } => {
            // Creation succeeded - show success
            super::success(format_switch_success_message(
                branch,
                path,
                *created_branch,
                base_branch.as_deref(),
                from_remote.as_deref(),
            ))?;
            // Show setup hint if shell integration not active
            if !is_directive_mode && !has_execute_command {
                super::shell_integration_hint(shell_integration_hint())?;
            }
        }
    }

    // Flush output (important for directive mode)
    super::flush()?;

    Ok(())
}

/// Execute the --execute command after hooks have run
pub fn execute_user_command(command: &str) -> anyhow::Result<()> {
    use worktrunk::styling::format_bash_with_gutter;

    // Show what command is being executed (section header + gutter content)
    super::progress("Executing (--execute):")?;
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
    super::warning(cformat!(
        "No worktree found for branch <bold>{branch_name}</>"
    ))?;

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
    let integration_reason = handle_branch_deletion_result(result, branch_name)?;

    if integration_reason.is_some() {
        let flag_note = get_flag_note(
            no_delete_branch,
            force_delete,
            integration_reason,
            default_branch.as_deref(),
        );
        super::success(cformat!("Removed branch <bold>{branch_name}</>{flag_note}"))?;
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

    // Handle detached HEAD case (no branch known)
    let Some(branch_name) = branch_name else {
        // No branch associated - just remove the worktree
        if background {
            super::progress(
                "Removing worktree in background (detached HEAD, no branch to delete)",
            )?;
            let remove_command = build_remove_command(worktree_path, None);
            spawn_detached(&repo, main_path, &remove_command, "detached", "remove")?;
        } else {
            let target_repo = worktrunk::git::Repository::at(worktree_path);
            let _ = target_repo.run_command(&["fsmonitor--daemon", "stop"]);
            if let Err(err) = repo.remove_worktree(worktree_path) {
                return Err(GitError::WorktreeRemovalFailed {
                    branch: "(detached)".into(),
                    path: worktree_path.to_path_buf(),
                    error: err.to_string(),
                }
                .into());
            }
            super::success("Removed worktree (detached HEAD, no branch to delete)")?;
        }
        super::flush()?;
        return Ok(());
    };

    // Execute pre-remove hooks in the worktree being removed
    // Non-zero exit aborts removal (FailFast strategy)
    if verify && let Ok(config) = WorktrunkConfig::load() {
        let target_repo = Repository::at(worktree_path);
        let ctx = CommandContext::new(
            &target_repo,
            &config,
            branch_name,
            worktree_path,
            main_path,
            false, // force=false, prompt for approval
        );
        execute_pre_remove_commands(&ctx, false)?;
    }

    if background {
        // Background mode: spawn detached process

        // Determine if we should delete the branch and why (check once upfront)
        let (should_delete_branch, integration_reason) = if no_delete_branch {
            (false, None)
        } else if force_delete {
            // Force delete requested - always delete
            (true, None)
        } else {
            // Check if branch is integrated (ancestor or matching tree content)
            let check_target = target_branch.unwrap_or("HEAD");
            let deletion_repo = worktrunk::git::Repository::at(main_path);
            let reason = get_integration_reason(&deletion_repo, branch_name, check_target);
            (reason.is_some(), reason)
        };

        // Build deletion_result in the format expected by get_flag_note
        let deletion_result: Option<Option<IntegrationReason>> = if should_delete_branch {
            if force_delete {
                Some(None) // force-deleted
            } else {
                Some(integration_reason) // integrated
            }
        } else {
            None // not deleted
        };

        let flag_note = get_flag_note(
            no_delete_branch,
            force_delete,
            deletion_result,
            target_branch,
        );

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
        super::progress(action)?;

        // Show hint for unmerged branches (same as synchronous path)
        if !no_delete_branch && !should_delete_branch {
            super::print(cformat!(
                "{HINT_EMOJI} <dim>Use </>wt remove -D<dim> to delete unmerged branches</>"
            ))?;
        }

        // Build command with the decision we already made
        let remove_command =
            build_remove_command(worktree_path, should_delete_branch.then_some(branch_name));

        // Spawn the removal in background - runs from main_path (where we cd'd to)
        spawn_detached(&repo, main_path, &remove_command, branch_name, "remove")?;

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
        let integration_reason = if !no_delete_branch {
            let deletion_repo = worktrunk::git::Repository::at(main_path);
            let check_target = target_branch.unwrap_or("HEAD");
            let result =
                delete_branch_if_safe(&deletion_repo, branch_name, check_target, force_delete);
            handle_branch_deletion_result(result, branch_name)?
        } else {
            None
        };

        // Show success message (includes emoji and color)
        super::success(format_remove_worktree_message(
            main_path,
            changed_directory,
            branch_name,
            branch,
            no_delete_branch,
            force_delete,
            integration_reason,
            target_branch,
        ))?;

        super::flush()?;
        Ok(())
    }
}

/// Execute a command with streaming output
///
/// Uses Stdio::inherit to preserve TTY behavior - this ensures commands like cargo detect they're
/// connected to a terminal and don't buffer their output.
///
/// If `redirect_stdout_to_stderr` is true, wraps the command in `{ command; } 1>&2` to merge
/// stdout into stderr. This ensures deterministic output ordering (all output flows through stderr).
/// Per CLAUDE.md: child process output goes to stderr, worktrunk output goes to stdout.
///
/// Returns error if command exits with non-zero status.
///
/// ## Signal Handling (Unix)
///
/// SIGINT (Ctrl-C) is handled by checking the child's exit status:
/// - If the child was killed by a signal, we return exit code 128 + signal number
/// - This follows Unix conventions (e.g., exit code 130 for SIGINT)
///
/// The child process receives SIGINT directly from the terminal (via Stdio::inherit).
pub(crate) fn execute_streaming(
    command: &str,
    working_dir: &std::path::Path,
    redirect_stdout_to_stderr: bool,
) -> anyhow::Result<()> {
    use std::process::Command;
    use worktrunk::git::WorktrunkError;

    let command_to_run = if redirect_stdout_to_stderr {
        // Use newline instead of semicolon before closing brace to support
        // multi-line commands with control structures (if/fi, for/done, etc.)
        format!("{{ {}\n}} 1>&2", command)
    } else {
        command.to_string()
    };

    let mut child = Command::new("sh")
        .arg("-c")
        .arg(&command_to_run)
        .current_dir(working_dir)
        .stdin(std::process::Stdio::null()) // Null stdin - child gets EOF immediately
        .stdout(std::process::Stdio::inherit()) // Preserve TTY for output
        .stderr(std::process::Stdio::inherit()) // Preserve TTY for errors
        // Prevent vergen "overridden" warning in nested cargo builds when run via `cargo run`.
        // Add more VERGEN_* variables here if we expand build.rs and hit similar issues.
        .env_remove("VERGEN_GIT_DESCRIBE")
        .spawn()
        .map_err(|e| {
            anyhow::Error::from(worktrunk::git::GitError::Other {
                message: format!("Failed to execute command: {}", e),
            })
        })?;

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
    execute_streaming(command, worktree_path, true)?;

    // Flush to ensure all output appears before we continue
    super::flush()?;

    Ok(())
}
