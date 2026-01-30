# Benchmark Guidelines

See `list.rs` header for the authoritative list of benchmark groups and run examples.

## Quick Start

```bash
# Fast iteration (skip slow benchmarks)
cargo bench --bench list -- --skip cold --skip real --skip divergent_branches

# Run specific group
cargo bench --bench list many_branches

# GH #461 scenario (200 branches on rust-lang/rust)
cargo bench --bench list real_repo_many_branches

# All benchmarks (~1 hour)
cargo bench --bench list
```

## Rust Repo Caching

Real repo benchmarks clone rust-lang/rust on first run (~2-5 minutes). The clone is cached in `target/bench-repos/` and reused. Corrupted caches are auto-recovered.

## Faster Iteration

**Skip slow benchmarks:**
```bash
cargo bench --bench list -- --skip cold --skip real
```

**Pattern matching:**
```bash
cargo bench --bench list scaling    # All scaling benchmarks
cargo bench --bench list -- --skip cold  # Warm cache only
```

## Expected Performance

**Modest repos** (500 commits, 100 files):
- Cold cache penalty: ~5-16% slower
- Scaling: Linear with worktree count

**Large repos** (rust-lang/rust):
- Cold cache penalty: ~4x slower for single worktree
- Scaling: Warm cache shows superlinear degradation, cold cache scales better

## Output Locations

- Results: `target/criterion/`
- Cached rust repo: `target/bench-repos/rust/`
- HTML reports: `target/criterion/*/report/index.html`

## Performance Investigation with wt-perf

Use `wt-perf` to set up benchmark repos and generate Chrome Trace Format for visualization.

### Setting up benchmark repos

```bash
# Set up a repo with 8 worktrees (persists at /tmp/wt-perf-typical-8)
cargo run -p wt-perf -- setup typical-8 --persist

# Available configs:
#   typical-N       - 500 commits, 100 files, N worktrees
#   branches-N      - N branches, 1 commit each
#   branches-N-M    - N branches, M commits each
#   divergent       - 200 branches × 20 commits (GH #461 scenario)
#   select-test     - Config for wt select testing

# Invalidate caches for cold run
cargo run -p wt-perf -- invalidate /tmp/wt-perf-typical-8/main
```

### Generating traces

```bash
# Generate trace.json for Perfetto/Chrome
RUST_LOG=debug wt list --branches 2>&1 | grep '\[wt-trace\]' | \
  cargo run -p wt-perf -- trace > trace.json

# Open in https://ui.perfetto.dev or chrome://tracing
```

### Querying with trace_processor

Install [trace_processor](https://perfetto.dev/docs/analysis/trace-processor) for SQL analysis:

```bash
curl -LO https://get.perfetto.dev/trace_processor && chmod +x trace_processor
```

Useful queries:

```bash
# Top 10 slowest commands
echo "SELECT name, ts/1e6 as start_ms, dur/1e6 as dur_ms FROM slice WHERE dur > 0 ORDER BY dur DESC LIMIT 10;" | trace_processor trace.json

# Milestone events (skeleton render, worker spawn, completion)
echo "SELECT name, ts/1e6 as ms FROM slice WHERE dur = 0 ORDER BY ts;" | trace_processor trace.json

# Task type breakdown
cat > /tmp/q.sql << 'EOF'
SELECT
  CASE WHEN name LIKE '%status%' THEN 'status'
       WHEN name LIKE '%rev-parse%tree%' THEN 'trees_match'
       WHEN name LIKE '%merge-tree%' THEN 'merge_tree'
       WHEN name LIKE '%is-ancestor%' THEN 'is_ancestor'
       WHEN name LIKE '%diff --name%' THEN 'file_changes'
       ELSE 'other' END as task_type,
  COUNT(*) as count,
  ROUND(SUM(dur)/1e6, 2) as total_ms
FROM slice WHERE dur > 0
GROUP BY task_type ORDER BY total_ms DESC;
EOF
trace_processor trace.json -q /tmp/q.sql

# Check parallel execution overlap between task types
cat > /tmp/q.sql << 'EOF'
WITH status_times AS (
  SELECT MIN(ts) as start_us, MAX(ts + dur) as end_us
  FROM slice WHERE name LIKE '%status%'
),
trees_times AS (
  SELECT MIN(ts) as start_us, MAX(ts + dur) as end_us
  FROM slice WHERE name LIKE '%rev-parse%tree%'
)
SELECT
  s.start_us/1e6 as status_start_ms, s.end_us/1e6 as status_end_ms,
  t.start_us/1e6 as trees_start_ms, t.end_us/1e6 as trees_end_ms,
  CASE WHEN s.end_us > t.start_us AND t.end_us > s.start_us THEN 'OVERLAP' ELSE 'SEQUENTIAL' END as parallel
FROM status_times s, trees_times t;
EOF
trace_processor trace.json -q /tmp/q.sql
```

### Generating traces from benchmark repos

```bash
# Trace on rust-lang/rust (must run benchmark first to clone)
RUST_LOG=debug cargo run --release -q -- -C target/bench-repos/rust list --branches 2>&1 | \
  grep '\[wt-trace\]' | cargo run -p wt-perf -- trace > rust-trace.json
```

## Key Performance Insights

**`git for-each-ref %(ahead-behind:BASE)` is O(commits), not O(refs)**

This command walks the commit graph to compute divergence. On rust-lang/rust:
- Takes ~2s regardless of how many refs are queried
- Only way to avoid it is to not enumerate branches at all

**Branch enumeration costs** (rust-lang/rust with 50 branches):
- No optimization: ~15-18s (expensive merge-base/merge-tree per branch)
- With skip_expensive_for_stale: ~2-3s (skips expensive ops for stale branches)
- Worktrees only: ~600ms (no branch enumeration)
