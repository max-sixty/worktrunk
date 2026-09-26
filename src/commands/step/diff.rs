//! `wt step diff` — show all changes since branching from the target.

use anyhow::Context;
use worktrunk::git::Repository;

/// Handle `wt step diff` command
///
/// Shows all changes since branching from the target: committed, staged, unstaged,
/// and untracked files in a single diff. Stages untracked files into a temp index
/// (`WorkingTree::temp_index_with_untracked`) so they appear in the diff
/// without mutating the real index — git's stat cache stays warm and tracked
/// files aren't re-hashed.
///
/// The diff base is what `wt merge` would squash from: the merge base with the
/// target, or with the target's upstream when the branch was built on an
/// upstream the local target lags ([`Repository::span_upstream`]). Without that,
/// a stale local `main` would count every upstream commit the branch picked up
/// as the branch's own.
///
/// `branch` selects which worktree to diff: when `Some`, the repo is rooted at
/// that branch's worktree so the diff (and its target resolution) operate there
/// rather than on the current directory. The branch must have a checked-out
/// worktree.
pub fn step_diff(
    branch: Option<&str>,
    target: Option<&str>,
    extra_args: &[String],
) -> anyhow::Result<()> {
    let repo = match branch {
        Some(b) => Repository::at(&Repository::current()?.require_worktree(b)?)?,
        None => Repository::current()?,
    };
    let wt = repo.current_worktree();

    let integration_target = repo.require_target_ref(target)?;
    let span_target = repo
        .span_upstream(&integration_target)?
        .unwrap_or(integration_target);
    let merge_base = repo
        .merge_base("HEAD", &span_target)?
        .context("No common ancestor with target branch")?;

    // Stream diff to stdout — git handles pager and coloring.
    wt.temp_index_with_untracked()?
        .stream_diff(&merge_base, extra_args)?;

    Ok(())
}
