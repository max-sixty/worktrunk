// Benchmarks for `wt step prune` end-to-end performance
//
// Prune has two cost centers with different shapes:
//
//   1. The scan — one integration check per worktree and per local branch,
//      parallel on the rayon pool. Dominated by the merge-tree/merge-base
//      probes, whose results persist in `.git/wt/cache/` (sha_cache), so the
//      first scan after new commits is cold and later scans are warm.
//   2. The removals — each integrated candidate runs the removal chain
//      (final clean check, fsmonitor stop, rename-to-trash, branch CAS
//      delete) serially under the write side of the scan lock, reusing the
//      removal plan its scan check computed.
//
// The fixture is `wt_perf::FixtureRecipe::Prune`: squash-merged candidates
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
//   - prune_e2e/live        — scan + removal of the squash-merged candidates
//                             from a fresh fixture per sample
//   - prune_large_repository/ — the same probe-cold and warm dry-runs on a
//     dry_run_probe_cold,     pinned large-repository corpus with both
//     dry_run_warm            populations at "dozens of worktrees" scale (~15 GiB
//                             fixture; first-ever run clones, minutes).
//                             Opt-in via `--features large-repository-benches` so
//                             the daily CI benchmarks job never builds it;
//                             full-cold and live at this scale are one-shot
//                             timelines, not criterion groups (see below)
//
// Run examples:
//   cargo bench --bench prune                # Synthetic variants
//   cargo bench --bench prune --features large-repository-benches prune_large_repository
//
// For phase attribution (scan vs per-candidate removal), trace one invocation
// instead: `wt-perf setup prune 4 8 --path /tmp/prune-repo`, then
// `wt-perf timeline -- -C /tmp/prune-repo step prune --dry-run --min-age 0s`
// and read the `prune-gather` / `prune-scan` / `prune-check:*` /
// `prune-remove:*` spans.

use std::cell::OnceCell;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use wt_perf::{
    CacheState, FixtureRecipe, PRUNE_LARGE_REPOSITORY_BACKDROP_PAIRS,
    PRUNE_LARGE_REPOSITORY_CANDIDATE_PAIRS, bench_wt, invalidate_probe_caches,
    linked_worktree_path, run_and_check, run_git_ok, wt_command,
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
    let mut cmd = wt_command(Path::new(env!("CARGO_BIN_EXE_wt")), repo, None);
    cmd.args(args);
    cmd
}

/// One-time fixture check, run once after setup and never inside a timed
/// loop: a dry-run scan must list exactly `expected` candidates. Catches a
/// fixture whose candidates prune doesn't detect, and detection false
/// positives against the unmerged backdrop (this once caught an invalidation
/// deleting worktree indexes and silently flipping the removability gate).
/// The timed iterations themselves assert only exit status (`bench_wt`); the
/// live fixture checks the destructive postcondition after timing.
fn verify_candidates(repo: &Path, expected: usize) {
    let output = run_and_check(&mut wt_cmd(repo, DRY_RUN_ARGS));
    let items: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let found = items.as_array().unwrap().len();
    assert_eq!(
        found, expected,
        "fixture check: expected {expected} candidates, dry-run listed {found}"
    );
}

fn assert_branch(repo: &Path, branch: &str, expected: bool) {
    assert_eq!(
        run_git_ok(
            repo,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}")
            ]
        ),
        expected,
        "branch {branch} presence differed from the live-prune fixture contract"
    );
}

fn assert_live_result(repo: &Path) {
    verify_candidates(repo, 0);
    for i in 0..MERGED {
        let worktree_branch = format!("merged-wt-{i}");
        let branch = format!("merged-br-{i}");
        assert!(
            !linked_worktree_path(repo, &worktree_branch).exists(),
            "measured prune left candidate worktree {worktree_branch}"
        );
        assert_branch(repo, &worktree_branch, false);
        assert_branch(repo, &branch, false);
    }
    for i in 0..UNMERGED {
        let worktree_branch = format!("feature-wt-{i}");
        let branch = format!("feature-{i:03}");
        assert!(
            linked_worktree_path(repo, &worktree_branch).is_dir(),
            "measured prune removed backdrop worktree {worktree_branch}"
        );
        assert_branch(repo, &worktree_branch, true);
        assert_branch(repo, &branch, true);
    }
}

fn bench_prune_e2e(c: &mut Criterion) {
    let mut group = c.benchmark_group("prune_e2e");

    // Dry-run repo: candidates present but never removed, so the fixture is
    // reusable across iterations without re-setup.
    let dry_fixture = FixtureRecipe::Prune {
        candidate_pairs: MERGED,
        backdrop_pairs: UNMERGED,
    }
    .create();
    verify_candidates(dry_fixture.path(), MERGED * 2);

    for (id, cache) in [
        // First prune after fetching the default branch.
        ("dry_run_probe_cold", CacheState::ProbeCold),
        // Steady-state re-scan where every probe hits sha_cache.
        ("dry_run_warm", CacheState::Warm),
    ] {
        group.bench_function(id, |b| {
            bench_wt(b, dry_fixture.path(), cache, || {
                wt_cmd(dry_fixture.path(), DRY_RUN_ARGS)
            });
        });
    }

    // Live: build and validate a fresh owned fixture outside each measured
    // duration. This avoids mutable template refs and repair logic while
    // preserving the exact candidate/backdrop contract.
    group.bench_function("live", |b| {
        b.iter_custom(|iterations| {
            let mut measured = Duration::ZERO;
            for _ in 0..iterations {
                let fixture = FixtureRecipe::Prune {
                    candidate_pairs: MERGED,
                    backdrop_pairs: UNMERGED,
                }
                .create();
                verify_candidates(fixture.path(), MERGED * 2);
                invalidate_probe_caches(fixture.path());

                let started = Instant::now();
                run_and_check(&mut wt_cmd(fixture.path(), LIVE_ARGS));
                measured += started.elapsed();
                assert_live_result(fixture.path());
            }
            measured
        });
    });

    group.finish();
}

/// Large-repository scan: a 331k-commit repo
/// with 12 squash-merged candidate pairs against a two-sided-diverged
/// backdrop of 24 worktrees + 24 orphan branches forked across the last 5000
/// commits — 36 linked worktrees, lots removable, more not. Scale is what
/// moves every per-item cost — `merge-base --is-ancestor` ~40 ms, `merge-tree
/// --write-tree` ~130 ms, `git status` over ~60k files — vs the synthetic
/// fixture where probes bottom out at subprocess spawn. The pinned source is
/// cached; each benchmark process builds one fresh ~36-worktree fixture.
///
/// Dry-runs only, in two flavors: warm (steady-state re-scan) and probe-cold
/// (`.git/wt/cache/` cleared per iteration — the "first prune after fetching
/// main" shape, where probes re-run at real cost but statuses stay
/// stat-warm). A full-cold scan and live removal are one-shot timeline
/// workloads at this scale: the former adds commit-graph rebuilding to the
/// probe cost, while the latter consumes its fixture. Use `wt-perf setup
/// large-repository-prune 12 24 --path <new-path>` for either.
fn bench_prune_large_repository(c: &mut Criterion) {
    // Opt-in only (`--features large-repository-benches`): the fixture is ~15 GiB —
    // bigger than a hosted CI runner's disk and the actions cache cap — so
    // the daily benchmarks workflow (plain `cargo bench`) must never build
    // it. `cfg!` keeps the body compiling either way.
    if !cfg!(feature = "large-repository-benches") {
        return;
    }

    let mut group = c.benchmark_group("prune_large_repository");
    let fixture = OnceCell::new();

    group.bench_function("dry_run_warm", |b| {
        // Built inside the closure: criterion invokes a bench closure only
        // when the CLI filter matches it, but runs this function (and any
        // eager setup in it) unconditionally. This keeps a filtered run
        // (`cargo bench --bench prune prune_e2e`) from cloning the large
        // corpus. Both variants share the fresh fixture when both match.
        let fixture = fixture.get_or_init(|| {
            FixtureRecipe::LargeRepositoryPrune {
                candidate_pairs: PRUNE_LARGE_REPOSITORY_CANDIDATE_PAIRS,
                backdrop_pairs: PRUNE_LARGE_REPOSITORY_BACKDROP_PAIRS,
            }
            .create()
        });
        let repo = fixture.path();
        verify_candidates(repo, PRUNE_LARGE_REPOSITORY_CANDIDATE_PAIRS * 2);
        bench_wt(b, repo, CacheState::Warm, || wt_cmd(repo, DRY_RUN_ARGS));
    });

    group.bench_function("dry_run_probe_cold", |b| {
        // Built inside the closure for the same filter-matching reason as
        // dry_run_warm above.
        let fixture = fixture.get_or_init(|| {
            FixtureRecipe::LargeRepositoryPrune {
                candidate_pairs: PRUNE_LARGE_REPOSITORY_CANDIDATE_PAIRS,
                backdrop_pairs: PRUNE_LARGE_REPOSITORY_BACKDROP_PAIRS,
            }
            .create()
        });
        let repo = fixture.path();
        verify_candidates(repo, PRUNE_LARGE_REPOSITORY_CANDIDATE_PAIRS * 2);
        bench_wt(b, repo, CacheState::ProbeCold, || {
            wt_cmd(repo, DRY_RUN_ARGS)
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
    targets = bench_prune_e2e, bench_prune_large_repository
}
criterion_main!(benches);
