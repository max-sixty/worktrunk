# Benchmark Guidelines

## Quick Start

```bash
# Run fast synthetic benchmarks (skip slow ones)
cargo bench --bench list -- --skip cold --skip real

# Run all benchmarks (includes real repo, takes ~1 hour)
cargo bench --bench list

# Run specific benchmark
cargo bench --bench list bench_list_by_worktree_count
cargo bench --bench list bench_list_cold_cache
cargo bench --bench list bench_list_real_repo
```

## Benchmark Types

### 1. Synthetic Benchmarks (Fast, ~5-10 minutes)
- `bench_list_by_worktree_count` - Scaling with worktree count (1-8), warm caches
- `bench_list_by_repo_profile` - Scaling with repo size (minimal/typical/large), warm caches
- `bench_list_sequential_vs_parallel` - Sequential vs parallel comparison
- `bench_list_cold_cache` - Cold cache performance (all git caches invalidated)

### 2. Real Repository Benchmarks (Slow, ~30-60 minutes)
- `bench_list_real_repo` - rust-lang/rust repo, warm caches
- `bench_list_real_repo_cold_cache` - rust-lang/rust repo, cold caches

## Rust Repo Caching

The rust-lang/rust repository is automatically cached:
- **First run**: Clones to `target/bench-repos/rust` (~2-5 minutes)
- **Subsequent runs**: Reuses cached clone (instant)
- **Clean cache**: `rm -rf target/bench-repos/` or `cargo clean`
- **Auto-recovery**: Corrupted caches are automatically removed and re-cloned

**No manual intervention needed** - the cache works automatically.

## Faster Iteration During Development

### Option 1: Skip Slow Benchmarks
```bash
# Skip cold cache and real repo benchmarks
cargo bench --bench list -- --skip cold --skip real

# Run only specific benchmark
cargo bench --bench list bench_list_by_worktree_count
```

### Option 2: Use Pattern Matching
```bash
# Run all benchmarks with "list_by" in the name
cargo bench --bench list "list_by"

# Run all "warm cache" benchmarks (excludes cold cache variants)
cargo bench --bench list -- --skip cold
```

### Option 3: Reduce Sample Size (Future)
Currently sample size is fixed at 30 samples per benchmark. To add a quick profile:

```rust
// In benches/list.rs, replace the criterion_group! with:
#[cfg(feature = "quick-bench")]
criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(10)  // 3x faster
        .measurement_time(std::time::Duration::from_secs(10))
        .warm_up_time(std::time::Duration::from_secs(2));
    targets = /* ... */
}

#[cfg(not(feature = "quick-bench"))]
criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(30)
        .measurement_time(std::time::Duration::from_secs(15))
        .warm_up_time(std::time::Duration::from_secs(3));
    targets = /* ... */
}
```

Then run: `cargo bench --bench list --features quick-bench`

## Understanding Benchmark Results

### Warm vs Cold Cache

**Warm cache** (default):
- Git caches populated from previous operations
- Represents typical interactive usage
- Results: Faster, but may hit bottlenecks with many worktrees

**Cold cache**:
- All git caches invalidated before measurement
- Simulates first-run performance
- Caches invalidated:
  - Index (`.git/index`) - speeds up `git status`
  - Commit graph (`.git/objects/info/commit-graph`) - speeds up `git rev-list`
  - Packed refs (`.git/packed-refs`) - speeds up ref resolution
- NOT invalidated:
  - Filesystem cache (OS-level, can't control)
  - Pack files (object storage, not a cache)

### Expected Performance

**Modest repos** (500 commits, 100 files):
- Cold cache penalty: ~5-16% slower
- Scaling: Linear with worktree count

**Large repos** (rust-lang/rust):
- Cold cache penalty: ~4x slower for single worktree
- Scaling: Warm cache shows superlinear degradation, cold cache scales much better
- Surprising result: Cold cache is faster than warm at 8 worktrees!

## Benchmark Output Locations

- Results: `target/criterion/`
- Cached rust repo: `target/bench-repos/rust/`
- Benchmark reports: `target/criterion/*/report/index.html`

## Common Issues

### "Git rev-parse failed in cached repo"
**Solution**: Already handled automatically - corrupted cache is removed and re-cloned.

### Benchmarks taking too long
**Solutions**:
1. Skip slow benchmarks: `cargo bench --bench list -- --skip cold --skip real`
2. Run specific benchmark: `cargo bench --bench list bench_list_by_worktree_count`
3. Use pattern matching to run subset

### Out of disk space
**Solution**: Clean the rust repo cache: `rm -rf target/bench-repos/`
