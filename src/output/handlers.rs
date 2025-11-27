//! Output handlers for worktree operations using the global output context

use crate::commands::process::spawn_detached;
use crate::commands::worktree::{RemoveResult, SwitchResult};
use crate::output::global::format_switch_success;
use worktrunk::git::GitError;
use worktrunk::git::Repository;
use worktrunk::path::format_path_for_display;
use worktrunk::shell::Shell;
use worktrunk::styling::{
    CYAN, CYAN_BOLD, GREEN, GREEN_BOLD, WARNING, WARNING_BOLD, format_with_gutter,
};

/// Check if a branch's content has been integrated into the target.
///
/// Returns true if the branch is safe to delete because either:
/// - The branch is an ancestor of the target (traditional merge), OR
/// - The branch's tree SHA matches the target's tree SHA (squash merge/rebase)
///
/// Returns false if neither condition is met, or if an error occurs (e.g., invalid refs).
/// This fail-safe default prevents accidental branch deletion when integration cannot
/// be determined.
fn is_branch_integrated(repo: &Repository, branch_name: &str, target: &str) -> bool {
    // Check traditional merge relationship
    if repo.is_ancestor(branch_name, target).unwrap_or(false) {
        return true;
    }

    // Check if tree content matches (handles squash merge/rebase)
    repo.trees_match(branch_name, target).unwrap_or(false)
}

/// Attempt to delete a branch if it's integrated or force_delete is set.
///
/// Returns:
/// - `Ok(true)` if branch was deleted
/// - `Ok(false)` if branch was not deleted (not integrated, and not force)
/// - `Err` if git command failed
fn delete_branch_if_safe(
    repo: &Repository,
    branch_name: &str,
    target: &str,
    force_delete: bool,
) -> anyhow::Result<bool> {
    if !force_delete && !is_branch_integrated(repo, branch_name, target) {
        return Ok(false);
    }
    repo.run_command(&["branch", "-D", branch_name])?;
    Ok(true)
}

/// Handle the result of a branch deletion attempt.
///
/// Shows appropriate warnings for non-deleted branches.
///
/// Returns:
/// - `Ok(true)` if branch was deleted
/// - `Ok(false)` if branch was not deleted (warning shown)
fn handle_branch_deletion_result(
    result: anyhow::Result<bool>,
    branch_name: &str,
) -> anyhow::Result<bool> {
    match result {
        Ok(true) => Ok(true),
        Ok(false) => {
            // Branch not integrated - show warning
            super::warning(format!(
                "{WARNING}Could not delete branch {WARNING_BOLD}{branch_name}{WARNING_BOLD:#}{WARNING:#}"
            ))?;
            super::gutter(format_with_gutter(
                &format!("error: the branch '{}' is not fully merged", branch_name),
                "",
                None,
            ))?;
            Ok(false)
        }
        Err(e) => {
            // Git command failed - show warning with error details
            super::warning(format!(
                "{WARNING}Could not delete branch {WARNING_BOLD}{branch_name}{WARNING_BOLD:#}{WARNING:#}"
            ))?;
            super::gutter(format_with_gutter(&e.to_string(), "", None))?;
            Ok(false)
        }
    }
}

/// Get flag acknowledgment note for remove messages
fn get_flag_note(no_delete_branch: bool, force_delete: bool, branch_deleted: bool) -> &'static str {
    if no_delete_branch {
        " (--no-delete-branch)"
    } else if force_delete && branch_deleted {
        " (--force-delete)"
    } else {
        ""
    }
}

/// Format message for remove worktree operation (includes emoji and color for consistency)
///
/// `branch_deleted` indicates whether branch deletion actually succeeded (not just attempted)
fn format_remove_worktree_message(
    main_path: &std::path::Path,
    changed_directory: bool,
    branch_name: &str,
    branch: Option<&str>,
    no_delete_branch: bool,
    force_delete: bool,
    branch_deleted: bool,
) -> String {
    // Build the action description based on actual outcome
    let action = if no_delete_branch || !branch_deleted {
        "Removed worktree"
    } else {
        "Removed worktree & branch"
    };

    // Show flag acknowledgment when applicable
    let flag_note = get_flag_note(no_delete_branch, force_delete, branch_deleted);

    let branch_display = branch.or(Some(branch_name));

    if changed_directory {
        if let Some(b) = branch_display {
            // Re-establish GREEN after each green_bold reset to prevent color leak
            format!(
                "{GREEN}{action} for {GREEN_BOLD}{b}{GREEN_BOLD:#}{GREEN}, changed directory to {GREEN_BOLD}{}{GREEN_BOLD:#}{GREEN:#}{flag_note}",
                format_path_for_display(main_path)
            )
        } else {
            format!(
                "{GREEN}{action}, changed directory to {GREEN_BOLD}{}{GREEN_BOLD:#}{GREEN:#}{flag_note}",
                format_path_for_display(main_path)
            )
        }
    } else if let Some(b) = branch_display {
        format!("{GREEN}{action} for {GREEN_BOLD}{b}{GREEN_BOLD:#}{GREEN:#}{flag_note}")
    } else {
        format!("{GREEN}{action}{GREEN:#}{flag_note}")
    }
}

/// Shell integration hint message (without emoji - hint() adds it automatically)
fn shell_integration_hint() -> String {
    use worktrunk::styling::HINT;
    format!("{HINT}Run `wt config shell install` to enable automatic cd{HINT:#}")
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
            let bold = worktrunk::styling::AnstyleStyle::new().bold();
            super::info(format!(
                "Already on worktree for {bold}{branch}{bold:#} at {bold}{}{bold:#}",
                format_path_for_display(path)
            ))?;
        }
        SwitchResult::Existing(path) => {
            // Check if we can cd or if shell integration is at least configured
            let is_configured = Shell::is_integration_configured().ok().flatten().is_some();

            if is_directive_mode || has_execute_command || is_configured {
                // Shell integration active, --execute provided, or configured - show success
                super::success(format_switch_success(branch, path, false, None))?;
            } else {
                // Shell integration not configured - show warning and setup hint
                super::warning(format!(
                    "{WARNING}Worktree for {WARNING_BOLD}{branch}{WARNING_BOLD:#}{WARNING} at {WARNING_BOLD}{}{WARNING_BOLD:#}{WARNING}; cannot cd (no shell integration){WARNING:#}",
                    format_path_for_display(path)
                ))?;
                super::shell_integration_hint(shell_integration_hint())?;
            }
        }
        SwitchResult::Created {
            path,
            created_branch,
            base_branch,
        } => {
            // Creation succeeded - show success
            super::success(format_switch_success(
                branch,
                path,
                *created_branch,
                base_branch.as_deref(),
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
    use worktrunk::styling::{CYAN, format_bash_with_gutter};

    // Show what command is being executed (section header + gutter content)
    super::progress(format!("{CYAN}Executing (--execute):{CYAN:#}"))?;
    super::gutter(format_bash_with_gutter(command, ""))?;

    super::execute(command)?;

    Ok(())
}

/// Build shell command for background worktree removal
///
/// `should_delete_branch` indicates whether to delete the branch after removing the worktree.
/// This decision is computed upfront (checking if branch is merged) before spawning the background process.
fn build_remove_command(
    worktree_path: &std::path::Path,
    branch_name: &str,
    should_delete_branch: bool,
) -> String {
    use shell_escape::escape;

    let worktree_path_str = worktree_path.to_string_lossy();
    let worktree_escaped = escape(worktree_path_str.as_ref().into());
    let branch_escaped = escape(branch_name.into());

    // Stop fsmonitor daemon first (best effort - ignore errors)
    // This prevents zombie daemons from accumulating when using builtin fsmonitor
    let stop_fsmonitor = format!(
        "git -C {} fsmonitor--daemon stop 2>/dev/null || true",
        worktree_escaped
    );

    if should_delete_branch {
        // Stop fsmonitor, remove worktree, and delete branch
        format!(
            "{} && git worktree remove {} && git branch -D {}",
            stop_fsmonitor, worktree_escaped, branch_escaped
        )
    } else {
        // Stop fsmonitor and remove the worktree
        format!(
            "{} && git worktree remove {}",
            stop_fsmonitor, worktree_escaped
        )
    }
}

/// Handle output for a remove operation
pub fn handle_remove_output(
    result: &RemoveResult,
    branch: Option<&str>,
    background: bool,
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
            branch_name,
            *no_delete_branch,
            *force_delete,
            target_branch.as_deref(),
            branch,
            background,
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
    use worktrunk::styling::GRAY;

    // Show info message that no worktree was found
    super::info(format!(
        "{GRAY}No worktree found for branch {branch_name}{GRAY:#}"
    ))?;

    // Attempt branch deletion (unless --no-delete-branch was specified)
    if no_delete_branch {
        // User explicitly requested no branch deletion - nothing more to do
        super::flush()?;
        return Ok(());
    }

    let repo = worktrunk::git::Repository::current();
    let result = delete_branch_if_safe(&repo, branch_name, "HEAD", force_delete);
    let deleted = handle_branch_deletion_result(result, branch_name)?;

    if deleted {
        let flag_note = if force_delete {
            " (--force-delete)"
        } else {
            ""
        };
        super::success(format!(
            "{GREEN}Removed branch {GREEN_BOLD}{branch_name}{GREEN_BOLD:#}{GREEN:#}{flag_note}"
        ))?;
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
    branch_name: &str,
    no_delete_branch: bool,
    force_delete: bool,
    target_branch: Option<&str>,
    branch: Option<&str>,
    background: bool,
) -> anyhow::Result<()> {
    // 1. Emit cd directive if needed - shell will execute this immediately
    if changed_directory {
        super::change_directory(main_path)?;
        super::flush()?; // Force flush to ensure shell processes the cd
    }

    let repo = worktrunk::git::Repository::current();

    if background {
        // Background mode: spawn detached process

        // Determine if we should delete the branch (check once upfront)
        let should_delete_branch = if no_delete_branch {
            false
        } else if force_delete {
            // Force delete requested - always delete
            true
        } else {
            // Check if branch is integrated (ancestor or matching tree content)
            let check_target = target_branch.unwrap_or("HEAD");
            let deletion_repo = worktrunk::git::Repository::at(main_path);
            is_branch_integrated(&deletion_repo, branch_name, check_target)
        };

        // Show progress message based on what we'll do
        let action = if no_delete_branch {
            format!(
                "{CYAN}Removing {CYAN_BOLD}{branch_name}{CYAN_BOLD:#}{CYAN} worktree in background; retaining branch (--no-delete-branch){CYAN:#}"
            )
        } else if should_delete_branch {
            if force_delete {
                format!(
                    "{CYAN}Removing {CYAN_BOLD}{branch_name}{CYAN_BOLD:#}{CYAN} worktree & branch in background (--force-delete){CYAN:#}"
                )
            } else {
                format!(
                    "{CYAN}Removing {CYAN_BOLD}{branch_name}{CYAN_BOLD:#}{CYAN} worktree & branch in background{CYAN:#}"
                )
            }
        } else {
            format!(
                "{CYAN}Removing {CYAN_BOLD}{branch_name}{CYAN_BOLD:#}{CYAN} worktree in background; retaining unmerged branch{CYAN:#}"
            )
        };
        super::progress(action)?;

        // Build command with the decision we already made
        let remove_command = build_remove_command(worktree_path, branch_name, should_delete_branch);

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
        let branch_deleted = if !no_delete_branch {
            let deletion_repo = worktrunk::git::Repository::at(main_path);
            let check_target = target_branch.unwrap_or("HEAD");
            let result =
                delete_branch_if_safe(&deletion_repo, branch_name, check_target, force_delete);
            handle_branch_deletion_result(result, branch_name)?
        } else {
            false
        };

        // Show success message (includes emoji and color)
        super::success(format_remove_worktree_message(
            main_path,
            changed_directory,
            branch_name,
            branch,
            no_delete_branch,
            force_delete,
            branch_deleted,
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
