// Benchmarks for `wt list` command
//
// Benchmark groups:
//   - skeleton: Time until skeleton appears (1, 4, 8 worktrees; warm + cold)
//   - complete: Full execution time (1, 4, 8 worktrees; warm + cold)
//   - worktree_scaling: Worktree count scaling (1, 4, 8 worktrees; warm + cold)
//   - real_repo: rust-lang/rust clone (1, 4, 8 worktrees; warm + cold)
//   - many_branches: 100 branches (warm + cold)
//   - divergent_branches: 200 branches × 20 commits on synthetic repo (warm + cold)
//   - real_repo_many_branches: 50 branches at different history depths / GH #461 (warm only)
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
use std::sync::OnceLock;
use tempfile::TempDir;

/// Lazy-initialized rust repo path.
static RUST_REPO: OnceLock<PathBuf> = OnceLock::new();

/// Unified benchmark configuration.
#[derive(Clone)]
struct BenchConfig {
    // Repo structure
    commits_on_main: usize,
    files: usize,
    branches: usize,
    commits_per_branch: usize,
    // Worktrees (0 = none)
    worktrees: usize,
    worktree_commits_ahead: usize,
    worktree_uncommitted_files: usize,
    // Cache state
    cold_cache: bool,
}

impl BenchConfig {
    /// Typical repo with worktrees (for skeleton, complete, worktree_scaling)
    const fn typical(worktrees: usize, cold_cache: bool) -> Self {
        Self {
            commits_on_main: 500,
            files: 100,
            branches: 0,
            commits_per_branch: 0,
            worktrees,
            worktree_commits_ahead: 10,
            worktree_uncommitted_files: 3,
            cold_cache,
        }
    }

    /// Branch-only config (for many_branches)
    const fn branches(count: usize, commits_per_branch: usize, cold_cache: bool) -> Self {
        Self {
            commits_on_main: 1,
            files: 1,
            branches: count,
            commits_per_branch,
            worktrees: 0,
            worktree_commits_ahead: 0,
            worktree_uncommitted_files: 0,
            cold_cache,
        }
    }

    /// Many divergent branches (GH #461 scenario: 200 branches × 20 commits)
    const fn many_divergent_branches(cold_cache: bool) -> Self {
        Self {
            commits_on_main: 100,
            files: 50,
            branches: 200,
            commits_per_branch: 20,
            worktrees: 0,
            worktree_commits_ahead: 0,
            worktree_uncommitted_files: 0,
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

/// Create a test repository from config.
fn create_test_repo(config: &BenchConfig) -> TempDir {
    let temp_dir = tempfile::tempdir().unwrap();
    let repo_path = temp_dir.path().join("main");
    std::fs::create_dir(&repo_path).unwrap();

    run_git(&repo_path, &["init", "-b", "main"]);
    run_git(&repo_path, &["config", "user.name", "Benchmark"]);
    run_git(&repo_path, &["config", "user.email", "bench@test.com"]);

    // Create initial file structure
    let num_files = config.files.max(1);
    for i in 0..num_files {
        let file_path = repo_path.join(format!("src/file_{}.rs", i));
        std::fs::create_dir_all(file_path.parent().unwrap()).unwrap();
        std::fs::write(
            &file_path,
            format!(
                "// File {i}\npub struct Module{i} {{ data: Vec<String> }}\npub fn function_{i}() -> i32 {{ {} }}\n",
                i * 42
            ),
        )
        .unwrap();
    }

    run_git(&repo_path, &["add", "."]);
    run_git(&repo_path, &["commit", "-m", "Initial commit"]);

    // Build commit history on main
    for i in 1..config.commits_on_main {
        let num_files_to_modify = 2 + (i % 2);
        for j in 0..num_files_to_modify {
            let file_idx = (i * 7 + j * 13) % num_files;
            let file_path = repo_path.join(format!("src/file_{}.rs", file_idx));
            let mut content = std::fs::read_to_string(&file_path).unwrap();
            content.push_str(&format!(
                "\npub fn function_{file_idx}_{i}() -> i32 {{ {} }}\n",
                i * 100 + j
            ));
            std::fs::write(&file_path, content).unwrap();
        }
        run_git(&repo_path, &["add", "."]);
        run_git(&repo_path, &["commit", "-m", &format!("Commit {i}")]);
    }

    // Create branches
    for i in 0..config.branches {
        let branch_name = format!("feature-{i:03}");
        run_git(&repo_path, &["checkout", "-b", &branch_name, "main"]);

        for j in 0..config.commits_per_branch {
            let feature_file = repo_path.join(format!("feature_{i:03}_{j}.rs"));
            std::fs::write(
                &feature_file,
                format!(
                    "// Feature {i} file {j}\npub fn feature_{i}_func_{j}() -> i32 {{ {} }}\n",
                    i * 100 + j
                ),
            )
            .unwrap();
            run_git(&repo_path, &["add", "."]);
            run_git(
                &repo_path,
                &["commit", "-m", &format!("Feature {branch_name} commit {j}")],
            );
        }
    }

    if config.branches > 0 {
        run_git(&repo_path, &["checkout", "main"]);
    }

    // Add worktrees
    for wt_num in 1..config.worktrees {
        let branch = format!("feature-wt-{wt_num}");
        let wt_path = temp_dir.path().join(format!("wt-{wt_num}"));

        let head_output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        let base_commit = String::from_utf8_lossy(&head_output.stdout)
            .trim()
            .to_string();

        run_git(
            &repo_path,
            &[
                "worktree",
                "add",
                "-b",
                &branch,
                wt_path.to_str().unwrap(),
                &base_commit,
            ],
        );

        // Add diverging commits
        for i in 0..config.worktree_commits_ahead {
            let file_path = wt_path.join(format!("feature_{wt_num}_file_{i}.txt"));
            std::fs::write(&file_path, format!("Feature {wt_num} content {i}\n")).unwrap();
            run_git(&wt_path, &["add", "."]);
            run_git(
                &wt_path,
                &["commit", "-m", &format!("Feature {wt_num} commit {i}")],
            );
        }

        // Add uncommitted changes
        for i in 0..config.worktree_uncommitted_files {
            let file_path = wt_path.join(format!("uncommitted_{i}.txt"));
            std::fs::write(&file_path, "Uncommitted content\n").unwrap();
        }
    }

    temp_dir
}

/// Invalidate git caches for cold benchmarks.
fn invalidate_caches(repo_path: &Path, num_worktrees: usize) {
    let git_dir = repo_path.join(".git");

    // Remove index files
    let _ = std::fs::remove_file(git_dir.join("index"));
    for i in 1..num_worktrees {
        let _ = std::fs::remove_file(
            git_dir
                .join("worktrees")
                .join(format!("wt-{i}"))
                .join("index"),
        );
    }

    // Remove commit graph
    let _ = std::fs::remove_file(git_dir.join("objects/info/commit-graph"));
    let _ = std::fs::remove_dir_all(git_dir.join("objects/info/commit-graphs"));

    // Remove packed refs
    let _ = std::fs::remove_file(git_dir.join("packed-refs"));
}

/// Set up fake remote for default branch detection.
fn setup_fake_remote(repo_path: &Path) {
    let refs_dir = repo_path.join(".git/refs/remotes/origin");
    std::fs::create_dir_all(&refs_dir).unwrap();
    std::fs::write(refs_dir.join("HEAD"), "ref: refs/remotes/origin/main\n").unwrap();
    let head_sha = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_path)
        .output()
        .unwrap();
    std::fs::write(refs_dir.join("main"), head_sha.stdout).unwrap();
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
            || invalidate_caches(repo_path, config.worktrees),
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
            let temp = create_test_repo(&config);
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
            let temp = create_test_repo(&config);
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
            let temp = create_test_repo(&config);
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

fn get_or_clone_rust_repo() -> PathBuf {
    RUST_REPO
        .get_or_init(|| {
            let cache_dir = std::env::current_dir().unwrap().join("target/bench-repos");
            let rust_repo = cache_dir.join("rust");

            if rust_repo.exists() {
                let output = Command::new("git")
                    .args(["rev-parse", "HEAD"])
                    .current_dir(&rust_repo)
                    .output();

                if output.is_ok_and(|o| o.status.success()) {
                    println!("Using cached rust repo at {}", rust_repo.display());
                    return rust_repo;
                }
                println!("Cached rust repo corrupted, re-cloning...");
                std::fs::remove_dir_all(&rust_repo).unwrap();
            }

            std::fs::create_dir_all(&cache_dir).unwrap();
            println!("Cloning rust-lang/rust (this will take several minutes)...");

            let clone_output = Command::new("git")
                .args([
                    "clone",
                    "https://github.com/rust-lang/rust.git",
                    rust_repo.to_str().unwrap(),
                ])
                .output()
                .unwrap();

            assert!(clone_output.status.success(), "Failed to clone rust repo");
            println!("Rust repo cloned successfully");
            rust_repo
        })
        .clone()
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
                    let rust_repo = get_or_clone_rust_repo();
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

                    // Add worktrees manually (can't use create_test_repo for external repo)
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
        let temp = create_test_repo(&config);
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
        let temp = create_test_repo(&config);
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
    let rust_repo = get_or_clone_rust_repo();
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
/// Scaling (rust-lang/rust repo):
/// - 20 branches at different depths: ~5s
/// - 50 branches at different depths: ~11s
/// - 100 branches at different depths: ~24s
/// - 200 branches at different depths: >30s (times out)
///
/// The slowdown comes from expensive merge-base calculations when branches have very different
/// ancestry depths in the commit graph.
fn bench_real_repo_many_branches(c: &mut Criterion) {
    let mut group = c.benchmark_group("real_repo_many_branches");
    group.measurement_time(std::time::Duration::from_secs(60));
    group.sample_size(10);

    let binary = get_release_binary();

    // Only test warm cache - cold cache would be extremely slow
    group.bench_function("warm", |b| {
        let temp = tempfile::tempdir().unwrap();
        let workspace_main = setup_rust_workspace_with_branches(&temp, 50);

        b.iter(|| {
            Command::new(&binary)
                .args(["list", "--branches"])
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
