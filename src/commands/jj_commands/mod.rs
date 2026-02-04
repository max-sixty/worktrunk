//! Jujutsu (jj) workspace commands.
//!
//! This module provides simplified command implementations that use jj workspaces
//! instead of git worktrees. These commands are a parallel implementation that
//! will eventually replace the git-based commands.

mod list;
pub mod remove;
pub mod switch;

pub use list::handle_list_jj;
pub use remove::{RemoveOptions, handle_remove_jj};
pub use switch::{SwitchOptions, handle_switch_jj};
