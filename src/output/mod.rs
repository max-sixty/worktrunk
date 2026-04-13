//! Output and presentation layer for worktree commands.
//!
//! # Architecture
//!
//! For regular output, use `eprintln!`/`println!` directly (from `worktrunk::styling`
//! for color support). This module handles shell integration directives (cd, exec)
//! that need to be communicated to the parent shell.
//!
//! ## Usage
//!
//! ```rust,ignore
//! use worktrunk::styling::{success_message, error_message, hint_message, eprintln};
//!
//! eprintln!("{}", success_message("Operation complete"));
//! output::change_directory(&path);
//! output::execute("git pull");
//! ```
//!
//! ## Shell Integration
//!
//! Two split directive files, one for each trust level:
//! - `WORKTRUNK_DIRECTIVE_CD_FILE` — raw path; the wrapper `cd`s to it.
//! - `WORKTRUNK_DIRECTIVE_EXEC_FILE` — arbitrary shell; the wrapper sources it.
//!
//! The legacy single-file `WORKTRUNK_DIRECTIVE_FILE` env var is still honored
//! for one release to bridge users who upgraded `wt` without restarting
//! their shell. See `global` for the `DirectiveMode` selection logic.
//!
//! When no directive env vars are set (direct binary call):
//! - Commands execute directly.
//! - Shell hints are shown for missing integration.
//!
//! See [`shell_integration`] module for the complete spec of warning messages.

pub(crate) mod commit_generation;
pub(crate) mod concurrent;
mod global;
pub(crate) mod handlers;
pub(crate) mod prompt;
pub(crate) mod shell_integration;

// Re-export the public API
pub(crate) use global::{
    change_directory, exec_would_be_refused, execute, is_shell_integration_active,
    mark_cwd_removed, post_hook_display_path, pre_hook_display_path, set_verbosity,
    terminate_output, to_logical_path, was_cwd_removed,
};
// Re-export output handlers
pub(crate) use handlers::{
    DirectivePassthrough, execute_shell_command, execute_user_command, handle_remove_output,
    handle_switch_output,
};
// Re-export shell integration functions
pub(crate) use shell_integration::{
    print_shell_install_result, print_shell_uninstall_result, print_skipped_shells,
    prompt_shell_integration,
};
// Re-export commit generation functions
pub(crate) use commit_generation::prompt_commit_generation;
