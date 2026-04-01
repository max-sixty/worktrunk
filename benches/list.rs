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
//       - warm: baseline (~15-18s)
//       - warm_optimized: with skip_expensive_for_stale (~2-3s)
//       - warm_worktrees_only: no branch enumeration (~600ms)
//   - timeout_effect: Compare with/without 500ms command timeout on rust repo / GH #461 fix
//
// Run examples:
//   cargo bench --bench list                         # All benchmarks
//   cargo bench --bench list skeleton                # Progressive rendering
//   cargo bench --bench list real_repo_many_branches # GH #461 scenario (large repo + many branches)
//   cargo bench --bench list timeout_effect          # Test timeout fix for GH #461
//   cargo bench --bench list -- --skip cold          # Skip cold cache variants
//   cargo bench --bench list -- --skip real          # Skip rust repo clone

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::path::{Path, PathBuf};
use std::process::Command;
use wt_perf::{
    RepoConfig, add_history_spread_branches, add_worktrees, clone_rust_repo, create_repo,
    invalidate_caches_auto, isolate_cmd, run_git, setup_fake_remote,
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
        isolate_cmd(&mut cmd, None);
        if let Some((key, value)) = env {
            cmd.env(key, value);
        }
        cmd
    };

    if config.cold_cache {
        b.iter_batched(
            || invalidate_caches_auto(repo_path),
            |_| {
                cmd_factory().output().unwrap();
            },
            criterion::BatchSize::SmallInput,
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
            let temp = create_repo(&config.repo);
            let repo_path = temp.path().join("repo");
            setup_fake_remote(&repo_path);

            group.bench_with_input(
                BenchmarkId::new(config.label(), worktrees),
                &config,
                |b, config| {
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
            let temp = create_repo(&config.repo);
            let repo_path = temp.path().join("repo");
            setup_fake_remote(&repo_path);

            group.bench_with_input(
                BenchmarkId::new(config.label(), worktrees),
                &config,
                |b, config| {
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
            let temp = create_repo(&config.repo);
            let repo_path = temp.path().join("repo");
            run_git(&repo_path, &["status"]);

            group.bench_with_input(
                BenchmarkId::new(config.label(), worktrees),
                &config,
                |b, config| {
                    run_benchmark(b, binary, &repo_path, config, &["list"], None);
                },
            );
        }
    }

    group.finish();
}

fn bench_real_repo(c: &mut Criterion) {
    let mut group = c.benchmark_group("real_repo");
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
                        isolate_cmd(&mut cmd, None);
                        cmd
                    };

                    if cold {
                        b.iter_batched(
                            || invalidate_caches_auto(&workspace_main),
                            |_| {
                                make_cmd().output().unwrap();
                            },
                            criterion::BatchSize::SmallInput,
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
        let temp = create_repo(&config.repo);
        let repo_path = temp.path().join("repo");
        run_git(&repo_path, &["status"]);

        group.bench_function(config.label(), |b| {
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
        let temp = create_repo(&config.repo);
        let repo_path = temp.path().join("repo");
        run_git(&repo_path, &["status"]);

        group.bench_function(config.label(), |b| {
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
/// Benchmarks three modes:
/// - `warm`: baseline with all branches, no optimization (~15-18s)
/// - `warm_optimized`: with skip_expensive_for_stale (what `wt switch` picker uses, ~2-3s)
/// - `warm_worktrees_only`: no branch enumeration (~600ms)
///
/// Key insight: `git for-each-ref %(ahead-behind:BASE)` is O(commits), not O(refs).
/// It must walk the commit graph to compute divergence, so it takes ~2s on rust-lang/rust
/// regardless of how many refs are queried. Skipping branch enumeration entirely avoids this.
fn bench_real_repo_many_branches(c: &mut Criterion) {
    let mut group = c.benchmark_group("real_repo_many_branches");
    group.measurement_time(std::time::Duration::from_secs(60));
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

    // Baseline: all branches, no optimization
    group.bench_function("warm", |b| {
        let (_temp, workspace_main) = setup_workspace();
        b.iter(|| {
            let mut cmd = Command::new(binary);
            cmd.args(["list", "--branches"])
                .current_dir(&workspace_main);
            isolate_cmd(&mut cmd, None);
            cmd.output().unwrap();
        });
    });

    // With skip_expensive_for_stale optimization (simulates wt switch picker behavior)
    group.bench_function("warm_optimized", |b| {
        let (_temp, workspace_main) = setup_workspace();
        b.iter(|| {
            let mut cmd = Command::new(binary);
            cmd.args(["list", "--branches"])
                .current_dir(&workspace_main);
            isolate_cmd(&mut cmd, None);
            cmd.env("WORKTRUNK_TEST_SKIP_EXPENSIVE_THRESHOLD", "1");
            cmd.output().unwrap();
        });
    });

    // Worktrees only: no branch enumeration, skips expensive %(ahead-behind) batch
    group.bench_function("warm_worktrees_only", |b| {
        let (_temp, workspace_main) = setup_workspace();
        b.iter(|| {
            let mut cmd = Command::new(binary);
            cmd.arg("list").current_dir(&workspace_main); // no --branches
            isolate_cmd(&mut cmd, None);
            cmd.output().unwrap();
        });
    });

    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(30)
        .measurement_time(std::time::Duration::from_secs(15))
        .warm_up_time(std::time::Duration::from_secs(3));
    targets = bench_skeleton, bench_full, bench_worktree_scaling, bench_real_repo, bench_many_branches, bench_divergent_branches, bench_real_repo_many_branches
}
criterion_main!(benches);
