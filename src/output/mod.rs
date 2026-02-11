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
//! When `WORKTRUNK_DIRECTIVE_FILE` env var is set (by shell wrapper):
//! - Shell commands (cd, exec) are written to that file
//! - Shell wrapper sources the file after wt exits
//! - This allows the parent shell to change directory
//!
//! When not set (direct binary call):
//! - Commands execute directly
//! - Shell hints are shown for missing integration
//!
//! See [`shell_integration`] module for the complete spec of warning messages.

pub(crate) mod commit_generation;
mod global;
pub(crate) mod handlers;
pub(crate) mod prompt;
pub(crate) mod shell_integration;

// Re-export the public API
pub(crate) use global::{
    change_directory, execute, is_shell_integration_active, post_hook_display_path,
    pre_hook_display_path, set_verbosity, terminate_output, to_logical_path,
};
// Re-export output handlers
pub(crate) use handlers::{
    execute_command_in_worktree, execute_user_command, handle_remove_output, handle_switch_output,
};
// Re-export shell integration functions
pub(crate) use shell_integration::{
    print_shell_install_result, print_skipped_shells, prompt_shell_integration,
};
// Re-export commit generation functions
pub(crate) use commit_generation::prompt_commit_generation;
