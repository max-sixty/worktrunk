// Benchmarks for `wt list` command
//
// Benchmark groups:
//   - skeleton: Time until the skeleton paints (1, 4, 8 worktrees; warm + cold)
//   - worktree_scaling: Full execution, worktree-count scaling (1, 4, 8 worktrees; warm + cold)
//   - full: One combined full-surface fixture — many worktrees AND many branches
//       in varied states, with branch divergence spread across history depth.
//       The realistic "everything at once" workload (warm + cold).
//   - divergent_branches: 200 branches × 20 commits / GH #461 deep-divergence stress (warm + cold)
//   - real_repo: rust-lang/rust clone (8 worktrees; warm + cold)
//   - real_repo_many_branches: 50 branches at different history depths / GH #461
//       - warm: all branches (first run expensive; subsequent runs hit persistent cache)
//       - warm_worktrees_only: no branch enumeration (~600ms)
//
// Attribution: a `full` wall time can't be split by side (worktree- and
// branch-side git subprocesses overlap on the rayon pool), so to see where a
// regression lands, trace one invocation and read the profile's BY CONTEXT /
// BY COMMAND TYPE tables — see `benches/CLAUDE.md` ("Analyzing a trace").
// For per-side regression tracking at criterion cadence, `worktree_scaling`
// is the worktree side and `divergent_branches` the branch side.
//
// Run examples (Criterion takes a positional substring FILTER; no --skip):
//   cargo bench --bench list                         # All benchmarks
//   cargo bench --bench list skeleton                # Progressive rendering
//   cargo bench --bench list full                    # Combined full-surface fixture
//   cargo bench --bench list real_repo_many_branches # GH #461 scenario (large repo + many branches)
//   cargo bench --bench list warm                    # Warm-cache variants (every group's warm rows)
//   cargo bench --bench list skeleton/warm           # Skeleton group, warm only

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::path::Path;
use wt_perf::{
    CacheState, FixtureRepo, RepoConfig, add_history_spread_branches, add_worktrees, bench_wt,
    clone_rust_repo, create_mixed_repo, create_repo, run_git, wt_command,
};

/// Run `wt` with `args` in `repo_path`, on a warm or cold cache.
///
/// Fixture-agnostic: callers build whatever repo shape they want, then pass
/// `cache` to pick the iteration strategy (see [`bench_wt`]).
fn run_benchmark(
    b: &mut criterion::Bencher,
    binary: &Path,
    repo_path: &Path,
    cache: CacheState,
    args: &[&str],
    env: Option<(&str, &str)>,
) {
    bench_wt(b, repo_path, cache, || {
        let mut cmd = wt_command(binary, repo_path, None);
        cmd.args(args);
        if let Some((key, value)) = env {
            cmd.env(key, value);
        }
        cmd
    });
}

fn bench_skeleton(c: &mut Criterion) {
    let mut group = c.benchmark_group("skeleton");
    let binary = Path::new(env!("CARGO_BIN_EXE_wt"));

    for worktrees in [1, 4, 8] {
        for cache in CacheState::WARM_AND_COLD {
            group.bench_with_input(
                BenchmarkId::new(cache.label(), worktrees),
                &cache,
                |b, &cache| {
                    let fixture = create_repo(&RepoConfig::typical(worktrees));
                    run_benchmark(
                        b,
                        binary,
                        fixture.path(),
                        cache,
                        &["list"],
                        Some(("WORKTRUNK_SKELETON_ONLY", "1")),
                    );
                },
            );
        }
    }

    group.finish();
}

fn bench_worktree_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("worktree_scaling");
    let binary = Path::new(env!("CARGO_BIN_EXE_wt"));

    for worktrees in [1, 4, 8] {
        for cache in CacheState::WARM_AND_COLD {
            group.bench_with_input(
                BenchmarkId::new(cache.label(), worktrees),
                &cache,
                |b, &cache| {
                    let fixture = create_repo(&RepoConfig::typical(worktrees));
                    run_git(fixture.path(), &["status"]);
                    run_benchmark(b, binary, fixture.path(), cache, &["list"], None);
                },
            );
        }
    }

    group.finish();
}

fn bench_real_repo(c: &mut Criterion) {
    let mut group = c.benchmark_group("real_repo");
    // `wt list` on rust-lang/rust runs ~2s warm — dominated by one deep
    // `git for-each-ref %(ahead-behind:main)` walk — and several times
    // that for cold, where each iteration also rebuilds eight 59k-entry
    // indexes via `git status`. Warm-path variance is that slowest single
    // subprocess, not measurement noise, so the inherited 30-sample / 15s
    // default just burns time: at >1s/iter Criterion can't fit 30 samples
    // in 15s, so it runs 30 single-iteration samples regardless. 10 is
    // Criterion's minimum (`sample_size` < 10 panics).
    //
    // 8 worktrees only: the worktree-count scaling shape is tracked at
    // criterion cadence by `worktree_scaling` (synthetic, 1/4/8); what's
    // unique here is real-repo magnitude and the cold penalty, which the
    // 8-worktree endpoints keep — each extra variant costs a fresh local
    // clone of rust-lang/rust plus its measurement window.
    group.measurement_time(std::time::Duration::from_secs(20));
    group.sample_size(10);

    let binary = Path::new(env!("CARGO_BIN_EXE_wt"));
    let worktrees = 8;

    for cache in CacheState::WARM_AND_COLD {
        group.bench_with_input(
            BenchmarkId::new(cache.label(), worktrees),
            &cache,
            |b, &cache| {
                let config = RepoConfig::typical(worktrees);
                let fixture = clone_rust_repo();
                add_worktrees(&config, fixture.path());
                run_git(fixture.path(), &["status"]);

                bench_wt(b, fixture.path(), cache, || {
                    let mut cmd = wt_command(binary, fixture.path(), None);
                    cmd.arg("list");
                    cmd
                });
            },
        );
    }

    group.finish();
}

fn bench_divergent_branches(c: &mut Criterion) {
    let mut group = c.benchmark_group("divergent_branches");
    group.measurement_time(std::time::Duration::from_secs(30));
    group.sample_size(10);

    let binary = Path::new(env!("CARGO_BIN_EXE_wt"));

    for cache in CacheState::WARM_AND_COLD {
        group.bench_function(cache.label(), |b| {
            let fixture = create_repo(&RepoConfig::many_divergent_branches());
            run_git(fixture.path(), &["status"]);
            run_benchmark(
                b,
                binary,
                fixture.path(),
                cache,
                &["list", "--branches", "--progressive"],
                None,
            );
        });
    }

    group.finish();
}

/// Set up rust repo workspace with branches at different history depths.
fn setup_rust_workspace_with_branches(num_branches: usize) -> FixtureRepo {
    let fixture = clone_rust_repo();
    add_history_spread_branches(fixture.path(), num_branches);
    run_git(fixture.path(), &["status"]);
    fixture
}

/// Benchmark GH #461 scenario: large real repo (rust-lang/rust) with branches at different
/// historical points.
///
/// This reproduces the `wt switch` interactive picker delay reported in #461. The key factor
/// is NOT commits per branch, but rather how far back in history branches diverge from each other.
///
/// Benchmarks two modes:
/// - `warm`: with all branches (first run expensive, subsequent runs hit the persistent cache)
/// - `warm_worktrees_only`: no branch enumeration (~600ms)
///
/// Key insight: `git for-each-ref %(ahead-behind:BASE)` is O(commits), not O(refs).
/// It must walk the commit graph to compute divergence, so it takes ~2s on rust-lang/rust
/// regardless of how many refs are queried. Skipping branch enumeration entirely avoids this.
fn bench_real_repo_many_branches(c: &mut Criterion) {
    let mut group = c.benchmark_group("real_repo_many_branches");
    // rust-lang/rust runs ~3.7s per `wt list --branches` iteration; warm-path
    // variance is dominated by the slowest single subprocess (a deep
    // `git merge-base` walking history), not measurement noise, so 10 samples
    // (criterion's minimum — `sample_size` < 10 panics) suffices. A 20s budget
    // is ≈ one iteration per sample (~37s/function), down from the
    // ~74s/function criterion spent filling the old 60s budget.
    group.measurement_time(std::time::Duration::from_secs(20));
    group.sample_size(10);

    let binary = Path::new(env!("CARGO_BIN_EXE_wt"));

    // Setup function - each bench_function creates its own fresh workspace
    // Uses setup_rust_workspace_with_branches plus a worktree for worktrees_only test
    let setup_workspace = || {
        let fixture = setup_rust_workspace_with_branches(50);

        // Add a second worktree (needed for worktrees_only to not auto-show branches)
        let wt_path = fixture.root().join("wt-test");
        run_git(
            fixture.path(),
            &[
                "worktree",
                "add",
                "-b",
                "test-worktree",
                wt_path.to_str().unwrap(),
                "HEAD",
            ],
        );

        fixture
    };

    // Baseline: all branches
    group.bench_function("warm", |b| {
        let fixture = setup_workspace();
        bench_wt(b, fixture.path(), CacheState::Warm, || {
            let mut cmd = wt_command(binary, fixture.path(), None);
            cmd.args(["list", "--branches"]);
            cmd
        });
    });

    // Worktrees only: no branch enumeration, skips expensive %(ahead-behind) batch
    group.bench_function("warm_worktrees_only", |b| {
        let fixture = setup_workspace();
        bench_wt(b, fixture.path(), CacheState::Warm, || {
            let mut cmd = wt_command(binary, fixture.path(), None);
            cmd.arg("list"); // no --branches
            cmd
        });
    });

    group.finish();
}

/// Combined full-surface `wt list`: many worktrees AND many branches in varied
/// states, with branch divergence spread across history depth — the whole
/// command exercised by one fixture instead of several narrow ones. This is the
/// realistic "lots of worktrees & branches, all in various states" workload.
///
/// `create_mixed_repo` builds the spread of `wt list` gates and tasks at once:
/// clean/dirty/staged working trees, merged/ahead/diverged branches, and the
/// GH #461 deep-divergence shape (branches forking at points spread across
/// history depth, so the `git for-each-ref %(ahead-behind)` walk has real
/// history to traverse).
///
/// To see *where* a regression lands, trace one invocation and read the
/// profile's BY CONTEXT / BY COMMAND TYPE tables — see `benches/CLAUDE.md`
/// ("Analyzing a trace"); a criterion wall time can't be decomposed by side
/// because the worktree- and branch-side git subprocesses run concurrently on
/// the rayon pool. For
/// per-side regression tracking at criterion cadence, `worktree_scaling`
/// isolates the worktree side and `divergent_branches` the branch-side walk.
///
/// Cold vs warm measure different costs. Warm (plain `b.iter`, disk SHA cache
/// kept hot by the criterion warm-up) is the *irreducible per-invocation* work:
/// the in-memory caches (`Arc<RepoCache>`, `WORKTREE_ROOTS`, `GIT_DIRS`,
/// `commit_tree`, `merge_base`) die with each `wt` process, so every re-run
/// re-forks whatever those cover while the disk SHA cache (ahead-behind,
/// is-ancestor, merge-tree, diff-stats) serves from file reads. Cold
/// invalidates `.git/wt/cache/` before each measured iteration, so it pays the
/// full #461 `%(ahead-behind)` walk and every integration probe from scratch.
///
/// Runs `list --branches --progressive` to exercise both worktree and branch
/// rows on the progressive render path (matching real TTY use), without the
/// network-touching `ci` column that `--full` would add.
fn bench_full(c: &mut Criterion) {
    let mut group = c.benchmark_group("full");
    // Heavy fixture (24 worktrees + 120 branches, deep history): the cold
    // variant runs well over the inherited 30-sample / 15s budget, so cap
    // samples at criterion's minimum and give a 20s window (≈ a few iters per
    // sample), matching the other heavy groups.
    group.measurement_time(std::time::Duration::from_secs(20));
    group.sample_size(10);

    let binary = Path::new(env!("CARGO_BIN_EXE_wt"));
    let (worktrees, branches) = (24usize, 120usize);

    for cache in CacheState::WARM_AND_COLD {
        group.bench_function(cache.label(), |b| {
            let fixture = create_mixed_repo(worktrees, branches);
            run_benchmark(
                b,
                binary,
                fixture.path(),
                cache,
                &["list", "--branches", "--progressive"],
                None,
            );
        });
    }

    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(30)
        .measurement_time(std::time::Duration::from_secs(15))
        .warm_up_time(std::time::Duration::from_secs(3));
    targets = bench_skeleton, bench_worktree_scaling, bench_full, bench_real_repo, bench_divergent_branches, bench_real_repo_many_branches
}
criterion_main!(benches);
