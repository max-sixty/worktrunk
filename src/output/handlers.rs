//! Output handlers for worktree operations using the global output context

use crate::commands::worktree::{RemoveResult, SwitchResult};
use worktrunk::git::GitError;
use worktrunk::styling::AnstyleStyle;

/// Format plain message for switch operation (no emoji - added by OutputContext)
fn format_switch_message_plain(result: &SwitchResult, branch: &str) -> String {
    let bold = AnstyleStyle::new().bold();

    match result {
        SwitchResult::ExistingWorktree(path) => {
            format!(
                "Switched to worktree for {bold}{branch}{bold:#} at {}",
                path.display()
            )
        }
        SwitchResult::CreatedWorktree {
            path,
            created_branch,
        } => {
            if *created_branch {
                format!(
                    "Created new worktree for {bold}{branch}{bold:#} at {}",
                    path.display()
                )
            } else {
                format!(
                    "Added worktree for {bold}{branch}{bold:#} at {}",
                    path.display()
                )
            }
        }
    }
}

/// Format plain message for remove operation (no emoji - added by OutputContext)
fn format_remove_message_plain(result: &RemoveResult) -> String {
    let bold = AnstyleStyle::new().bold();

    match result {
        RemoveResult::AlreadyOnDefault(branch) => {
            format!("Already on default branch {bold}{branch}{bold:#}")
        }
        RemoveResult::RemovedWorktree { primary_path } => {
            format!(
                "Removed worktree, returned to primary at {}",
                primary_path.display()
            )
        }
        RemoveResult::SwitchedToDefault(branch) => {
            format!("Switched to default branch {bold}{branch}{bold:#}")
        }
        RemoveResult::RemovedOtherWorktree { branch } => {
            format!("Removed worktree for {bold}{branch}{bold:#}")
        }
    }
}

/// Shell integration hint message
fn shell_integration_hint() -> &'static str {
    "To enable automatic cd, run: wt configure-shell"
}

/// Handle output for a switch operation
pub fn handle_switch_output(
    result: &SwitchResult,
    branch: &str,
    execute: Option<&str>,
) -> Result<(), GitError> {
    // Set target directory for command execution
    super::change_directory(result.path()).map_err(|e| GitError::CommandFailed(e.to_string()))?;

    // Show success message (plain text - formatting added by OutputContext)
    super::success(format_switch_message_plain(result, branch))
        .map_err(|e| GitError::CommandFailed(e.to_string()))?;

    // Execute command if provided
    if let Some(cmd) = execute {
        super::execute(cmd).map_err(|e| GitError::CommandFailed(e.to_string()))?;
    } else {
        // No execute command: show shell integration hint (only in interactive mode)
        super::progress(format!("\n{}", shell_integration_hint()))
            .map_err(|e| GitError::CommandFailed(e.to_string()))?;
    }

    // Flush output (important for directive mode)
    super::flush().map_err(|e| GitError::CommandFailed(e.to_string()))?;

    Ok(())
}

/// Handle output for a remove operation
pub fn handle_remove_output(result: &RemoveResult) -> Result<(), GitError> {
    // For removed worktree, set target directory for shell to cd to
    if let RemoveResult::RemovedWorktree { primary_path } = result {
        super::change_directory(primary_path)
            .map_err(|e| GitError::CommandFailed(e.to_string()))?;
    }

    // Show success message
    super::success(format_remove_message_plain(result))
        .map_err(|e| GitError::CommandFailed(e.to_string()))?;

    // Flush output
    super::flush().map_err(|e| GitError::CommandFailed(e.to_string()))?;

    Ok(())
}

/// Execute a command in a worktree directory
pub fn execute_command_in_worktree(
    worktree_path: &std::path::Path,
    command: &str,
) -> Result<(), GitError> {
    use std::process::Command;

    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(worktree_path)
        .output()
        .map_err(|e| GitError::CommandFailed(format!("Failed to execute command: {}", e)))?;

    if !output.status.success() {
        return Err(GitError::CommandFailed(format!(
            "Command failed with exit code: {}",
            output.status
        )));
    }

    // Print command output
    if !output.stdout.is_empty() {
        use worktrunk::styling::println;
        println!("{}", String::from_utf8_lossy(&output.stdout).trim_end());
    }
    if !output.stderr.is_empty() {
        use worktrunk::styling::eprintln;
        eprintln!("{}", String::from_utf8_lossy(&output.stderr).trim_end());
    }

    Ok(())
}
