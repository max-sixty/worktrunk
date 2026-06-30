//! Performance testing and tracing tools for worktrunk.
//!
//! This crate provides:
//! - Benchmark repository setup (used by `benches/list.rs`, `benches/time_to_first_output.rs`)
//! - Cache invalidation for cold benchmark runs
//! - Trace analysis utilities
//! - Shared benchmark helpers (`run_git`, `run_git_ok`, …)
//!
//! For wt-subprocess isolation, benches use
//! [`worktrunk::testing::isolate_subprocess_env`] directly.
//!
//! # Library Usage
//!
//! ```rust,ignore
//! use wt_perf::{RepoConfig, create_repo, invalidate_caches_auto};
//!
//! // Create a test repo with 8 worktrees
//! let temp = create_repo(&RepoConfig::typical(8));
//! let repo_path = temp.path().join("repo");
//!
//! // Invalidate caches for cold benchmark
//! invalidate_caches_auto(&repo_path);
//! ```
//!
//! See `wt-perf --help` for CLI usage.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use tempfile::TempDir;
use worktrunk::testing::{NULL_DEVICE, configure_git_cmd};

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

    /// Config for testing `wt switch` interactive picker (6 worktrees with varying commits).
    pub const fn picker_test() -> Self {
        Self {
            commits_on_main: 3,
            files: 3,
            branches: 2, // feature-000, feature-001 (no worktree)
            commits_per_branch: 0,
            worktrees: 6,
            worktree_commits_ahead: 15, // feature worktree has many commits
            worktree_uncommitted_files: 1,
        }
    }
}

/// Build a `git` command isolated from host context, with config
/// redirected to `NULL_DEVICE`. Thin call-site wrapper around
/// [`configure_git_cmd`] — every git invocation in this crate goes
/// through here. Doesn't set `current_dir`; callers do that explicitly
/// when they have a target.
fn git_command() -> Command {
    let mut cmd = Command::new("git");
    configure_git_cmd(&mut cmd, Path::new(NULL_DEVICE));
    cmd
}

/// Run a git command in the given directory. Panics on failure.
pub fn run_git(path: &Path, args: &[&str]) {
    let output = git_command().args(args).current_dir(path).output().unwrap();
    assert!(
        output.status.success(),
        "Git command failed: {:?}\nstderr: {}\nstdout: {}\npath: {}",
        args,
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout),
        path.display()
    );
}

/// Run a git command, returning whether it succeeded. Does not panic.
pub fn run_git_ok(path: &Path, args: &[&str]) -> bool {
    git_command()
        .args(args)
        .current_dir(path)
        .output()
        .is_ok_and(|o| o.status.success())
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
    // Disable all background auto-maintenance: rapid commits in the
    // build loop trigger detached `git gc` / `git maintenance` runs
    // whose pack-and-prune steps race the foreground `git add` /
    // `git commit`, producing intermittent "invalid object ..." /
    // "unable to create temporary file" / "failed to insert into
    // database" failures partway through a 500-commit fixture. Modern
    // git enables both `gc.auto` (loose-object threshold) and
    // `maintenance.auto` (the post-command hook scheduler) by default,
    // so we have to silence both.
    run_git(&repo_path, &["config", "gc.auto", "0"]);
    run_git(&repo_path, &["config", "gc.autoPackLimit", "0"]);
    run_git(&repo_path, &["config", "maintenance.auto", "false"]);

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

    add_worktrees(config, &repo_path);

    // Set up fake remote for default branch detection
    setup_fake_remote(&repo_path);

    // Pack objects and write the commit-graph once, after all refs
    // exist. Auto-maintenance is disabled (see above), so we do this
    // explicitly — the goal is a mature-repo shape: one packfile, a
    // commit-graph, no loose-object lookup overhead. Without this,
    // benches measure cold-clone-shaped repos, which exaggerates
    // per-object I/O cost relative to what users see on day-N repos.
    run_git(&repo_path, &["gc"]);
}

/// Add worktrees to an existing repo using worktrunk naming convention.
///
/// Creates `config.worktrees - 1` linked worktrees as siblings of `repo_path`
/// (e.g., `repo.feature-wt-1`), each with diverging commits and uncommitted files
/// controlled by `config.worktree_commits_ahead` and `config.worktree_uncommitted_files`.
pub fn add_worktrees(config: &RepoConfig, repo_path: &Path) {
    let repo_name = repo_path.file_name().unwrap().to_str().unwrap();
    let parent_dir = repo_path.parent().unwrap();

    for wt_num in 1..config.worktrees {
        let branch = format!("feature-wt-{wt_num}");
        let wt_path = parent_dir.join(format!("{repo_name}.{branch}"));

        let head_output = git_command()
            .args(["rev-parse", "HEAD"])
            .current_dir(repo_path)
            .output()
            .unwrap();
        let base_commit = String::from_utf8_lossy(&head_output.stdout)
            .trim()
            .to_string();

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

        for i in 0..config.worktree_commits_ahead {
            let file_path = wt_path.join(format!("feature_{wt_num}_file_{i}.txt"));
            std::fs::write(&file_path, format!("Feature {wt_num} content {i}\n")).unwrap();
            run_git(&wt_path, &["add", "."]);
            run_git(
                &wt_path,
                &["commit", "-m", &format!("Feature {wt_num} commit {i}")],
            );
        }

        for i in 0..config.worktree_uncommitted_files {
            let file_path = wt_path.join(format!("uncommitted_{i}.txt"));
            std::fs::write(&file_path, "Uncommitted content\n").unwrap();
        }
    }
}

/// Set up a fake remote for default branch detection.
pub fn setup_fake_remote(repo_path: &Path) {
    let refs_dir = repo_path.join(".git/refs/remotes/origin");
    std::fs::create_dir_all(&refs_dir).unwrap();
    std::fs::write(refs_dir.join("HEAD"), "ref: refs/remotes/origin/main\n").unwrap();
    let head_sha = git_command()
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_path)
        .output()
        .unwrap();
    std::fs::write(refs_dir.join("main"), head_sha.stdout).unwrap();
}

/// Invalidate caches for any repo (auto-detects worktrees).
///
/// Resolves the git common directory from `repo_path/.git` — handling
/// linked worktrees, where `.git` is a file holding a gitdir pointer
/// rather than a directory — so the same cache is cleared regardless
/// of which worktree of a repo `repo_path` names.
///
/// Clears:
/// - Git's index (main + linked worktrees) — fsmonitor/stat warmup
/// - Commit graph (`objects/info/commit-graph*`)
/// - All of `.git/wt/cache/` — worktrunk's persistent SHA-keyed caches
///   (merge-tree-conflicts, merge-add-probe, is-ancestor, has-added-changes,
///   diff-stats) plus sibling caches (ci-status, summaries)
/// - `worktrunk.default-branch` in git config — worktrunk's cache of the
///   default branch name (repopulated on next `wt` invocation via
///   `origin/HEAD` or `git ls-remote`)
///
/// Does NOT clear user-modifiable state: `worktrunk.history`,
/// `worktrunk.hints.*`, `worktrunk.state.<branch>.*`, `.git/wt/logs/`,
/// `.git/wt/trash/`. These don't affect read-path performance, and benches
/// may rely on them (e.g., branch markers set during setup).
pub fn invalidate_caches_auto(repo_path: &Path) {
    let Some(git_dir) = resolve_git_common_dir(repo_path) else {
        return;
    };

    // Remove main index + every linked worktree's index.
    let _ = std::fs::remove_file(git_dir.join("index"));
    if let Ok(entries) = std::fs::read_dir(git_dir.join("worktrees")) {
        for entry in entries.flatten() {
            let _ = std::fs::remove_file(entry.path().join("index"));
        }
    }

    // Commit graph: legacy single-file plus chained-graph dir.
    let _ = std::fs::remove_file(git_dir.join("objects/info/commit-graph"));
    let _ = std::fs::remove_dir_all(git_dir.join("objects/info/commit-graphs"));

    // Note: `packed-refs` is intentionally NOT removed. After `create_repo_at`
    // runs an explicit `git gc`, every loose ref under `refs/heads/`,
    // `refs/remotes/`, etc. is packed into `packed-refs` and the loose files
    // are pruned. Deleting `packed-refs` in that state leaves the repo with
    // no resolvable refs — `rev-parse main` fails, and any bench that reads
    // through a branch (e.g. the `with_vars` alias's `{{ commit }}` template
    // var) blows up with a template-expansion error. The file is git's
    // primary ref storage post-gc, not a cache, so there's no cold-state to
    // simulate by deleting it.

    // All worktrunk persistent caches: every kind dir under wt/cache/.
    let _ = std::fs::remove_dir_all(git_dir.join("wt/cache"));

    // Worktrunk's default-branch cache lives in git config; we have no
    // safe way to edit that file ourselves (escaping rules), so shell
    // out. Exit 5 = key absent (harmless); anything else is a real
    // failure and we want it loud, since the bench's cold-cache
    // invariant depends on this succeeding.
    let result = git_command()
        .args(["config", "--unset", "worktrunk.default-branch"])
        .current_dir(repo_path)
        .output();
    match result {
        Ok(o) if o.status.success() => {}
        Ok(o) if o.status.code() == Some(5) => {}
        Ok(o) => eprintln!(
            "wt-perf invalidate: `git config --unset worktrunk.default-branch` failed (exit {:?}): {}",
            o.status.code(),
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => eprintln!("wt-perf invalidate: failed to spawn git: {e}"),
    }
}

/// Resolve git's common directory for `repo_path` from the filesystem.
///
/// - Normal repo: `<repo>/.git` is a directory — use it directly.
/// - Linked worktree: `<repo>/.git` is a file containing
///   `gitdir: <main>/.git/worktrees/<name>`. The common dir is the
///   parent of that worktree-private dir's parent.
///
/// Returns `None` for bare repos (no `.git` entry) or non-repo paths;
/// the caller treats that as "nothing to invalidate."
fn resolve_git_common_dir(repo_path: &Path) -> Option<PathBuf> {
    let dot_git = repo_path.join(".git");
    let file_type = std::fs::symlink_metadata(&dot_git).ok()?.file_type();

    if file_type.is_dir() {
        return Some(dot_git);
    }
    if !file_type.is_file() {
        return None;
    }

    // `.git` is a gitdir pointer: `gitdir: <path>` (path may be relative
    // to repo_path). Strip `worktrees/<name>` to reach the common dir.
    let content = std::fs::read_to_string(&dot_git).ok()?;
    let gitdir = content.lines().find_map(|l| l.strip_prefix("gitdir: "))?;
    let pointed = PathBuf::from(gitdir.trim());
    let pointed = if pointed.is_absolute() {
        pointed
    } else {
        repo_path.join(pointed)
    };
    pointed.parent()?.parent().map(Path::to_path_buf)
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
                let output = git_command()
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

            let clone_output = git_command()
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

/// Clone rust-lang/rust into `temp/repo` for benchmarking.
///
/// Returns the clone path. Configures git user for commits.
/// The `temp` dir must outlive usage.
pub fn clone_rust_repo(temp: &TempDir) -> PathBuf {
    let rust_repo = ensure_rust_repo();
    let workspace_main = temp.path().join("repo");

    let clone_output = git_command()
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

    workspace_main
}

/// Create branches pointing at different depths in the repo's commit history.
///
/// Samples `count` commits evenly spread across the last 5000 commits and
/// creates `feature-NNN` branches pointing at them. This reproduces the
/// GH #461 scenario where branch divergence depth (not count) drives cost.
pub fn add_history_spread_branches(repo_path: &Path, count: usize) {
    let log_output = git_command()
        .args(["log", "--oneline", "-n", "5000", "--format=%H"])
        .current_dir(repo_path)
        .output()
        .unwrap();
    let log_str = String::from_utf8_lossy(&log_output.stdout);
    // Guard the degenerate inputs: `count == 0` would divide by zero, and
    // `count > 5000` would yield `step == 0`, which panics `step_by`. Both
    // `max(1)`s preserve the spread for every in-range count.
    let step = (5000 / count.max(1)).max(1);
    let commits: Vec<&str> = log_str.lines().step_by(step).take(count).collect();

    for (i, commit) in commits.iter().enumerate() {
        let branch_name = format!("feature-{i:03}");
        run_git(repo_path, &["branch", &branch_name, commit]);
    }
}

/// `git rev-parse HEAD` in `path`, trimmed.
fn head_sha(path: &Path) -> String {
    let out = git_command()
        .args(["rev-parse", "HEAD"])
        .current_dir(path)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Append a line to a tracked file (creating it if missing). Used to make
/// working-tree edits in the mixed-state fixture.
fn append_line(path: &Path, rel: &str, line: &str) {
    let file = path.join(rel);
    let mut content = std::fs::read_to_string(&file).unwrap_or_default();
    content.push_str(line);
    content.push('\n');
    std::fs::write(&file, content).unwrap();
}

/// Create a repo with `worktrees` linked worktrees AND `branches` branchless
/// branches, each in a deterministic rotation of states, for the warm-cache
/// `wt list` re-run benchmark.
///
/// Unlike [`RepoConfig`] (every worktree/branch identical), this exercises the
/// full spread of `wt list` gates and tasks — clean vs dirty working trees,
/// merged vs ahead vs diverged branches — which is the realistic shape of "a
/// huge number of worktrees & branches, all in various states". Returns the
/// `TempDir`; the main worktree is at `temp.path().join("repo")`, linked
/// worktrees are siblings (`repo.wt-NNNN`).
///
/// Worktree states cycle by index % 4:
/// 0. clean, several commits ahead of base
/// 1. unstaged modification (dirty working tree)
/// 2. staged + unstaged + untracked (full dirty mix)
/// 3. clean, sitting exactly at base
///
/// Branch states cycle by index % 4:
/// 0. at an older checkpoint (ancestor of base — integration-positive)
/// 1. ahead of base with its own commits (unmerged)
/// 2. diverged: own commits from an older checkpoint while base advanced
/// 3. identical to the base tip (trees match — squash-merge shape)
pub fn create_mixed_repo(worktrees: usize, branches: usize) -> TempDir {
    let temp = tempfile::tempdir().unwrap();
    create_mixed_repo_at(worktrees, branches, &temp.path().join("repo"));
    temp
}

/// [`create_mixed_repo`] at a caller-chosen path (used by `wt-perf setup
/// mixed-W-B`). The main worktree is created at `repo`; linked worktrees are
/// siblings.
pub fn create_mixed_repo_at(worktrees: usize, branches: usize, repo: &Path) {
    const FILES: usize = 50;
    const BASE_COMMITS: usize = 120;

    let repo = repo.to_path_buf();
    std::fs::create_dir_all(&repo).unwrap();

    run_git(&repo, &["init", "-b", "main"]);
    run_git(&repo, &["config", "user.name", "Benchmark"]);
    run_git(&repo, &["config", "user.email", "bench@test.com"]);
    // Disable background auto-maintenance (see create_repo_at for why).
    run_git(&repo, &["config", "gc.auto", "0"]);
    run_git(&repo, &["config", "gc.autoPackLimit", "0"]);
    run_git(&repo, &["config", "maintenance.auto", "false"]);

    for i in 0..FILES {
        let p = repo.join(format!("src/file_{i}.rs"));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(
            &p,
            format!("// file {i}\npub fn f_{i}() -> i32 {{ {i} }}\n"),
        )
        .unwrap();
    }
    run_git(&repo, &["add", "."]);
    run_git(&repo, &["commit", "-q", "-m", "Initial commit"]);

    // Build base history, recording checkpoints for "behind"/"diverged" branches.
    let mut checkpoints = vec![head_sha(&repo)];
    for c in 1..BASE_COMMITS {
        append_line(
            &repo,
            &format!("src/file_{}.rs", c % FILES),
            &format!("pub fn f_{c}() {{}}"),
        );
        run_git(&repo, &["add", "."]);
        run_git(&repo, &["commit", "-q", "-m", &format!("Commit {c}")]);
        if c % 20 == 0 {
            checkpoints.push(head_sha(&repo));
        }
    }
    let base_tip = head_sha(&repo);

    // Branches without worktrees, in varied states. States 1 and 2 check out
    // in the main worktree and return to main; the loop always ends on main.
    for i in 0..branches {
        let name = format!("br-{i:04}");
        match i % 4 {
            0 => run_git(
                &repo,
                &["branch", &name, &checkpoints[i % checkpoints.len()]],
            ),
            1 => {
                run_git(&repo, &["checkout", "-q", "-b", &name, &base_tip]);
                for j in 0..=(i % 3) {
                    std::fs::write(repo.join(format!("br_{i}_{j}.rs")), format!("// {i}/{j}\n"))
                        .unwrap();
                    run_git(&repo, &["add", "."]);
                    run_git(
                        &repo,
                        &["commit", "-q", "-m", &format!("br {i} commit {j}")],
                    );
                }
                run_git(&repo, &["checkout", "-q", "main"]);
            }
            2 => {
                run_git(
                    &repo,
                    &[
                        "checkout",
                        "-q",
                        "-b",
                        &name,
                        &checkpoints[i % checkpoints.len()],
                    ],
                );
                std::fs::write(
                    repo.join(format!("br_{i}_d.rs")),
                    format!("// diverge {i}\n"),
                )
                .unwrap();
                run_git(&repo, &["add", "."]);
                run_git(&repo, &["commit", "-q", "-m", &format!("br {i} diverge")]);
                run_git(&repo, &["checkout", "-q", "main"]);
            }
            _ => run_git(&repo, &["branch", &name, &base_tip]),
        }
    }

    // Mature-repo shape: pack refs and write the commit-graph once, after every
    // branch ref exists but before the worktrees (freshly added worktrees carry
    // loose refs and uncommitted state — realistic, and keeps gc away from the
    // dirty indexes below).
    setup_fake_remote(&repo);
    run_git(&repo, &["gc", "-q"]);

    // Linked worktrees are siblings named `<repo-dir>.<branch>` (worktrunk
    // convention), derived from the repo's own directory name so the path is
    // correct whether the repo is the tempdir's `repo` or a custom `setup` path.
    let parent = repo.parent().unwrap();
    let repo_name = repo.file_name().unwrap().to_str().unwrap().to_string();
    for j in 0..worktrees {
        let branch = format!("wt-{j:04}");
        let wt = parent.join(format!("{repo_name}.{branch}"));
        run_git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &branch,
                wt.to_str().unwrap(),
                &base_tip,
            ],
        );
        match j % 4 {
            0 => {
                for k in 0..=(1 + j % 3) {
                    std::fs::write(wt.join(format!("wt_{j}_{k}.txt")), format!("wt {j}/{k}\n"))
                        .unwrap();
                    run_git(&wt, &["add", "."]);
                    run_git(&wt, &["commit", "-q", "-m", &format!("wt {j} commit {k}")]);
                }
            }
            1 => append_line(&wt, "src/file_0.rs", &format!("// unstaged edit {j}")),
            2 => {
                append_line(&wt, "src/file_1.rs", &format!("// staged edit {j}"));
                run_git(&wt, &["add", "src/file_1.rs"]);
                append_line(&wt, "src/file_2.rs", &format!("// unstaged edit {j}"));
                std::fs::write(wt.join(format!("untracked_{j}.txt")), "untracked\n").unwrap();
            }
            _ => {}
        }
    }
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
/// - `picker-test` - config for wt switch interactive picker testing
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
        "picker-test" => Some(RepoConfig::picker_test()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: `create_repo_at` ends with `git gc`, which packs every loose
    /// ref into `.git/packed-refs` and prunes the loose copies. A prior version
    /// of `invalidate_caches_auto` deleted `packed-refs`, which after gc was
    /// the only copy of `refs/heads/main` — leaving the repo with no resolvable
    /// refs and breaking the `with_vars` alias bench at `dispatch/with_vars/*`.
    #[test]
    fn invalidate_preserves_refs_after_gc() {
        let temp = create_repo(&RepoConfig {
            commits_on_main: 1,
            files: 1,
            branches: 0,
            commits_per_branch: 0,
            worktrees: 1,
            worktree_commits_ahead: 0,
            worktree_uncommitted_files: 0,
        });
        let repo_path = temp.path().join("repo");

        let rev_parse_main = || {
            git_command()
                .args(["rev-parse", "main"])
                .current_dir(&repo_path)
                .output()
                .unwrap()
        };

        let before = rev_parse_main();
        assert!(
            before.status.success(),
            "setup precondition: `rev-parse main` succeeds"
        );

        invalidate_caches_auto(&repo_path);

        let after = rev_parse_main();
        assert!(
            after.status.success(),
            "`refs/heads/main` must survive `invalidate_caches_auto` (stderr: {})",
            String::from_utf8_lossy(&after.stderr)
        );
        assert_eq!(before.stdout, after.stdout);
    }

    /// Regression: degenerate `count` values must not panic. `count == 0`
    /// divided into `5000`, and `count > 5000` flooring `step` to 0 for
    /// `step_by`, both panicked before the `max(1)` guards.
    #[test]
    fn history_spread_handles_degenerate_counts() {
        let temp = create_repo(&RepoConfig {
            commits_on_main: 3,
            files: 1,
            branches: 0,
            commits_per_branch: 0,
            worktrees: 1,
            worktree_commits_ahead: 0,
            worktree_uncommitted_files: 0,
        });
        let repo_path = temp.path().join("repo");

        // count == 0: no branches created, no divide-by-zero.
        add_history_spread_branches(&repo_path, 0);
        // count far above the 5000 log cap: step floors to 0 without the guard.
        add_history_spread_branches(&repo_path, 6000);
    }
}
