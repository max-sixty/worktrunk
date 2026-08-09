// Benchmarks for `wt list` command
//
// Benchmark groups:
//   - skeleton: Time until the skeleton paints (1 and 8 worktrees; warm + cold)
//   - worktree_scaling: Full execution at 1 and 8 worktrees (warm + cold)
//   - full: One combined full-surface fixture — many worktrees AND many branches
//       in varied states, with branch divergence spread across history depth.
//       The realistic "everything at once" workload (warm + cold).
//   - divergent_branches: 200 branches spread across history depth / GH #461 stress (warm + cold)
//   - large_repository: one pinned upstream corpus with 8 worktrees and 50 branches
//       spread through history; default list (warm + cold) and --branches (warm)
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
//   cargo bench --bench list large_repository       # Large-repository workload
//   cargo bench --bench list warm                    # Warm-cache variants (every group's warm rows)
//   cargo bench --bench list skeleton/warm           # Skeleton group, warm only

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::cell::OnceCell;
use std::path::Path;
use wt_perf::{
    CacheState, FixtureRecipe, bench_wt, run_git, standard_benchmark_profile, wt_command,
};

const WORKTREE_COUNTS: [usize; 2] = [1, 8];

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
    let binary = &worktrunk::testing::wt_bin();

    for total_worktrees in WORKTREE_COUNTS {
        for cache in CacheState::WARM_AND_COLD {
            group.bench_with_input(
                BenchmarkId::new(cache.label(), total_worktrees),
                &cache,
                |b, &cache| {
                    let fixture = FixtureRecipe::generated(total_worktrees - 1).create();
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
    let binary = &worktrunk::testing::wt_bin();

    for total_worktrees in WORKTREE_COUNTS {
        for cache in CacheState::WARM_AND_COLD {
            group.bench_with_input(
                BenchmarkId::new(cache.label(), total_worktrees),
                &cache,
                |b, &cache| {
                    let fixture = FixtureRecipe::generated(total_worktrees - 1).create();
                    run_git(fixture.path(), &["status"]);
                    run_benchmark(b, binary, fixture.path(), cache, &["list"], None);
                },
            );
        }
    }

    group.finish();
}

fn bench_large_repository(c: &mut Criterion) {
    let mut group = c.benchmark_group("large_repository");
    // On rust-lang/rust these rows take seconds. Variance is dominated by the
    // slowest git history walk rather than measurement noise, so Criterion's
    // minimum 10 samples is enough. All rows use one canonical state: eight
    // worktrees and fifty branches spread through history.
    //
    // Default `list` still captures the repository's refs and ahead/behind
    // state, but creates tasks only for worktree rows. `--branches` adds every
    // local branch row and its integration work. The default warm/cold pair
    // measures the large-repository cache penalty; the warm `--branches` row
    // covers the GH #461 history-depth workload without duplicating the warm
    // default row.
    group.measurement_time(std::time::Duration::from_secs(20));
    group.sample_size(10);

    let binary = &worktrunk::testing::wt_bin();
    let fixture = OnceCell::new();

    // Cold goes last because it removes the commit graph. Each warm row has
    // its own Criterion warm-up, so sharing the repository doesn't share the
    // command-specific cache state being measured.
    for (mode, cache, args) in [
        ("default", CacheState::Warm, &["list"][..]),
        ("branches", CacheState::Warm, &["list", "--branches"][..]),
        ("default", CacheState::Cold, &["list"][..]),
    ] {
        group.bench_with_input(
            BenchmarkId::new(mode, cache.label()),
            &cache,
            |b, &cache| {
                let fixture = fixture.get_or_init(|| {
                    let fixture = FixtureRecipe::Imported.create();
                    run_git(fixture.path(), &["status"]);
                    fixture
                });
                run_benchmark(b, binary, fixture.path(), cache, args, None);
            },
        );
    }

    group.finish();
}

fn bench_divergent_branches(c: &mut Criterion) {
    let mut group = c.benchmark_group("divergent_branches");
    group.measurement_time(std::time::Duration::from_secs(30));
    group.sample_size(10);

    let binary = &worktrunk::testing::wt_bin();

    for cache in CacheState::WARM_AND_COLD {
        group.bench_function(cache.label(), |b| {
            let fixture = FixtureRecipe::Generated {
                linked_worktrees: 0,
                branchless_branches: 200,
                remote_tracking_refs: 0,
            }
            .create();
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

/// Combined full-surface `wt list`: many worktrees AND many branches in varied
/// states, with branch divergence spread across history depth — the whole
/// command exercised by one fixture instead of several narrow ones. This is the
/// realistic "lots of worktrees & branches, all in various states" workload.
///
/// The generated fixture builds the spread of `wt list` gates and tasks at once:
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
    // Heavy fixture (24 linked worktrees + 120 branchless branches, deep
    // history): the cold variant runs well over the inherited 30-sample / 15s
    // budget. Cap samples at criterion's minimum and give a 20s window (≈ a
    // few iters per sample), matching the other heavy groups.
    group.measurement_time(std::time::Duration::from_secs(20));
    group.sample_size(10);

    let binary = &worktrunk::testing::wt_bin();
    let (linked_worktrees, branchless_branches) = (24usize, 120usize);

    for cache in CacheState::WARM_AND_COLD {
        group.bench_function(cache.label(), |b| {
            let fixture = FixtureRecipe::Generated {
                linked_worktrees,
                branchless_branches,
                remote_tracking_refs: 0,
            }
            .create();
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
    config = standard_benchmark_profile();
    targets = bench_skeleton, bench_worktree_scaling, bench_full, bench_large_repository, bench_divergent_branches
}
criterion_main!(benches);
