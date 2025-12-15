//! Worktrunk error types and formatting
//!
//! This module provides typed error handling:
//!
//! - **`GitError`** - A typed enum for domain errors that can be pattern-matched
//!   and tested. Use `.into()` to convert to `anyhow::Error` while preserving the
//!   type for pattern matching. Display produces styled output for users.
//!
//! - **`WorktrunkError`** - A minimal enum for semantic errors that need
//!   special handling (exit codes, silent errors).

use std::path::PathBuf;

use color_print::{cformat, cwrite};

use super::HookType;
use crate::path::format_path_for_display;
use crate::styling::{
    ERROR_EMOJI, HINT_EMOJI, error_message, format_with_gutter, hint_message, info_message,
};

/// Domain errors for git and worktree operations.
///
/// This enum provides structured error data that can be pattern-matched and tested.
/// Each variant stores the data needed to construct a user-facing error message.
/// Display produces styled output with emoji and colors.
///
/// # Usage
///
/// ```ignore
/// // Return a typed error (Display produces styled output)
/// return Err(GitError::DetachedHead { action: Some("merge".into()) }.into());
///
/// // Pattern match on errors
/// if let Some(GitError::BranchAlreadyExists { branch }) = err.downcast_ref() {
///     println!("Branch {} exists", branch);
/// }
/// ```
#[derive(Debug, Clone)]
pub enum GitError {
    // Git state errors
    DetachedHead {
        action: Option<String>,
    },
    UncommittedChanges {
        action: Option<String>,
        /// Branch or worktree identifier (for multi-worktree operations)
        worktree: Option<String>,
    },
    BranchAlreadyExists {
        branch: String,
    },
    InvalidReference {
        reference: String,
    },

    // Worktree errors
    WorktreeMissing {
        branch: String,
    },
    NoWorktreeFound {
        branch: String,
    },
    RemoteOnlyBranch {
        branch: String,
        remote: String,
    },
    WorktreePathOccupied {
        branch: String,
        path: PathBuf,
        occupant: Option<String>,
    },
    WorktreePathExists {
        path: PathBuf,
    },
    WorktreeCreationFailed {
        branch: String,
        base_branch: Option<String>,
        error: String,
    },
    WorktreeRemovalFailed {
        branch: String,
        path: PathBuf,
        error: String,
    },
    CannotRemoveMainWorktree,

    // Merge/push errors
    ConflictingChanges {
        files: Vec<String>,
        worktree_path: PathBuf,
    },
    NotFastForward {
        target_branch: String,
        commits_formatted: String,
        in_merge_context: bool,
    },
    MergeCommitsFound,
    RebaseConflict {
        target_branch: String,
        git_output: String,
    },
    NotRebased {
        target_branch: String,
    },
    PushFailed {
        error: String,
    },

    // Validation/other errors
    NotInteractive,
    HookCommandNotFound {
        name: String,
        available: Vec<String>,
    },
    ParseError {
        message: String,
    },
    LlmCommandFailed {
        command: String,
        error: String,
        /// Full command to reproduce the failure, e.g., "wt step commit --show-prompt | llm"
        reproduction_command: Option<String>,
    },
    ProjectConfigNotFound {
        config_path: PathBuf,
    },
    Other {
        message: String,
    },
}

impl std::error::Error for GitError {}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitError::DetachedHead { action } => {
                let message = match action {
                    Some(action) => format!("Cannot {action}: not on a branch (detached HEAD)"),
                    None => "Not on a branch (detached HEAD)".to_string(),
                };
                write!(
                    f,
                    "{}\n\n{}",
                    error_message(&message),
                    hint_message(cformat!(
                        "Switch to a branch first with <bright-black>git switch <<branch>></>"
                    ))
                )
            }

            GitError::UncommittedChanges { action, worktree } => {
                let message = match (action, worktree) {
                    (Some(action), Some(wt)) => {
                        cformat!("Cannot {action}: <bold>{wt}</> has uncommitted changes")
                    }
                    (Some(action), None) => {
                        cformat!("Cannot {action}: working tree has uncommitted changes")
                    }
                    (None, Some(wt)) => {
                        cformat!("<bold>{wt}</> has uncommitted changes")
                    }
                    (None, None) => cformat!("Working tree has uncommitted changes"),
                };
                write!(
                    f,
                    "{}\n\n{}",
                    error_message(&message),
                    hint_message("Commit or stash changes first")
                )
            }

            GitError::BranchAlreadyExists { branch } => {
                write!(
                    f,
                    "{}\n\n{}",
                    error_message(cformat!("Branch <bold>{branch}</> already exists")),
                    hint_message(cformat!(
                        "Remove <bright-black>--create</> flag to switch to the existing branch"
                    ))
                )
            }

            GitError::InvalidReference { reference } => {
                write!(
                    f,
                    "{}\n\n{}",
                    error_message(cformat!("Branch <bold>{reference}</> not found")),
                    hint_message(cformat!(
                        "Use <bright-black>--create</> to create a new branch, or <bright-black>wt list --branches --remotes</> for available branches"
                    ))
                )
            }

            GitError::WorktreeMissing { branch } => {
                write!(
                    f,
                    "{}\n\n{}",
                    error_message(cformat!("Worktree directory missing for <bold>{branch}</>")),
                    hint_message(cformat!(
                        "Run <bright-black>git worktree prune</> to clean up"
                    ))
                )
            }

            GitError::NoWorktreeFound { branch } => {
                write!(
                    f,
                    "{}",
                    error_message(cformat!("No worktree found for branch <bold>{branch}</>"))
                )
            }

            GitError::RemoteOnlyBranch { branch, remote } => {
                cwrite!(
                    f,
                    "{ERROR_EMOJI} <red>Branch <bold>{branch}</> exists only on remote ({remote}/{branch})</>\n\n{HINT_EMOJI} <dim>Use <bright-black>wt switch {branch}</> to create a local worktree</>"
                )
            }

            GitError::WorktreePathOccupied {
                branch,
                path,
                occupant,
            } => {
                let path_display = format_path_for_display(path);
                let error_suffix = if let Some(occupant_branch) = occupant {
                    cformat!("existing worktree, on <bold>{occupant_branch}</>")
                } else {
                    "existing worktree, detached".to_string()
                };
                // Hint is self-contained (includes both path and branch)
                let hint = hint_message(cformat!(
                    "Switch the worktree at <bright-black>{path_display}</> to <bright-black>{branch}</>"
                ));
                let command = format!("cd {path_display} && git switch {branch}");
                write!(
                    f,
                    "{}\n\n{}\n{}",
                    error_message(cformat!(
                        "Cannot create worktree for <bold>{branch}</> at <bold>{path_display}</>: {error_suffix}"
                    )),
                    hint,
                    format_with_gutter(&command, "", None)
                )
            }

            GitError::WorktreePathExists { path } => {
                let path_display = format_path_for_display(path);
                write!(
                    f,
                    "{}\n\n{}",
                    error_message(cformat!(
                        "Directory already exists: <bold>{path_display}</>"
                    )),
                    hint_message(cformat!(
                        "Remove with <bright-black>rm -rf {path_display}</> or use a different branch name"
                    ))
                )
            }

            GitError::WorktreeCreationFailed {
                branch,
                base_branch,
                error,
            } => {
                let header = if let Some(base) = base_branch {
                    error_message(cformat!(
                        "Failed to create worktree for <bold>{branch}</> from base <bold>{base}</>"
                    ))
                } else {
                    error_message(cformat!("Failed to create worktree for <bold>{branch}</>"))
                };
                write!(f, "{}", format_error_block(header, error))
            }

            GitError::WorktreeRemovalFailed {
                branch,
                path,
                error,
            } => {
                let path_display = format_path_for_display(path);
                let header = error_message(cformat!(
                    "Failed to remove worktree for <bold>{branch}</> @ <bold>{path_display}</>"
                ));
                write!(f, "{}", format_error_block(header, error))
            }

            GitError::CannotRemoveMainWorktree => {
                write!(
                    f,
                    "{}",
                    error_message("The main worktree cannot be removed")
                )
            }

            GitError::ConflictingChanges {
                files,
                worktree_path,
            } => {
                write!(
                    f,
                    "{}\n\n",
                    error_message("Cannot push: conflicting uncommitted changes in:")
                )?;
                if !files.is_empty() {
                    let joined_files = files.join("\n");
                    write!(f, "{}", format_with_gutter(&joined_files, "", None))?;
                }
                let path_display = format_path_for_display(worktree_path);
                write!(
                    f,
                    "\n{}",
                    hint_message(format!(
                        "Commit or stash these changes in {path_display} first"
                    ))
                )
            }

            GitError::NotFastForward {
                target_branch,
                commits_formatted,
                in_merge_context,
            } => {
                write!(
                    f,
                    "{}",
                    error_message(cformat!(
                        "Can't push to local <bold>{target_branch}</> branch: it has newer commits"
                    ))
                )?;
                if !commits_formatted.is_empty() {
                    write!(f, "\n{}", format_with_gutter(commits_formatted, "", None))?;
                }
                // Context-appropriate hint
                if *in_merge_context {
                    write!(
                        f,
                        "\n{}",
                        hint_message(cformat!(
                            "Run <bright-black>wt merge</> again to incorporate these changes"
                        ))
                    )
                } else {
                    write!(
                        f,
                        "\n{}",
                        hint_message(cformat!(
                            "Use <bright-black>wt step rebase</> or <bright-black>wt merge</> to rebase onto <bold>{target_branch}</>"
                        ))
                    )
                }
            }

            GitError::MergeCommitsFound => {
                write!(
                    f,
                    "{}\n\n{}",
                    error_message("Found merge commits in push range"),
                    hint_message(cformat!(
                        "Use <bright-black>--allow-merge-commits</> to push non-linear history"
                    ))
                )
            }

            GitError::RebaseConflict {
                target_branch,
                git_output,
            } => {
                write!(
                    f,
                    "{}",
                    error_message(cformat!("Rebase onto <bold>{target_branch}</> incomplete"))
                )?;
                if !git_output.is_empty() {
                    write!(f, "\n{}", format_with_gutter(git_output, "", None))
                } else {
                    write!(
                        f,
                        "\n\n{}\n{}",
                        hint_message(cformat!(
                            "Resolve conflicts and run <bright-black>git rebase --continue</>"
                        )),
                        hint_message(cformat!(
                            "Or abort with <bright-black>git rebase --abort</>"
                        ))
                    )
                }
            }

            GitError::NotRebased { target_branch } => {
                write!(
                    f,
                    "{}\n\n{}",
                    error_message(cformat!("Branch not rebased onto <bold>{target_branch}</>")),
                    hint_message(cformat!(
                        "Run <bright-black>wt rebase</> first or remove <bright-black>--no-rebase</>"
                    ))
                )
            }

            GitError::PushFailed { error } => {
                let header = error_message("Push failed");
                write!(f, "{}", format_error_block(header, error))
            }

            GitError::NotInteractive => {
                write!(
                    f,
                    "{}\n\n{}",
                    error_message("Cannot prompt for approval in non-interactive environment"),
                    hint_message(cformat!(
                        "In CI/CD, use <bright-black>--force</> to skip prompts. To pre-approve commands, use <bright-black>wt hook approvals add</>"
                    ))
                )
            }

            GitError::HookCommandNotFound { name, available } => {
                if available.is_empty() {
                    write!(
                        f,
                        "{}",
                        error_message(cformat!(
                            "No command named <bold>{name}</> (hook has no named commands)"
                        ))
                    )
                } else {
                    let available_str = available
                        .iter()
                        .map(|s| cformat!("<bold>{s}</>"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    write!(
                        f,
                        "{}",
                        error_message(cformat!(
                            "No command named <bold>{name}</> (available: {available_str})"
                        ))
                    )
                }
            }

            GitError::LlmCommandFailed {
                command,
                error,
                reproduction_command,
            } => {
                let error_header = error_message("Commit generation command failed");
                let error_block = format_error_block(error_header, error);
                // Show full pipeline command if available, otherwise just the LLM command
                let display_command = reproduction_command.as_ref().unwrap_or(command);
                let command_gutter = format_with_gutter(display_command, "", None);
                write!(
                    f,
                    "{}\n\n{}\n{}",
                    error_block.trim_end(),
                    info_message("Ran command:"),
                    command_gutter.trim_end()
                )
            }

            GitError::ProjectConfigNotFound { config_path } => {
                let path_display = format_path_for_display(config_path);
                write!(
                    f,
                    "{}\n\n{}",
                    error_message("No project configuration found"),
                    hint_message(cformat!("Create a config file at: <bold>{path_display}</>"))
                )
            }

            GitError::ParseError { message } => {
                write!(f, "{}", error_message(message))
            }

            GitError::Other { message } => {
                write!(f, "{}", error_message(message))
            }
        }
    }
}

/// Semantic errors that require special handling in main.rs
///
/// Most errors use anyhow::bail! with formatted messages. This enum is only
/// for cases that need exit code extraction or special handling.
#[derive(Debug)]
pub enum WorktrunkError {
    /// Child process exited with non-zero code (preserves exit code for signals)
    ChildProcessExited { code: i32, message: String },
    /// Hook command failed
    HookCommandFailed {
        hook_type: HookType,
        command_name: Option<String>,
        error: String,
        exit_code: Option<i32>,
    },
    /// Command was not approved by user (silent error)
    CommandNotApproved,
    /// Error already displayed, just exit with given code (silent error)
    AlreadyDisplayed { exit_code: i32 },
}

impl std::fmt::Display for WorktrunkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorktrunkError::ChildProcessExited { message, .. } => {
                write!(f, "{}", error_message(message))
            }
            WorktrunkError::HookCommandFailed {
                hook_type,
                command_name,
                error,
                ..
            } => {
                // Note: Callers that support --no-verify should add the hint themselves
                if let Some(name) = command_name {
                    write!(
                        f,
                        "{}",
                        error_message(cformat!(
                            "{hook_type} command failed: <bold>{name}</>: {error}"
                        ))
                    )
                } else {
                    write!(
                        f,
                        "{}",
                        error_message(format!("{hook_type} command failed: {error}"))
                    )
                }
            }
            WorktrunkError::CommandNotApproved => {
                Ok(()) // on_skip callback handles the printing
            }
            WorktrunkError::AlreadyDisplayed { .. } => {
                Ok(()) // error already shown via output functions
            }
        }
    }
}

impl std::error::Error for WorktrunkError {}

/// Extract exit code from WorktrunkError, if applicable
pub fn exit_code(err: &anyhow::Error) -> Option<i32> {
    // Check for wrapped HookErrorWithHint first
    if let Some(wrapper) = err.downcast_ref::<HookErrorWithHint>() {
        return exit_code(&wrapper.inner);
    }
    err.downcast_ref::<WorktrunkError>().and_then(|e| match e {
        WorktrunkError::ChildProcessExited { code, .. } => Some(*code),
        WorktrunkError::HookCommandFailed { exit_code, .. } => *exit_code,
        WorktrunkError::CommandNotApproved => None,
        WorktrunkError::AlreadyDisplayed { exit_code } => Some(*exit_code),
    })
}

/// Check if error is CommandNotApproved (silent error)
pub fn is_command_not_approved(err: &anyhow::Error) -> bool {
    err.downcast_ref::<WorktrunkError>()
        .is_some_and(|e| matches!(e, WorktrunkError::CommandNotApproved))
}

/// If the error is a HookCommandFailed, wrap it to add a hint about using --no-verify.
///
/// ## When to use
///
/// Use this for commands where a hook runs as a side effect of the user's intent:
/// - `wt merge` - user wants to merge, hooks run as part of that
/// - `wt commit` - user wants to commit, pre-commit hooks run
/// - `wt switch --create` - user wants a worktree, post-create hooks run
///
/// ## When NOT to use
///
/// Don't use for `wt hook <type>` - the user explicitly asked to run hooks,
/// so suggesting `--no-verify` makes no sense.
pub fn add_hook_skip_hint(err: anyhow::Error) -> anyhow::Error {
    // Extract hook_type first (if applicable), then decide whether to wrap
    let hook_type = err
        .downcast_ref::<WorktrunkError>()
        .and_then(|wt_err| match wt_err {
            WorktrunkError::HookCommandFailed { hook_type, .. } => Some(*hook_type),
            _ => None,
        });

    match hook_type {
        Some(hook_type) => HookErrorWithHint {
            inner: err,
            hook_type,
        }
        .into(),
        None => err,
    }
}

/// Wrapper that displays a HookCommandFailed error with the --no-verify hint.
/// Created by `add_hook_skip_hint()` for commands that support `--no-verify`.
#[derive(Debug)]
pub struct HookErrorWithHint {
    inner: anyhow::Error,
    hook_type: HookType,
}

impl std::fmt::Display for HookErrorWithHint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Display the original error (always HookCommandFailed - validated by add_hook_skip_hint)
        write!(f, "{}", self.inner)?;
        // Add the hint
        write!(
            f,
            "\n\n{}",
            hint_message(cformat!(
                "Use <bright-black>--no-verify</> to skip {} commands",
                self.hook_type
            ))
        )
    }
}

impl std::error::Error for HookErrorWithHint {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.inner.source()
    }
}

/// Format an error with header and gutter content
fn format_error_block(header: String, error: &str) -> String {
    let trimmed = error.trim();
    if trimmed.is_empty() {
        header
    } else {
        format!("{header}\n{}", format_with_gutter(trimmed, "", None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;

    #[test]
    fn snapshot_detached_head_display() {
        let err = GitError::DetachedHead { action: None };
        assert_snapshot!(err.to_string(), @r"
        ❌ [31mNot on a branch (detached HEAD)[39m

        💡 [2mSwitch to a branch first with [90mgit switch <branch>[39m[22m
        ");
    }

    #[test]
    fn snapshot_uncommitted_with_worktree_display() {
        let err = GitError::UncommittedChanges {
            action: Some("merge".into()),
            worktree: Some("wt".into()),
        };
        assert_snapshot!(err.to_string(), @r"
        ❌ [31mCannot merge: [1mwt[22m has uncommitted changes[39m

        💡 [2mCommit or stash changes first[22m
        ");
    }

    #[test]
    fn snapshot_into_preserves_type_for_display() {
        // .into() preserves type so we can downcast and use Display
        let err: anyhow::Error = GitError::BranchAlreadyExists {
            branch: "main".into(),
        }
        .into();

        let downcast = err.downcast_ref::<GitError>().expect("Should downcast");
        assert_snapshot!(downcast.to_string(), @r"
        ❌ [31mBranch [1mmain[22m already exists[39m

        💡 [2mRemove [90m--create[39m flag to switch to the existing branch[22m
        ");
    }

    #[test]
    fn test_pattern_matching_with_into() {
        let err: anyhow::Error = GitError::BranchAlreadyExists {
            branch: "main".into(),
        }
        .into();

        if let Some(GitError::BranchAlreadyExists { branch }) = err.downcast_ref::<GitError>() {
            assert_eq!(branch, "main");
        } else {
            panic!("Failed to downcast and pattern match");
        }
    }

    #[test]
    fn snapshot_worktree_error_with_path() {
        let err = GitError::WorktreePathExists {
            path: PathBuf::from("/some/path"),
        };
        assert_snapshot!(err.to_string(), @r"
        ❌ [31mDirectory already exists: [1m/some/path[22m[39m

        💡 [2mRemove with [90mrm -rf /some/path[39m or use a different branch name[22m
        ");
    }
}
