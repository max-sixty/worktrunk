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
//   - picker_preview/warm/typical-8
//   - picker_preview/cold/typical-8
//
// Run examples:
//   cargo bench --bench picker_preview                 # all variants
//   cargo bench --bench picker_preview warm            # warm only
//   cargo bench --bench picker_preview -- --exact picker_preview/warm/typical-8

#[cfg(not(unix))]
fn main() {
    // Picker is Unix-only; benchmark is a no-op on Windows.
}

#[cfg(unix)]
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
#[cfg(unix)]
use std::path::Path;
#[cfg(unix)]
use std::process::Command;
#[cfg(unix)]
use worktrunk::testing::isolate_subprocess_env;
#[cfg(unix)]
use wt_perf::{CacheState, RepoConfig, bench_wt, create_repo, setup_fake_remote};

#[cfg(unix)]
fn bench_picker_preview(c: &mut Criterion) {
    let mut group = c.benchmark_group("picker_preview");
    // The picker workload runs a few hundred ms warm on typical-8 (the
    // ~1.4s median quoted in `src/commands/picker/mod.rs` is on a different
    // fixture; measured ~320ms / ~370ms warm/cold here on a 14-core M-series
    // box). Sticking with `sample_size(10)` per #2685's lead and budgeting
    // 35s gives Criterion enough headroom to fit 10 samples without the
    // "increase target time" warning under either cache mode.
    group.sample_size(10);
    group.measurement_time(std::time::Duration::from_secs(35));

    let binary = Path::new(env!("CARGO_BIN_EXE_wt"));

    for worktrees in [8] {
        for cold in [false, true] {
            let label = if cold { "cold" } else { "warm" };
            let config = RepoConfig::typical(worktrees);

            group.bench_with_input(
                BenchmarkId::new(label, format!("typical-{worktrees}")),
                &(config, cold),
                |b, (config, cold)| {
                    let temp = create_repo(config);
                    let repo_path = temp.path().join("repo");
                    setup_fake_remote(&repo_path);

                    let make_cmd = || {
                        let mut cmd = Command::new(binary);
                        cmd.args(["switch", "--no-cd"]).current_dir(&repo_path);
                        isolate_subprocess_env(&mut cmd, None);
                        cmd.env("WORKTRUNK_PREVIEW_BENCH", "1");
                        cmd
                    };

                    // Cold matters here: the picker writes to
                    // `.git/wt/cache/picker-preview/` (Log / BranchDiff /
                    // UpstreamDiff entries), so without invalidation iter 1
                    // measures real cost and iter 2+ measure cache hits.
                    let cache = if *cold {
                        CacheState::Cold
                    } else {
                        CacheState::Warm
                    };
                    bench_wt(b, &repo_path, cache, make_cmd);
                },
            );
        }
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
