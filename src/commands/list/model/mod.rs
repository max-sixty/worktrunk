//! Data models for the list command.
//!
//! This module contains the data structures used by `wt list` to represent
//! worktrees, branches, and their various states.
//!
//! # Module Organization
//!
//! - [`state`] - State enums for worktree and branch status (Divergence, MainState, etc.)
//! - [`stats`] - Statistics types (AheadBehind, BranchDiffTotals, UpstreamStatus)
//! - [`status_symbols`] - Status symbol rendering (StatusSymbols, PositionMask)
//! - [`item`] - Core list item types (ListItem, WorktreeData, DisplayFields)
//! - [`statusline_segment`] - Statusline output with smart truncation

pub mod item;
pub mod state;
pub mod stats;
pub mod status_symbols;
pub mod statusline_segment;

// Re-export public types at the module level for convenience.
// These re-exports are used by sibling modules (e.g., json_output.rs, render.rs)
// via `crate::commands::list::model::...` paths. The allow is needed because
// rustc doesn't track re-export usage across module boundaries.
#[allow(unused_imports)]
pub use item::{DisplayFields, ItemKind, ListData, ListItem, WorktreeData};
#[allow(unused_imports)]
pub use state::{ActiveGitOperation, Divergence, MainState, OperationState, WorktreeState};
#[allow(unused_imports)]
pub use stats::{ActiveUpstream, AheadBehind, BranchDiffTotals, CommitDetails, UpstreamStatus};
#[allow(unused_imports)]
pub use status_symbols::{PositionMask, StatusSymbols, WorkingTreeStatus};
#[allow(unused_imports)]
pub use statusline_segment::StatuslineSegment;
