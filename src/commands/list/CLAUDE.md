# List Command Architecture

## Skeleton-First Rendering

The `wt list` command uses skeleton-first rendering: a placeholder table appears
immediately (~50ms), then cells fill in as data arrives. This gives users
instant feedback even when git operations are slow.

**The skeleton must render as fast as possible.** Every operation before the
skeleton adds perceived latency. Users notice 50ms vs 150ms.

## Rendering Phases

### Phase 1: Pre-Skeleton

Minimal operations before showing anything. Runs a **fixed number of git commands**
(O(1), not O(N) per worktree) through batching. See `collect/mod.rs` module
docstring for the exact command list and first-run behavior.

### Phase 2: Skeleton Render

The skeleton shows:
- Branch names (known from worktree list)
- Paths (known from worktree list)
- Placeholder gutter symbols (`·`)
- Loading indicators for computed columns

### Phase 3: Post-Skeleton

Everything else runs after the skeleton appears:
- Previous branch lookup, integration target calculation
- URL template expansion (parallelized)
- All background tasks (status, diffs, CI, URL health checks)

Results update cells progressively as they complete.

## Adding New Features

Default: defer to post-skeleton. Only add a pre-skeleton operation when the
skeleton literally cannot render without the data — column sizing is the one
current exception (the CI column reads its cached width hint pre-skeleton; see
the `collect/mod.rs` docstring inventory). Template expansion and other file
I/O wait; new columns can render a placeholder until data arrives.

## Benchmarking Skeleton Time

```bash
WORKTRUNK_SKELETON_ONLY=1 hyperfine 'wt list'
```

Measures pure skeleton latency. Target: <60ms.

## Code Structure

- `collect/` — orchestrates collection, manages pre/post-skeleton phases, task definitions and execution (see `collect/mod.rs` module docstring for phase details)
- `render.rs` — row formatting, skeleton rows, cell rendering
- `layout.rs` — column width calculation
- `progressive_table.rs` — terminal rendering with in-place updates
