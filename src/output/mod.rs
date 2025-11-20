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
//! - **Directive**: Machine-readable directives on stdout, user messages on stderr
//!   - stdout (parsed by shell): `__WORKTRUNK_CD__<path>\0`, `__WORKTRUNK_EXEC__<cmd>\0`
//!   - stderr (streams to terminal): Success messages, progress updates, warnings

pub mod directive;
pub mod global;
pub mod handlers;
pub mod interactive;

// Re-export the public API
pub use global::{
    OutputMode, change_directory, error, execute, flush, flush_for_stderr_prompt, gutter, hint,
    info, initialize, progress, raw, raw_terminal, shell_integration_hint, success,
    terminate_output, warning,
};

// blank_line is only used by select command which is Unix-only
#[cfg(unix)]
pub use global::blank_line;

// Re-export output handlers
pub use handlers::{
    execute_command_in_worktree, execute_user_command, handle_remove_output, handle_switch_output,
};

use std::path::Path;
use worktrunk::path::format_path_for_display;

/// Format a switch success message with a consistent location phrase
///
/// Both interactive and directive modes now use the human-friendly
/// `"Created new worktree for {branch} from {base} at {path}"` wording so
/// users see the same message regardless of how worktrunk is invoked.
pub(crate) fn format_switch_success_message(
    branch: &str,
    path: &Path,
    created_branch: bool,
    base_branch: Option<&str>,
) -> String {
    use worktrunk::styling::{GREEN, GREEN_BOLD};

    let action = if created_branch {
        "Created new worktree for"
    } else {
        "Switched to worktree for"
    };

    // Re-establish GREEN after each green_bold reset to prevent color leak
    match base_branch {
        Some(base) => format!(
            "{GREEN}{action} {GREEN_BOLD}{branch}{GREEN_BOLD:#}{GREEN} from {GREEN_BOLD}{base}{GREEN_BOLD:#}{GREEN} at {GREEN_BOLD}{}{GREEN_BOLD:#}{GREEN:#}",
            format_path_for_display(path)
        ),
        None => format!(
            "{GREEN}{action} {GREEN_BOLD}{branch}{GREEN_BOLD:#}{GREEN} at {GREEN_BOLD}{}{GREEN_BOLD:#}{GREEN:#}",
            format_path_for_display(path)
        ),
    }
}
