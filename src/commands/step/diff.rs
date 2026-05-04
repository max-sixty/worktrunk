//! `wt step diff` — show all changes since branching from the target.

use anyhow::Context;
use worktrunk::git::Repository;
use worktrunk::shell_exec::Cmd;

/// Handle `wt step diff` command
///
/// Shows all changes since branching from the target: committed, staged, unstaged,
/// and untracked files in a single diff. Copies the real index to preserve git's stat
/// cache (avoiding re-reads of unchanged files), then registers untracked files with
/// `git add -N` so they appear in the diff.
pub fn step_diff(target: Option<&str>, extra_args: &[String]) -> anyhow::Result<()> {
    let repo = Repository::current()?;
    let wt = repo.current_worktree();

    // Get and validate target ref
    let integration_target = repo.require_target_ref(target)?;

    // Get merge base
    let merge_base = repo
        .merge_base("HEAD", &integration_target)?
        .context("No common ancestor with target branch")?;

    let current_branch = wt.branch()?.unwrap_or_else(|| "HEAD".to_string());

    // Copy the real index so git's stat cache is warm for tracked files, then
    // register untracked files with `git add -N .` so they appear in the diff.
    // This avoids re-reading and hashing every tracked file during `git diff`.
    let worktree_root = wt.root()?;

    let real_index = wt.git_dir()?.join("index");
    let temp_index = tempfile::NamedTempFile::new().context("Failed to create temporary index")?;
    let temp_index_path = temp_index
        .path()
        .to_str()
        .context("Temporary index path is not valid UTF-8")?;

    std::fs::copy(&real_index, temp_index.path()).context("Failed to copy index file")?;

    // Register untracked files as intent-to-add (tracked files already have entries)
    Cmd::new("git")
        .args(["add", "--intent-to-add", "."])
        .current_dir(&worktree_root)
        .context(&current_branch)
        .env("GIT_INDEX_FILE", temp_index_path)
        .run()
        .context("Failed to register untracked files")?;

    // Stream diff to stdout — git handles pager and coloring
    let mut diff_args = vec!["diff".to_string(), merge_base];
    diff_args.extend_from_slice(extra_args);
    Cmd::new("git")
        .args(&diff_args)
        .current_dir(&worktree_root)
        .context(&current_branch)
        .env("GIT_INDEX_FILE", temp_index_path)
        .stream()?;

    Ok(())
}
