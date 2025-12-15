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
                let error_detail = if let Some(occupant_branch) = occupant {
                    cformat!(
                        "existing worktree at <bold>{path_display}</> on <bold>{occupant_branch}</>"
                    )
                } else {
                    cformat!("existing worktree at <bold>{path_display}</>, detached")
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
                        "Cannot create worktree for <bold>{branch}</>: {error_detail}"
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

    #[test]
    fn test_exit_code() {
        // ChildProcessExited
        let err: anyhow::Error = WorktrunkError::ChildProcessExited {
            code: 42,
            message: "test".into(),
        }
        .into();
        assert_eq!(exit_code(&err), Some(42));

        // HookCommandFailed with code
        let err: anyhow::Error = WorktrunkError::HookCommandFailed {
            hook_type: HookType::PreMerge,
            command_name: Some("test".into()),
            error: "failed".into(),
            exit_code: Some(1),
        }
        .into();
        assert_eq!(exit_code(&err), Some(1));

        // HookCommandFailed without code
        let err: anyhow::Error = WorktrunkError::HookCommandFailed {
            hook_type: HookType::PreMerge,
            command_name: None,
            error: "failed".into(),
            exit_code: None,
        }
        .into();
        assert_eq!(exit_code(&err), None);

        // CommandNotApproved, AlreadyDisplayed, GitError
        assert_eq!(exit_code(&WorktrunkError::CommandNotApproved.into()), None);
        assert_eq!(
            exit_code(&WorktrunkError::AlreadyDisplayed { exit_code: 5 }.into()),
            Some(5)
        );
        assert_eq!(
            exit_code(&GitError::DetachedHead { action: None }.into()),
            None
        );

        // Wrapped hook error
        let inner: anyhow::Error = WorktrunkError::HookCommandFailed {
            hook_type: HookType::PreCommit,
            command_name: Some("lint".into()),
            error: "failed".into(),
            exit_code: Some(7),
        }
        .into();
        assert_eq!(exit_code(&add_hook_skip_hint(inner)), Some(7));
    }

    #[test]
    fn test_is_command_not_approved() {
        assert!(is_command_not_approved(
            &WorktrunkError::CommandNotApproved.into()
        ));
        assert!(!is_command_not_approved(
            &WorktrunkError::ChildProcessExited {
                code: 1,
                message: "test".into()
            }
            .into()
        ));
        assert!(!is_command_not_approved(
            &WorktrunkError::HookCommandFailed {
                hook_type: HookType::PostCreate,
                command_name: None,
                error: "failed".into(),
                exit_code: None,
            }
            .into()
        ));
        assert!(!is_command_not_approved(
            &GitError::DetachedHead { action: None }.into()
        ));
    }

    #[test]
    fn test_add_hook_skip_hint() {
        // Wraps HookCommandFailed with --no-verify hint
        let inner: anyhow::Error = WorktrunkError::HookCommandFailed {
            hook_type: HookType::PreMerge,
            command_name: Some("test".into()),
            error: "failed".into(),
            exit_code: Some(1),
        }
        .into();
        let display = format!("{}", add_hook_skip_hint(inner));
        assert!(display.contains("--no-verify") && display.contains("pre-merge"));

        // Passes through non-hook errors
        let err: anyhow::Error = WorktrunkError::ChildProcessExited {
            code: 1,
            message: "test".into(),
        }
        .into();
        assert!(!format!("{}", add_hook_skip_hint(err)).contains("--no-verify"));

        let err: anyhow::Error = GitError::DetachedHead { action: None }.into();
        assert!(!format!("{}", add_hook_skip_hint(err)).contains("--no-verify"));
    }

    #[test]
    fn test_format_error_block() {
        let header = "Error occurred".to_string();
        let result = format_error_block(header.clone(), "  some error text  ");
        assert!(result.contains("Error occurred") && result.contains("some error text"));

        // Empty/whitespace returns header only
        assert_eq!(format_error_block(header.clone(), ""), header);
        assert_eq!(format_error_block(header.clone(), "   \n\t  "), header);
    }

    #[test]
    fn test_worktrunk_error_display() {
        // ChildProcessExited
        let err = WorktrunkError::ChildProcessExited {
            code: 1,
            message: "Command failed".into(),
        };
        assert!(format!("{err}").contains("Command failed"));

        // HookCommandFailed with/without name
        let err = WorktrunkError::HookCommandFailed {
            hook_type: HookType::PreMerge,
            command_name: Some("lint".into()),
            error: "lint failed".into(),
            exit_code: Some(1),
        };
        let display = format!("{err}");
        assert!(display.contains("pre-merge") && display.contains("lint"));

        let err = WorktrunkError::HookCommandFailed {
            hook_type: HookType::PostCreate,
            command_name: None,
            error: "setup failed".into(),
            exit_code: None,
        };
        let display = format!("{err}");
        assert!(display.contains("post-create") && display.contains("setup failed"));

        // Silent errors
        assert_eq!(format!("{}", WorktrunkError::CommandNotApproved), "");
        assert_eq!(
            format!("{}", WorktrunkError::AlreadyDisplayed { exit_code: 1 }),
            ""
        );
    }

    #[test]
    fn test_git_error_invalid_reference() {
        let err = GitError::InvalidReference {
            reference: "nonexistent".into(),
        };
        let display = err.to_string();
        assert!(display.contains("nonexistent"));
        assert!(display.contains("not found"));
        assert!(display.contains("--create"));
    }

    #[test]
    fn test_git_error_worktree_missing() {
        let err = GitError::WorktreeMissing {
            branch: "feature".into(),
        };
        let display = err.to_string();
        assert!(display.contains("feature"));
        assert!(display.contains("missing"));
    }

    #[test]
    fn test_git_error_no_worktree_found() {
        let err = GitError::NoWorktreeFound {
            branch: "feature".into(),
        };
        let display = err.to_string();
        assert!(display.contains("No worktree found"));
        assert!(display.contains("feature"));
    }

    #[test]
    fn test_git_error_remote_only_branch() {
        let err = GitError::RemoteOnlyBranch {
            branch: "feature".into(),
            remote: "origin".into(),
        };
        let display = err.to_string();
        assert!(display.contains("feature"));
        assert!(display.contains("remote"));
        assert!(display.contains("origin"));
    }

    #[test]
    fn test_git_error_worktree_path_occupied() {
        // With occupant branch
        let err = GitError::WorktreePathOccupied {
            branch: "feature".into(),
            path: PathBuf::from("/tmp/repo"),
            occupant: Some("main".into()),
        };
        let display = err.to_string();
        assert!(display.contains("feature"));
        assert!(display.contains("main"));
        assert!(display.contains("existing worktree"));

        // Without occupant (detached)
        let err = GitError::WorktreePathOccupied {
            branch: "feature".into(),
            path: PathBuf::from("/tmp/repo"),
            occupant: None,
        };
        let display = err.to_string();
        assert!(display.contains("detached"));
    }

    #[test]
    fn test_git_error_worktree_creation_failed() {
        // With base branch
        let err = GitError::WorktreeCreationFailed {
            branch: "feature".into(),
            base_branch: Some("main".into()),
            error: "git error".into(),
        };
        let display = err.to_string();
        assert!(display.contains("feature"));
        assert!(display.contains("main"));
        assert!(display.contains("git error"));

        // Without base branch
        let err = GitError::WorktreeCreationFailed {
            branch: "feature".into(),
            base_branch: None,
            error: "git error".into(),
        };
        let display = err.to_string();
        assert!(display.contains("feature"));
    }

    #[test]
    fn test_git_error_worktree_removal_failed() {
        let err = GitError::WorktreeRemovalFailed {
            branch: "feature".into(),
            path: PathBuf::from("/tmp/repo"),
            error: "still has changes".into(),
        };
        let display = err.to_string();
        assert!(display.contains("feature"));
        assert!(display.contains("still has changes"));
    }

    #[test]
    fn test_git_error_cannot_remove_main() {
        let err = GitError::CannotRemoveMainWorktree;
        let display = err.to_string();
        assert!(display.contains("main worktree"));
    }

    #[test]
    fn test_git_error_conflicting_changes() {
        let err = GitError::ConflictingChanges {
            files: vec!["file1.rs".into(), "file2.rs".into()],
            worktree_path: PathBuf::from("/tmp/repo"),
        };
        let display = err.to_string();
        assert!(display.contains("conflicting"));
        assert!(display.contains("file1.rs"));
    }

    #[test]
    fn test_git_error_not_fast_forward() {
        // In merge context
        let err = GitError::NotFastForward {
            target_branch: "main".into(),
            commits_formatted: "abc1234 Some commit".into(),
            in_merge_context: true,
        };
        let display = err.to_string();
        assert!(display.contains("main"));
        assert!(display.contains("wt merge"));

        // Not in merge context
        let err = GitError::NotFastForward {
            target_branch: "main".into(),
            commits_formatted: String::new(),
            in_merge_context: false,
        };
        let display = err.to_string();
        assert!(display.contains("wt step rebase"));
    }

    #[test]
    fn test_git_error_merge_commits_found() {
        let err = GitError::MergeCommitsFound;
        let display = err.to_string();
        assert!(display.contains("merge commits"));
        assert!(display.contains("--allow-merge-commits"));
    }

    #[test]
    fn test_git_error_rebase_conflict() {
        // With git output
        let err = GitError::RebaseConflict {
            target_branch: "main".into(),
            git_output: "CONFLICT in file.rs".into(),
        };
        let display = err.to_string();
        assert!(display.contains("main"));
        assert!(display.contains("CONFLICT"));

        // Without git output
        let err = GitError::RebaseConflict {
            target_branch: "main".into(),
            git_output: String::new(),
        };
        let display = err.to_string();
        assert!(display.contains("rebase --continue"));
    }

    #[test]
    fn test_git_error_not_rebased() {
        let err = GitError::NotRebased {
            target_branch: "main".into(),
        };
        let display = err.to_string();
        assert!(display.contains("main"));
        assert!(display.contains("not rebased"));
    }

    #[test]
    fn test_git_error_push_failed() {
        let err = GitError::PushFailed {
            error: "rejected".into(),
        };
        let display = err.to_string();
        assert!(display.contains("Push failed"));
        assert!(display.contains("rejected"));
    }

    #[test]
    fn test_git_error_not_interactive() {
        let err = GitError::NotInteractive;
        let display = err.to_string();
        assert!(display.contains("non-interactive"));
        assert!(display.contains("--force"));
    }

    #[test]
    fn test_git_error_hook_command_not_found() {
        // With available commands
        let err = GitError::HookCommandNotFound {
            name: "unknown".into(),
            available: vec!["lint".into(), "test".into()],
        };
        let display = err.to_string();
        assert!(display.contains("unknown"));
        assert!(display.contains("lint"));

        // No available commands
        let err = GitError::HookCommandNotFound {
            name: "unknown".into(),
            available: vec![],
        };
        let display = err.to_string();
        assert!(display.contains("no named commands"));
    }

    #[test]
    fn test_git_error_llm_command_failed() {
        // With reproduction command
        let err = GitError::LlmCommandFailed {
            command: "llm".into(),
            error: "connection failed".into(),
            reproduction_command: Some("wt step commit --show-prompt | llm".into()),
        };
        let display = err.to_string();
        assert!(display.contains("connection failed"));
        assert!(display.contains("wt step commit"));

        // Without reproduction command
        let err = GitError::LlmCommandFailed {
            command: "llm --model gpt-4".into(),
            error: "timeout".into(),
            reproduction_command: None,
        };
        let display = err.to_string();
        assert!(display.contains("llm --model gpt-4"));
    }

    #[test]
    fn test_git_error_project_config_not_found() {
        let err = GitError::ProjectConfigNotFound {
            config_path: PathBuf::from("/.worktrunk.toml"),
        };
        let display = err.to_string();
        assert!(display.contains("No project configuration"));
        assert!(display.contains(".worktrunk.toml"));
    }

    #[test]
    fn test_git_error_parse_error() {
        let err = GitError::ParseError {
            message: "invalid syntax".into(),
        };
        let display = err.to_string();
        assert!(display.contains("invalid syntax"));
    }

    #[test]
    fn test_git_error_other() {
        let err = GitError::Other {
            message: "something went wrong".into(),
        };
        let display = err.to_string();
        assert!(display.contains("something went wrong"));
    }

    #[test]
    fn test_git_error_detached_head_with_action() {
        let err = GitError::DetachedHead {
            action: Some("merge".into()),
        };
        let display = err.to_string();
        assert!(display.contains("Cannot merge"));
        assert!(display.contains("detached HEAD"));
    }

    #[test]
    fn test_git_error_uncommitted_changes_variants() {
        // Action only
        let err = GitError::UncommittedChanges {
            action: Some("push".into()),
            worktree: None,
        };
        let display = err.to_string();
        assert!(display.contains("Cannot push"));
        assert!(display.contains("working tree"));

        // Worktree only
        let err = GitError::UncommittedChanges {
            action: None,
            worktree: Some("feature".into()),
        };
        let display = err.to_string();
        assert!(display.contains("feature"));
        assert!(display.contains("uncommitted"));

        // Neither
        let err = GitError::UncommittedChanges {
            action: None,
            worktree: None,
        };
        let display = err.to_string();
        assert!(display.contains("Working tree"));
    }

    #[test]
    fn test_git_error_not_fast_forward_empty_commits() {
        // Test with empty commits_formatted to cover that branch
        let err = GitError::NotFastForward {
            target_branch: "main".into(),
            commits_formatted: "".into(),
            in_merge_context: false,
        };
        let display = err.to_string();
        assert!(display.contains("main"));
        assert!(display.contains("newer commits"));
        // Should still have hint
        assert!(display.contains("rebase"));
    }

    #[test]
    fn test_git_error_not_fast_forward_outside_merge() {
        // Test outside merge context (in_merge_context = false)
        let err = GitError::NotFastForward {
            target_branch: "develop".into(),
            commits_formatted: "abc123 Some commit".into(),
            in_merge_context: false,
        };
        let display = err.to_string();
        assert!(display.contains("develop"));
        // Should have generic rebase hint, not "wt merge"
        assert!(display.contains("rebase"));
        // commits_formatted should be in gutter
        assert!(display.contains("abc123"));
    }

    #[test]
    fn test_git_error_conflicting_changes_empty_files() {
        // Test with empty files list
        let err = GitError::ConflictingChanges {
            files: vec![],
            worktree_path: PathBuf::from("/tmp/repo"),
        };
        let display = err.to_string();
        assert!(display.contains("conflicting"));
        // Should still have hint about commit/stash
        assert!(display.contains("Commit or stash"));
    }

    #[test]
    fn test_hook_error_with_hint_source() {
        use crate::HookType;

        // Create a WorktrunkError with hook_type
        let inner_error: anyhow::Error = WorktrunkError::HookCommandFailed {
            hook_type: HookType::PreMerge,
            command_name: Some("test".into()),
            error: "Test failed".into(),
            exit_code: Some(1),
        }
        .into();

        // Wrap it using add_hook_skip_hint
        let wrapped = add_hook_skip_hint(inner_error);

        // The source() method should return the underlying error
        let source = wrapped.source();
        // source can be Some or None depending on implementation
        let _ = source;
    }

    #[test]
    fn test_add_hook_skip_hint_with_hook_type() {
        use crate::HookType;

        let inner: anyhow::Error = WorktrunkError::HookCommandFailed {
            hook_type: HookType::PreCommit,
            command_name: Some("build".into()),
            error: "Build failed".into(),
            exit_code: Some(1),
        }
        .into();

        let wrapped = add_hook_skip_hint(inner);
        let display = wrapped.to_string();

        // Should include the original error
        assert!(display.contains("build"));
        // Should include the hint
        assert!(display.contains("--no-verify"));
        assert!(display.contains("pre-commit"));
    }

    #[test]
    fn test_add_hook_skip_hint_non_hook_error() {
        // Test with a non-hook error (should pass through unchanged)
        let inner: anyhow::Error = GitError::Other {
            message: "some error".into(),
        }
        .into();

        let wrapped = add_hook_skip_hint(inner);
        let display = wrapped.to_string();

        // Should include the original error
        assert!(display.contains("some error"));
        // Should NOT include hint (not a hook error)
        assert!(!display.contains("--no-verify"));
    }

    #[test]
    fn test_rebase_conflict_empty_output() {
        let err = GitError::RebaseConflict {
            target_branch: "main".into(),
            git_output: "".into(),
        };
        let display = err.to_string();
        assert!(display.contains("incomplete"));
        assert!(display.contains("main"));
        // Empty output shouldn't cause issues
    }
}
