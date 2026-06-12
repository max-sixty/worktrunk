//! `wt step diff` — show all changes since branching from the target.

use anyhow::Context;
use worktrunk::git::Repository;

/// Handle `wt step diff` command
///
/// Shows all changes since branching from the target: committed, staged, unstaged,
/// and untracked files in a single diff. Stages untracked files into a temp index
/// (`WorkingTree::temp_index`) so they appear in the diff without mutating the
/// real index — git's stat cache stays warm and tracked files aren't re-hashed.
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
        Some(b) => {
            let worktree_path =
                Repository::current()?
                    .worktree_for_branch(b)?
                    .ok_or_else(|| worktrunk::git::GitError::WorktreeNotFound {
                        branch: b.to_string(),
                    })?;
            Repository::at(&worktree_path)?
        }
        None => Repository::current()?,
    };
    let wt = repo.current_worktree();

    let integration_target = repo.require_target_ref(target)?;
    let merge_base = repo
        .merge_base("HEAD", &integration_target)?
        .context("No common ancestor with target branch")?;

    let idx = wt.temp_index()?;
    idx.git(["add", "--intent-to-add", "."])
        .run()
        .context("Failed to register untracked files")?;

    // Stream diff to stdout — git handles pager and coloring.
    let mut diff_args = vec!["diff".to_string(), merge_base];
    diff_args.extend_from_slice(extra_args);
    idx.git(diff_args).stream()?;

    Ok(())
}
