//! Step commands for the merge workflow and standalone worktree utilities.
//!
//! Merge steps:
//! - `commit::step_commit` - Commit working tree changes
//! - `squash::handle_squash` - Squash commits into one
//! - `squash::step_show_squash_prompt` - Show squash prompt without executing
//! - `rebase::handle_rebase` - Rebase onto target branch
//! - `diff::step_diff` - Show all changes since branching
//!
//! Standalone:
//! - `copy_ignored::step_copy_ignored` - Copy gitignored files matching .worktreeinclude
//! - `promote::handle_promote` - Swap a branch into the main worktree
//! - `prune::step_prune` - Remove worktrees merged into the default branch
//! - `relocate::step_relocate` - Move worktrees to expected paths

pub(crate) mod commit;
pub(crate) mod copy_ignored;
pub(crate) mod diff;
pub(crate) mod promote;
pub(crate) mod prune;
pub(crate) mod rebase;
pub(crate) mod relocate;
mod shared;
pub(crate) mod squash;

pub(crate) use commit::step_commit;
pub(crate) use copy_ignored::step_copy_ignored;
pub(crate) use diff::step_diff;
pub(crate) use promote::{PromoteResult, handle_promote};
pub(crate) use prune::step_prune;
pub(crate) use rebase::{RebaseResult, handle_rebase};
pub(crate) use relocate::step_relocate;
pub(crate) use squash::{
    SquashResult, handle_squash, step_dry_run_squash, step_show_squash_prompt,
};
