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
//   - prune_e2e/live        — scan + removal of the squash-merged candidates,
//                             restored at the same commits before each sample
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

use std::cell::Cell;
use std::path::Path;
use std::process::Command;

use criterion::{Criterion, criterion_group, criterion_main};
use wt_perf::{
    CacheState, FixtureRecipe, FixtureRepo, LargeRepositoryPruneFixture,
    PRUNE_LARGE_REPOSITORY_BACKDROP_PAIRS, PRUNE_LARGE_REPOSITORY_CANDIDATE_PAIRS, bench_wt,
    invalidate_probe_caches, linked_worktree_path, run_and_check, run_git, run_git_ok, wt_command,
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
/// live fixture checks the destructive postcondition before restoring its
/// fixed candidate refs.
fn verify_candidates(repo: &Path, expected: usize) {
    let output = run_and_check(&mut wt_cmd(repo, DRY_RUN_ARGS));
    let items: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let found = items.as_array().unwrap().len();
    assert_eq!(
        found, expected,
        "fixture check: expected {expected} candidates, dry-run listed {found}"
    );
}

const PRUNE_BASELINE_REF: &str = "refs/wt-perf/prune/main";

/// A live-prune fixture whose candidate commits and default-branch history are
/// fixed for the full Criterion run.
struct PruneLiveFixture {
    repo: FixtureRepo,
}

impl PruneLiveFixture {
    fn create() -> Self {
        let repo = FixtureRecipe::Prune {
            candidate_pairs: MERGED,
            backdrop_pairs: UNMERGED,
        }
        .create();
        run_git(repo.path(), &["update-ref", PRUNE_BASELINE_REF, "main"]);
        for i in 0..MERGED {
            for branch in [format!("merged-wt-{i}"), format!("merged-br-{i}")] {
                run_git(
                    repo.path(),
                    &[
                        "update-ref",
                        &candidate_template_ref(&branch),
                        &format!("refs/heads/{branch}"),
                    ],
                );
            }
        }
        Self { repo }
    }

    fn path(&self) -> &Path {
        self.repo.path()
    }

    /// The initial sample uses the candidates created with the fixture.
    /// Later samples first prove that the measured run consumed exactly those
    /// refs, then check the same commits out again.
    fn prepare(&self, previous_run: bool) {
        if previous_run {
            self.assert_consumed();
            self.restore_candidates();
        } else {
            self.assert_intact();
        }
        invalidate_probe_caches(self.path());
    }

    fn restore_candidates(&self) {
        for i in 0..MERGED {
            let branch = format!("merged-wt-{i}");
            let worktree = linked_worktree_path(self.path(), &branch);
            run_git(
                self.path(),
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    &branch,
                    worktree.to_str().unwrap(),
                    &candidate_template_ref(&branch),
                ],
            );
        }
        for i in 0..MERGED {
            let branch = format!("merged-br-{i}");
            run_git(
                self.path(),
                &["branch", &branch, &candidate_template_ref(&branch)],
            );
        }
        self.assert_intact();
    }

    fn assert_intact(&self) {
        self.assert_main_unchanged();
        for i in 0..MERGED {
            let worktree_branch = format!("merged-wt-{i}");
            let branch = format!("merged-br-{i}");
            assert!(
                linked_worktree_path(self.path(), &worktree_branch).is_dir(),
                "live prune candidate worktree {worktree_branch} is missing"
            );
            assert_branch(self.path(), &worktree_branch, true);
            assert_branch(self.path(), &branch, true);
            assert_same_commit(
                self.path(),
                &format!("refs/heads/{worktree_branch}"),
                &candidate_template_ref(&worktree_branch),
            );
            assert_same_commit(
                self.path(),
                &format!("refs/heads/{branch}"),
                &candidate_template_ref(&branch),
            );
        }
        self.assert_backdrop_intact();
    }

    fn assert_consumed(&self) {
        self.assert_main_unchanged();
        for i in 0..MERGED {
            let worktree_branch = format!("merged-wt-{i}");
            let branch = format!("merged-br-{i}");
            assert!(
                !linked_worktree_path(self.path(), &worktree_branch).exists(),
                "measured prune left candidate worktree {worktree_branch}"
            );
            assert_branch(self.path(), &worktree_branch, false);
            assert_branch(self.path(), &branch, false);
        }
        self.assert_backdrop_intact();
    }

    fn assert_main_unchanged(&self) {
        assert_same_commit(self.path(), "main", PRUNE_BASELINE_REF);
    }

    fn assert_backdrop_intact(&self) {
        for i in 0..UNMERGED {
            let worktree_branch = format!("feature-wt-{i}");
            let branch = format!("feature-{i:03}");
            assert!(
                linked_worktree_path(self.path(), &worktree_branch).is_dir(),
                "measured prune removed backdrop worktree {worktree_branch}"
            );
            assert_branch(self.path(), &worktree_branch, true);
            assert_branch(self.path(), &branch, true);
        }
    }
}

fn candidate_template_ref(branch: &str) -> String {
    format!("refs/wt-perf/prune/{branch}")
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

/// Mutual ancestry is an object-ID equality check for commits.
fn assert_same_commit(repo: &Path, left: &str, right: &str) {
    assert!(
        run_git_ok(repo, &["merge-base", "--is-ancestor", left, right])
            && run_git_ok(repo, &["merge-base", "--is-ancestor", right, left]),
        "{left} and {right} no longer name the same commit"
    );
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

    // Live: scan + removal. Fixture setup records each candidate tip under a
    // private ref. Every iteration restores those exact tips, so main and the
    // candidate commit graph stay fixed across samples. The one-time dry-run
    // check below validates the workload; untimed preparation clears its probe
    // cache before every measured sample.
    //
    // Restoration right after removal is safe without waiting for the
    // detached `rm -rf`: prune stages the worktree into `.git/wt/trash/`
    // (rename), prunes metadata, and CAS-deletes the branch synchronously
    // before it exits, so path and branch name are free; the background rm
    // only ever touches the staged trash entry. (Contrast with
    // `benches/remove.rs`, which removes the *current* worktree — that path
    // leaves a placeholder directory plus a background `rmdir` that would
    // race the recreation.)
    let live_fixture = PruneLiveFixture::create();
    verify_candidates(live_fixture.path(), MERGED * 2);
    let ran = Cell::new(false);

    // Setup combines candidate restoration with probe-only invalidation, so
    // this arm can't go through `bench_wt` — the same carve-out as
    // `remove_e2e`.
    group.bench_function("live", |b| {
        b.iter_batched(
            || live_fixture.prepare(ran.get()),
            |_| {
                run_and_check(&mut wt_cmd(live_fixture.path(), LIVE_ARGS));
                ran.set(true);
            },
            criterion::BatchSize::PerIteration,
        );
    });
    if ran.get() {
        live_fixture.assert_consumed();
    }

    group.finish();
}

/// Large-repository scan ([`LargeRepositoryPruneFixture::acquire`]): a
/// 331k-commit repo
/// with 12 squash-merged candidate pairs against a two-sided-diverged
/// backdrop of 24 worktrees + 24 orphan branches forked across the last 5000
/// commits — 36 linked worktrees, lots removable, more not. Scale is what
/// moves every per-item cost — `merge-base --is-ancestor` ~40 ms, `merge-tree
/// --write-tree` ~130 ms, `git status` over ~60k files — vs the synthetic
/// fixture where probes bottom out at subprocess spawn. First acquisition
/// clones the pinned corpus from the network, then builds ~36 worktrees;
/// both are cached under target/wt-perf/bench-repos across runs.
///
/// Dry-runs only, in two flavors: warm (steady-state re-scan) and probe-cold
/// (`.git/wt/cache/` cleared per iteration — the "first prune after fetching
/// main" shape, where probes re-run at real cost but statuses stay
/// stat-warm). A full-cold scan and live removal are one-shot timeline
/// workloads at this scale: the former adds commit-graph rebuilding to the
/// probe cost, while the latter consumes candidates whose restoration takes
/// minutes. `wt-perf setup prune 12 24 --base large-repository` prepares
/// either run; the next
/// [`LargeRepositoryPruneFixture::acquire`] repairs consumed candidates.
fn bench_prune_large_repository(c: &mut Criterion) {
    // Opt-in only (`--features large-repository-benches`): the fixture is ~15 GiB —
    // bigger than a hosted CI runner's disk and the actions cache cap — so
    // the daily benchmarks workflow (plain `cargo bench`) must never build
    // it. `cfg!` keeps the body compiling either way.
    if !cfg!(feature = "large-repository-benches") {
        return;
    }

    let mut group = c.benchmark_group("prune_large_repository");

    group.bench_function("dry_run_warm", |b| {
        // Built inside the closure: criterion invokes a bench closure only
        // when the CLI filter matches it, but runs this function (and any
        // eager setup in it) unconditionally. This keeps a filtered run
        // (`cargo bench --bench prune prune_e2e`) from cloning the large
        // corpus. Repeat invocations re-validate the cached fixture in a few
        // git commands.
        let fixture = LargeRepositoryPruneFixture::acquire(
            PRUNE_LARGE_REPOSITORY_CANDIDATE_PAIRS,
            PRUNE_LARGE_REPOSITORY_BACKDROP_PAIRS,
        );
        let repo = fixture.path();
        verify_candidates(repo, PRUNE_LARGE_REPOSITORY_CANDIDATE_PAIRS * 2);
        bench_wt(b, repo, CacheState::Warm, || wt_cmd(repo, DRY_RUN_ARGS));
    });

    group.bench_function("dry_run_probe_cold", |b| {
        // Built inside the closure for the same filter-matching reason as
        // dry_run_warm above; a second call re-validates cheaply.
        let fixture = LargeRepositoryPruneFixture::acquire(
            PRUNE_LARGE_REPOSITORY_CANDIDATE_PAIRS,
            PRUNE_LARGE_REPOSITORY_BACKDROP_PAIRS,
        );
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
