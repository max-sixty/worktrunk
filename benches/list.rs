// Benchmarks for `wt list` command
//
// Benchmark groups:
//   - skeleton: Time until skeleton appears (1, 4, 8 worktrees; warm + cold)
//   - full: Full execution time (1, 4, 8 worktrees; warm + cold)
//   - worktree_scaling: Worktree count scaling (1, 4, 8 worktrees; warm + cold)
//   - real_repo: rust-lang/rust clone (1, 4, 8 worktrees; warm + cold)
//   - many_branches: 100 branches (warm + cold)
//   - divergent_branches: 200 branches × 20 commits on synthetic repo (warm + cold)
//   - real_repo_many_branches: 50 branches at different history depths / GH #461
//       - warm: all branches (first run expensive; subsequent runs hit persistent cache)
//       - warm_worktrees_only: no branch enumeration (~600ms)
//   - timeout_effect: Compare with/without 500ms command timeout on rust repo / GH #461 fix
//
// Run examples (Criterion takes a positional substring FILTER; no --skip):
//   cargo bench --bench list                         # All benchmarks
//   cargo bench --bench list skeleton                # Progressive rendering
//   cargo bench --bench list real_repo_many_branches # GH #461 scenario (large repo + many branches)
//   cargo bench --bench list timeout_effect          # Test timeout fix for GH #461
//   cargo bench --bench list warm                    # Warm-cache variants (every group's warm rows)
//   cargo bench --bench list skeleton/warm           # Skeleton group, warm only

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::path::{Path, PathBuf};
use std::process::Command;
use worktrunk::testing::isolate_subprocess_env;
use wt_perf::{
    RepoConfig, add_history_spread_branches, add_worktrees, clone_rust_repo, create_mixed_repo,
    create_repo, invalidate_caches_auto, run_git, setup_fake_remote,
};

/// Benchmark configuration wrapping RepoConfig with cache state.
#[derive(Clone)]
struct BenchConfig {
    repo: RepoConfig,
    cold_cache: bool,
}

impl BenchConfig {
    const fn typical(worktrees: usize, cold_cache: bool) -> Self {
        Self {
            repo: RepoConfig::typical(worktrees),
            cold_cache,
        }
    }

    const fn branches(count: usize, commits_per_branch: usize, cold_cache: bool) -> Self {
        Self {
            repo: RepoConfig::branches(count, commits_per_branch),
            cold_cache,
        }
    }

    const fn many_divergent_branches(cold_cache: bool) -> Self {
        Self {
            repo: RepoConfig::many_divergent_branches(),
            cold_cache,
        }
    }

    fn label(&self) -> &'static str {
        if self.cold_cache { "cold" } else { "warm" }
    }
}

/// Run a benchmark with the given config.
fn run_benchmark(
    b: &mut criterion::Bencher,
    binary: &Path,
    repo_path: &Path,
    config: &BenchConfig,
    args: &[&str],
    env: Option<(&str, &str)>,
) {
    let cmd_factory = || {
        let mut cmd = Command::new(binary);
        cmd.args(args).current_dir(repo_path);
        isolate_subprocess_env(&mut cmd, None);
        if let Some((key, value)) = env {
            cmd.env(key, value);
        }
        cmd
    };

    if config.cold_cache {
        // `BatchSize::PerIteration` (not `SmallInput`): under `SmallInput`,
        // criterion calls `setup` for an entire batch up front and then runs
        // the timed routines back-to-back — so only the first `wt` per batch
        // is cold and the rest hit a freshly populated `.git/wt/cache/`,
        // biasing "cold" warm. `PerIteration` invalidates immediately before
        // every measured iteration; the setup is far cheaper than a `wt`
        // subprocess, so per-iter `Instant::now` overhead doesn't dominate.
        b.iter_batched(
            || invalidate_caches_auto(repo_path),
            |_| {
                cmd_factory().output().unwrap();
            },
            criterion::BatchSize::PerIteration,
        );
    } else {
        b.iter(|| {
            cmd_factory().output().unwrap();
        });
    }
}

fn bench_skeleton(c: &mut Criterion) {
    let mut group = c.benchmark_group("skeleton");
    let binary = Path::new(env!("CARGO_BIN_EXE_wt"));

    for worktrees in [1, 4, 8] {
        for cold in [false, true] {
            let config = BenchConfig::typical(worktrees, cold);

            group.bench_with_input(
                BenchmarkId::new(config.label(), worktrees),
                &config,
                |b, config| {
                    let temp = create_repo(&config.repo);
                    let repo_path = temp.path().join("repo");
                    setup_fake_remote(&repo_path);
                    run_benchmark(
                        b,
                        binary,
                        &repo_path,
                        config,
                        &["list"],
                        Some(("WORKTRUNK_SKELETON_ONLY", "1")),
                    );
                },
            );
        }
    }

    group.finish();
}

fn bench_full(c: &mut Criterion) {
    let mut group = c.benchmark_group("full");
    let binary = Path::new(env!("CARGO_BIN_EXE_wt"));

    for worktrees in [1, 4, 8] {
        for cold in [false, true] {
            let config = BenchConfig::typical(worktrees, cold);

            group.bench_with_input(
                BenchmarkId::new(config.label(), worktrees),
                &config,
                |b, config| {
                    let temp = create_repo(&config.repo);
                    let repo_path = temp.path().join("repo");
                    setup_fake_remote(&repo_path);
                    run_benchmark(b, binary, &repo_path, config, &["list"], None);
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
        for cold in [false, true] {
            let config = BenchConfig::typical(worktrees, cold);

            group.bench_with_input(
                BenchmarkId::new(config.label(), worktrees),
                &config,
                |b, config| {
                    let temp = create_repo(&config.repo);
                    let repo_path = temp.path().join("repo");
                    run_git(&repo_path, &["status"]);
                    run_benchmark(b, binary, &repo_path, config, &["list"], None);
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
    // that for cold/8, where each iteration also rebuilds eight 59k-entry
    // indexes via `git status`. Warm-path variance is that slowest single
    // subprocess, not measurement noise, so the inherited 30-sample / 15s
    // default just burns time: at >1s/iter Criterion can't fit 30 samples
    // in 15s, so it runs 30 single-iteration samples regardless. 10 is
    // Criterion's minimum (`sample_size` < 10 panics); the 20s budget
    // caps the cheap warm variants at ≤2 iterations per sample. Cuts the
    // group's measured time ~3× — the expensive cold variants drop from
    // 30 iterations to 10.
    group.measurement_time(std::time::Duration::from_secs(20));
    group.sample_size(10);

    let binary = Path::new(env!("CARGO_BIN_EXE_wt"));

    for worktrees in [1, 4, 8] {
        for cold in [false, true] {
            let label = if cold { "cold" } else { "warm" };

            group.bench_with_input(
                BenchmarkId::new(label, worktrees),
                &(worktrees, cold),
                |b, &(worktrees, cold)| {
                    let config = RepoConfig::typical(worktrees);
                    let temp = tempfile::tempdir().unwrap();
                    let workspace_main = clone_rust_repo(&temp);
                    add_worktrees(&config, &workspace_main);
                    run_git(&workspace_main, &["status"]);

                    let make_cmd = || {
                        let mut cmd = Command::new(binary);
                        cmd.arg("list").current_dir(&workspace_main);
                        isolate_subprocess_env(&mut cmd, None);
                        cmd
                    };

                    if cold {
                        // `PerIteration` so every measured run is actually
                        // cold — see `run_benchmark` above for the rationale.
                        b.iter_batched(
                            || invalidate_caches_auto(&workspace_main),
                            |_| {
                                make_cmd().output().unwrap();
                            },
                            criterion::BatchSize::PerIteration,
                        );
                    } else {
                        b.iter(|| {
                            make_cmd().output().unwrap();
                        });
                    }
                },
            );
        }
    }

    group.finish();
}

fn bench_many_branches(c: &mut Criterion) {
    let mut group = c.benchmark_group("many_branches");
    let binary = Path::new(env!("CARGO_BIN_EXE_wt"));

    for cold in [false, true] {
        let config = BenchConfig::branches(100, 2, cold);

        group.bench_function(config.label(), |b| {
            let temp = create_repo(&config.repo);
            let repo_path = temp.path().join("repo");
            run_git(&repo_path, &["status"]);
            run_benchmark(
                b,
                binary,
                &repo_path,
                &config,
                &["list", "--branches", "--progressive"],
                None,
            );
        });
    }

    group.finish();
}

fn bench_divergent_branches(c: &mut Criterion) {
    let mut group = c.benchmark_group("divergent_branches");
    group.measurement_time(std::time::Duration::from_secs(30));
    group.sample_size(10);

    let binary = Path::new(env!("CARGO_BIN_EXE_wt"));

    for cold in [false, true] {
        let config = BenchConfig::many_divergent_branches(cold);

        group.bench_function(config.label(), |b| {
            let temp = create_repo(&config.repo);
            let repo_path = temp.path().join("repo");
            run_git(&repo_path, &["status"]);
            run_benchmark(
                b,
                binary,
                &repo_path,
                &config,
                &["list", "--branches", "--progressive"],
                None,
            );
        });
    }

    group.finish();
}

/// Set up rust repo workspace with branches at different history depths.
/// Returns the workspace path (temp dir must outlive usage).
fn setup_rust_workspace_with_branches(temp: &tempfile::TempDir, num_branches: usize) -> PathBuf {
    let workspace_main = clone_rust_repo(temp);
    add_history_spread_branches(&workspace_main, num_branches);
    run_git(&workspace_main, &["status"]);
    workspace_main
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
        let temp = tempfile::tempdir().unwrap();
        let workspace_main = setup_rust_workspace_with_branches(&temp, 50);

        // Add a second worktree (needed for worktrees_only to not auto-show branches)
        let wt_path = temp.path().join("wt-test");
        run_git(
            &workspace_main,
            &[
                "worktree",
                "add",
                "-b",
                "test-worktree",
                wt_path.to_str().unwrap(),
                "HEAD",
            ],
        );

        (temp, workspace_main)
    };

    // Baseline: all branches
    group.bench_function("warm", |b| {
        let (_temp, workspace_main) = setup_workspace();
        b.iter(|| {
            let mut cmd = Command::new(binary);
            cmd.args(["list", "--branches"])
                .current_dir(&workspace_main);
            isolate_subprocess_env(&mut cmd, None);
            cmd.output().unwrap();
        });
    });

    // Worktrees only: no branch enumeration, skips expensive %(ahead-behind) batch
    group.bench_function("warm_worktrees_only", |b| {
        let (_temp, workspace_main) = setup_workspace();
        b.iter(|| {
            let mut cmd = Command::new(binary);
            cmd.arg("list").current_dir(&workspace_main); // no --branches
            isolate_subprocess_env(&mut cmd, None);
            cmd.output().unwrap();
        });
    });

    group.finish();
}

// TODO(bench-full-combined): promote this into one canonical "full"-scenario
// benchmark covering lots of worktrees AND branches across cold + warm cache
// (the existing `full` group is worktrees-only despite its name; the branch
// groups have no worktrees). With a combined fixture, use the trace attribution
// to isolate where the cost lands — the per-command `args.context` grouping in
// `benches/CLAUDE.md` (query #3) buckets time per worktree, and worktree-only
// vs branch-only variants separate worktree-side tasks (status/diff/write-tree)
// from branch-side tasks (ahead-behind/merge-tree/integration), so a regression
// can be pinned to a feature rather than the whole command. `rerun_warm` below
// is the warm seed of that.

/// Warm-cache re-run: many worktrees AND many branches in varied states, with
/// `.git/wt/cache/` already populated — the steady-state "re-run `wt list`"
/// cost a user pays on every invocation once the persistent SHA cache is warm.
///
/// Unlike the cold variants, this measures *irreducible per-invocation* work:
/// the in-memory caches (`Arc<RepoCache>`, `WORKTREE_ROOTS`, `GIT_DIRS`,
/// `commit_tree`, `merge_base`) die with each `wt` process, so every re-run
/// re-forks whatever those cover, while the disk SHA cache (ahead-behind,
/// is-ancestor, merge-tree, diff-stats) serves from file reads. `b.iter`
/// (no cache invalidation) keeps the disk cache warm across iterations; the
/// criterion warm-up populates it before the first measured run.
///
/// Runs `list --branches --progressive` to exercise both worktree and branch
/// rows on the progressive render path (matching real TTY use), without the
/// network-touching `ci` column that `--full` would add.
fn bench_rerun_warm(c: &mut Criterion) {
    let mut group = c.benchmark_group("rerun_warm");
    // ~24 worktrees + 120 branches runs ~300ms/iter; the inherited 30-sample /
    // 15s budget can't fit 30, so cap samples at criterion's minimum and give
    // a 20s window (≈ a few iters per sample).
    group.measurement_time(std::time::Duration::from_secs(20));
    group.sample_size(10);

    let binary = Path::new(env!("CARGO_BIN_EXE_wt"));

    let (worktrees, branches) = (24usize, 120usize);
    group.bench_with_input(
        BenchmarkId::from_parameter(format!("{worktrees}wt_{branches}br")),
        &(worktrees, branches),
        |b, &(worktrees, branches)| {
            let temp = create_mixed_repo(worktrees, branches);
            let repo_path = temp.path().join("repo");
            b.iter(|| {
                let mut cmd = Command::new(binary);
                cmd.args(["list", "--branches", "--progressive"])
                    .current_dir(&repo_path);
                isolate_subprocess_env(&mut cmd, None);
                cmd.output().unwrap();
            });
        },
    );

    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(30)
        .measurement_time(std::time::Duration::from_secs(15))
        .warm_up_time(std::time::Duration::from_secs(3));
    targets = bench_skeleton, bench_full, bench_worktree_scaling, bench_real_repo, bench_many_branches, bench_divergent_branches, bench_real_repo_many_branches, bench_rerun_warm
}
criterion_main!(benches);
