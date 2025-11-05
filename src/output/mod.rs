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
    OutputMode, change_directory, execute, flush, gutter, hint, info, initialize, progress,
    success, terminate_output, warning,
};

// Re-export output handlers
pub use handlers::{
    execute_command_in_worktree, execute_user_command, handle_remove_output, handle_switch_output,
};

use std::path::Path;

/// Format a switch success message with mode-specific location phrase
///
/// The message format differs between interactive and directive modes:
/// - Interactive: "Created new worktree for {branch} from {base} at {path}"
/// - Directive: "Created new worktree for {branch} from {base}: {path}"
pub(crate) fn format_switch_success_message(
    branch: &str,
    path: &Path,
    created_branch: bool,
    base_branch: Option<&str>,
    use_past_tense: bool,
) -> String {
    use worktrunk::styling::{GREEN, SUCCESS_EMOJI};
    let green_bold = GREEN.bold();

    let action = if created_branch {
        "Created new worktree for"
    } else {
        "Switched to worktree for"
    };
    let base_suffix = base_branch
        .map(|b| format!(" from {green_bold}{b}{green_bold:#}{GREEN}"))
        .unwrap_or_default();
    let location = if use_past_tense { ":" } else { " at" };

    format!(
        "{SUCCESS_EMOJI} {GREEN}{action} {green_bold}{branch}{green_bold:#}{GREEN}{base_suffix}{location} {green_bold}{}{green_bold:#}{GREEN:#}",
        path.display()
    )
}
