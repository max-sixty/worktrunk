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
