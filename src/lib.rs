//! Jujutsu (jj) workspace management for parallel workflows.
//!
//! Worktrunk is a CLI tool — see <https://worktrunk.dev> for documentation
//! and the [README](https://github.com/max-sixty/worktrunk) for an overview.
//!
//! This version has been converted to use Jujutsu (jj) workspaces instead of
//! git worktrees. The core concepts are similar:
//! - git worktree → jj workspace
//! - git branch → jj bookmark
//!
//! The library API is not stable. If you're building tooling that integrates
//! with worktrunk, please [open an issue](https://github.com/max-sixty/worktrunk/issues)
//! to discuss your use case.

pub mod config;
pub mod git; // Keep git module for reference/migration
pub mod jj;  // New jj module
pub mod path;
pub mod shell;
pub mod shell_exec;
pub mod styling;
pub mod sync;
pub mod trace;
pub mod utils;

// Re-export HookType from git module (for backward compatibility during transition)
// TODO: Migrate commands to use jj module, then switch to jj::HookType
pub use git::HookType;
