# Repository Caching Strategy

## What Changes During Execution?

Most data is stable for the duration of a command. The only things worktrunk modifies are:

- **Worktree list** — `wt switch --create`, `wt remove` create/remove worktrees
- **Working tree state** — `wt merge` commits, stages files
- **Git config** — `wt config` modifies settings

Everything else (remote URLs, project config, branch metadata) is read-only.

## Caching Implementation

`Repository` holds its cache directly via `Arc<RepoCache>`. Cloning a Repository
shares the cache — all clones see the same cached values.

**Key patterns:**

- **Command entry points** create Repository via `Repository::current()` or `Repository::at(path)`
- **Parallel tasks** (e.g., `wt list`) clone the Repository, sharing the cache
- **Tests** naturally get isolation since each test creates its own Repository

**Currently cached:**

- `git_common_dir` — computed at construction, stored on struct
- `worktree_root()` — per-worktree, keyed by path
- `worktree_base()` — derived from git_common_dir and is_bare
- `is_bare()` — git config, doesn't change
- `current_branch()` — per-worktree, keyed by path
- `project_identifier()` — derived from remote URL
- `primary_remote()` — git config, doesn't change
- `primary_remote_url()` — derived from primary_remote, doesn't change
- `default_branch()` — from git config or detection, doesn't change
- `integration_target()` — effective target for integration checks (local default or upstream if ahead)
- `merge_base()` — keyed by (commit1, commit2) pair
- `ahead_behind` — keyed by (base_ref, branch_name), populated by `batch_ahead_behind()`
- `project_config` — loaded from .config/wt.toml

**Not cached (intentionally):**

- `is_dirty()` — changes as we stage/commit
- `list_worktrees()` — changes as we create/remove worktrees

**Adding new cached methods:**

1. Add field to `RepoCache` struct: `field_name: OnceCell<T>`
2. Access via `self.cache.field_name`
3. Return owned values (String, PathBuf, bool)

```rust
// For repo-wide values (same for all clones)
pub fn cached_value(&self) -> anyhow::Result<String> {
    self.cache
        .field_name
        .get_or_init(|| { /* compute value */ })
        .clone()
}

// For per-worktree values (different per worktree path)
// Use DashMap for concurrent access
pub fn cached_per_worktree(&self, path: &Path) -> String {
    self.cache
        .field_name
        .entry(path.to_path_buf())
        .or_insert_with(|| { /* compute value */ })
        .clone()
}
```
