// Benchmarks for `wt step prune` end-to-end performance
//
// Prune has two cost centers with different shapes:
//
//   1. The scan — one integration check per worktree and per local branch,
//      parallel on the rayon pool. Dominated by the merge-tree/merge-base
//      probes, whose results persist in `.git/wt/cache/` (sha_cache), so the
//      first scan after new commits is cold and later scans are warm.
//   2. The removals — each integrated candidate runs the full removal chain
//      (pre-remove re-checks, fsmonitor stop, rename-to-trash, branch CAS
//      delete) serially under the write side of the scan lock.
//
// The fixture is `wt_perf::create_prune_repo_at`: squash-merged candidates
// (integrated by content — the expensive probe path, the post-PR-squash shape
// prune typically removes) against a backdrop of unmerged worktrees and
// branches that are scanned every run but never removed.
//
// Benchmark variants:
//   - prune_e2e/dry_run_cold — full scan, sha_cache invalidated per iteration
//   - prune_e2e/dry_run_warm — full scan, caches warm (steady-state re-run)
//   - prune_e2e/live        — scan + removal of the squash-merged candidates,
//                             which are re-created before every iteration
//
// Run examples:
//   cargo bench --bench prune                # All variants
//   cargo bench --bench prune dry_run        # Both dry-run variants
//
// For phase attribution (scan vs per-candidate removal), trace one invocation
// instead: `wt-perf setup prune-4-8 --path /tmp/prune-repo --persist`, then
// `wt-perf timeline -- -C /tmp/prune-repo step prune --dry-run --min-age 0s`
// and read the `prune-gather` / `prune-scan` / `prune-check:*` /
// `prune-remove:*` spans.

use std::cell::Cell;
use std::path::Path;
use std::process::Command;

use criterion::{Criterion, criterion_group, criterion_main};
use worktrunk::testing::isolate_subprocess_env;
use wt_perf::{
    add_squash_merged, create_prune_repo_at, invalidate_caches_auto, restore_worktree_indexes,
};

/// Squash-merged candidates per population (worktrees and orphan branches) —
/// what the live variant removes and re-creates every iteration.
const MERGED: usize = 4;
/// Unmerged worktrees and orphan branches — scanned every run, never removed.
const UNMERGED: usize = 8;

/// One invocation shape for both dry-run variants, so cold and warm can never
/// silently benchmark different commands.
const DRY_RUN_ARGS: &[&str] = &[
    "step",
    "prune",
    "--dry-run",
    "--min-age",
    "0s",
    "--format",
    "json",
];
const LIVE_ARGS: &[&str] = &["step", "prune", "--min-age", "0s", "--format", "json"];

fn bench_prune_e2e(c: &mut Criterion) {
    let mut group = c.benchmark_group("prune_e2e");
    let binary = Path::new(env!("CARGO_BIN_EXE_wt"));

    let make_cmd = |repo: &Path, args: &[&str]| {
        let mut cmd = Command::new(binary);
        cmd.args(args).current_dir(repo);
        isolate_subprocess_env(&mut cmd, None);
        cmd
    };
    let run = |repo: &Path, args: &[&str], label: &str| -> Vec<u8> {
        let output = make_cmd(repo, args).output().unwrap();
        assert!(
            output.status.success(),
            "{label} failed:\nstderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    };
    // Guard against silently measuring a degraded scan: every run must list
    // exactly the fixture's candidates (MERGED worktrees + MERGED branches)
    // as planned (dry run) or removed (live). This is what caught the
    // invalidated-index state flipping the removability gate, and on the live
    // variant it fails the offending iteration directly instead of the next
    // iteration's re-creation tripping over a leftover branch.
    let assert_candidates = |stdout: &[u8], label: &str| {
        let items: serde_json::Value = serde_json::from_slice(stdout).unwrap();
        let found = items.as_array().unwrap().len();
        assert_eq!(
            found,
            MERGED * 2,
            "{label}: expected {} candidates, run listed {found}",
            MERGED * 2
        );
    };

    // Dry-run repo: candidates present but never removed, so the fixture is
    // reusable across iterations without re-setup.
    let temp_dry = tempfile::tempdir().unwrap();
    let repo_dry = temp_dry.path().join("repo");
    create_prune_repo_at(MERGED, UNMERGED, &repo_dry);

    // Cold: every iteration re-pays the integration probes that sha_cache
    // would otherwise absorb — the "first prune after fetching main" cost.
    // `BatchSize::PerIteration` so the invalidation runs before every
    // measured iter (under `SmallInput`, only iter 1 per batch is cold).
    // The index restore is load-bearing: without it the invalidated indexes
    // make every worktree read dirty and prune's removability gate silently
    // drops the worktree candidates (see `restore_worktree_indexes`).
    group.bench_function("dry_run_cold", |b| {
        b.iter_batched(
            || {
                invalidate_caches_auto(&repo_dry);
                restore_worktree_indexes(&repo_dry);
            },
            |_| {
                let stdout = run(&repo_dry, DRY_RUN_ARGS, "dry_run_cold");
                assert_candidates(&stdout, "dry_run_cold");
            },
            criterion::BatchSize::PerIteration,
        );
    });

    // Warm: the steady-state re-scan where every probe hits sha_cache.
    group.bench_function("dry_run_warm", |b| {
        b.iter(|| {
            let stdout = run(&repo_dry, DRY_RUN_ARGS, "dry_run_warm");
            assert_candidates(&stdout, "dry_run_warm");
        });
    });

    // Live: scan + removal. The repo starts with no candidates (merged = 0);
    // each iteration's setup adds a fresh round of squash-merged worktrees
    // and branches, and the measured run removes them. `round` uniquifies the
    // committed content — re-using a round's files would make the squash
    // merge empty and the setup commit fail.
    //
    // Re-creation right after removal is safe without waiting for the
    // detached `rm -rf`: prune stages the worktree into `.git/wt/trash/`
    // (rename), prunes metadata, and CAS-deletes the branch synchronously
    // before it exits, so path and branch name are free; the background rm
    // only ever touches the staged trash entry. (Contrast with
    // `benches/remove.rs`, which removes the *current* worktree — that path
    // leaves a placeholder directory plus a background `rmdir` that would
    // race the recreation.)
    let temp_live = tempfile::tempdir().unwrap();
    let repo_live = temp_live.path().join("repo");
    create_prune_repo_at(0, UNMERGED, &repo_live);
    let round = Cell::new(0usize);

    group.bench_function("live", |b| {
        b.iter_batched(
            || {
                add_squash_merged(&repo_live, MERGED, round.get());
                round.set(round.get() + 1);
            },
            |_| {
                let stdout = run(&repo_live, LIVE_ARGS, "live");
                assert_candidates(&stdout, "live");
            },
            criterion::BatchSize::PerIteration,
        );
    });

    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(10)
        .measurement_time(std::time::Duration::from_secs(20))
        .warm_up_time(std::time::Duration::from_secs(3));
    targets = bench_prune_e2e
}
criterion_main!(benches);
