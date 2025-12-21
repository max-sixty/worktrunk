# Worktrunk demo

A powerful demo for worktrunk.

## Quick Start

1. Clone the repo
2. Run `wt list`
3. Switch worktrees with `wt switch`

## Commands

- `wt list` - Show worktrees
- `wt switch` - Switch worktree
- `wt merge` - Merge and cleanup

## API Reference

### Core Functions

#### `list_worktrees()`

Returns all worktrees in the repository.

```rust
pub fn list_worktrees(repo: &Repository) -> Result<Vec<Worktree>> {
    let worktrees = repo.worktrees()?;
    worktrees.iter().map(|name| {
        let wt = repo.find_worktree(name)?;
        Ok(Worktree::from_git(wt))
    }).collect()
}
```

#### `switch_worktree()`

Switches to the specified worktree.

```rust
pub fn switch_worktree(name: &str) -> Result<()> {
    let path = find_worktree_path(name)?;
    std::env::set_current_dir(path)?;
    Ok(())
}
```

#### `create_worktree()`

Creates a new worktree for the given branch.

```rust
pub fn create_worktree(branch: &str, base: &str) -> Result<PathBuf> {
    let repo = Repository::open_from_env()?;
    let path = generate_worktree_path(&repo, branch)?;
    repo.worktree(branch, &path, Some(&base))?;
    Ok(path)
}
```

#### `merge_worktree()`

Merges the current branch into main and cleans up.

```rust
pub fn merge_worktree(opts: MergeOptions) -> Result<()> {
    let branch = current_branch()?;
    rebase_onto_main(&branch)?;
    fast_forward_main(&branch)?;
    if !opts.keep_worktree {
        remove_worktree(&branch)?;
    }
    Ok(())
}
```

### Helper Functions

#### `find_worktree_path()`

Resolves a worktree name to its filesystem path.

#### `generate_worktree_path()`

Generates a path for a new worktree based on naming conventions.

#### `current_branch()`

Returns the name of the currently checked out branch.

#### `rebase_onto_main()`

Rebases the given branch onto the main branch.

#### `fast_forward_main()`

Fast-forwards main to include the rebased commits.

#### `remove_worktree()`

Removes a worktree and optionally deletes its branch.

## Error Handling

All functions return `Result<T>` with detailed error types:

- `WorktreeNotFound` - The specified worktree doesn't exist
- `BranchInUse` - The branch is checked out in another worktree
- `MergeConflict` - Conflicts detected during rebase
- `DirtyWorkingTree` - Uncommitted changes present

## Performance Notes

- Listing uses parallel git operations for speed
- Diff calculations are cached per-session
- Remote fetches happen in background threads
