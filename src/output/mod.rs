//! Output and presentation layer for worktree commands.
//!
//! # Architecture
//!
//! For regular output, use `eprintln!`/`println!` directly (from `worktrunk::styling`
//! for color support); [`print_json`] serializes a `--format=json` answer to stdout.
//! This module handles the cd directive that must be communicated to the
//! parent shell.
//!
//! ## Usage
//!
//! ```rust,ignore
//! use worktrunk::styling::{success_message, error_message, hint_message, eprintln};
//!
//! eprintln!("{}", success_message("Operation complete"));
//! output::change_directory(&path);
//! output::execute(vec!["git".into(), "pull".into()]);
//! ```
//!
//! ## Shell Integration
//!
//! Shell integration uses one directive file:
//! - `WORKTRUNK_DIRECTIVE_CD_FILE` — raw path; the wrapper `cd`s to it.
//!
//! `--execute` programs always run directly. Without the CD directive, shell
//! hints explain why the parent shell cannot follow the directory change.
//!
//! See [`shell_integration`] module for the complete spec of warning messages.

pub(crate) mod commit_generation;
pub(crate) mod concurrent;
mod global;
pub(crate) mod handlers;
mod json;
pub(crate) mod prompt;
pub(crate) mod shell_integration;

// Re-export the public API
pub(crate) use global::{
    change_directory, execute, is_shell_integration_active, mark_cwd_removed,
    post_hook_display_path, pre_hook_display_path, print_outdated_shell_wrapper_hint_once,
    retired_shell_wrapper_active, set_verbosity, terminate_output, to_logical_path,
    was_cwd_removed,
};
// Re-export output handlers
pub(crate) use handlers::{
    BackgroundFallbackMode, DirectivePassthrough, RemovalExecution, execute_shell_command,
    execute_user_command, handle_remove_output, handle_switch_output,
    retained_unmerged_branch_messages,
};
// Re-export shell integration functions
pub(crate) use shell_integration::{
    print_shell_install_result, print_shell_uninstall_result, print_skipped_shells,
    prompt_shell_integration,
};
// Re-export commit generation functions
pub(crate) use commit_generation::prompt_commit_generation;
// Re-export the JSON answer printer
pub(crate) use json::print_json;
