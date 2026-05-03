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
//! let repo_path = temp.path().join("main");
//!
//! // Invalidate caches for cold benchmark
//! invalidate_caches_auto(&repo_path);
//! ```
//!
//! See `wt-perf --help` for CLI usage.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;
use tempfile::TempDir;
use worktrunk::testing::{NULL_DEVICE, configure_git_cmd};
use worktrunk::trace::{TraceEntry, TraceEntryKind, TraceResult};

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
            branches: 2, // no-worktree-1, no-worktree-2
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
/// - `packed-refs`
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

    let _ = std::fs::remove_file(git_dir.join("packed-refs"));

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
    let step = 5000 / count;
    let commits: Vec<&str> = log_str.lines().step_by(step).take(count).collect();

    for (i, commit) in commits.iter().enumerate() {
        let branch_name = format!("feature-{i:03}");
        run_git(repo_path, &["branch", &branch_name, commit]);
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

/// Render parsed entries as a column-aligned, start-time-sorted timeline.
///
/// `wall` is the externally-measured spawn → wait duration. The trace
/// can't see the prelude (argv parsing, dyld, time before `init_logging`
/// sets the trace epoch) or the exit path, so reporting `wall` lets
/// readers see how much of the process the trace actually accounts for —
/// the gap between `traced` and `wall` is the unobserved overhead.
///
/// Column alignment uses `tabwriter`'s elastic tabstops (write `\t`-separated
/// rows, padding is computed at flush). Durations are rendered via
/// `Duration`'s `Debug` impl, which produces compact units (`999µs`, `4.5ms`,
/// `1.5s`) — matches what we want without a dedicated humanization crate.
//
// Lives in the lib (rather than next to the `Timeline` subcommand in
// `main.rs`) so the inline tests below don't compile under the `bin/wt-perf`
// target. The package's `[lib]` and `[[bin]]` targets normalize to the same
// `wt_perf` artifact basename, and cargo-affected's runner shim can't
// disambiguate which target a test binary belongs to when both ship tests
// (the marker probe only fires for `kind = "test" | "bench" | "example"`).
// Keeping unit tests in the lib avoids the collision.
pub fn render_timeline(entries: &[TraceEntry], wall: Duration) -> String {
    let mut sorted: Vec<&TraceEntry> = entries.iter().collect();
    sorted.sort_by_key(|e| e.start_time_us.unwrap_or(0));

    let mut tw = tabwriter::TabWriter::new(Vec::<u8>::new())
        .minwidth(2)
        .padding(2);
    writeln!(tw, "ts(ms)\tdur\ttid\tkind\tname").unwrap();
    for e in &sorted {
        let (kind, dur, name) = describe(e);
        let ts_ms = e.start_time_us.unwrap_or(0) as f64 / 1_000.0;
        let tid = e
            .thread_id
            .map(|t| t.to_string())
            .unwrap_or_else(|| "-".into());
        writeln!(tw, "{ts_ms:.3}\t{dur:?}\t{tid}\t{kind}\t{name}").unwrap();
    }
    tw.flush().unwrap();
    let mut out = String::from_utf8(tw.into_inner().unwrap()).unwrap();

    // Summary: subprocess totals + traced span + true process wall.
    let cmds: Vec<(Duration, String)> = sorted
        .iter()
        .filter_map(|e| match &e.kind {
            TraceEntryKind::Command { duration, .. } => {
                let (_, _, name) = describe(e);
                Some((*duration, name))
            }
            _ => None,
        })
        .collect();
    let cmd_total: Duration = cmds.iter().map(|(d, _)| *d).sum();
    let slowest = cmds.iter().max_by_key(|(d, _)| *d);
    let traced = Duration::from_micros(
        sorted
            .iter()
            .map(|e| e.start_time_us.unwrap_or(0) + duration_of(e).as_micros() as u64)
            .max()
            .unwrap_or(0)
            .saturating_sub(
                sorted
                    .iter()
                    .map(|e| e.start_time_us.unwrap_or(0))
                    .min()
                    .unwrap_or(0),
            ),
    );
    let untraced = wall.saturating_sub(traced);

    out.push('\n');
    if let Some((dur, name)) = slowest {
        let plural = if cmds.len() == 1 { "" } else { "es" };
        out.push_str(&format!(
            "{} subprocess{plural} totaling {cmd_total:?} (slowest: {dur:?} {name})\n",
            cmds.len(),
        ));
    } else {
        out.push_str("0 subprocesses\n");
    }
    out.push_str(&format!(
        "traced: {traced:?} (first → last [wt-trace] record)\n"
    ));
    out.push_str(&format!(
        "wall:   {wall:?} (spawn → wait; +{untraced:?} untraced prelude/epilogue)\n"
    ));
    out
}

fn describe(e: &TraceEntry) -> (&'static str, Duration, String) {
    match &e.kind {
        TraceEntryKind::Command {
            command,
            duration,
            result,
        } => {
            let mut label = match e.context.as_deref() {
                Some(c) => format!("{command} [{c}]"),
                None => command.clone(),
            };
            match result {
                TraceResult::Completed { success: false } => label.push_str("  (ok=false)"),
                TraceResult::Error { message } => label.push_str(&format!("  (err: {message})")),
                TraceResult::Completed { success: true } => {}
            }
            ("cmd", *duration, label)
        }
        TraceEntryKind::Span { name, duration } => ("span", *duration, name.clone()),
        TraceEntryKind::Instant { name } => ("event", Duration::ZERO, name.clone()),
    }
}

fn duration_of(e: &TraceEntry) -> Duration {
    match &e.kind {
        TraceEntryKind::Command { duration, .. } | TraceEntryKind::Span { duration, .. } => {
            *duration
        }
        TraceEntryKind::Instant { .. } => Duration::ZERO,
    }
}

#[cfg(test)]
mod timeline_tests {
    use super::*;

    fn span(name: &str, ts_us: u64, dur_us: u64, tid: u64) -> TraceEntry {
        TraceEntry {
            context: None,
            kind: TraceEntryKind::Span {
                name: name.to_string(),
                duration: Duration::from_micros(dur_us),
            },
            start_time_us: Some(ts_us),
            thread_id: Some(tid),
        }
    }

    fn cmd(
        cmd: &str,
        ctx: Option<&str>,
        ts_us: u64,
        dur_us: u64,
        tid: u64,
        ok: bool,
    ) -> TraceEntry {
        TraceEntry {
            context: ctx.map(|s| s.to_string()),
            kind: TraceEntryKind::Command {
                command: cmd.to_string(),
                duration: Duration::from_micros(dur_us),
                result: TraceResult::Completed { success: ok },
            },
            start_time_us: Some(ts_us),
            thread_id: Some(tid),
        }
    }

    #[test]
    fn renders_sorted_timeline_with_summary() {
        // Emit order swaps span and child cmd (parent finishes after child),
        // so this exercises the sort-by-start-time guarantee. Durations are
        // chosen so std `Duration` Debug renders compact (no trailing
        // sub-millisecond precision): 4ms, 4.1ms, 280µs, 8µs.
        let entries = vec![
            cmd("git rev-parse HEAD", Some("repo"), 50, 4_000, 1, true),
            span("prewarm", 30, 4_100, 1),
            span("init_logging", 0, 8, 1),
            span("user_config_load", 4_200, 280, 38),
        ];
        // Wall = 6ms; traced = 4.48ms (4.2ms start → 4.48ms end);
        // untraced prelude/epilogue = 6 - 4.48 = ~1.52ms.
        insta::assert_snapshot!(
            render_timeline(&entries, Duration::from_micros(6_000)),
            @r"
        ts(ms)  dur    tid  kind  name
        0.000   8µs    1    span  init_logging
        0.030   4.1ms  1    span  prewarm
        0.050   4ms    1    cmd   git rev-parse HEAD [repo]
        4.200   280µs  38   span  user_config_load

        1 subprocess totaling 4ms (slowest: 4ms git rev-parse HEAD [repo])
        traced: 4.48ms (first → last [wt-trace] record)
        wall:   6ms (spawn → wait; +1.52ms untraced prelude/epilogue)
        "
        );
    }

    #[test]
    fn cmd_failure_annotates_name() {
        let entries = vec![cmd("git foo", None, 0, 1_000, 1, false)];
        insta::assert_snapshot!(
            render_timeline(&entries, Duration::from_millis(2)),
            @r"
        ts(ms)  dur  tid  kind  name
        0.000   1ms  1    cmd   git foo  (ok=false)

        1 subprocess totaling 1ms (slowest: 1ms git foo  (ok=false))
        traced: 1ms (first → last [wt-trace] record)
        wall:   2ms (spawn → wait; +1ms untraced prelude/epilogue)
        "
        );
    }
}
