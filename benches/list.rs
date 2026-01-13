// Benchmarks for `wt list` command
//
// Benchmark groups:
//   - skeleton: Time until skeleton appears (1, 4, 8 worktrees; warm + cold)
//   - complete: Full execution time (1, 4, 8 worktrees; warm + cold)
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
use wt_perf::{RepoConfig, create_repo, ensure_rust_repo, invalidate_caches, setup_fake_remote};

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

fn run_git(path: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Git command failed: {:?}\nstderr: {}\nstdout: {}\npath: {}",
        args,
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout),
        path.display()
    );
}

fn get_release_binary() -> PathBuf {
    let build_output = Command::new("cargo")
        .args(["build", "--release"])
        .output()
        .unwrap();
    assert!(
        build_output.status.success(),
        "Failed to build release binary: {}",
        String::from_utf8_lossy(&build_output.stderr)
    );
    std::env::current_dir().unwrap().join("target/release/wt")
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
        if let Some((key, value)) = env {
            cmd.env(key, value);
        }
        cmd
    };

    if config.cold_cache {
        b.iter_batched(
            || invalidate_caches(repo_path, config.repo.worktrees),
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
    let binary = get_release_binary();

    for worktrees in [1, 4, 8] {
        for cold in [false, true] {
            let config = BenchConfig::typical(worktrees, cold);
            let temp = create_repo(&config.repo);
            let repo_path = temp.path().join("main");
            setup_fake_remote(&repo_path);

            group.bench_with_input(
                BenchmarkId::new(config.label(), worktrees),
                &config,
                |b, config| {
                    run_benchmark(
                        b,
                        &binary,
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

fn bench_complete(c: &mut Criterion) {
    let mut group = c.benchmark_group("complete");
    let binary = get_release_binary();

    for worktrees in [1, 4, 8] {
        for cold in [false, true] {
            let config = BenchConfig::typical(worktrees, cold);
            let temp = create_repo(&config.repo);
            let repo_path = temp.path().join("main");
            setup_fake_remote(&repo_path);

            group.bench_with_input(
                BenchmarkId::new(config.label(), worktrees),
                &config,
                |b, config| {
                    run_benchmark(b, &binary, &repo_path, config, &["list"], None);
                },
            );
        }
    }

    group.finish();
}

fn bench_worktree_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("worktree_scaling");
    let binary = get_release_binary();

    for worktrees in [1, 4, 8] {
        for cold in [false, true] {
            let config = BenchConfig::typical(worktrees, cold);
            let temp = create_repo(&config.repo);
            let repo_path = temp.path().join("main");
            run_git(&repo_path, &["status"]);

            group.bench_with_input(
                BenchmarkId::new(config.label(), worktrees),
                &config,
                |b, config| {
                    run_benchmark(b, &binary, &repo_path, config, &["list"], None);
                },
            );
        }
    }

    group.finish();
}

fn bench_real_repo(c: &mut Criterion) {
    let mut group = c.benchmark_group("real_repo");
    let binary = get_release_binary();

    for worktrees in [1, 4, 8] {
        for cold in [false, true] {
            let label = if cold { "cold" } else { "warm" };

            group.bench_with_input(
                BenchmarkId::new(label, worktrees),
                &(worktrees, cold),
                |b, &(worktrees, cold)| {
                    let rust_repo = ensure_rust_repo();
                    let temp = tempfile::tempdir().unwrap();
                    let workspace_main = temp.path().join("main");

                    let clone_output = Command::new("git")
                        .args([
                            "clone",
                            "--local",
                            rust_repo.to_str().unwrap(),
                            workspace_main.to_str().unwrap(),
                        ])
                        .output()
                        .unwrap();
                    assert!(
                        clone_output.status.success(),
                        "Failed to clone rust repo to workspace"
                    );

                    run_git(&workspace_main, &["config", "user.name", "Benchmark"]);
                    run_git(&workspace_main, &["config", "user.email", "bench@test.com"]);

                    // Add worktrees manually (can't use create_repo for external repo)
                    for wt_num in 1..worktrees {
                        let branch = format!("feature-wt-{wt_num}");
                        let wt_path = temp.path().join(format!("wt-{wt_num}"));

                        let head_output = Command::new("git")
                            .args(["rev-parse", "HEAD"])
                            .current_dir(&workspace_main)
                            .output()
                            .unwrap();
                        let base_commit = String::from_utf8_lossy(&head_output.stdout)
                            .trim()
                            .to_string();

                        run_git(
                            &workspace_main,
                            &[
                                "worktree",
                                "add",
                                "-b",
                                &branch,
                                wt_path.to_str().unwrap(),
                                &base_commit,
                            ],
                        );

                        for i in 0..10 {
                            let file_path = wt_path.join(format!("feature_{wt_num}_file_{i}.txt"));
                            std::fs::write(&file_path, format!("Feature {wt_num} content {i}\n"))
                                .unwrap();
                            run_git(&wt_path, &["add", "."]);
                            run_git(
                                &wt_path,
                                &["commit", "-m", &format!("Feature {wt_num} commit {i}")],
                            );
                        }

                        for i in 0..3 {
                            let file_path = wt_path.join(format!("uncommitted_{i}.txt"));
                            std::fs::write(&file_path, "Uncommitted content\n").unwrap();
                        }
                    }

                    if cold {
                        b.iter_batched(
                            || invalidate_caches(&workspace_main, worktrees),
                            |_| {
                                Command::new(&binary)
                                    .arg("list")
                                    .current_dir(&workspace_main)
                                    .output()
                                    .unwrap();
                            },
                            criterion::BatchSize::SmallInput,
                        );
                    } else {
                        run_git(&workspace_main, &["status"]);
                        b.iter(|| {
                            Command::new(&binary)
                                .arg("list")
                                .current_dir(&workspace_main)
                                .output()
                                .unwrap();
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
    let binary = get_release_binary();

    for cold in [false, true] {
        let config = BenchConfig::branches(100, 2, cold);
        let temp = create_repo(&config.repo);
        let repo_path = temp.path().join("main");
        run_git(&repo_path, &["status"]);

        group.bench_function(config.label(), |b| {
            run_benchmark(
                b,
                &binary,
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

    let binary = get_release_binary();

    for cold in [false, true] {
        let config = BenchConfig::many_divergent_branches(cold);
        let temp = create_repo(&config.repo);
        let repo_path = temp.path().join("main");
        run_git(&repo_path, &["status"]);

        group.bench_function(config.label(), |b| {
            run_benchmark(
                b,
                &binary,
                &repo_path,
                &config,
                &["list", "--branches", "--progressive"],
                None,
            );
        });
    }

    group.finish();
}

/// Helper to set up rust repo workspace with branches at different history depths.
/// Returns the workspace path (temp dir must outlive usage).
fn setup_rust_workspace_with_branches(temp: &tempfile::TempDir, num_branches: usize) -> PathBuf {
    let rust_repo = ensure_rust_repo();
    let workspace_main = temp.path().join("main");

    // Clone rust repo locally
    let clone_output = Command::new("git")
        .args([
            "clone",
            "--local",
            rust_repo.to_str().unwrap(),
            workspace_main.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        clone_output.status.success(),
        "Failed to clone rust repo to workspace"
    );

    // Get commits spread across history
    let log_output = Command::new("git")
        .args(["log", "--oneline", "-n", "5000", "--format=%H"])
        .current_dir(&workspace_main)
        .output()
        .unwrap();
    let log_str = String::from_utf8_lossy(&log_output.stdout);
    let step = 5000 / num_branches;
    let commits: Vec<&str> = log_str.lines().step_by(step).take(num_branches).collect();

    // Create branches pointing to different historical commits
    for (i, commit) in commits.iter().enumerate() {
        let branch_name = format!("feature-{i:03}");
        run_git(&workspace_main, &["branch", &branch_name, commit]);
    }

    // Warm the cache
    run_git(&workspace_main, &["status"]);

    workspace_main
}

/// Benchmark GH #461 scenario: large real repo (rust-lang/rust) with branches at different
/// historical points.
///
/// This reproduces the `wt select` delay reported in #461. The key factor is NOT commits
/// per branch, but rather how far back in history branches diverge from each other.
///
/// Benchmarks three modes:
/// - `warm`: baseline with all branches, no optimization (~15-18s)
/// - `warm_optimized`: with skip_expensive_for_stale (what `wt select` uses, ~2-3s)
/// - `warm_worktrees_only`: no branch enumeration (~600ms)
///
/// Key insight: `git for-each-ref %(ahead-behind:BASE)` is O(commits), not O(refs).
/// It must walk the commit graph to compute divergence, so it takes ~2s on rust-lang/rust
/// regardless of how many refs are queried. Skipping branch enumeration entirely avoids this.
fn bench_real_repo_many_branches(c: &mut Criterion) {
    let mut group = c.benchmark_group("real_repo_many_branches");
    group.measurement_time(std::time::Duration::from_secs(60));
    group.sample_size(10);

    let binary = get_release_binary();

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
            Command::new(&binary)
                .args(["list", "--branches"])
                .current_dir(&workspace_main)
                .output()
                .unwrap();
        });
    });

    // With skip_expensive_for_stale optimization (simulates wt select behavior)
    group.bench_function("warm_optimized", |b| {
        let (_temp, workspace_main) = setup_workspace();
        b.iter(|| {
            Command::new(&binary)
                .args(["list", "--branches"])
                .env("WORKTRUNK_TEST_SKIP_EXPENSIVE_THRESHOLD", "1")
                .current_dir(&workspace_main)
                .output()
                .unwrap();
        });
    });

    // Worktrees only: no branch enumeration, skips expensive %(ahead-behind) batch
    group.bench_function("warm_worktrees_only", |b| {
        let (_temp, workspace_main) = setup_workspace();
        b.iter(|| {
            Command::new(&binary)
                .arg("list") // no --branches
                .current_dir(&workspace_main)
                .output()
                .unwrap();
        });
    });

    group.finish();
}

/// Benchmark the effect of command timeout on GH #461 scenario.
///
/// Compares `wt list --branches` with and without the 500ms timeout.
/// The timeout kills slow git commands (merge-tree, rev-list) that would
/// otherwise block the TUI for seconds.
fn bench_timeout_effect(c: &mut Criterion) {
    let mut group = c.benchmark_group("timeout_effect");
    group.measurement_time(std::time::Duration::from_secs(60));
    group.sample_size(10);

    let binary = get_release_binary();

    // Set up workspace once for both benchmarks
    let temp = tempfile::tempdir().unwrap();
    let workspace_main = setup_rust_workspace_with_branches(&temp, 50);

    // Without timeout (baseline)
    group.bench_function("no_timeout", |b| {
        b.iter(|| {
            Command::new(&binary)
                .args(["list", "--branches"])
                .current_dir(&workspace_main)
                .output()
                .unwrap();
        });
    });

    // With 500ms timeout (GH #461 fix)
    group.bench_function("timeout_500ms", |b| {
        b.iter(|| {
            Command::new(&binary)
                .args(["list", "--branches"])
                .env("WORKTRUNK_COMMAND_TIMEOUT_MS", "500")
                .current_dir(&workspace_main)
                .output()
                .unwrap();
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
    targets = bench_skeleton, bench_complete, bench_worktree_scaling, bench_real_repo, bench_many_branches, bench_divergent_branches, bench_real_repo_many_branches, bench_timeout_effect
}
criterion_main!(benches);
