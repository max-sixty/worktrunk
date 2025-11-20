//! Git error types and formatting

use std::path::PathBuf;

use super::HookType;
use crate::path::format_path_for_display;

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
    /// Cannot prompt for approval in non-interactive environment
    NotInteractive,
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
    /// Cannot remove main worktree
    CannotRemoveMainWorktree,
}

impl GitError {
    /// Create a formatted user-facing error message with emoji and styling.
    ///
    /// Use this for application errors (not raw git output).
    /// For raw git stderr, use `CommandFailed` directly.
    pub fn message(msg: impl Into<String>) -> Self {
        use crate::styling::{ERROR, ERROR_EMOJI};
        let msg = msg.into();
        GitError::CommandFailed(format!("{ERROR_EMOJI} {ERROR}{msg}{ERROR:#}"))
    }
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use crate::styling::{ERROR, ERROR_BOLD, ERROR_EMOJI, HINT, HINT_EMOJI};

        match self {
            // Generic error - raw passthrough, no formatting
            // Specific error variants should be used for git command failures that need formatting
            GitError::CommandFailed(msg) => write!(f, "{}", msg),

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
            } => {
                // Build optional command name suffix
                let name_suffix = command_name
                    .as_ref()
                    .map(|n| format!(": {ERROR_BOLD}{n}{ERROR_BOLD:#}"))
                    .unwrap_or_default();

                write!(
                    f,
                    "{ERROR_EMOJI} {ERROR}{hook_type} command failed{name_suffix}: {error}{ERROR:#}\n\n{HINT_EMOJI} {HINT}Use --no-verify to skip {hook_type} commands{HINT:#}"
                )
            }

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
                    format_path_for_display(worktree_path)
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

            // Not interactive (cannot prompt)
            GitError::NotInteractive => {
                write!(
                    f,
                    "{ERROR_EMOJI} {ERROR}Cannot prompt for approval in non-interactive environment{ERROR:#}\n\n{HINT_EMOJI} {HINT}In CI/CD, use --force to skip prompts. To pre-approve commands, use 'wt beta ask-approvals'{HINT:#}"
                )
            }

            // Child process exited with non-zero code
            // Display with error emoji - main.rs will use the exit code
            GitError::ChildProcessExited { code: _, message } => {
                write!(f, "{ERROR_EMOJI} {ERROR}{message}{ERROR:#}")
            }

            // Push failed
            GitError::PushFailed { error } => {
                let header = format!("{ERROR_EMOJI} {ERROR}Push failed{ERROR:#}");
                write!(f, "{}", format_error_block(header, error))
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
                    format_path_for_display(path)
                )
            }

            GitError::WorktreeCreationFailed {
                branch,
                base_branch,
                error,
            } => {
                // Build optional base branch suffix (includes re-establishing ERROR color)
                let base_suffix = base_branch
                    .as_ref()
                    .map(|base| format!("{ERROR} from base {ERROR_BOLD}{base}{ERROR_BOLD:#}"))
                    .unwrap_or_default();

                let header = format!(
                    "{ERROR_EMOJI} {ERROR}Failed to create worktree for {ERROR_BOLD}{branch}{ERROR_BOLD:#}{base_suffix}{ERROR:#}"
                );
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
                    format_path_for_display(path)
                );
                write!(f, "{}", format_error_block(header, error))
            }

            GitError::BranchDeletionFailed { branch, error } => {
                let header = format!(
                    "{ERROR_EMOJI} {ERROR}Failed to delete branch {ERROR_BOLD}{branch}{ERROR_BOLD:#}{ERROR:#}"
                );
                write!(f, "{}", format_error_block(header, error))
            }

            GitError::CannotRemoveMainWorktree => {
                write!(
                    f,
                    "{ERROR_EMOJI} {ERROR}Cannot remove main worktree{ERROR:#}\n\n{HINT_EMOJI} {HINT}Use 'wt remove' from a secondary worktree, or 'wt switch' to change branches{HINT:#}"
                )
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
        format!("{header}\n{}", format_with_gutter(trimmed, "", None))
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
