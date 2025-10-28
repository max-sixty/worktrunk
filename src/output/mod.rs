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
//! // 1. Initialize once in main()
//! let mode = if internal {
//!     OutputMode::Directive
//! } else {
//!     OutputMode::Interactive
//! };
//! output::initialize(mode);
//!
//! // 2. Use anywhere in the codebase
//! output::success("Operation complete");
//! output::change_directory(&path);
//! output::execute("git pull");
//! output::flush();
//! ```
//!
//! ## Output Modes
//!
//! - **Interactive**: Colors, emojis, shell hints, direct command execution
//! - **Directive**: Plain text with NUL-terminated directives for shell integration
//!   - `__WORKTRUNK_CD__<path>\0` - Change directory
//!   - `__WORKTRUNK_EXEC__<cmd>\0` - Execute command
//!   - `<message>\0` - Success message

pub mod directive;
pub mod global;
pub mod handlers;
pub mod interactive;

// Re-export the public API
pub use global::{
    OutputMode, change_directory, execute, flush, initialize, is_interactive, progress, success,
};

// Re-export output handlers
pub use handlers::{execute_command_in_worktree, handle_remove_output, handle_switch_output};
