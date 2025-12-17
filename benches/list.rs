// Benchmarks for `wt list` command
//
// Benchmark groups (Criterion group names in parentheses):
//
// 1. Progressive rendering (skeleton vs complete):
//    - skeleton: Time until skeleton appears (progressive mode, warm cache)
//    - skeleton_cold: Same with packed-refs invalidated
//    - complete: Full execution time (warm cache)
//    - complete_cold: Same with packed-refs invalidated
//
// 2. Scaling benchmarks:
//    - worktree_scaling: Worktree count scaling (1-8 worktrees, warm cache)
//    - worktree_scaling_cold: Same with all caches invalidated (index, commit-graph, packed-refs)
//    - profile_scaling: Repo size scaling (minimal/typical/large profiles)
//    - sequential_vs_parallel: Compare execution strategies
//
// 3. Real repository (rust-lang/rust):
//    - real_repo: Warm cache (1-8 worktrees)
//    - real_repo_cold: All caches invalidated (1, 4, 8 worktrees)
//
// 4. Many branches (`wt list --branches`):
//    - many_branches: 25/50/100 branches (warm cache)
//    - many_branches_cold: Same with packed-refs invalidated
//
// Run examples:
//   cargo bench --bench list                    # All benchmarks
//   cargo bench --bench list skeleton           # Progressive rendering benchmarks
//   cargo bench --bench list worktree_scaling   # Worktree scaling (warm + cold)
//   cargo bench --bench list many_branches      # Many branches (no rust repo clone)
//   cargo bench --bench list real_repo          # Real repo benchmarks only
//   cargo bench --bench list -- --skip cold     # Skip all cold cache variants
//
// Note: Real repo benchmarks clone rust-lang/rust on first run (~2-5 minutes).
// The clone is cached in target/bench-repos/ and reused across runs.
//
// Cold cache variants:
// - "_cold" suffix means caches invalidated before each iteration
// - skeleton_cold, complete_cold, many_branches_cold: Only packed-refs invalidated
// - worktree_scaling_cold, real_repo_cold: All caches invalidated (index, commit-graph, packed-refs)
//
// Note: Filesystem cache (OS-level) and pack files are not invalidated. Pack files are
// part of git's object storage, not a cache.

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use tempfile::TempDir;

/// Lazy-initialized rust repo path. Only clones when a benchmark actually needs it,
/// allowing filtering to skip the expensive clone for unrelated benchmarks.
static RUST_REPO: OnceLock<PathBuf> = OnceLock::new();

/// Benchmark configuration profiles representing different repo sizes
struct BenchmarkProfile {
    name: &'static str,
    commits: usize,
    files: usize,
    commits_ahead: usize,
    commits_behind: usize,
    uncommitted_files: usize,
}

const PROFILES: &[BenchmarkProfile] = &[
    BenchmarkProfile {
        name: "minimal",
        commits: 10,
        files: 10,
        commits_ahead: 0,
        commits_behind: 0, // Skip for now - causes git checkout issues
        uncommitted_files: 0,
    },
    BenchmarkProfile {
        name: "typical",
        commits: 500,
        files: 100,
        commits_ahead: 10,
        commits_behind: 0, // Skip for now - causes git checkout issues
        uncommitted_files: 3,
    },
    BenchmarkProfile {
        name: "large",
        commits: 1000,
        files: 200,
        commits_ahead: 50,
        commits_behind: 0, // Skip for now - causes git checkout issues
        uncommitted_files: 10,
    },
];

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

/// Build release binary and return path.
/// This is idempotent - cargo skips rebuild if already up-to-date.
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

/// Create a realistic repository with actual commit history and file changes
fn create_realistic_repo(commits: usize, files: usize) -> TempDir {
    let temp_dir = tempfile::tempdir().unwrap();
    let repo_path = temp_dir.path().join("main");
    std::fs::create_dir(&repo_path).unwrap();

    // Initialize repository
    run_git(&repo_path, &["init", "-b", "main"]);
    run_git(&repo_path, &["config", "user.name", "Benchmark"]);
    run_git(&repo_path, &["config", "user.email", "bench@test.com"]);

    // Create initial file structure
    for i in 0..files {
        let file_path = repo_path.join(format!("src/file_{}.rs", i));
        std::fs::create_dir_all(file_path.parent().unwrap()).unwrap();
        let content = format!(
            "// File {}\n\
             pub struct Module{} {{\n\
                 data: Vec<String>,\n\
             }}\n\n\
             pub fn function_{}() -> i32 {{\n\
                 {}\n\
             }}\n",
            i,
            i,
            i,
            i * 42
        );
        std::fs::write(&file_path, content).unwrap();
    }

    run_git(&repo_path, &["add", "."]);
    run_git(&repo_path, &["commit", "-m", "Initial commit"]);

    // Build commit history with realistic diffs
    for i in 1..commits {
        // Modify 2-3 files per commit for realistic git operations
        let num_files_to_modify = 2 + (i % 2);
        for j in 0..num_files_to_modify {
            let file_idx = (i * 7 + j * 13) % files; // Pseudo-random file selection
            let file_path = repo_path.join(format!("src/file_{}.rs", file_idx));
            let mut content = std::fs::read_to_string(&file_path).unwrap();
            content.push_str(&format!(
                "\npub fn function_{}_{}() -> i32 {{\n    {}\n}}\n",
                file_idx,
                i,
                i * 100 + j
            ));
            std::fs::write(&file_path, content).unwrap();
        }

        run_git(&repo_path, &["add", "."]);
        run_git(&repo_path, &["commit", "-m", &format!("Commit {}", i)]);
    }

    temp_dir
}

/// Add a worktree with diverged branch and uncommitted changes
fn add_worktree_with_divergence(
    temp_dir: &TempDir,
    repo_path: &Path,
    wt_num: usize,
    commits_ahead: usize,
    commits_behind: usize,
    uncommitted_files: usize,
) {
    let branch = format!("feature-{}", wt_num);
    let wt_path = temp_dir.path().join(format!("wt-{}", wt_num));

    // Get current HEAD to diverge from
    let head_output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_path)
        .output()
        .unwrap();
    let base_commit = String::from_utf8_lossy(&head_output.stdout)
        .trim()
        .to_string();

    // Create worktree at current HEAD
    run_git(
        repo_path,
        &[
            "worktree",
            "add",
            "-b",
            &branch,
            wt_path.to_str().unwrap(),
            &base_commit,
        ],
    );

    // Add diverging commits in worktree (creates "ahead" status)
    for i in 0..commits_ahead {
        let file_path = wt_path.join(format!("feature_{}_file_{}.txt", wt_num, i));
        let content = format!(
            "Feature {} content {}\n\
             This is a realistic file with multiple lines\n\
             to make git diff operations non-trivial.\n",
            wt_num, i
        );
        std::fs::write(&file_path, content).unwrap();
        run_git(&wt_path, &["add", "."]);
        run_git(
            &wt_path,
            &["commit", "-m", &format!("Feature {} commit {}", wt_num, i)],
        );
    }

    // Add uncommitted changes (exercises git diff HEAD)
    for i in 0..uncommitted_files {
        let file_path = wt_path.join(format!("uncommitted_{}.txt", i));
        std::fs::write(&file_path, "Uncommitted content\n").unwrap();
    }

    // Add commits to main branch (creates "behind" status for worktree)
    if commits_behind > 0 {
        // Ensure we're on the main branch
        run_git(repo_path, &["checkout", "main"]);

        for i in 0..commits_behind {
            let file_path = repo_path.join(format!("main_advance_{}.txt", i));
            std::fs::write(&file_path, format!("Main content {}\n", i)).unwrap();
            run_git(repo_path, &["add", "."]);
            run_git(repo_path, &["commit", "-m", &format!("Main advance {}", i)]);
        }
    }
}

/// Benchmark time to render skeleton (before cell filling starts).
///
/// Uses WORKTRUNK_SKELETON_ONLY=1 to exit after skeleton is rendered.
/// This measures the actual worktrunk code path: git I/O, parsing, layout, render skeleton.
fn bench_skeleton(c: &mut Criterion) {
    let mut group = c.benchmark_group("skeleton");

    let binary = get_release_binary();
    let profile = &PROFILES[1];

    for num_worktrees in [1, 4, 8] {
        let temp = create_realistic_repo(profile.commits, profile.files);
        let repo_path = temp.path().join("main");

        for i in 1..num_worktrees {
            add_worktree_with_divergence(
                &temp,
                &repo_path,
                i,
                profile.commits_ahead,
                profile.commits_behind,
                profile.uncommitted_files,
            );
        }

        // Set up cached default branch
        let refs_dir = repo_path.join(".git/refs/remotes/origin");
        std::fs::create_dir_all(&refs_dir).unwrap();
        std::fs::write(refs_dir.join("HEAD"), "ref: refs/remotes/origin/main\n").unwrap();
        let head_sha = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        std::fs::write(refs_dir.join("main"), head_sha.stdout).unwrap();

        group.bench_with_input(
            BenchmarkId::from_parameter(num_worktrees),
            &num_worktrees,
            |b, _| {
                b.iter(|| {
                    Command::new(&binary)
                        .arg("list")
                        .env("WORKTRUNK_SKELETON_ONLY", "1")
                        .current_dir(&repo_path)
                        .output()
                        .unwrap();
                });
            },
        );
    }

    group.finish();
}

/// Cold cache variant of skeleton benchmark.
/// Invalidates packed-refs before each measurement.
fn bench_skeleton_cold(c: &mut Criterion) {
    let mut group = c.benchmark_group("skeleton_cold");

    let binary = get_release_binary();
    let profile = &PROFILES[1];

    for num_worktrees in [1, 4, 8] {
        let temp = create_realistic_repo(profile.commits, profile.files);
        let repo_path = temp.path().join("main");

        for i in 1..num_worktrees {
            add_worktree_with_divergence(
                &temp,
                &repo_path,
                i,
                profile.commits_ahead,
                profile.commits_behind,
                profile.uncommitted_files,
            );
        }

        // Set up cached default branch
        let refs_dir = repo_path.join(".git/refs/remotes/origin");
        std::fs::create_dir_all(&refs_dir).unwrap();
        std::fs::write(refs_dir.join("HEAD"), "ref: refs/remotes/origin/main\n").unwrap();
        let head_sha = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        std::fs::write(refs_dir.join("main"), head_sha.stdout).unwrap();

        let packed_refs = repo_path.join(".git/packed-refs");

        group.bench_with_input(
            BenchmarkId::from_parameter(num_worktrees),
            &num_worktrees,
            |b, _| {
                b.iter_batched(
                    || {
                        // Invalidate packed-refs before each iteration
                        if packed_refs.exists() {
                            std::fs::remove_file(&packed_refs).unwrap();
                        }
                    },
                    |_| {
                        Command::new(&binary)
                            .arg("list")
                            .env("WORKTRUNK_SKELETON_ONLY", "1")
                            .current_dir(&repo_path)
                            .output()
                            .unwrap();
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

/// Benchmark `wt list` full execution time with warm caches.
///
/// This measures the actual worktrunk code path including git operations, parsing,
/// layout calculation, and rendering - i.e., when all data is filled in.
fn bench_complete(c: &mut Criterion) {
    let mut group = c.benchmark_group("complete");

    let binary = get_release_binary();
    let profile = &PROFILES[1];

    for num_worktrees in [1, 4, 8] {
        let temp = create_realistic_repo(profile.commits, profile.files);
        let repo_path = temp.path().join("main");

        for i in 1..num_worktrees {
            add_worktree_with_divergence(
                &temp,
                &repo_path,
                i,
                profile.commits_ahead,
                profile.commits_behind,
                profile.uncommitted_files,
            );
        }

        // Set up cached default branch
        let refs_dir = repo_path.join(".git/refs/remotes/origin");
        std::fs::create_dir_all(&refs_dir).unwrap();
        std::fs::write(refs_dir.join("HEAD"), "ref: refs/remotes/origin/main\n").unwrap();
        let head_sha = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        std::fs::write(refs_dir.join("main"), head_sha.stdout).unwrap();

        group.bench_with_input(
            BenchmarkId::from_parameter(num_worktrees),
            &num_worktrees,
            |b, _| {
                b.iter(|| {
                    Command::new(&binary)
                        .arg("list")
                        .current_dir(&repo_path)
                        .output()
                        .unwrap();
                });
            },
        );
    }

    group.finish();
}

/// Cold cache variant of complete benchmark.
/// Invalidates packed-refs before each measurement.
fn bench_complete_cold(c: &mut Criterion) {
    let mut group = c.benchmark_group("complete_cold");

    let binary = get_release_binary();
    let profile = &PROFILES[1];

    for num_worktrees in [1, 4, 8] {
        let temp = create_realistic_repo(profile.commits, profile.files);
        let repo_path = temp.path().join("main");

        for i in 1..num_worktrees {
            add_worktree_with_divergence(
                &temp,
                &repo_path,
                i,
                profile.commits_ahead,
                profile.commits_behind,
                profile.uncommitted_files,
            );
        }

        // Set up cached default branch
        let refs_dir = repo_path.join(".git/refs/remotes/origin");
        std::fs::create_dir_all(&refs_dir).unwrap();
        std::fs::write(refs_dir.join("HEAD"), "ref: refs/remotes/origin/main\n").unwrap();
        let head_sha = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        std::fs::write(refs_dir.join("main"), head_sha.stdout).unwrap();

        let packed_refs = repo_path.join(".git/packed-refs");

        group.bench_with_input(
            BenchmarkId::from_parameter(num_worktrees),
            &num_worktrees,
            |b, _| {
                b.iter_batched(
                    || {
                        // Invalidate packed-refs before each iteration
                        if packed_refs.exists() {
                            std::fs::remove_file(&packed_refs).unwrap();
                        }
                    },
                    |_| {
                        Command::new(&binary)
                            .arg("list")
                            .current_dir(&repo_path)
                            .output()
                            .unwrap();
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

fn bench_worktree_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("worktree_scaling");

    let binary = get_release_binary();

    // Use "typical" profile for this benchmark
    let profile = &PROFILES[1];

    // Test with different worktree counts to find crossover point
    for num_worktrees in [1, 2, 3, 4, 6, 8] {
        // Setup repo ONCE per worktree count (wt list is read-only, so reuse is safe)
        let temp = create_realistic_repo(profile.commits, profile.files);
        let repo_path = temp.path().join("main");

        // Add worktrees with divergence
        for i in 1..num_worktrees {
            add_worktree_with_divergence(
                &temp,
                &repo_path,
                i,
                profile.commits_ahead,
                profile.commits_behind,
                profile.uncommitted_files,
            );
        }

        // Warm up git's internal caches
        run_git(&repo_path, &["status"]);

        group.bench_with_input(
            BenchmarkId::from_parameter(num_worktrees),
            &num_worktrees,
            |b, _| {
                b.iter(|| {
                    Command::new(&binary)
                        .arg("list")
                        .current_dir(&repo_path)
                        .output()
                        .unwrap();
                });
            },
        );
    }

    group.finish();
}

fn bench_worktree_scaling_cold(c: &mut Criterion) {
    let mut group = c.benchmark_group("worktree_scaling_cold");

    let binary = get_release_binary();

    // Use "typical" profile for this benchmark
    let profile = &PROFILES[1];

    // Test with fewer data points since cold cache benchmarks are slower
    for num_worktrees in [1, 4, 8] {
        let temp = create_realistic_repo(profile.commits, profile.files);
        let repo_path = temp.path().join("main");

        // Add worktrees with divergence
        for i in 1..num_worktrees {
            add_worktree_with_divergence(
                &temp,
                &repo_path,
                i,
                profile.commits_ahead,
                profile.commits_behind,
                profile.uncommitted_files,
            );
        }

        let git_dir = repo_path.join(".git");

        // Collect paths to all git caches
        // 1. Index files (main repo + worktrees)
        let mut index_paths = vec![git_dir.join("index")];
        for i in 1..num_worktrees {
            let wt_index = git_dir
                .join("worktrees")
                .join(format!("wt-{}", i))
                .join("index");
            index_paths.push(wt_index);
        }

        // 2. Commit graph cache
        let commit_graph_dir = git_dir.join("objects").join("info");
        let commit_graph = commit_graph_dir.join("commit-graph");
        let commit_graphs_dir = commit_graph_dir.join("commit-graphs");

        // 3. Packed refs cache
        let packed_refs = git_dir.join("packed-refs");

        group.bench_with_input(
            BenchmarkId::from_parameter(num_worktrees),
            &num_worktrees,
            |b, _| {
                b.iter_batched(
                    || {
                        // Setup phase - remove all git caches (not measured)

                        // Remove index files
                        for index_path in &index_paths {
                            if index_path.exists() {
                                std::fs::remove_file(index_path).unwrap_or_else(|_| {
                                    panic!("Failed to remove index: {}", index_path.display())
                                });
                            }
                        }

                        // Remove commit graph
                        if commit_graph.exists() {
                            std::fs::remove_file(&commit_graph).unwrap_or_else(|_| {
                                panic!("Failed to remove commit-graph: {}", commit_graph.display())
                            });
                        }
                        if commit_graphs_dir.exists() {
                            std::fs::remove_dir_all(&commit_graphs_dir).unwrap_or_else(|_| {
                                panic!(
                                    "Failed to remove commit-graphs: {}",
                                    commit_graphs_dir.display()
                                )
                            });
                        }

                        // Remove packed refs
                        if packed_refs.exists() {
                            std::fs::remove_file(&packed_refs).unwrap_or_else(|_| {
                                panic!("Failed to remove packed-refs: {}", packed_refs.display())
                            });
                        }
                    },
                    |_| {
                        // Measured phase - only wt list execution
                        Command::new(&binary)
                            .arg("list")
                            .current_dir(&repo_path)
                            .output()
                            .unwrap();
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

fn bench_profile_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("profile_scaling");

    let binary = get_release_binary();

    // Fixed worktree count to isolate repo size impact
    let num_worktrees = 4;

    for profile in PROFILES {
        // Setup repo ONCE per profile (wt list is read-only)
        let temp = create_realistic_repo(profile.commits, profile.files);
        let repo_path = temp.path().join("main");

        for i in 1..num_worktrees {
            add_worktree_with_divergence(
                &temp,
                &repo_path,
                i,
                profile.commits_ahead,
                profile.commits_behind,
                profile.uncommitted_files,
            );
        }

        run_git(&repo_path, &["status"]);

        group.bench_with_input(
            BenchmarkId::from_parameter(profile.name),
            profile,
            |b, _profile| {
                b.iter(|| {
                    Command::new(&binary)
                        .arg("list")
                        .current_dir(&repo_path)
                        .output()
                        .unwrap();
                });
            },
        );
    }

    group.finish();
}

fn bench_sequential_vs_parallel(c: &mut Criterion) {
    let mut group = c.benchmark_group("sequential_vs_parallel");

    let binary = get_release_binary();

    let profile = &PROFILES[1]; // typical profile

    // Test at 3 data points to minimize overhead while still showing the comparison
    for num_worktrees in [1, 4, 8] {
        let temp = create_realistic_repo(profile.commits, profile.files);
        let repo_path = temp.path().join("main");

        for i in 1..num_worktrees {
            add_worktree_with_divergence(
                &temp,
                &repo_path,
                i,
                profile.commits_ahead,
                profile.commits_behind,
                profile.uncommitted_files,
            );
        }

        run_git(&repo_path, &["status"]);

        // Benchmark parallel implementation (default)
        group.bench_with_input(
            BenchmarkId::new("parallel", num_worktrees),
            &num_worktrees,
            |b, _| {
                b.iter(|| {
                    Command::new(&binary)
                        .arg("list")
                        .current_dir(&repo_path)
                        .output()
                        .unwrap();
                });
            },
        );
    }

    group.finish();
}

/// Get or clone the rust-lang/rust repository to a cached location.
/// This persists across benchmark runs but is cleaned by `cargo clean`.
///
/// Uses lazy initialization via OnceLock to avoid cloning when benchmarks
/// are filtered to only run non-real-repo tests.
fn get_or_clone_rust_repo() -> PathBuf {
    RUST_REPO
        .get_or_init(|| {
            let cache_dir = std::env::current_dir().unwrap().join("target/bench-repos");
            let rust_repo = cache_dir.join("rust");

            if rust_repo.exists() {
                // Verify it's a valid git repo
                let output = Command::new("git")
                    .args(["rev-parse", "HEAD"])
                    .current_dir(&rust_repo)
                    .output();

                match output {
                    Ok(out) if out.status.success() => {
                        println!("Using cached rust repo at {}", rust_repo.display());
                        return rust_repo;
                    }
                    Ok(_) | Err(_) => {
                        // Corrupted or incomplete clone - remove and re-clone
                        println!(
                            "Cached rust repo at {} is corrupted, removing and re-cloning...",
                            rust_repo.display()
                        );
                        std::fs::remove_dir_all(&rust_repo).unwrap_or_else(|e| {
                            panic!("Failed to remove corrupted rust repo: {}", e);
                        });
                        // Fall through to clone
                    }
                }
            }

            // Clone the repo
            std::fs::create_dir_all(&cache_dir).unwrap();
            println!(
                "Cloning rust-lang/rust to {} (this will take several minutes)...",
                rust_repo.display()
            );

            // Do a full clone to ensure we have all history. This takes longer on first run
            // (~5-10 minutes) but is cached in target/bench-repos/ for subsequent runs.
            let clone_output = Command::new("git")
                .args([
                    "clone",
                    "https://github.com/rust-lang/rust.git",
                    rust_repo.to_str().unwrap(),
                ])
                .output()
                .unwrap();

            assert!(
                clone_output.status.success(),
                "Failed to clone rust repo: {}",
                String::from_utf8_lossy(&clone_output.stderr)
            );

            println!("Rust repo cloned successfully");
            rust_repo
        })
        .clone()
}

fn bench_real_repo(c: &mut Criterion) {
    let mut group = c.benchmark_group("real_repo");

    let binary = get_release_binary();

    // Test with different worktree counts
    for num_worktrees in [1, 2, 4, 6, 8] {
        group.bench_with_input(
            BenchmarkId::new("rust_repo", num_worktrees),
            &num_worktrees,
            |b, &num_worktrees| {
                // All expensive setup is inside the closure, so it only runs
                // when this benchmark ID matches the filter. Combined with
                // OnceLock, the rust repo is cloned at most once per process.
                let rust_repo = get_or_clone_rust_repo();

                // Create a temporary workspace for this benchmark run
                let temp = tempfile::tempdir().unwrap();
                let workspace_main = temp.path().join("main");

                // Copy the rust repo to the temp location (git worktree needs the original)
                // Use git clone --local for fast copy with shared objects
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
                    "Failed to clone rust repo to workspace: {}",
                    String::from_utf8_lossy(&clone_output.stderr)
                );

                run_git(&workspace_main, &["config", "user.name", "Benchmark"]);
                run_git(&workspace_main, &["config", "user.email", "bench@test.com"]);

                // Add worktrees with realistic changes
                for i in 1..num_worktrees {
                    add_worktree_with_divergence(
                        &temp,
                        &workspace_main,
                        i,
                        10, // commits ahead
                        0,  // commits behind (skip for now)
                        3,  // uncommitted files
                    );
                }

                // Warm up git's internal caches
                run_git(&workspace_main, &["status"]);

                b.iter(|| {
                    Command::new(&binary)
                        .arg("list")
                        .current_dir(&workspace_main)
                        .output()
                        .unwrap();
                });
            },
        );
    }

    group.finish();
}

fn bench_real_repo_cold(c: &mut Criterion) {
    let mut group = c.benchmark_group("real_repo_cold");

    let binary = get_release_binary();

    // Test with fewer data points since cold cache + large repo is slower
    for num_worktrees in [1, 4, 8] {
        group.bench_with_input(
            BenchmarkId::new("rust_repo_cold", num_worktrees),
            &num_worktrees,
            |b, &num_worktrees| {
                // All expensive setup is inside the closure, so it only runs
                // when this benchmark ID matches the filter. Combined with
                // OnceLock, the rust repo is cloned at most once per process.
                let rust_repo = get_or_clone_rust_repo();

                // Create a temporary workspace for this benchmark run
                let temp = tempfile::tempdir().unwrap();
                let workspace_main = temp.path().join("main");

                // Copy the rust repo to the temp location (git worktree needs the original)
                // Use git clone --local for fast copy with shared objects
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
                    "Failed to clone rust repo to workspace: {}",
                    String::from_utf8_lossy(&clone_output.stderr)
                );

                run_git(&workspace_main, &["config", "user.name", "Benchmark"]);
                run_git(&workspace_main, &["config", "user.email", "bench@test.com"]);

                // Add worktrees with realistic changes
                for i in 1..num_worktrees {
                    add_worktree_with_divergence(
                        &temp,
                        &workspace_main,
                        i,
                        10, // commits ahead
                        0,  // commits behind (skip for now)
                        3,  // uncommitted files
                    );
                }

                let git_dir = workspace_main.join(".git");

                // Collect paths to all git caches
                let mut index_paths = vec![git_dir.join("index")];
                for i in 1..num_worktrees {
                    let wt_index = git_dir
                        .join("worktrees")
                        .join(format!("wt-{}", i))
                        .join("index");
                    index_paths.push(wt_index);
                }

                let commit_graph_dir = git_dir.join("objects").join("info");
                let commit_graph = commit_graph_dir.join("commit-graph");
                let commit_graphs_dir = commit_graph_dir.join("commit-graphs");
                let packed_refs = git_dir.join("packed-refs");

                b.iter_batched(
                    || {
                        // Setup phase - remove all git caches (not measured)
                        for index_path in &index_paths {
                            if index_path.exists() {
                                std::fs::remove_file(index_path).unwrap_or_else(|_| {
                                    panic!("Failed to remove index: {}", index_path.display())
                                });
                            }
                        }

                        if commit_graph.exists() {
                            std::fs::remove_file(&commit_graph).unwrap_or_else(|_| {
                                panic!("Failed to remove commit-graph: {}", commit_graph.display())
                            });
                        }
                        if commit_graphs_dir.exists() {
                            std::fs::remove_dir_all(&commit_graphs_dir).unwrap_or_else(|_| {
                                panic!(
                                    "Failed to remove commit-graphs: {}",
                                    commit_graphs_dir.display()
                                )
                            });
                        }

                        if packed_refs.exists() {
                            std::fs::remove_file(&packed_refs).unwrap_or_else(|_| {
                                panic!("Failed to remove packed-refs: {}", packed_refs.display())
                            });
                        }
                    },
                    |_| {
                        // Measured phase - only wt list execution
                        Command::new(&binary)
                            .arg("list")
                            .current_dir(&workspace_main)
                            .output()
                            .unwrap();
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

/// Create a repository with many branches, each with unique commits.
/// No worktrees are created - this tests `wt list --branches` performance.
fn create_repo_with_many_branches(num_branches: usize) -> TempDir {
    let temp_dir = tempfile::tempdir().unwrap();
    let repo_path = temp_dir.path().join("main");
    std::fs::create_dir(&repo_path).unwrap();

    // Initialize repository
    run_git(&repo_path, &["init", "-b", "main"]);
    run_git(&repo_path, &["config", "user.name", "Benchmark"]);
    run_git(&repo_path, &["config", "user.email", "bench@test.com"]);

    // Create initial commit on main
    let file_path = repo_path.join("README.md");
    std::fs::write(&file_path, "# Benchmark Repository\n").unwrap();
    run_git(&repo_path, &["add", "."]);
    run_git(&repo_path, &["commit", "-m", "Initial commit"]);

    // Create branches with unique commits
    for i in 0..num_branches {
        let branch_name = format!("feature-{:03}", i);

        // Create branch from main
        run_git(&repo_path, &["checkout", "-b", &branch_name, "main"]);

        // Add 1-3 unique commits per branch (varies by branch index)
        let num_commits = 1 + (i % 3);
        for j in 0..num_commits {
            let feature_file = repo_path.join(format!("feature_{:03}_{}.rs", i, j));
            let content = format!(
                "// Feature {} file {}\n\
                 pub fn feature_{}_func_{}() -> i32 {{\n\
                     {}\n\
                 }}\n",
                i,
                j,
                i,
                j,
                i * 100 + j
            );
            std::fs::write(&feature_file, content).unwrap();
            run_git(&repo_path, &["add", "."]);
            run_git(
                &repo_path,
                &[
                    "commit",
                    "-m",
                    &format!("Feature {} commit {}", branch_name, j),
                ],
            );
        }
    }

    // Return to main
    run_git(&repo_path, &["checkout", "main"]);

    temp_dir
}

/// Benchmark `wt list --branches` with many branches.
/// Tests performance scaling with 25, 50, and 100 branches (no worktrees).
fn bench_many_branches(c: &mut Criterion) {
    let mut group = c.benchmark_group("many_branches");

    let binary = get_release_binary();

    // Test with different branch counts to measure scaling
    for num_branches in [25, 50, 100] {
        let temp = create_repo_with_many_branches(num_branches);
        let repo_path = temp.path().join("main");

        // Warm up git's internal caches
        run_git(&repo_path, &["status"]);

        group.bench_with_input(
            BenchmarkId::from_parameter(num_branches),
            &num_branches,
            |b, _| {
                b.iter(|| {
                    Command::new(&binary)
                        .args(["list", "--branches", "--progressive"])
                        .current_dir(&repo_path)
                        .output()
                        .unwrap();
                });
            },
        );
    }

    group.finish();
}

/// Cold cache variant of many branches benchmark.
/// Invalidates packed-refs before each measurement to simulate first-run performance.
fn bench_many_branches_cold(c: &mut Criterion) {
    let mut group = c.benchmark_group("many_branches_cold");

    let binary = get_release_binary();

    // Test with different branch counts to measure scaling under cold cache
    for num_branches in [25, 50, 100] {
        let temp = create_repo_with_many_branches(num_branches);
        let repo_path = temp.path().join("main");

        let packed_refs = repo_path.join(".git/packed-refs");

        group.bench_with_input(
            BenchmarkId::from_parameter(num_branches),
            &num_branches,
            |b, _| {
                b.iter_batched(
                    || {
                        // Invalidate packed-refs before each iteration
                        if packed_refs.exists() {
                            std::fs::remove_file(&packed_refs).unwrap();
                        }
                    },
                    |_| {
                        Command::new(&binary)
                            .args(["list", "--branches", "--progressive"])
                            .current_dir(&repo_path)
                            .output()
                            .unwrap();
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(30)
        .measurement_time(std::time::Duration::from_secs(15))
        .warm_up_time(std::time::Duration::from_secs(3));
    targets = bench_skeleton, bench_skeleton_cold, bench_complete, bench_complete_cold, bench_worktree_scaling, bench_profile_scaling, bench_sequential_vs_parallel, bench_worktree_scaling_cold, bench_real_repo, bench_real_repo_cold, bench_many_branches, bench_many_branches_cold
}
criterion_main!(benches);
