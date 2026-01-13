//! Performance testing and tracing tools for worktrunk.
//!
//! This crate provides:
//! - Benchmark repository setup (used by `benches/list.rs`)
//! - Cache invalidation for cold benchmark runs
//! - Trace analysis utilities
//!
//! # Library Usage
//!
//! ```rust,ignore
//! use wt_perf::{RepoConfig, create_repo, invalidate_caches};
//!
//! // Create a test repo with 8 worktrees
//! let temp = create_repo(&RepoConfig::typical(8));
//! let repo_path = temp.path().join("main");
//!
//! // Invalidate caches for cold benchmark
//! invalidate_caches(&repo_path, 8);
//! ```
//!
//! # CLI Usage
//!
//! ```bash
//! # Set up a benchmark repo
//! cargo run -p wt-perf -- setup typical-8
//!
//! # Invalidate caches
//! cargo run -p wt-perf -- invalidate /path/to/repo
//!
//! # Parse trace logs
//! RUST_LOG=debug wt list 2>&1 | grep wt-trace | cargo run -p wt-perf -- trace
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use tempfile::TempDir;

/// Lazy-initialized rust repo path.
static RUST_REPO: OnceLock<PathBuf> = OnceLock::new();

/// Configuration for creating a benchmark repository.
#[derive(Clone, Debug)]
pub struct RepoConfig {
    /// Number of commits on main branch
    pub commits_on_main: usize,
    /// Number of files in the repo
    pub files: usize,
    /// Number of branches (without worktrees)
    pub branches: usize,
    /// Commits per branch
    pub commits_per_branch: usize,
    /// Number of worktrees (including main)
    pub worktrees: usize,
    /// Commits ahead of main per worktree
    pub worktree_commits_ahead: usize,
    /// Uncommitted files per worktree
    pub worktree_uncommitted_files: usize,
}

impl RepoConfig {
    /// Typical repo with worktrees (500 commits, 100 files).
    ///
    /// Good for skeleton rendering and general worktree benchmarks.
    pub const fn typical(worktrees: usize) -> Self {
        Self {
            commits_on_main: 500,
            files: 100,
            branches: 0,
            commits_per_branch: 0,
            worktrees,
            worktree_commits_ahead: 10,
            worktree_uncommitted_files: 3,
        }
    }

    /// Branch-focused config (minimal history, many branches).
    pub const fn branches(count: usize, commits_per_branch: usize) -> Self {
        Self {
            commits_on_main: 1,
            files: 1,
            branches: count,
            commits_per_branch,
            worktrees: 0,
            worktree_commits_ahead: 0,
            worktree_uncommitted_files: 0,
        }
    }

    /// Many divergent branches (GH #461 scenario: 200 branches × 20 commits).
    pub const fn many_divergent_branches() -> Self {
        Self {
            commits_on_main: 100,
            files: 50,
            branches: 200,
            commits_per_branch: 20,
            worktrees: 0,
            worktree_commits_ahead: 0,
            worktree_uncommitted_files: 0,
        }
    }

    /// Config for testing `wt select` (6 worktrees with varying commits).
    pub const fn select_test() -> Self {
        Self {
            commits_on_main: 3,
            files: 3,
            branches: 2, // no-worktree-1, no-worktree-2
            commits_per_branch: 0,
            worktrees: 6,
            worktree_commits_ahead: 15, // feature worktree has many commits
            worktree_uncommitted_files: 1,
        }
    }
}

/// Run a git command in the given directory.
fn run_git(path: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
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

/// Create a test repository from config.
///
/// Returns a `TempDir` containing the repo. The main worktree is at `temp.path().join("repo")`.
/// Additional worktrees are siblings: `temp.path().join("repo.feature-wt-N")`.
pub fn create_repo(config: &RepoConfig) -> TempDir {
    let temp_dir = tempfile::tempdir().unwrap();
    create_repo_at(config, &temp_dir.path().join("repo"));
    temp_dir
}

/// Create a test repository at a specific path.
///
/// Uses worktrunk naming convention:
/// - Main worktree: `base_path`
/// - Feature worktrees: `base_path.feature-wt-N` (siblings in parent directory)
pub fn create_repo_at(config: &RepoConfig, base_path: &Path) {
    let repo_path = base_path.to_path_buf();
    std::fs::create_dir_all(&repo_path).unwrap();

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

    // Create branches (without worktrees)
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
        let wt_path = base_path.join(format!("wt-{wt_num}"));

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

    // Set up fake remote for default branch detection
    setup_fake_remote(&repo_path);
}

/// Set up a fake remote for default branch detection.
pub fn setup_fake_remote(repo_path: &Path) {
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

/// Invalidate git caches for cold benchmarks.
///
/// Removes:
/// - Index files (main + worktrees)
/// - Commit graph
/// - Packed refs
pub fn invalidate_caches(repo_path: &Path, num_worktrees: usize) {
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

/// Invalidate caches for any repo (auto-detects worktrees).
pub fn invalidate_caches_auto(repo_path: &Path) {
    let git_dir = repo_path.join(".git");

    // Remove main index
    let _ = std::fs::remove_file(git_dir.join("index"));

    // Remove all worktree indexes
    let worktrees_dir = git_dir.join("worktrees");
    if worktrees_dir.exists()
        && let Ok(entries) = std::fs::read_dir(&worktrees_dir)
    {
        for entry in entries.flatten() {
            let index = entry.path().join("index");
            let _ = std::fs::remove_file(index);
        }
    }

    // Remove commit graph
    let _ = std::fs::remove_file(git_dir.join("objects/info/commit-graph"));
    let _ = std::fs::remove_dir_all(git_dir.join("objects/info/commit-graphs"));

    // Remove packed refs
    let _ = std::fs::remove_file(git_dir.join("packed-refs"));
}

/// Get or clone the rust-lang/rust repository for real-world benchmarks.
///
/// The repo is cached at `target/bench-repos/rust` and reused across runs.
pub fn ensure_rust_repo() -> PathBuf {
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
                    eprintln!("Using cached rust repo at {}", rust_repo.display());
                    return rust_repo;
                }
                eprintln!("Cached rust repo corrupted, re-cloning...");
                std::fs::remove_dir_all(&rust_repo).unwrap();
            }

            std::fs::create_dir_all(&cache_dir).unwrap();
            eprintln!("Cloning rust-lang/rust (this will take several minutes)...");

            let clone_output = Command::new("git")
                .args([
                    "clone",
                    "https://github.com/rust-lang/rust.git",
                    rust_repo.to_str().unwrap(),
                ])
                .output()
                .unwrap();

            assert!(clone_output.status.success(), "Failed to clone rust repo");
            eprintln!("Rust repo cloned successfully");
            rust_repo
        })
        .clone()
}

/// Canonicalize path without Windows `\\?\` prefix.
pub fn canonicalize(path: &Path) -> std::io::Result<PathBuf> {
    dunce::canonicalize(path)
}

/// Parse a config string into a RepoConfig.
///
/// Supported formats:
/// - `typical-N` - typical repo with N worktrees
/// - `branches-N` - N branches with 1 commit each
/// - `branches-N-M` - N branches with M commits each
/// - `divergent` - many divergent branches (GH #461)
/// - `select-test` - config for wt select testing
pub fn parse_config(s: &str) -> Option<RepoConfig> {
    if let Some(n) = s.strip_prefix("typical-") {
        let worktrees: usize = n.parse().ok()?;
        return Some(RepoConfig::typical(worktrees));
    }

    if let Some(rest) = s.strip_prefix("branches-") {
        let parts: Vec<&str> = rest.split('-').collect();
        match parts.as_slice() {
            [count] => {
                let count: usize = count.parse().ok()?;
                return Some(RepoConfig::branches(count, 1));
            }
            [count, commits] => {
                let count: usize = count.parse().ok()?;
                let commits: usize = commits.parse().ok()?;
                return Some(RepoConfig::branches(count, commits));
            }
            _ => return None,
        }
    }

    match s {
        "divergent" => Some(RepoConfig::many_divergent_branches()),
        "select-test" => Some(RepoConfig::select_test()),
        _ => None,
    }
}
