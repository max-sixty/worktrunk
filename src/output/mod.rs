//! Output and presentation layer for worktree commands.
//!
//! # Architecture
//!
//! Global context-based output system similar to logging frameworks (`log`, `tracing`).
//! Initialize once at program start with `initialize(OutputMode)`, then use
//! output functions anywhere: `print()`, `change_directory()`, `execute()`, etc.
//!
//! ## Design
//!
//! **Global state** with `OnceLock`:
//!
//! ```rust,ignore
//! static GLOBAL_MODE: OnceLock<OutputMode> = OnceLock::new();
//! static OUTPUT_STATE: OnceLock<Mutex<OutputState>> = OnceLock::new();
//! ```
//!
//! The mode is set once at initialization and readable by all threads.
//! State (target_dir, exec_command) is stored globally for main thread operations.
//!
//! All output functions check the mode directly - no handler structs or traits.
//!
//! ## Usage Pattern
//!
//! ```rust,ignore
//! use worktrunk::styling::{success_message, error_message, hint_message};
//!
//! // 1. Initialize once in main()
//! let mode = if internal {
//!     OutputMode::Directive(shell)
//! } else {
//!     OutputMode::Interactive
//! };
//! output::initialize(mode);
//!
//! // 2. Use anywhere in the codebase
//! output::print(success_message("Operation complete"));
//! output::change_directory(&path);
//! output::execute("git pull");
//! output::flush();
//! ```
//!
//! ## Output Modes
//!
//! - **Interactive**: Colors, emojis, shell hints, direct command execution
//!   - Messages to stderr, data (JSON) to stdout for piping
//! - **Directive**: Shell script protocol for shell integration
//!   - All messages to stderr, stdout reserved for shell script at end
//!   - stdout: Shell script emitted at end (e.g., `cd '/path'`)
//!   - stderr: Success messages, progress updates, warnings (streams in real-time)

mod global;
pub mod handlers;

// Re-export the public API
pub use global::{
    OutputMode, blank, change_directory, data, data_raw, execute, flush, flush_for_stderr_prompt,
    gutter, initialize, print, shell_integration_hint, table, terminate_output,
};
// Re-export output handlers
pub use handlers::{
    execute_command_in_worktree, execute_user_command, handle_remove_output, handle_switch_output,
};
