//! Output handlers for worktree operations using the global output context

use crate::commands::worktree::{RemoveResult, SwitchResult};
use crate::output::global::format_switch_success;
use worktrunk::git::{GitError, GitResultExt};

/// Format message for switch operation (mode-specific via output system)
fn format_switch_message(result: &SwitchResult, branch: &str) -> String {
    match result {
        SwitchResult::ExistingWorktree(path) => {
            // created_branch=false means we switched to existing worktree
            format_switch_success(branch, path, false)
        }
        SwitchResult::CreatedWorktree {
            path,
            created_branch,
        } => {
            // Pass through whether we created a new branch
            format_switch_success(branch, path, *created_branch)
        }
    }
}

/// Format message for remove operation (includes emoji and color for consistency)
fn format_remove_message(result: &RemoveResult, branch: Option<&str>) -> String {
    use worktrunk::styling::{GREEN, SUCCESS_EMOJI};
    let green_bold = GREEN.bold();

    match result {
        RemoveResult::AlreadyOnDefault(branch) => {
            format!(
                "{SUCCESS_EMOJI} {GREEN}Already on default branch {green_bold}{branch}{green_bold:#}{GREEN:#}"
            )
        }
        RemoveResult::RemovedWorktree {
            primary_path,
            changed_directory,
            ..
        } => {
            let branch_suffix = branch
                .map(|b| format!(" for {green_bold}{b}{green_bold:#}"))
                .unwrap_or_default();
            if *changed_directory {
                format!(
                    "{SUCCESS_EMOJI} {GREEN}Removed worktree{branch_suffix}, returned to primary at {green_bold}{}{green_bold:#}{GREEN:#}",
                    primary_path.display()
                )
            } else {
                format!("{SUCCESS_EMOJI} {GREEN}Removed worktree{branch_suffix}{GREEN:#}")
            }
        }
        RemoveResult::SwitchedToDefault(branch) => {
            format!(
                "{SUCCESS_EMOJI} {GREEN}Switched to default branch {green_bold}{branch}{green_bold:#}{GREEN:#}"
            )
        }
    }
}

/// Shell integration hint message
fn shell_integration_hint() -> &'static str {
    "To enable automatic cd, run: wt config shell"
}

/// Handle output for a switch operation
pub fn handle_switch_output(
    result: &SwitchResult,
    branch: &str,
    has_execute_command: bool,
) -> Result<(), GitError> {
    // Set target directory for command execution
    super::change_directory(result.path())?;

    // Show success message (includes emoji and color)
    super::success(format_switch_message(result, branch))?;

    // If no execute command provided: show shell integration hint
    // (suppressed in directive mode since user already has integration)
    if !has_execute_command {
        super::hint(format!("\n{}", shell_integration_hint()))?;
    }

    // Flush output (important for directive mode)
    super::flush()?;

    Ok(())
}

/// Execute the --execute command after hooks have run
pub fn execute_user_command(command: &str) -> Result<(), GitError> {
    use worktrunk::styling::{CYAN, format_bash_with_gutter};

    // Show what command is being executed (matches post-create/post-start format)
    super::progress(format!("🔄 {CYAN}Executing (--execute):{CYAN:#}"))?;
    super::progress(format_bash_with_gutter(command, ""))?;

    super::execute(command)?;

    Ok(())
}

/// Handle output for a remove operation
pub fn handle_remove_output(result: &RemoveResult, branch: Option<&str>) -> Result<(), GitError> {
    // For removed worktree: emit cd directive BEFORE deletion so shell changes directory instantly
    if let RemoveResult::RemovedWorktree {
        primary_path,
        worktree_path,
        changed_directory,
    } = result
    {
        // 1. Emit cd directive if needed - shell will execute this immediately
        if *changed_directory {
            super::change_directory(primary_path)?;
            super::flush()?; // Force flush to ensure shell processes the cd
        }

        // 2. Do the deletion (shell already changed directory if needed)
        // Progress message already shown at start of handle_remove()
        let repo = worktrunk::git::Repository::current();
        repo.remove_worktree(worktree_path)
            .git_context("Failed to remove worktree")?;
    }

    // Show success message (includes emoji and color)
    super::success(format_remove_message(result, branch))?;

    // Flush output
    super::flush()?;

    Ok(())
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
        // Use Stdio::inherit() to preserve TTY behavior
        // This prevents commands like cargo from buffering output
        .spawn()
        .map_err(|e| io::Error::other(format!("Failed to execute command: {}", e)))?;

    // Wait for command to complete
    let status = child
        .wait()
        .map_err(|e| io::Error::other(format!("Failed to wait for command: {}", e)))?;

    if !status.success() {
        return Err(io::Error::other(format!(
            "Command failed with exit code: {}",
            status
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
    // Convert io::Error to GitError::CommandFailed to preserve error message formatting
    execute_streaming(command, worktree_path, true)
        .map_err(|e| GitError::CommandFailed(e.to_string()))?;

    // Flush to ensure all output appears before we continue
    super::flush()?;

    // Terminate output (adds NUL in directive mode, no-op in interactive)
    super::terminate_output()?;

    Ok(())
}
