# Benchmark Guidelines

See `list.rs` and `time_to_first_output.rs` headers for benchmark groups and run examples.

## Quick Start

```bash
# Fast iteration (skip slow benchmarks)
cargo bench --bench list -- --skip cold --skip real --skip divergent_branches

# Run specific group
cargo bench --bench list many_branches

# GH #461 scenario (200 branches on rust-lang/rust)
cargo bench --bench list real_repo_many_branches

# All list benchmarks (~1 hour)
cargo bench --bench list

# Time-to-first-output benchmarks
cargo bench --bench time_to_first_output            # all commands
cargo bench --bench time_to_first_output -- remove  # just remove
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

## WORKTRUNK_FIRST_OUTPUT

Setting `WORKTRUNK_FIRST_OUTPUT=1` causes commands to exit at the point where first
user-visible output would appear. Used by `time_to_first_output` benchmarks to measure
startup latency without output rendering or post-output work (mismatch warnings, hooks).

Supported commands: `switch`, `remove`, `list`.

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
#   picker-test     - Config for wt switch interactive picker testing

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

### Performance questions

Three questions drive `wt list` performance work:

1. **Where does time go?** Which subprocess types consume the most total time? The category with the highest `total_ms` is where optimization effort has the most impact.

2. **How parallel are we?** Total subprocess time divided by wall time gives a parallelism factor. A factor of 4.0 means 4 commands running concurrently on average. Close to 1.0 means mostly serial execution with headroom to parallelize.

3. **What's on the critical path?** The critical path passes through serial phases (setup, finalization) plus the slowest work item in the parallel phase. We don't have good queries for this yet — the trace format doesn't capture task dependencies, and rayon's work-stealing means thread IDs don't map to worktrees. The queries below are a starting point (phase boundaries from milestones, per-worktree time from args) but don't give a real critical path answer. Visualizing the trace in Perfetto is more useful here.

### Queries

```bash
# 1. Where does time go? — slowest individual commands
echo "SELECT name, ts/1e6 as start_ms, dur/1e6 as dur_ms FROM slice WHERE dur > 0 ORDER BY dur DESC LIMIT 10;" | trace_processor trace.json

# 1. Where does time go? — total time by command type
cat > /tmp/q.sql << 'EOF'
SELECT
  CASE WHEN name LIKE '%patch-id%' THEN 'patch_id'
       WHEN name LIKE '%diff-tree%' THEN 'diff_tree'
       WHEN name LIKE '%log -p%' THEN 'log_patches'
       WHEN name LIKE '%merge-tree%' THEN 'merge_tree'
       WHEN name LIKE '%is-ancestor%' THEN 'is_ancestor'
       WHEN name LIKE '%diff --name%' THEN 'file_changes'
       WHEN name LIKE '%diff --numstat%' THEN 'diff_numstat'
       WHEN name LIKE '%diff --cached%' THEN 'diff_cached'
       WHEN name LIKE '% diff main...%' THEN 'diff_3dot'
       WHEN name LIKE '% diff HEAD%' THEN 'diff_wt'
       WHEN name LIKE '%rev-parse%{tree}%' THEN 'trees_match'
       WHEN name LIKE '%for-each-ref%' THEN 'for_each_ref'
       WHEN name LIKE '%worktree list%' THEN 'worktree_list'
       WHEN name LIKE '%stash create%' THEN 'stash_create'
       WHEN name LIKE '%sparse-checkout%' THEN 'sparse_checkout'
       WHEN name LIKE '%rev-list%' THEN 'rev_list'
       WHEN name LIKE '%claude -p%' THEN 'llm_summary'
       WHEN name LIKE '%status%' THEN 'status'
       WHEN name LIKE '%merge-base%' THEN 'merge_base'
       WHEN name LIKE '%log %' THEN 'log'
       WHEN name LIKE '%config%' THEN 'config'
       WHEN name LIKE '%rev-parse%' THEN 'rev_parse'
       ELSE 'other' END as task_type,
  COUNT(*) as count,
  ROUND(SUM(dur)/1e6, 2) as total_ms,
  ROUND(MAX(dur)/1e6, 2) as max_ms,
  ROUND(AVG(dur)/1e6, 2) as avg_ms
FROM slice WHERE dur > 0
GROUP BY task_type ORDER BY total_ms DESC;
EOF
trace_processor trace.json -q /tmp/q.sql

# 2. How parallel are we? — subprocess time vs subprocess span
# parallelism ≈ 1.0 → serial; higher → concurrent execution is helping
# (span = first subprocess start to last subprocess end; excludes wt's non-subprocess overhead)
cat > /tmp/q.sql << 'EOF'
SELECT
  ROUND(SUM(dur)/1e6, 1) as total_subprocess_ms,
  ROUND((MAX(ts + dur) - MIN(ts))/1e6, 1) as span_ms,
  ROUND(CAST(SUM(dur) AS FLOAT) / (MAX(ts + dur) - MIN(ts)), 1) as parallelism
FROM slice WHERE dur > 0;
EOF
trace_processor trace.json -q /tmp/q.sql

# 3. What's on the critical path? — phase durations
# Shows time between milestones: serial setup, parallel work, finalization
# Key milestones: "Skeleton rendered", "Parallel execution started", "All results drained"
cat > /tmp/q.sql << 'EOF'
SELECT
  name,
  ROUND(ts/1e6, 1) as ms,
  ROUND((ts - LAG(ts) OVER (ORDER BY ts))/1e6, 1) as phase_ms
FROM slice WHERE dur = 0
ORDER BY ts;
EOF
trace_processor trace.json -q /tmp/q.sql

# 3. What's on the critical path? — parallel bottleneck (per-worktree)
# The worktree with the highest total_ms is the likely parallel bottleneck
cat > /tmp/q.sql << 'EOF'
SELECT
  EXTRACT_ARG(arg_set_id, 'args.context') as worktree,
  COUNT(*) as commands,
  ROUND(SUM(dur)/1e6, 1) as total_ms
FROM slice WHERE dur > 0
GROUP BY worktree ORDER BY total_ms DESC;
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
