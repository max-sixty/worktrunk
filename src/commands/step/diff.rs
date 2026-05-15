//! `wt step diff` — show all changes since branching from the target.

use anyhow::Context;
use worktrunk::git::Repository;

/// Handle `wt step diff` command
///
/// Shows all changes since branching from the target: committed, staged, unstaged,
/// and untracked files in a single diff. Stages untracked files into a temp index
/// (`WorkingTree::temp_index`) so they appear in the diff without mutating the
/// real index — git's stat cache stays warm and tracked files aren't re-hashed.
pub fn step_diff(target: Option<&str>, extra_args: &[String]) -> anyhow::Result<()> {
    let repo = Repository::current()?;
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
