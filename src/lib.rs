//! Git worktree management for parallel workflows.
//!
//! Worktrunk is a CLI tool — see <https://worktrunk.dev> for documentation
//! and the [README](https://github.com/max-sixty/worktrunk) for an overview.
//!
//! The library API is not stable. If you're building tooling that integrates
//! with worktrunk, please [open an issue](https://github.com/max-sixty/worktrunk/issues)
//! to discuss your use case.

pub mod config;
pub mod git;
pub mod path;
pub mod shell;
pub mod shell_exec;
pub mod styling;
pub mod sync;
pub mod trace;
pub mod utils;

// Re-export HookType for convenience
pub use git::HookType;
