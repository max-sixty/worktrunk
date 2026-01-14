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

use std::borrow::Cow;
use std::path::PathBuf;

use color_print::{cformat, cwrite};
use shell_escape::escape;

use super::HookType;
use crate::path::format_path_for_display;
use crate::styling::{
    ERROR_SYMBOL, HINT_SYMBOL, error_message, format_with_gutter, hint_message, info_message,
    suggest_command,
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
        /// Branch name (for multi-worktree operations)
        branch: Option<String>,
    },
    BranchAlreadyExists {
        branch: String,
    },
    InvalidReference {
        reference: String,
    },

    // Worktree errors
    NotInWorktree {
        /// The action that requires being in a worktree
        action: Option<String>,
    },
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
        branch: String,
        path: PathBuf,
        create: bool,
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
    WorktreeLocked {
        branch: String,
        path: PathBuf,
        reason: Option<String>,
    },

    // Merge/push errors
    ConflictingChanges {
        target_branch: String,
        files: Vec<String>,
        worktree_path: PathBuf,
    },
    NotFastForward {
        target_branch: String,
        commits_formatted: String,
        in_merge_context: bool,
    },
    RebaseConflict {
        target_branch: String,
        git_output: String,
    },
    NotRebased {
        target_branch: String,
    },
    PushFailed {
        target_branch: String,
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
    WorktreeIncludeParseError {
        error: String,
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
    WorktreeNotFound {
        branch: String,
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
                    "{}\n{}",
                    error_message(&message),
                    hint_message(cformat!(
                        "To switch to a branch, run <bright-black>git switch <<branch>></>"
                    ))
                )
            }

            GitError::UncommittedChanges { action, branch } => {
                let message = match (action, branch) {
                    (Some(action), Some(b)) => {
                        cformat!("Cannot {action}: <bold>{b}</> has uncommitted changes")
                    }
                    (Some(action), None) => {
                        cformat!("Cannot {action}: working tree has uncommitted changes")
                    }
                    (None, Some(b)) => {
                        cformat!("<bold>{b}</> has uncommitted changes")
                    }
                    (None, None) => cformat!("Working tree has uncommitted changes"),
                };
                write!(
                    f,
                    "{}\n{}",
                    error_message(&message),
                    hint_message("Commit or stash changes first")
                )
            }

            GitError::BranchAlreadyExists { branch } => {
                let switch_cmd = suggest_command("switch", &[branch], &[]);
                write!(
                    f,
                    "{}\n{}",
                    error_message(cformat!("Branch <bold>{branch}</> already exists")),
                    hint_message(cformat!(
                        "To switch to the existing branch, remove <bright-black>--create</>; run <bright-black>{switch_cmd}</>"
                    ))
                )
            }

            GitError::InvalidReference { reference } => {
                let create_cmd = suggest_command("switch", &[reference], &["--create"]);
                let list_cmd = suggest_command("list", &[], &["--branches", "--remotes"]);
                write!(
                    f,
                    "{}\n{}",
                    error_message(cformat!("Branch <bold>{reference}</> not found")),
                    hint_message(cformat!(
                        "To create a new branch, run <bright-black>{create_cmd}</>; to list branches, run <bright-black>{list_cmd}</>"
                    ))
                )
            }

            GitError::NotInWorktree { action } => {
                let message = match action {
                    Some(action) => format!("Cannot {action}: not in a worktree"),
                    None => "Not in a worktree".to_string(),
                };
                write!(
                    f,
                    "{}\n{}",
                    error_message(&message),
                    hint_message(cformat!(
                        "Run this command from inside a worktree, or specify a branch name"
                    ))
                )
            }

            GitError::WorktreeMissing { branch } => {
                write!(
                    f,
                    "{}\n{}",
                    error_message(cformat!("Worktree directory missing for <bold>{branch}</>")),
                    hint_message(cformat!(
                        "To clean up, run <bright-black>git worktree prune</>"
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
                let cmd = suggest_command("switch", &[branch], &[]);
                cwrite!(
                    f,
                    "{ERROR_SYMBOL} <red>Branch <bold>{branch}</> exists only on remote ({remote}/{branch})</>\n{HINT_SYMBOL} <dim>To create a local worktree, run <bright-black>{cmd}</></>"
                )
            }

            GitError::WorktreePathOccupied {
                branch,
                path,
                occupant,
            } => {
                let path_display = format_path_for_display(path);
                let reason = if let Some(occupant_branch) = occupant {
                    cformat!(
                        "there's a worktree at the expected path <bold>{path_display}</> on branch <bold>{occupant_branch}</>"
                    )
                } else {
                    cformat!(
                        "there's a detached worktree at the expected path <bold>{path_display}</>"
                    )
                };
                // Use actual path for command (not display path with ~, which won't expand in single quotes)
                let path_str = path.to_string_lossy();
                let path_escaped = escape(Cow::Borrowed(path_str.as_ref()));
                let command = format!("cd {path_escaped} && git switch {branch}");
                write!(
                    f,
                    "{}\n{}",
                    error_message(cformat!("Cannot switch to <bold>{branch}</> — {reason}")),
                    hint_message(cformat!(
                        "To switch the worktree at <bright-black>{path_display}</> to <bright-black>{branch}</>, run <bright-black>{command}</>"
                    ))
                )
            }

            GitError::WorktreePathExists {
                branch,
                path,
                create,
            } => {
                let path_display = format_path_for_display(path);
                let path_str = path.to_string_lossy();
                let path_escaped = escape(Cow::Borrowed(path_str.as_ref()));
                let flags: &[&str] = if *create {
                    &["--create", "--clobber"]
                } else {
                    &["--clobber"]
                };
                let switch_cmd = suggest_command("switch", &[branch], flags);
                write!(
                    f,
                    "{}\n{}",
                    error_message(cformat!(
                        "Directory already exists: <bold>{path_display}</>"
                    )),
                    hint_message(cformat!(
                        "To remove manually, run <bright-black>rm -rf {path_escaped}</>; to overwrite (with backup), run <bright-black>{switch_cmd}</>"
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

            GitError::WorktreeLocked {
                branch,
                path,
                reason,
            } => {
                let reason_text = match reason {
                    Some(r) if !r.is_empty() => format!(" ({r})"),
                    _ => String::new(),
                };
                let path_display = format_path_for_display(path);
                write!(
                    f,
                    "{}\n{}",
                    error_message(cformat!(
                        "Cannot remove <bold>{branch}</>, worktree is locked{reason_text}"
                    )),
                    hint_message(cformat!(
                        "To unlock, run <bright-black>git worktree unlock {path_display}</>"
                    ))
                )
            }

            GitError::ConflictingChanges {
                target_branch,
                files,
                worktree_path,
            } => {
                write!(
                    f,
                    "{}",
                    error_message(cformat!(
                        "Can't push to local <bold>{target_branch}</> branch: conflicting uncommitted changes"
                    ))
                )?;
                if !files.is_empty() {
                    let joined_files = files.join("\n");
                    write!(f, "\n{}\n", format_with_gutter(&joined_files, None))?;
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
                    write!(f, "\n{}\n", format_with_gutter(commits_formatted, None))?;
                }
                // Context-appropriate hint
                let merge_cmd = suggest_command("merge", &[target_branch], &[]);
                if *in_merge_context {
                    write!(
                        f,
                        "\n{}",
                        hint_message(cformat!(
                            "To incorporate these changes, run <bright-black>{merge_cmd}</> again"
                        ))
                    )
                } else {
                    let rebase_cmd = suggest_command("step", &["rebase", target_branch], &[]);
                    write!(
                        f,
                        "\n{}",
                        hint_message(cformat!(
                            "To rebase onto <bold>{target_branch}</>, run <bright-black>{rebase_cmd}</>"
                        ))
                    )
                }
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
                    write!(f, "\n{}", format_with_gutter(git_output, None))
                } else {
                    write!(
                        f,
                        "\n{}\n{}",
                        hint_message(cformat!(
                            "To continue after resolving conflicts, run <bright-black>git rebase --continue</>"
                        )),
                        hint_message(cformat!(
                            "To abort, run <bright-black>git rebase --abort</>"
                        ))
                    )
                }
            }

            GitError::NotRebased { target_branch } => {
                let rebase_cmd = suggest_command("step", &["rebase", target_branch], &[]);
                write!(
                    f,
                    "{}\n{}",
                    error_message(cformat!("Branch not rebased onto <bold>{target_branch}</>")),
                    hint_message(cformat!(
                        "Remove <bright-black>--no-rebase</>; or to rebase first, run <bright-black>{rebase_cmd}</>"
                    ))
                )
            }

            GitError::PushFailed {
                target_branch,
                error,
            } => {
                let header = error_message(cformat!(
                    "Can't push to local <bold>{target_branch}</> branch"
                ));
                write!(f, "{}", format_error_block(header, error))
            }

            GitError::NotInteractive => {
                let approvals_cmd = suggest_command("hook", &["approvals", "add"], &[]);
                write!(
                    f,
                    "{}\n{}",
                    error_message("Cannot prompt for approval in non-interactive environment"),
                    hint_message(cformat!(
                        "To skip prompts in CI/CD, add <bright-black>--yes</>; to pre-approve commands, run <bright-black>{approvals_cmd}</>"
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
                let command_gutter = format_with_gutter(display_command, None);
                write!(
                    f,
                    "{}\n{}\n{}",
                    error_block,
                    info_message("Ran command:"),
                    command_gutter
                )
            }

            GitError::ProjectConfigNotFound { config_path } => {
                let path_display = format_path_for_display(config_path);
                write!(
                    f,
                    "{}\n{}",
                    error_message("No project configuration found"),
                    hint_message(cformat!("Create a config file at: <bold>{path_display}</>"))
                )
            }

            GitError::ParseError { message } => {
                write!(f, "{}", error_message(message))
            }

            GitError::WorktreeIncludeParseError { error } => {
                let header = error_message(cformat!("Error parsing <bold>.worktreeinclude</>"));
                write!(f, "{}", format_error_block(header, error))
            }

            GitError::WorktreeNotFound { branch } => {
                write!(
                    f,
                    "{}",
                    error_message(cformat!("No worktree found for branch <bold>{branch}</>"))
                )
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
        // Can't derive command from hook type (e.g., PreRemove is used by both `wt remove` and `wt merge`)
        write!(
            f,
            "\n{}",
            hint_message(cformat!(
                "To skip {} hooks, re-run with <bright-black>--no-verify</>",
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
fn format_error_block(header: impl Into<String>, error: &str) -> String {
    let header = header.into();
    let trimmed = error.trim();
    if trimmed.is_empty() {
        header
    } else {
        format!("{header}\n{}", format_with_gutter(trimmed, None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;

    #[test]
    fn snapshot_into_preserves_type_for_display() {
        // .into() preserves type so we can downcast and use Display
        let err: anyhow::Error = GitError::BranchAlreadyExists {
            branch: "main".into(),
        }
        .into();

        let downcast = err.downcast_ref::<GitError>().expect("Should downcast");
        assert_snapshot!(downcast.to_string(), @"
        [31m✗[39m [31mBranch [1mmain[22m already exists[39m
        [2m↳[22m [2mTo switch to the existing branch, remove [90m--create[39m; run [90mwt switch main[39m[22m
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
    fn snapshot_worktree_error_with_path_and_create() {
        let err = GitError::WorktreePathExists {
            branch: "feature".to_string(),
            path: PathBuf::from("/some/path"),
            create: true,
        };
        assert_snapshot!(err.to_string(), @"
        [31m✗[39m [31mDirectory already exists: [1m/some/path[22m[39m
        [2m↳[22m [2mTo remove manually, run [90mrm -rf /some/path[39m; to overwrite (with backup), run [90mwt switch feature --create --clobber[39m[22m
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
    fn test_git_error_not_in_worktree() {
        // With action
        let err = GitError::NotInWorktree {
            action: Some("resolve '@'".into()),
        };
        let display = err.to_string();
        assert!(display.contains("Cannot resolve '@'"));
        assert!(display.contains("not in a worktree"));
        assert!(display.contains("specify a branch name"));

        // Without action
        let err = GitError::NotInWorktree { action: None };
        let display = err.to_string();
        assert!(display.contains("Not in a worktree"));
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
        assert!(display.contains("Cannot switch to"));
        assert!(display.contains("feature"));
        assert!(display.contains("there's a worktree at the expected path"));
        assert!(display.contains("on branch"));
        assert!(display.contains("main"));
        assert!(display.contains("To switch the worktree at"));
        assert!(display.contains(", run ")); // ANSI codes follow, then command
        assert!(display.contains("cd /tmp/repo && git switch feature"));

        // Without occupant (detached)
        let err = GitError::WorktreePathOccupied {
            branch: "feature".into(),
            path: PathBuf::from("/tmp/repo"),
            occupant: None,
        };
        let display = err.to_string();
        assert!(display.contains("detached worktree"));
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
    fn test_git_error_worktree_locked_with_reason() {
        let err = GitError::WorktreeLocked {
            branch: "feature".into(),
            path: PathBuf::from("/tmp/repo.feature"),
            reason: Some("Testing lock".into()),
        };
        let display = err.to_string();
        assert!(display.contains("Cannot remove"));
        assert!(display.contains("feature"));
        assert!(display.contains(", worktree is locked"));
        assert!(display.contains("(Testing lock)"));
        assert!(display.contains("git worktree unlock /tmp/repo.feature"));
    }

    #[test]
    fn test_git_error_worktree_locked_no_reason() {
        // When git outputs "locked" without a reason, we get Some("")
        let err = GitError::WorktreeLocked {
            branch: "feature".into(),
            path: PathBuf::from("/tmp/repo.feature"),
            reason: Some("".into()),
        };
        let display = err.to_string();
        assert!(display.contains("Cannot remove"));
        assert!(display.contains("feature"));
        assert!(display.contains(", worktree is locked"));
        assert!(
            !display.contains("locked ("),
            "should not show parentheses without reason"
        );
        assert!(display.contains("git worktree unlock /tmp/repo.feature"));
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
    fn test_git_error_uncommitted_changes_variants() {
        // Action only
        let err = GitError::UncommittedChanges {
            action: Some("push".into()),
            branch: None,
        };
        let display = err.to_string();
        assert!(display.contains("Cannot push"));
        assert!(display.contains("working tree"));

        // Branch only
        let err = GitError::UncommittedChanges {
            action: None,
            branch: Some("feature".into()),
        };
        let display = err.to_string();
        assert!(display.contains("feature"));
        assert!(display.contains("uncommitted"));

        // Neither
        let err = GitError::UncommittedChanges {
            action: None,
            branch: None,
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
            target_branch: "main".into(),
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
