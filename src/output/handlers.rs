//! Output handlers for worktree operations using the global output context

use crate::commands::process::spawn_detached;
use crate::commands::worktree::{RemoveResult, SwitchResult};
use crate::output::global::format_switch_success;
use worktrunk::git::GitError;
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{
    CYAN, CYAN_BOLD, GREEN, GREEN_BOLD, WARNING, WARNING_BOLD, format_with_gutter,
};

/// Format message for switch operation (mode-specific via output system)
fn format_switch_message(result: &SwitchResult, branch: &str) -> (String, bool) {
    match result {
        SwitchResult::AlreadyAt(path) => {
            // Note: output::info() adds the INFO_EMOJI automatically
            let bold = worktrunk::styling::AnstyleStyle::new().bold();
            (
                format!(
                    "Already on worktree for {bold}{branch}{bold:#} at {bold}{}{bold:#}",
                    format_path_for_display(path)
                ),
                true, // is_info
            )
        }
        SwitchResult::Existing(path) => {
            // created_branch=false means we switched to existing worktree
            (format_switch_success(branch, path, false, None), false)
        }
        SwitchResult::Created {
            path,
            created_branch,
            base_branch,
        } => {
            // Pass through whether we created a new branch and the base branch
            (
                format_switch_success(branch, path, *created_branch, base_branch.as_deref()),
                false,
            )
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

/// Format message for remove operation (includes emoji and color for consistency)
///
/// `branch_deleted` indicates whether branch deletion actually succeeded (not just attempted)
fn format_remove_message(
    result: &RemoveResult,
    branch: Option<&str>,
    branch_deleted: bool,
) -> String {
    let RemoveResult::RemovedWorktree {
        primary_path,
        changed_directory,
        branch_name,
        no_delete_branch,
        force_delete,
        ..
    } = result;

    // Build the action description based on actual outcome
    let action = if *no_delete_branch || !branch_deleted {
        "Removed worktree"
    } else {
        "Removed worktree & branch"
    };

    // Show flag acknowledgment when applicable
    let flag_note = get_flag_note(*no_delete_branch, *force_delete, branch_deleted);

    let branch_display = branch.or(Some(branch_name));

    if *changed_directory {
        if let Some(b) = branch_display {
            // Re-establish GREEN after each green_bold reset to prevent color leak
            format!(
                "{GREEN}{action} for {GREEN_BOLD}{b}{GREEN_BOLD:#}{GREEN}, changed directory to {GREEN_BOLD}{}{GREEN_BOLD:#}{GREEN:#}{flag_note}",
                format_path_for_display(primary_path)
            )
        } else {
            format!(
                "{GREEN}{action}, changed directory to {GREEN_BOLD}{}{GREEN_BOLD:#}{GREEN:#}{flag_note}",
                format_path_for_display(primary_path)
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
    format!("{HINT}To enable automatic cd, run: wt config shell{HINT:#}")
}

/// Handle output for a switch operation
pub fn handle_switch_output(
    result: &SwitchResult,
    branch: &str,
    has_execute_command: bool,
) -> Result<(), GitError> {
    // Set target directory for command execution
    super::change_directory(result.path())?;

    // Show message (success or info based on result)
    let (message, is_info) = format_switch_message(result, branch);
    if is_info {
        super::info(message)?;
    } else {
        super::success(message)?;
    }

    // If no execute command provided: show shell integration hint
    // (suppressed in directive mode since user already has integration)
    if !has_execute_command {
        super::shell_integration_hint(shell_integration_hint())?;
    }

    // Flush output (important for directive mode)
    super::flush()?;

    Ok(())
}

/// Execute the --execute command after hooks have run
pub fn execute_user_command(command: &str) -> Result<(), GitError> {
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

    if should_delete_branch {
        // Remove worktree and delete branch
        format!(
            "git worktree remove {} && git branch -D {}",
            worktree_escaped, branch_escaped
        )
    } else {
        // Just remove the worktree
        format!("git worktree remove {}", worktree_escaped)
    }
}

/// Handle output for a remove operation
pub fn handle_remove_output(
    result: &RemoveResult,
    branch: Option<&str>,
    strict_branch_deletion: bool,
    background: bool,
) -> Result<(), GitError> {
    let RemoveResult::RemovedWorktree {
        primary_path,
        worktree_path,
        changed_directory,
        branch_name,
        no_delete_branch,
        force_delete,
        target_branch,
    } = result;

    // 1. Emit cd directive if needed - shell will execute this immediately
    if *changed_directory {
        super::change_directory(primary_path)?;
        super::flush()?; // Force flush to ensure shell processes the cd
    }

    let repo = worktrunk::git::Repository::current();

    if background {
        // Background mode: spawn detached process

        // Determine if we should delete the branch (check once upfront)
        let should_delete_branch = if *no_delete_branch {
            false
        } else if *force_delete {
            // Force delete requested - always delete
            true
        } else {
            // Check if branch is fully merged to target
            let check_target = target_branch.as_deref().unwrap_or("HEAD");
            let deletion_repo = worktrunk::git::Repository::at(primary_path);
            deletion_repo
                .is_ancestor(branch_name, check_target)
                .unwrap_or(false)
        };

        // Show progress message based on what we'll do
        let action = if *no_delete_branch {
            format!(
                "{CYAN}Removing {CYAN_BOLD}{branch_name}{CYAN_BOLD:#}{CYAN} in background; retaining branch (--no-delete-branch){CYAN:#}"
            )
        } else if should_delete_branch {
            if *force_delete {
                format!(
                    "{CYAN}Removing {CYAN_BOLD}{branch_name}{CYAN_BOLD:#}{CYAN} & branch in background (--force-delete){CYAN:#}"
                )
            } else {
                format!(
                    "{CYAN}Removing {CYAN_BOLD}{branch_name}{CYAN_BOLD:#}{CYAN} & branch in background{CYAN:#}"
                )
            }
        } else {
            format!(
                "{CYAN}Removing {CYAN_BOLD}{branch_name}{CYAN_BOLD:#}{CYAN} in background; retaining unmerged branch{CYAN:#}"
            )
        };
        super::progress(action)?;

        // Build command with the decision we already made
        let remove_command = build_remove_command(worktree_path, branch_name, should_delete_branch);

        // Spawn the removal in background - runs from primary_path (where we cd'd to)
        spawn_detached(&repo, primary_path, &remove_command, branch_name, "remove")?;

        super::flush()?;
        Ok(())
    } else {
        // Synchronous mode: remove immediately and report actual results
        // Track whether branch was actually deleted (will be computed based on deletion attempt)
        if let Err(err) = repo.remove_worktree(worktree_path) {
            return Err(match err {
                GitError::CommandFailed(msg) => GitError::WorktreeRemovalFailed {
                    branch: branch_name.clone(),
                    path: worktree_path.clone(),
                    error: msg,
                },
                other => other,
            });
        }

        // Delete the branch (unless --no-delete-branch was specified)
        let branch_deleted = if !no_delete_branch {
            let deletion_repo = worktrunk::git::Repository::at(primary_path);

            // Use git branch -D if force_delete is true, otherwise check if merged first
            let delete_result = if *force_delete {
                // Force delete - use -D directly
                deletion_repo.run_command(&["branch", "-D", branch_name])
            } else {
                let check_target = target_branch.as_deref().unwrap_or("HEAD");

                // Check if branch is merged to target using is_ancestor
                match deletion_repo.is_ancestor(branch_name, check_target) {
                    Ok(true) => {
                        // Branch is an ancestor of target (fully merged), safe to delete
                        deletion_repo.run_command(&["branch", "-D", branch_name])
                    }
                    Ok(false) | Err(_) => {
                        // Branch is not fully merged to target
                        Err(worktrunk::git::GitError::CommandFailed(format!(
                            "error: the branch '{}' is not fully merged",
                            branch_name
                        )))
                    }
                }
            };

            match delete_result {
                Ok(_) => true,
                Err(e) => {
                    if strict_branch_deletion {
                        return Err(match e {
                            GitError::CommandFailed(msg) => GitError::BranchDeletionFailed {
                                branch: branch_name.clone(),
                                error: msg,
                            },
                            other => other,
                        });
                    }

                    // If branch deletion fails in non-strict mode, show a warning but don't error
                    super::warning(format!(
                        "{WARNING}Could not delete branch {WARNING_BOLD}{branch_name}{WARNING_BOLD:#}{WARNING:#}"
                    ))?;

                    // Show the git error in a gutter-formatted block (raw output, no styling)
                    let raw_error = match &e {
                        GitError::CommandFailed(msg) => msg.as_str(),
                        _ => &e.to_string(),
                    };
                    super::gutter(format_with_gutter(raw_error, "", None))?;
                    false
                }
            }
        } else {
            false
        };

        // Show success message (includes emoji and color)
        super::success(format_remove_message(result, branch, branch_deleted))?;
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
pub(crate) fn execute_streaming(
    command: &str,
    working_dir: &std::path::Path,
    redirect_stdout_to_stderr: bool,
) -> std::io::Result<()> {
    use std::io;
    use std::process::Command;

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
        .spawn()
        .map_err(|e| io::Error::other(format!("Failed to execute command: {}", e)))?;

    // Wait for command to complete
    let status = child
        .wait()
        .map_err(|e| io::Error::other(format!("Failed to wait for command: {}", e)))?;

    if !status.success() {
        // Get the exit code if available (None means terminated by signal on some platforms)
        let code = status.code().unwrap_or(1);
        return Err(io::Error::other(format!(
            "CHILD_EXIT_CODE:{} exit status: {}",
            code, code
        )));
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
/// Calls terminate_output() after completion to handle mode-specific cleanup
/// (NUL terminator in directive mode, no-op in interactive mode).
pub fn execute_command_in_worktree(
    worktree_path: &std::path::Path,
    command: &str,
) -> Result<(), GitError> {
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
    // io::Error is automatically converted to GitError, parsing exit codes via From impl
    execute_streaming(command, worktree_path, true)?;

    // Flush to ensure all output appears before we continue
    super::flush()?;

    // Terminate output (adds NUL in directive mode, no-op in interactive)
    super::terminate_output()?;

    Ok(())
}
