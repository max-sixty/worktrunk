# Repository Caching Strategy

## What Changes During Execution?

Most data is stable for the duration of a command. The only things worktrunk modifies are:

- **Worktree list** — `wt switch --create`, `wt remove` create/remove worktrees
- **Working tree state** — `wt merge` commits, stages files
- **Git config** — `wt config` modifies settings

Everything else (remote URLs, project config, branch metadata) is read-only.

## Caching Implementation

`Repository` uses a global cache keyed by `git_common_dir`. The cache is
shared across all `Repository` instances within a process.

**Architecture note:** Using a HashMap keyed by `git_common_dir` is slightly
wasteful since every `Repository` instance must compute `git rev-parse
--git-common-dir` to look up the cache. However, tests require isolation (each
test creates different repos that need their own cached values), and the
HashMap approach is the simplest way to achieve this. Alternative approaches
(test-only cache clearing with RwLock instead of OnceCell) have their own
trade-offs.

**Currently cached:**
- `git_common_dir()` — cached per-instance (also used as HashMap key)
- `worktree_root()` — per-worktree, keyed by path
- `worktree_base()` — derived from git_common_dir and is_bare
- `is_bare()` — git config, doesn't change
- `current_branch()` — per-worktree, keyed by path
- `project_identifier()` — derived from remote URL
- `primary_remote()` — git config, doesn't change
- `default_branch()` — from git config or detection, doesn't change
- `merge_base()` — keyed by (commit1, commit2) pair
- `ahead_behind` — keyed by (base_ref, branch_name), populated by `batch_ahead_behind()`

**Not cached (intentionally):**
- `is_dirty()` — changes as we stage/commit
- `list_worktrees()` — changes as we create/remove worktrees

**Adding new cached methods:**

1. Add field to `RepoCache` struct: `field_name: OnceCell<T>`
2. Use `with_cache()` helper to access the shared cache
3. Return owned values (String, PathBuf, bool)

```rust
// For repo-wide values (same for all worktrees)
pub fn cached_value(&self) -> anyhow::Result<String> {
    self.with_cache(|cache| {
        cache
            .field_name
            .get_or_init(|| { /* compute value */ })
            .clone()
    })
}

// For per-worktree values (different per worktree path)
pub fn cached_per_worktree(&self) -> anyhow::Result<String> {
    let worktree_key = self.path.clone();

    self.with_cache(|cache| {
        // Check cache first
        if let Ok(map) = cache.field_name.read()
            && let Some(cached) = map.get(&worktree_key)
        {
            return cached.clone();
        }

        // Cache miss - compute and store
        let result = /* compute value */;
        if let Ok(mut map) = cache.field_name.write() {
            map.insert(worktree_key, result.clone());
        }
        result
    })
}
```
