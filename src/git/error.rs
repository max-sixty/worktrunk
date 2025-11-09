//! Git error types and formatting

use std::path::PathBuf;

use super::HookType;

#[derive(Debug)]
pub enum GitError {
    /// Generic error with a message
    CommandFailed(String),
    /// Error for parsing failures
    ParseError(String),
    /// Repository is in detached HEAD state
    DetachedHead,
    /// Working tree has untracked files
    UntrackedFiles,
    /// Hook command failed
    HookCommandFailed {
        hook_type: HookType,
        command_name: Option<String>,
        error: String,
        exit_code: Option<i32>,
    },
    /// Working tree has uncommitted changes
    UncommittedChanges,
    /// Branch already exists (when trying to create)
    BranchAlreadyExists { branch: String },
    /// Worktree directory is missing
    WorktreeMissing { branch: String },
    /// No worktree found for branch
    NoWorktreeFound { branch: String },
    /// Cannot push due to conflicting uncommitted changes
    ConflictingChanges {
        files: Vec<String>,
        worktree_path: PathBuf,
    },
    /// Push is not a fast-forward
    NotFastForward {
        target_branch: String,
        commits_formatted: String,
        files_formatted: String,
    },
    /// Found merge commits in push range
    MergeCommitsFound,
    /// Command was not approved by user
    CommandNotApproved,
    /// Child process exited with non-zero code (preserves exit code for signals)
    ChildProcessExited { code: i32, message: String },
    /// Push operation failed
    PushFailed { error: String },
    /// Rebase resulted in a conflict or incomplete state
    RebaseConflict {
        state: String,
        target_branch: String,
        git_output: String,
    },
    /// Worktree path already exists on filesystem
    WorktreePathExists { path: PathBuf },
    /// Creating a worktree failed
    WorktreeCreationFailed {
        branch: String,
        base_branch: Option<String>,
        error: String,
    },
    /// Switching branches failed
    SwitchFailed { branch: String, error: String },
    /// Removing a worktree directory failed
    WorktreeRemovalFailed {
        branch: String,
        path: PathBuf,
        error: String,
    },
    /// Deleting a branch failed
    BranchDeletionFailed { branch: String, error: String },
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use crate::styling::{ERROR, ERROR_BOLD, ERROR_EMOJI, HINT, HINT_EMOJI};

        match self {
            // Generic error - format with emoji and styling
            // Skip formatting if message already contains ANSI codes (from nested error formatting)
            GitError::CommandFailed(msg) => {
                if msg.contains("\x1b[") || msg.contains("[31m") || msg.contains("[0m") {
                    // Already formatted, just pass through
                    write!(f, "{}", msg)
                } else {
                    write!(f, "{ERROR_EMOJI} {ERROR}{msg}{ERROR:#}")
                }
            }

            // ParseError messages need formatting
            GitError::ParseError(msg) => {
                write!(f, "{ERROR_EMOJI} {ERROR}{msg}{ERROR:#}")
            }

            // Detached HEAD error
            GitError::DetachedHead => {
                write!(
                    f,
                    "{ERROR_EMOJI} {ERROR}Not on a branch (detached HEAD){ERROR:#}\n\n{HINT_EMOJI} {HINT}You are in detached HEAD state{HINT:#}"
                )
            }

            // Untracked files error
            GitError::UntrackedFiles => {
                write!(
                    f,
                    "{ERROR_EMOJI} {ERROR}Working tree has untracked files{ERROR:#}\n\n{HINT_EMOJI} {HINT}Add them with 'git add' and try again{HINT:#}"
                )
            }

            // Hook command failed
            GitError::HookCommandFailed {
                hook_type,
                command_name,
                error,
                exit_code: _,
            } => match command_name {
                Some(name) => write!(
                    f,
                    "{ERROR_EMOJI} {ERROR}{hook_type} command failed: {ERROR_BOLD}{name}{ERROR_BOLD:#}{ERROR:#}\n\n{error}\n\n{HINT_EMOJI} {HINT}Use --no-verify to skip {hook_type} commands{HINT:#}"
                ),
                None => write!(
                    f,
                    "{ERROR_EMOJI} {ERROR}{hook_type} command failed{ERROR:#}\n\n{error}\n\n{HINT_EMOJI} {HINT}Use --no-verify to skip {hook_type} commands{HINT:#}"
                ),
            },

            // Uncommitted changes
            GitError::UncommittedChanges => {
                write!(
                    f,
                    "{ERROR_EMOJI} {ERROR}Working tree has uncommitted changes{ERROR:#}\n\n{HINT_EMOJI} {HINT}Commit or stash them first{HINT:#}"
                )
            }

            // Branch already exists
            GitError::BranchAlreadyExists { branch } => {
                write!(
                    f,
                    "{ERROR_EMOJI} {ERROR}Branch {ERROR_BOLD}{branch}{ERROR_BOLD:#}{ERROR} already exists{ERROR:#}\n\n{HINT_EMOJI} {HINT}Remove --create flag to switch to it{HINT:#}"
                )
            }

            // Worktree missing
            GitError::WorktreeMissing { branch } => {
                write!(
                    f,
                    "{ERROR_EMOJI} {ERROR}Worktree directory missing for {ERROR_BOLD}{branch}{ERROR_BOLD:#}{ERROR:#}\n\n{HINT_EMOJI} {HINT}Run 'git worktree prune' to clean up{HINT:#}"
                )
            }

            // No worktree found
            GitError::NoWorktreeFound { branch } => {
                write!(
                    f,
                    "{ERROR_EMOJI} {ERROR}No worktree found for branch {ERROR_BOLD}{branch}{ERROR_BOLD:#}{ERROR:#}"
                )
            }

            // Conflicting changes
            GitError::ConflictingChanges {
                files,
                worktree_path,
            } => {
                use crate::styling::format_with_gutter;

                write!(
                    f,
                    "{ERROR_EMOJI} {ERROR}Cannot push: conflicting uncommitted changes in:{ERROR:#}\n\n"
                )?;
                if !files.is_empty() {
                    let joined_files = files.join("\n");
                    write!(f, "{}", format_with_gutter(&joined_files, "", None))?;
                }
                write!(
                    f,
                    "\n{HINT_EMOJI} {HINT}Commit or stash these changes in {} first{HINT:#}",
                    worktree_path.display()
                )
            }

            // Not fast-forward
            GitError::NotFastForward {
                target_branch,
                commits_formatted,
                files_formatted,
            } => {
                writeln!(
                    f,
                    "{ERROR_EMOJI} {ERROR}Can't push to local {ERROR_BOLD}{target_branch}{ERROR_BOLD:#} branch: it has newer commits{ERROR:#}"
                )?;

                // Show the formatted commit log
                if !commits_formatted.is_empty() {
                    writeln!(f)?;
                    write!(f, "{}", commits_formatted)?;
                }

                // Show the formatted diff stat
                if !files_formatted.is_empty() {
                    writeln!(f)?;
                    write!(f, "{}", files_formatted)?;
                }

                write!(
                    f,
                    "\n{HINT_EMOJI} {HINT}Use 'wt merge' to rebase your changes onto {target_branch}{HINT:#}"
                )
            }

            // Merge commits found
            GitError::MergeCommitsFound => {
                write!(
                    f,
                    "{ERROR_EMOJI} {ERROR}Found merge commits in push range{ERROR:#}\n\n{HINT_EMOJI} {HINT}Use --allow-merge-commits to push non-linear history{HINT:#}"
                )
            }

            // Command not approved
            GitError::CommandNotApproved => {
                Ok(()) // on_skip callback handles the printing
            }

            // Child process exited with non-zero code
            // Just display the message - main.rs will use the exit code
            GitError::ChildProcessExited { code: _, message } => {
                write!(f, "{}", message)
            }

            // Push failed
            GitError::PushFailed { error } => {
                write!(f, "{ERROR_EMOJI} {ERROR}Push failed: {error}{ERROR:#}")
            }

            // Rebase conflict
            GitError::RebaseConflict {
                state: _,
                target_branch,
                git_output,
            } => {
                use crate::styling::format_with_gutter;

                write!(
                    f,
                    "{ERROR_EMOJI} {ERROR}Rebase onto {ERROR_BOLD}{target_branch}{ERROR_BOLD:#}{ERROR} incomplete{ERROR:#}"
                )?;

                if !git_output.is_empty() {
                    writeln!(f)?;
                    write!(f, "{}", format_with_gutter(git_output, "", None))?;
                } else {
                    // Fallback hints if no git output (edge case)
                    write!(
                        f,
                        "\n\n{HINT_EMOJI} {HINT}Resolve conflicts and run 'git rebase --continue'{HINT:#}\n{HINT_EMOJI} {HINT}Or abort with 'git rebase --abort'{HINT:#}"
                    )?;
                }

                Ok(())
            }

            // Worktree path already exists
            GitError::WorktreePathExists { path } => {
                write!(
                    f,
                    "{ERROR_EMOJI} {ERROR}Directory already exists: {ERROR_BOLD}{}{ERROR_BOLD:#}{ERROR:#}\n\n{HINT_EMOJI} {HINT}Remove the directory or use a different branch name{HINT:#}",
                    path.display()
                )
            }

            GitError::WorktreeCreationFailed {
                branch,
                base_branch,
                error,
            } => {
                let header = match base_branch {
                    Some(base) => format!(
                        "{ERROR_EMOJI} {ERROR}Failed to create worktree for {ERROR_BOLD}{branch}{ERROR_BOLD:#}{ERROR} from base {ERROR_BOLD}{base}{ERROR_BOLD:#}{ERROR:#}"
                    ),
                    None => format!(
                        "{ERROR_EMOJI} {ERROR}Failed to create worktree for {ERROR_BOLD}{branch}{ERROR_BOLD:#}{ERROR:#}"
                    ),
                };
                write!(f, "{}", format_error_block(header, error))
            }

            GitError::SwitchFailed { branch, error } => {
                let header = format!(
                    "{ERROR_EMOJI} {ERROR}Failed to switch to {ERROR_BOLD}{branch}{ERROR_BOLD:#}{ERROR:#}"
                );
                write!(f, "{}", format_error_block(header, error))
            }

            GitError::WorktreeRemovalFailed {
                branch,
                path,
                error,
            } => {
                let header = format!(
                    "{ERROR_EMOJI} {ERROR}Failed to remove worktree for {ERROR_BOLD}{branch}{ERROR_BOLD:#}{ERROR} at {ERROR_BOLD}{}{ERROR_BOLD:#}{ERROR:#}",
                    path.display()
                );
                write!(f, "{}", format_error_block(header, error))
            }

            GitError::BranchDeletionFailed { branch, error } => {
                let header = format!(
                    "{ERROR_EMOJI} {ERROR}Failed to delete branch {ERROR_BOLD}{branch}{ERROR_BOLD:#}{ERROR:#}"
                );
                write!(f, "{}", format_error_block(header, error))
            }
        }
    }
}

fn format_error_block(header: String, error: &str) -> String {
    use crate::styling::format_with_gutter;
    let trimmed = error.trim();
    if trimmed.is_empty() {
        header
    } else {
        format!("{header}\n\n{}", format_with_gutter(trimmed, "", None))
    }
}

impl std::error::Error for GitError {}

// Automatic conversion from io::Error to GitError
// This eliminates the need for manual .map_err() on output functions
// Parses exit codes from error messages to preserve signal information
//
// Protocol: execute_streaming() embeds exit codes in error messages as:
//   "CHILD_EXIT_CODE:{code} {original_message}"
// This allows passing exit codes through io::Error (which doesn't carry codes)
// while preserving the full error context.
impl From<std::io::Error> for GitError {
    fn from(e: std::io::Error) -> Self {
        let msg = e.to_string();
        // Parse exit code from error message (format: "CHILD_EXIT_CODE:130 Command failed...")
        if let Some(rest) = msg.strip_prefix("CHILD_EXIT_CODE:")
            && let Some(space_idx) = rest.find(' ')
            && let Ok(code) = rest[..space_idx].parse::<i32>()
        {
            let message = rest[space_idx + 1..].to_string();
            return GitError::ChildProcessExited { code, message };
        }
        GitError::CommandFailed(msg)
    }
}
