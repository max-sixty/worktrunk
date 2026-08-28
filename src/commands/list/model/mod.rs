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
//! - [`item`] - Core list item types (ListItem, WorktreeData, ItemKind)
//! - [`statusline_segment`] - Statusline output with smart truncation

pub mod item;
pub mod state;
pub mod stats;
pub mod status_symbols;
pub mod statusline_segment;

// Re-export public types at the module level for convenience. Sibling modules
// (e.g. json_output.rs, render.rs) reach them via
// `crate::commands::list::model::...`. Every name here has such a caller, so
// the set stays warning-clean on its own: a re-export that loses its last
// caller shows up as `unused_imports` rather than being carried indefinitely.
pub use item::{BranchScope, Collected, ItemKind, ListData, ListItem, WorktreeData};
pub use state::{Divergence, MainState, OperationState, WorktreeState};
pub use stats::{AheadBehind, BranchDiffTotals, CommitDetails, UpstreamStatus};
pub use status_symbols::{PositionMask, StatusSymbols, WorkingTreeStatus};
pub use statusline_segment::StatuslineSegment;
