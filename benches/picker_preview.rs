// Benchmarks for the `wt switch` picker's preview pre-compute workload
//
// Unix-only: `wt switch` (no branch arg) only routes to the interactive
// picker on Unix; on Windows it prints "interactive picker unavailable"
// and exits with code 2 before `WORKTRUNK_PREVIEW_BENCH` is consulted.
// Gating with `cfg(unix)` keeps `cargo bench` runnable on Windows by
// emitting an empty `main` there.
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
use wt_perf::{RepoConfig, create_repo, invalidate_caches_auto, setup_fake_remote};

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

                    if *cold {
                        // The picker writes to `.git/wt/cache/picker-preview/`
                        // (Log / BranchDiff / UpstreamDiff entries). Without
                        // invalidation, iter 1 measures real cost and iter 2+
                        // measure cache hits.
                        //
                        // `BatchSize::PerIteration` (not `SmallInput`):
                        // under `SmallInput`, criterion calls `setup` for an
                        // entire batch up front and then runs the timed
                        // routines back-to-back — so the first `wt switch`
                        // in a batch is cold but the rest hit a freshly
                        // populated `.git/wt/cache/`, biasing the "cold"
                        // measurement warm. `PerIteration` invalidates
                        // immediately before every measured iteration; the
                        // setup itself is far cheaper than a `wt switch`
                        // invocation, so per-iteration `Instant::now`
                        // overhead doesn't dominate.
                        b.iter_batched(
                            || invalidate_caches_auto(&repo_path),
                            |_| {
                                let output = make_cmd().output().unwrap();
                                assert!(
                                    output.status.success(),
                                    "Benchmark command failed:\nstderr: {}",
                                    String::from_utf8_lossy(&output.stderr)
                                );
                            },
                            criterion::BatchSize::PerIteration,
                        );
                    } else {
                        b.iter(|| {
                            let output = make_cmd().output().unwrap();
                            assert!(
                                output.status.success(),
                                "Benchmark command failed:\nstderr: {}",
                                String::from_utf8_lossy(&output.stderr)
                            );
                        });
                    }
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
