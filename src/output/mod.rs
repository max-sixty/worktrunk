//! Output and presentation layer for worktree commands.
//!
//! # Architecture
//!
//! Global context-based output system similar to logging frameworks (`log`, `tracing`).
//! State is lazily initialized on first use — no explicit initialization required.
//!
//! ## Usage
//!
//! ```rust,ignore
//! use worktrunk::styling::{success_message, error_message, hint_message};
//!
//! output::print(success_message("Operation complete"));
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

mod global;
pub(crate) mod handlers;
pub(crate) mod shell_integration;

// Re-export the public API
// TODO(verbose-output): verbose_level, is_verbose reserved per commit e13776c
#[allow(unused_imports)]
pub(crate) use global::{
    blank, change_directory, execute, flush, is_shell_integration_active, is_verbose,
    post_hook_display_path, pre_hook_display_path, print, set_verbose_level, stdout,
    terminate_output, verbose_level,
};
// Re-export output handlers
pub(crate) use handlers::{
    execute_command_in_worktree, execute_user_command, handle_remove_output, handle_switch_output,
};
// Re-export shell integration functions
pub(crate) use shell_integration::{
    print_shell_install_result, print_skipped_shells, prompt_shell_integration,
};
