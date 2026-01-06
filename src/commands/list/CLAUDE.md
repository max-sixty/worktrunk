# List Command Architecture

## Skeleton-First Rendering

The `wt list` command uses skeleton-first rendering: a placeholder table appears
immediately (~50ms), then cells fill in as data arrives. This gives users
instant feedback even when git operations are slow.

**The skeleton must render as fast as possible.** Every operation before the
skeleton adds perceived latency. Users notice 50ms vs 150ms.

## Rendering Phases

### Phase 1: Pre-Skeleton (KEEP THIS MINIMAL)

Operations that MUST complete before showing anything:

1. `git worktree list` — enumerate worktrees
2. `git config worktrunk.default-branch` — identify main branch for sorting
3. `git show -s --format='%H %ct'` — batch timestamp fetch for sorting
4. `git rev-parse --is-bare-repository` — layout decision (show Path column?)
5. Path canonicalization — detect current worktree (no git command)
6. Project config check — read `.config/wt.toml` to check if URL column needed
7. Layout calculation — column widths from branch/path lengths (no git command)

With `--branches`/`--remotes`:
- `git for-each-ref refs/heads` / `refs/remotes`

**DO NOT add operations here without very good reason.** Template expansion,
per-item computations, network I/O — all must wait until after skeleton.

**TODO: Phase 0 optimization** — Consider a "Phase 0" that renders just the first
few columns (Branch, Status) without computing widths for all columns. This would
show *something* even faster, then expand the table as more column widths are
computed. Trade-off: table width might shift as columns are added.

### Phase 2: Skeleton Render

The skeleton shows:
- Branch names (known from worktree list)
- Paths (known from worktree list)
- Placeholder gutter symbols (`·`)
- Loading indicators for computed columns

### Phase 3: Post-Skeleton

Everything else runs after the skeleton appears:

- Previous branch lookup (`get_switch_previous`)
- Integration target calculation
- URL template expansion (parallelized in task spawning)
- All background tasks (status, diffs, CI, URL health checks)

These operations update cells progressively as they complete.

**URL column example:** The skeleton allocates space for the URL column using a
fast heuristic (checks for `hash_port` in template, no expansion). When hyperlinks
are supported, the display is `:PORT` (6 chars) instead of the full URL. Template
expansion happens in task spawning (Phase 3), parallelized across worktrees.
Two-phase update:

1. URL appears immediately in normal styling (sent before health check task)
2. If health check fails (port not listening), URL dims

This ensures URLs appear as fast as possible — users see the URL right away,
then it dims only if the dev server isn't running.

## Adding New Features

When adding a new column or feature, ask:

1. **Does it need data before skeleton?** Usually no. The skeleton can show a
   placeholder or omit the column until data arrives.

2. **Can template expansion wait?** Yes. Expand templates post-skeleton, then
   update the relevant cells.

3. **Does it require file I/O?** If so, it belongs post-skeleton. Reading config
   files, checking file existence, etc. all add latency.

**Default answer: defer to post-skeleton.** Only add pre-skeleton operations
when the skeleton literally cannot render without the data (e.g., we need branch
names to show anything useful).

## Benchmarking Skeleton Time

```bash
WORKTRUNK_SKELETON_ONLY=1 hyperfine 'wt list'
```

This exits immediately after rendering the skeleton, measuring pure skeleton
latency. Target: <60ms.

## Code Structure

- `collect.rs` — orchestrates collection, manages pre/post-skeleton phases
- `collect_progressive_impl.rs` — background task definitions and execution
- `render.rs` — row formatting, skeleton rows, cell rendering
- `layout.rs` — column width calculation
- `progressive_table.rs` — terminal rendering with in-place updates
