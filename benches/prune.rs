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
// prune typically removes) against a two-sided-diverged backdrop of unmerged
// worktrees and branches (forked deep in history while main advanced) that
// are scanned every run but never removed.
//
// Benchmark variants:
//   - prune_e2e/dry_run_probe_cold — full scan, `.git/wt/cache/` cleared per
//                             iteration (probe-cold: the "first prune after
//                             new commits on main" shape; git's own caches
//                             stay warm, as after a real fetch)
//   - prune_e2e/dry_run_warm — full scan, caches warm (steady-state re-run)
//   - prune_e2e/live        — scan + removal of the squash-merged candidates,
//                             which are re-created before every iteration
//   - prune_real_repo/      — the same probe-cold and warm dry-runs on a
//     dry_run_probe_cold,     rust-lang/rust clone with both populations at
//     dry_run_warm            "dozens of worktrees" scale (cached ~15 GiB
//                             fixture; first-ever run clones, minutes).
//                             Opt-in via `--features real-repo-benches` so
//                             the daily CI benchmarks job never builds it;
//                             full-cold and live at this scale are one-shot
//                             timelines, not criterion groups (see below)
//
// Run examples:
//   cargo bench --bench prune                # Synthetic variants
//   cargo bench --bench prune --features real-repo-benches prune_real_repo
//
// For phase attribution (scan vs per-candidate removal), trace one invocation
// instead: `wt-perf setup prune-4-8 --path /tmp/prune-repo`, then
// `wt-perf timeline -- -C /tmp/prune-repo step prune --dry-run --min-age 0s`
// and read the `prune-gather` / `prune-scan` / `prune-check:*` /
// `prune-remove:*` spans.

use std::cell::Cell;
use std::path::Path;
use std::process::Command;

use criterion::{Criterion, criterion_group, criterion_main};
use worktrunk::testing::isolate_subprocess_env;
use wt_perf::{
    CacheState, PRUNE_REAL_MERGED, PRUNE_REAL_UNMERGED, add_squash_merged, bench_wt,
    create_prune_repo_at, ensure_prune_real_repo, run_and_check,
};

/// Squash-merged candidates per population (worktrees and orphan branches) —
/// what the live variant removes and re-creates every iteration.
const MERGED: usize = 4;
/// Unmerged worktrees and orphan branches — scanned every run, never removed.
const UNMERGED: usize = 8;

/// One invocation shape for every dry-run variant and the fixture check, so
/// the verified command and the timed commands can never silently diverge.
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

/// Build the `wt <args>` command for `repo`.
fn wt_cmd(repo: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(Path::new(env!("CARGO_BIN_EXE_wt")));
    cmd.args(args).current_dir(repo);
    isolate_subprocess_env(&mut cmd, None);
    cmd
}

/// One-time fixture check, run once after setup and never inside a timed
/// loop: a dry-run scan must list exactly `expected` candidates. Catches a
/// fixture whose candidates prune doesn't detect, and detection false
/// positives against the unmerged backdrop (this once caught an invalidation
/// deleting worktree indexes and silently flipping the removability gate).
/// The timed iterations themselves assert only exit status (`bench_wt`); a
/// live run that removes nothing surfaces on the next iteration's
/// re-creation instead — branch names don't carry the round, so
/// `add_squash_merged` fails loudly on the collision.
fn verify_candidates(repo: &Path, expected: usize) {
    let output = wt_cmd(repo, DRY_RUN_ARGS).output().unwrap();
    assert!(
        output.status.success(),
        "fixture-check dry-run failed:\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let items: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let found = items.as_array().unwrap().len();
    assert_eq!(
        found, expected,
        "fixture check: expected {expected} candidates, dry-run listed {found}"
    );
}

fn bench_prune_e2e(c: &mut Criterion) {
    let mut group = c.benchmark_group("prune_e2e");

    // Dry-run repo: candidates present but never removed, so the fixture is
    // reusable across iterations without re-setup.
    let temp_dry = tempfile::tempdir().unwrap();
    let repo_dry = temp_dry.path().join("repo");
    create_prune_repo_at(MERGED, UNMERGED, &repo_dry);
    verify_candidates(&repo_dry, MERGED * 2);

    // Probe-cold: every iteration re-pays the integration probes that
    // sha_cache would otherwise absorb — the "first prune after fetching
    // main" cost, with git's own caches staying warm as after a real fetch.
    group.bench_function("dry_run_probe_cold", |b| {
        bench_wt(b, &repo_dry, CacheState::ProbeCold, || {
            wt_cmd(&repo_dry, DRY_RUN_ARGS)
        });
    });

    // Warm: the steady-state re-scan where every probe hits sha_cache.
    group.bench_function("dry_run_warm", |b| {
        bench_wt(b, &repo_dry, CacheState::Warm, || {
            wt_cmd(&repo_dry, DRY_RUN_ARGS)
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
    verify_candidates(&repo_live, 0);
    let round = Cell::new(0usize);

    // Setup here is candidate re-creation, not invalidation, so this arm
    // can't go through `bench_wt` — the same carve-out as `remove_e2e`.
    group.bench_function("live", |b| {
        b.iter_batched(
            || {
                add_squash_merged(&repo_live, MERGED, round.get());
                round.set(round.get() + 1);
            },
            |_| run_and_check(&mut wt_cmd(&repo_live, LIVE_ARGS)),
            criterion::BatchSize::PerIteration,
        );
    });

    group.finish();
}

/// Rust-lang/rust-scale scan ([`ensure_prune_real_repo`]): a 331k-commit repo
/// with 12 squash-merged candidate pairs against a two-sided-diverged
/// backdrop of 24 worktrees + 24 orphan branches forked across the last 5000
/// commits — 36 linked worktrees, lots removable, more not. Scale is what
/// moves every per-item cost — `merge-base --is-ancestor` ~40 ms, `merge-tree
/// --write-tree` ~130 ms, `git status` over ~60k files — vs the synthetic
/// fixture where probes bottom out at subprocess spawn. First run clones
/// rust-lang/rust from the network (minutes), then builds ~36 worktrees at
/// ~3 s each; both are cached under target/wt-perf/bench-repos across runs.
///
/// Dry-runs only, in two flavors: warm (steady-state re-scan) and probe-cold
/// (`.git/wt/cache/` cleared per iteration — the "first prune after fetching
/// main" shape, where probes re-run at real cost but statuses stay
/// stat-warm). Probe-cold is criterion-feasible precisely because the
/// invalidation leaves the indexes alone; a *full*-cold iteration deletes
/// indexes and must rebuild them via `git reset` (a missing index reads as
/// all-tracked-files-deleted), and the rebuilt indexes carry no stat data,
/// so all 36 statuses re-hash ~60k files — ~1 min per iteration, turning
/// criterion's 10-sample minimum into a ~10-minute benchmark of a
/// mostly-artificial state. A live iteration would consume
/// the candidates and pay a minutes-long re-creation per sample. Measure
/// full-cold and live at this scale as one-shots instead (`wt-perf setup
/// prune-real`, then `wt-perf timeline -- -C <repo> step prune --min-age
/// 0s`); the next `ensure_prune_real_repo` call repairs the consumed
/// candidates.
fn bench_prune_real_repo(c: &mut Criterion) {
    // Opt-in only (`--features real-repo-benches`): the fixture is ~15 GiB —
    // bigger than a hosted CI runner's disk and the actions cache cap — so
    // the daily benchmarks workflow (plain `cargo bench`) must never build
    // it. `cfg!` keeps the body compiling either way.
    if !cfg!(feature = "real-repo-benches") {
        return;
    }

    let mut group = c.benchmark_group("prune_real_repo");

    group.bench_function("dry_run_warm", |b| {
        // Built inside the closure: criterion invokes a bench closure only
        // when the CLI filter matches it, but runs this function (and any
        // eager setup in it) unconditionally. This keeps a filtered run
        // (`cargo bench --bench prune prune_e2e`) from cloning
        // rust-lang/rust — or worse, racing a concurrent `wt-perf setup
        // prune-real` and wiping the fixture it is mid-build. Repeat
        // invocations re-validate the cached fixture in a few git commands.
        let repo = ensure_prune_real_repo(PRUNE_REAL_MERGED, PRUNE_REAL_UNMERGED);
        verify_candidates(&repo, PRUNE_REAL_MERGED * 2);
        bench_wt(b, &repo, CacheState::Warm, || wt_cmd(&repo, DRY_RUN_ARGS));
    });

    group.bench_function("dry_run_probe_cold", |b| {
        // Built inside the closure for the same filter-matching reason as
        // dry_run_warm above; a second call re-validates cheaply.
        let repo = ensure_prune_real_repo(PRUNE_REAL_MERGED, PRUNE_REAL_UNMERGED);
        verify_candidates(&repo, PRUNE_REAL_MERGED * 2);
        bench_wt(b, &repo, CacheState::ProbeCold, || {
            wt_cmd(&repo, DRY_RUN_ARGS)
        });
    });

    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(10)
        .measurement_time(std::time::Duration::from_secs(20))
        .warm_up_time(std::time::Duration::from_secs(3));
    targets = bench_prune_e2e, bench_prune_real_repo
}
criterion_main!(benches);
