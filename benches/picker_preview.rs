// Benchmarks for the `wt switch` picker's preview pre-compute workload
//
// Unix-gated by choice, not capability: the picker now runs on Windows too
// (the `WORKTRUNK_PREVIEW_BENCH` path bypasses skim's TTY check on every
// platform), but standing up the `wt_perf` fixtures and subprocess harness
// below on Windows isn't worth it for a non-required bench. `cfg(unix)` emits
// an empty `main` there so `cargo bench` still builds.
//
// What this measures
// ------------------
// `wt switch` (interactive picker) submits one preview-compute task per row
// (worktree/branch) into the global rayon pool. Each task gathers the data
// that fills the picker's preview pane — diff stats, log lines, ahead/behind
// — and stores it in the in-memory preview cache. The user-visible quantity
// to optimize is "time from picker launch to all previews ready": that's
// the responsiveness window where j/k navigation should land on warm
// content.
//
// We measure that wall clock headlessly by spawning `wt` with
// `WORKTRUNK_PREVIEW_BENCH=1`, which runs the full picker prelude (collect,
// speculative spawn, skeleton, initial precompute, deferred precompute) and
// then exits right after `orchestrator.wait_for_idle()` — before skim
// launches and before any JSON serialization / stderr drain. The PTY route
// (option 2 from the task: "spawn → first interactive-ready point") would
// require a TTY harness; the documented nextest/SIGTTOU pain on
// `shell-integration-tests` (see project `CLAUDE.md`) makes that a follow-up
// rather than a prerequisite. The headless path captures the full pool
// workload, which is the variable the optimization work in #2662 / #2683 /
// #2685 / #2704 actually pushes on.
//
// Benchmark variants:
//   - picker_preview/warm/8-worktrees
//   - picker_preview/cold/8-worktrees
//
// Run examples:
//   cargo bench --bench picker_preview                 # all variants
//   cargo bench --bench picker_preview warm            # warm only
//   cargo bench --bench picker_preview -- --exact picker_preview/warm/8-worktrees

#[cfg(not(unix))]
fn main() {
    // This benchmark is intentionally a no-op on Windows.
}

#[cfg(unix)]
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
#[cfg(unix)]
use wt_perf::{CacheState, FixtureRecipe, bench_wt, wt_command};

#[cfg(unix)]
fn bench_picker_preview(c: &mut Criterion) {
    let mut group = c.benchmark_group("picker_preview");
    // Use Criterion's minimum sample count and enough measurement time for the
    // full preview workload under either cache mode.
    group.sample_size(10);
    group.measurement_time(std::time::Duration::from_secs(35));

    let binary = &worktrunk::testing::wt_bin();
    let total_worktrees = 8;

    for cache in CacheState::WARM_AND_COLD {
        group.bench_with_input(
            BenchmarkId::new(cache.label(), format!("{total_worktrees}-worktrees")),
            &cache,
            |b, &cache| {
                let fixture = FixtureRecipe::generated(total_worktrees - 1).create();

                let make_cmd = || {
                    let mut cmd = wt_command(binary, fixture.path(), None);
                    cmd.args(["switch", "--no-cd"])
                        .env("WORKTRUNK_PREVIEW_BENCH", "1");
                    cmd
                };

                // Cold matters here: the picker writes to
                // `.git/wt/cache/picker-preview/` (Log / BranchDiff /
                // UpstreamDiff entries), so without invalidation iter 1
                // measures real cost and iter 2+ measure cache hits.
                bench_wt(b, fixture.path(), cache, make_cmd);
            },
        );
    }

    group.finish();
}

#[cfg(unix)]
criterion_group! {
    name = benches;
    config = Criterion::default()
        .warm_up_time(std::time::Duration::from_secs(3));
    targets = bench_picker_preview
}
#[cfg(unix)]
criterion_main!(benches);
