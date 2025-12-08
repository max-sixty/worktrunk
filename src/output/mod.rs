//! Output and presentation layer for worktree commands.
//!
//! # Architecture
//!
//! Global context-based output system similar to logging frameworks (`log`, `tracing`).
//! Initialize once at program start with `initialize(OutputMode)`, then use
//! output functions anywhere: `success()`, `change_directory()`, `execute()`, etc.
//!
//! ## Design
//!
//! **Thread-local storage** stores the output handler globally:
//!
//! ```rust,ignore
//! thread_local! {
//!     static OUTPUT_CONTEXT: RefCell<OutputHandler> = ...;
//! }
//! ```
//!
//! Each thread gets its own output context. `RefCell` provides interior mutability
//! for mutation through shared references (runtime borrow checking).
//!
//! **Enum dispatch** routes calls to the appropriate handler:
//!
//! ```rust,ignore
//! enum OutputHandler {
//!     Interactive(InteractiveOutput),  // Human-friendly with colors
//!     Directive(DirectiveOutput),      // Machine-readable for shell integration
//! }
//! ```
//!
//! This enables static dispatch and compiler optimizations.
//!
//! ## Usage Pattern
//!
//! ```rust,ignore
//! use worktrunk::styling::{success_message, error_message, hint_message};
//!
//! // 1. Initialize once in main()
//! let mode = if internal {
//!     OutputMode::Directive
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
//! - **Directive**: Shell script on stdout (at end), user messages on stderr (streaming)
//!   - stdout: Shell script emitted at end (e.g., `cd '/path'`)
//!   - stderr: Success messages, progress updates, warnings (streams in real-time)

pub mod directive;
pub mod global;
pub mod handlers;
pub mod interactive;
mod traits;

// Re-export the public API
pub use global::{
    OutputMode, blank, change_directory, data, execute, flush, flush_for_stderr_prompt, gutter,
    initialize, print, shell_integration_hint, table, terminate_output,
};
// Re-export output handlers
pub use handlers::{
    execute_command_in_worktree, execute_user_command, handle_remove_output, handle_switch_output,
};
