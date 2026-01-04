# Repository Caching Strategy

## What Changes During Execution?

Most data is stable for the duration of a command. The only things worktrunk modifies are:

- **Worktree list** — `wt switch --create`, `wt remove` create/remove worktrees
- **Working tree state** — `wt merge` commits, stages files
- **Git config** — `wt config` modifies settings

Everything else (remote URLs, project config, branch metadata) is read-only.

## Caching Implementation

`Repository` uses `OnceCell` for per-instance caching. Since instances are short-lived (command duration), we can cache aggressively:

**Currently cached:**
- `git_common_dir()` — never changes
- `worktree_root()` — never changes
- `worktree_base()` — derived from git_common_dir and is_bare
- `is_bare()` — git config, doesn't change
- `current_branch()` — we don't switch branches within a worktree
- `project_identifier()` — derived from remote URL
- `primary_remote()` — git config, doesn't change
- `default_branch()` — from git config or detection, doesn't change

**Not cached (intentionally):**
- `is_dirty()` — changes as we stage/commit
- `list_worktrees()` — changes as we create/remove worktrees

**Adding new cached methods:**

1. Add field to `RepoCache` struct: `field_name: OnceCell<T>`
2. Use `get_or_try_init()` pattern for fallible initialization
3. Return `&T` (for references) or `.copied()`/`.cloned()` (for values)

```rust
// Return reference (avoids allocation)
pub fn cached_ref(&self) -> anyhow::Result<&str> {
    self.cache
        .field_name
        .get_or_try_init(|| { /* compute String */ })
        .map(String::as_str)
}

// Return owned value (when caller needs ownership)
pub fn cached_value(&self) -> anyhow::Result<bool> {
    self.cache
        .field_name
        .get_or_try_init(|| { /* compute bool */ })
        .copied()
}
```
