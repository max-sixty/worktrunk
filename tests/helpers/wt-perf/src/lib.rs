//! Performance testing and tracing tools for worktrunk.
//!
//! This crate provides:
//! - Benchmark repository setup (shared by all subprocess benchmarks)
//! - Cache invalidation for cold benchmark runs
//! - Trace analysis utilities
//! - Shared benchmark helpers (`bench_wt`, `wt_command`, `run_git`, …)
//!
//! # Library Usage
//!
//! ```rust,ignore
//! use wt_perf::{FixtureRecipe, invalidate_caches_auto};
//!
//! // Create the generated repo with 8 total worktrees.
//! let fixture = FixtureRecipe::generated(7).create();
//!
//! // Invalidate caches for cold benchmark
//! invalidate_caches_auto(fixture.path());
//! ```
//!
//! See `wt-perf --help` for CLI usage.

use clap::Subcommand;
use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;
use worktrunk::testing::{allow_network_transports, configure_git_cmd, isolate_subprocess_env};

mod imported_fixture {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../benches/imported-fixture"
    ));
}
use imported_fixture::{CORPUS as IMPORTED_CORPUS, REVISION as IMPORTED_REVISION};
/// Worktrees in the canonical imported fixture, including
/// the primary worktree.
const IMPORTED_TOTAL_WORKTREES: usize = 8;
/// Branches in the canonical imported fixture, spread over
/// the most recent 5,000 commits.
const IMPORTED_HISTORY_SPREAD_BRANCHES: usize = 50;
const GENERATED_DEFAULT_BRANCHES: usize = 50;

/// An owned temporary benchmark fixture.
///
/// Every ephemeral fixture has the same layout: the primary worktree is
/// `<root>/repo`, and linked worktrees are siblings named
/// `<root>/repo.<branch>`. Keeping the [`TempDir`] and canonical paths in one
/// value prevents benches from each re-deriving the layout (and accidentally
/// dropping the tempdir while a path into it is still in use).
pub struct FixtureRepo {
    root: TempDir,
    repo: PathBuf,
}

impl FixtureRepo {
    /// Create a fixture with its primary worktree at `<temp>/repo`.
    fn create(build: impl FnOnce(&Path)) -> Self {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        build(&repo);
        Self { root, repo }
    }

    /// Create a fixture in `parent`, keeping large temporary fixtures on the
    /// same volume as the shared source cache.
    fn create_in(parent: &Path, build: impl FnOnce(&Path)) -> Self {
        std::fs::create_dir_all(parent).unwrap_or_else(|error| {
            panic!(
                "failed to create fixture directory {}: {error}",
                parent.display()
            )
        });
        let root = tempfile::Builder::new()
            .prefix("fixture-")
            .tempdir_in(parent)
            .unwrap_or_else(|error| {
                panic!(
                    "failed to create temporary fixture in {}: {error}",
                    parent.display()
                )
            });
        let repo = root.path().join("repo");
        build(&repo);
        Self { root, repo }
    }

    /// Path to the fixture's primary worktree.
    pub fn path(&self) -> &Path {
        &self.repo
    }

    /// Root containing the primary and linked worktrees.
    pub fn root(&self) -> &Path {
        self.root.path()
    }

    /// Path to the linked worktree for `branch`.
    pub fn worktree_path(&self, branch: &str) -> PathBuf {
        linked_worktree_path(&self.repo, branch)
    }
}

/// Derive worktrunk's sibling path for a linked worktree.
pub fn linked_worktree_path(repo_path: &Path, branch: &str) -> PathBuf {
    let repo_name = repo_path.file_name().unwrap().to_str().unwrap();
    repo_path
        .parent()
        .unwrap()
        .join(format!("{repo_name}.{branch}"))
}

/// Low-level parameters for the simple generated repository builder.
///
/// Unit tests use this private builder for focused repository states without
/// adding them to the benchmark fixture catalog.
#[cfg(test)]
struct SimpleRepoConfig {
    /// Number of commits on main branch
    commits_on_main: usize,
    /// Number of files in the repo
    files: usize,
    /// Number of worktrees (including main)
    total_worktrees: usize,
    /// Commits ahead of main per worktree
    worktree_commits_ahead: usize,
    /// Uncommitted files per worktree
    worktree_uncommitted_files: usize,
}

/// The benchmark fixture catalog.
///
/// Variants name corpus provenance. Both build ordinary Git repositories;
/// worktree, branch, and remote-ref counts are population dimensions within a
/// base, and prune candidates are added afterward.
#[derive(Clone, Copy, Subcommand)]
pub enum FixtureRecipe {
    /// Locally generated corpus with heterogeneous branch and worktree state.
    Generated {
        /// Linked worktrees, excluding the primary worktree.
        #[arg(default_value_t = 7)]
        linked_worktrees: usize,
        /// Branches without linked worktrees.
        #[arg(default_value_t = GENERATED_DEFAULT_BRANCHES)]
        branchless_branches: usize,
        /// Additional remote-tracking refs.
        #[arg(default_value_t = 0)]
        remote_tracking_refs: usize,
    },
    /// Pinned rust-lang/rust corpus with heterogeneous history-spread state.
    Imported,
}

impl FixtureRecipe {
    /// The standard generated state: history-spread branches remain constant
    /// while a benchmark varies only the linked-worktree population.
    pub const fn generated(linked_worktrees: usize) -> Self {
        Self::Generated {
            linked_worktrees,
            branchless_branches: GENERATED_DEFAULT_BRANCHES,
            remote_tracking_refs: 0,
        }
    }

    /// Build an owned ephemeral fixture.
    pub fn create(self) -> FixtureRepo {
        let build = |repo: &Path| self.create_at(repo);
        if matches!(self, Self::Imported) {
            FixtureRepo::create_in(&imported_cache_dir().join("runs"), build)
        } else {
            FixtureRepo::create(build)
        }
    }

    /// Build an ephemeral fixture at a caller-chosen primary-worktree path.
    pub fn create_at(self, base_path: &Path) {
        match self {
            Self::Generated {
                linked_worktrees,
                branchless_branches,
                remote_tracking_refs,
            } => {
                build_generated_repo_at(
                    linked_worktrees,
                    branchless_branches,
                    remote_tracking_refs,
                    base_path,
                );
            }
            Self::Imported => {
                clone_imported_at(base_path);
                add_history_spread_branches(base_path, IMPORTED_HISTORY_SPREAD_BRANCHES);
                add_imported_linked_worktrees(base_path, IMPORTED_TOTAL_WORKTREES - 1);
            }
        }
    }
}

/// Add the canonical heterogeneous linked-worktree rotation.
///
/// Destructive benchmarks use this after committing scenario-specific
/// configuration on the primary worktree but before recording a candidate.
pub fn add_heterogeneous_linked_worktrees(repo_path: &Path, linked_worktrees: usize) {
    let base_tip = head_sha(repo_path);
    add_heterogeneous_worktrees(repo_path, linked_worktrees, &base_tip);
}

/// Add the heterogeneous imported worktree population without leaving a clean
/// worktree integrated into the default branch. The generated corpus retains
/// that state; the imported prune overlay needs its candidate count exact, so
/// the clean base-tip worktrees get a commit of their own. The dirty base-tip
/// states stay integrated by ref and are excluded from the count only because
/// `wt step prune` skips worktrees with working-tree changes.
fn add_imported_linked_worktrees(repo_path: &Path, linked_worktrees: usize) {
    add_heterogeneous_linked_worktrees(repo_path, linked_worktrees);
    for i in (3..linked_worktrees).step_by(4) {
        let worktree = linked_worktree_path(repo_path, &format!("wt-{i:04}"));
        let marker = format!("imported_wt_{i}.txt");
        std::fs::write(worktree.join(&marker), format!("imported worktree {i}\n")).unwrap();
        run_git(&worktree, &["add", &marker]);
        run_git(
            &worktree,
            &["commit", "-m", &format!("Advance imported worktree {i}")],
        );
    }
}

/// Build a `git` command isolated from host context, with the host's
/// config denied by the hermetic floor. Thin call-site wrapper around
/// [`configure_git_cmd`] — every git invocation in this crate goes
/// through here. Doesn't set `current_dir`; callers do that explicitly
/// when they have a target. Network transports are denied; the upstream
/// fixture clone re-permits them via [`allow_network_transports`].
fn git_command() -> Command {
    let mut cmd = Command::new("git");
    configure_git_cmd(&mut cmd);
    cmd
}

/// Cache state a [`bench_wt`] iteration starts from.
#[derive(Clone, Copy)]
pub enum CacheState {
    /// Persistent caches stay hot across iterations (steady-state re-run).
    Warm,
    /// [`invalidate_caches_auto`] per iteration: git's commit graph plus
    /// worktrunk's caches — a first run against fresh, equivalent repo state.
    Cold,
    /// [`invalidate_probe_caches`] per iteration: only `.git/wt/cache/` —
    /// the first scan after new commits land, git's state staying warm.
    ProbeCold,
}

impl CacheState {
    /// The warm/cold pair used by benchmark groups that cover both states.
    pub const WARM_AND_COLD: [Self; 2] = [Self::Warm, Self::Cold];

    /// Stable Criterion label for this cache state.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Warm => "warm",
            Self::Cold => "cold",
            Self::ProbeCold => "probe_cold",
        }
    }
}

/// Criterion profile for the standard subprocess benchmark cadence.
pub fn standard_benchmark_profile() -> criterion::Criterion {
    criterion::Criterion::default()
        .sample_size(30)
        .measurement_time(std::time::Duration::from_secs(15))
        .warm_up_time(std::time::Duration::from_secs(3))
}

/// Build a `wt` command with the benchmark subprocess environment isolated
/// from the developer's git, shell, and worktrunk configuration.
pub fn wt_command(binary: &Path, repo_path: &Path, user_config: Option<&Path>) -> Command {
    let mut cmd = Command::new(binary);
    cmd.current_dir(repo_path);
    isolate_subprocess_env(&mut cmd, user_config);
    cmd
}

/// Run a `wt` benchmark iteration function under criterion, warm or cold.
///
/// The one place the warm/cold iteration strategy lives: warm uses plain
/// `Bencher::iter` (persistent caches stay hot across iterations); the cold
/// states invalidate the repo's caches immediately before every measured
/// iteration — see [`CacheState`] for what each clears.
///
/// Cold uses `iter_batched` with `BatchSize::PerIteration`, not `SmallInput`:
/// under `SmallInput`, criterion calls the setup once for an entire batch up
/// front and then times the routines back-to-back, so only the first run per
/// batch is actually cold — the rest hit a `.git/wt/cache/` the previous run
/// just repopulated, biasing the "cold" median warm. `PerIteration` runs
/// setup → time(routine) per iteration, so every measured run is genuinely
/// cold; the invalidation is far cheaper than a `wt` subprocess, so the
/// per-iteration `Instant::now` overhead doesn't dominate. When this fix
/// landed, cold variance tightened (e.g. `first_output/remove` spread
/// 2.4ms → 0.65ms) and medians rose to their true cold cost.
///
/// `make_cmd` builds a fresh command per iteration; the child's exit status is
/// asserted so a benchmark can't silently time a failing command. That status
/// check is the only per-iteration validation — anything stronger (fixture
/// shape, candidate counts) belongs in a one-time check outside the timed
/// loop, not on the measured path.
pub fn bench_wt(
    b: &mut criterion::Bencher,
    repo_path: &Path,
    cache: CacheState,
    mut make_cmd: impl FnMut() -> Command,
) {
    let mut run = move || {
        run_and_check(&mut make_cmd());
    };
    let invalidate: fn(&Path) = match cache {
        CacheState::Warm => {
            b.iter(run);
            return;
        }
        CacheState::Cold => invalidate_caches_auto,
        CacheState::ProbeCold => invalidate_probe_caches,
    };
    b.iter_batched(
        || invalidate(repo_path),
        |_| run(),
        criterion::BatchSize::PerIteration,
    );
}

/// Spawn the command, wait, and panic with its stderr if it failed.
///
/// Returns the captured output so benchmarks with a load-bearing output
/// contract can validate it once without reimplementing the status check.
pub fn run_and_check(cmd: &mut Command) -> Output {
    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "benchmark command failed:\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
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

/// Run a prepared git command, panicking on failure and returning trimmed
/// stdout. Shared body of [`capture_git`] and [`git_stdout`].
fn run_capture(cmd: &mut Command, path: &Path) -> String {
    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "Git command failed: {:?}\nstderr: {}\npath: {}",
        cmd,
        String::from_utf8_lossy(&output.stderr),
        path.display()
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Run a git command in the given directory, panicking on failure and
/// returning trimmed stdout.
fn capture_git(path: &Path, args: &[&str]) -> String {
    run_capture(git_command().args(args).current_dir(path), path)
}

/// Run a git command, returning whether it succeeded. Does not panic.
pub fn run_git_ok(path: &Path, args: &[&str]) -> bool {
    git_command()
        .args(args)
        .current_dir(path)
        .output()
        .is_ok_and(|o| o.status.success())
}

/// `git init` a fixture repo at `repo_path` (creating the directory) with the
/// benchmark identity and all background auto-maintenance disabled.
///
/// Auto-maintenance must be off: rapid commits in a fixture build loop
/// trigger detached `git gc` / `git maintenance` runs whose pack-and-prune
/// steps race the foreground `git add` / `git commit`, producing intermittent
/// "invalid object ..." / "unable to create temporary file" / "failed to
/// insert into database" failures partway through a 500-commit fixture.
/// Modern git enables both `gc.auto` (loose-object threshold) and
/// `maintenance.auto` (the post-command hook scheduler) by default, so both
/// are silenced. Fixture builders run an explicit `git gc` once at the end
/// instead, for a mature-repo shape.
fn init_bench_repo(repo_path: &Path) {
    std::fs::create_dir_all(repo_path).unwrap();
    run_git(repo_path, &["init", "-b", "main"]);
    run_git(repo_path, &["config", "user.name", "Benchmark"]);
    run_git(repo_path, &["config", "user.email", "bench@test.com"]);
    run_git(repo_path, &["config", "gc.auto", "0"]);
    run_git(repo_path, &["config", "gc.autoPackLimit", "0"]);
    run_git(repo_path, &["config", "maintenance.auto", "false"]);
}

/// Run a git plumbing command against a scratch `GIT_INDEX_FILE`, panicking on
/// failure and returning trimmed stdout. Used to build commits without
/// touching the repo's working tree or real index (see
/// `add_diverged_backdrop`).
fn git_stdout(path: &Path, args: &[&str], index_file: &Path) -> String {
    run_capture(
        git_command()
            .args(args)
            .current_dir(path)
            .env("GIT_INDEX_FILE", index_file),
        path,
    )
}

/// Create a test repository at a specific path.
///
/// Uses worktrunk naming convention:
/// - Main worktree: `base_path`
/// - Feature worktrees: `base_path.feature-wt-N` (siblings in parent directory)
#[cfg(test)]
fn build_simple_repo_at(config: &SimpleRepoConfig, base_path: &Path) {
    let repo_path = base_path.to_path_buf();
    init_bench_repo(&repo_path);

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

    add_simple_worktrees(config, &repo_path);

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
/// Creates `config.total_worktrees - 1` linked worktrees as siblings of `repo_path`
/// (e.g., `repo.feature-wt-1`), each with diverging commits and uncommitted files
/// controlled by `config.worktree_commits_ahead` and `config.worktree_uncommitted_files`.
#[cfg(test)]
fn add_simple_worktrees(config: &SimpleRepoConfig, repo_path: &Path) {
    for wt_num in 1..config.total_worktrees {
        let branch = format!("feature-wt-{wt_num}");
        let wt_path = linked_worktree_path(repo_path, &branch);

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

/// Create `count` remote-tracking refs under `refs/remotes/origin/`, on top of
/// the `origin/main` + `origin/HEAD` pair [`setup_fake_remote`] writes.
///
/// The refs are spread round-robin over the commits already on `main` rather
/// than all pointing at `HEAD`, which would leave `%(committerdate)` re-reading
/// one object and understate the scan. It does not reach one object per ref:
/// the round-robin can only spread over the history it has, so at
/// the generated recipe's 200 commits, a four-digit ref count lands ~7 refs on
/// each of ~200 distinct commits. Git parses a given object once and reuses it
/// for the rest of the `for-each-ref`, so the scan pays ~200 parses plus a
/// cheap iteration hit per ref, where a real long-lived clone — whose remote
/// tips are mostly distinct commits — would pay a parse per ref.
///
/// So this under-weights object reads relative to the clone it models, and
/// deliberately: closing the gap needs a history as deep as the ref count, and
/// `BASE_COMMITS` is shared with `full`, so deepening it would slow that
/// fixture and shift the repo `full` has been measured on for the sake of a
/// per-object parse this bench is not primarily about. The ref-count dimension
/// is what it exists to vary.
///
/// Timestamps are all `TEST_EPOCH` regardless — fixture commit dates are pinned
/// — so the sort is not what this measures.
///
/// Names are `remote-only-<i>`, which no fixture uses for a local branch, so
/// every one of them survives the "skip remotes shadowed by a local branch"
/// filter in `branches_for_completion` and reaches the candidate list.
///
/// One `update-ref --stdin` fork writes the whole batch; the per-ref
/// alternative costs a fork each and dominates fixture build time at the
/// four-digit counts this exists to model.
fn add_remote_tracking_refs(count: usize, repo_path: &Path) {
    if count == 0 {
        return;
    }

    let commits: Vec<String> = capture_git(repo_path, &["rev-list", "HEAD"])
        .lines()
        .map(str::to_string)
        .collect();
    assert!(
        !commits.is_empty(),
        "fixture has no commits to point refs at"
    );

    let mut stdin = String::new();
    for i in 0..count {
        let sha = &commits[i % commits.len()];
        stdin.push_str(&format!(
            "create refs/remotes/origin/remote-only-{i} {sha}\n"
        ));
    }

    let mut child = git_command()
        .args(["update-ref", "--stdin"])
        .current_dir(repo_path)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // Take the handle so it drops (closing the pipe) before the wait —
    // git reads to EOF, so holding it open here deadlocks.
    let mut pipe = child.stdin.take().unwrap();
    std::io::Write::write_all(&mut pipe, stdin.as_bytes()).unwrap();
    drop(pipe);
    let status = child.wait().unwrap();
    assert!(status.success(), "git update-ref --stdin failed: {status}");
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
///
/// Worktree indexes are deliberately preserved. An index carries staged state,
/// not just stat/fsmonitor acceleration; removing it makes git report every
/// tracked file as staged for deletion and changes which candidates commands
/// see. A benchmark's cold and warm variants must differ only in cache state.
pub fn invalidate_caches_auto(repo_path: &Path) {
    let Some(git_dir) = resolve_git_common_dir(repo_path) else {
        return;
    };

    // Commit graph: legacy single-file plus chained-graph dir.
    remove_file_if_exists(&git_dir.join("objects/info/commit-graph"));
    remove_dir_if_exists(&git_dir.join("objects/info/commit-graphs"));

    // Note: `packed-refs` is intentionally NOT removed. After fixture setup
    // runs an explicit `git gc`, every loose ref under `refs/heads/`,
    // `refs/remotes/`, etc. is packed into `packed-refs` and the loose files
    // are pruned. Deleting `packed-refs` in that state leaves the repo with
    // no resolvable refs — `rev-parse main` fails, and any bench that reads
    // through a branch (e.g. the `with_vars` alias's `{{ commit }}` template
    // var) blows up with a template-expansion error. The file is git's
    // primary ref storage post-gc, not a cache, so there's no cold-state to
    // simulate by deleting it.

    // All worktrunk persistent caches: every kind dir under wt/cache/.
    invalidate_probe_caches(repo_path);

    // Worktrunk's default-branch cache lives in git config; we have no
    // safe way to edit that file ourselves (escaping rules), so shell
    // out. Exit 5 = key absent (harmless); anything else is a real
    // failure and we want it loud, since the bench's cold-cache
    // invariant depends on this succeeding.
    let output = git_command()
        .args(["config", "--unset", "worktrunk.default-branch"])
        .current_dir(repo_path)
        .output()
        .unwrap_or_else(|error| {
            panic!("failed to run `git config --unset worktrunk.default-branch`: {error}")
        });
    assert!(
        output.status.success() || output.status.code() == Some(5),
        "`git config --unset worktrunk.default-branch` failed (exit {:?}): {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
}

/// Invalidate only worktrunk's persistent probe caches (`.git/wt/cache/`).
///
/// Models the recurring cold scan: after new commits land on the default
/// branch, every sha_cache entry keyed on the old tips misses, while git's
/// own state stays warm — indexes keep their stat data, and the commit graph
/// and `worktrunk.default-branch` config entry survive a fetch. This is the
/// "first `wt step prune` after fetching main" shape. Like
/// [`invalidate_caches_auto`], it preserves worktree indexes so clean-worktree
/// gates see the same repository state as a warm run.
pub fn invalidate_probe_caches(repo_path: &Path) {
    let Some(git_dir) = resolve_git_common_dir(repo_path) else {
        return;
    };
    remove_dir_if_exists(&git_dir.join("wt/cache"));
}

/// Remove a cache file, treating only absence as an already-cold cache.
fn remove_file_if_exists(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => panic!("failed to remove cache file {}: {error}", path.display()),
    }
}

/// Remove a cache directory, treating only absence as an already-cold cache.
fn remove_dir_if_exists(path: &Path) {
    match std::fs::remove_dir_all(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => panic!(
            "failed to remove cache directory {}: {error}",
            path.display()
        ),
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

/// Root of wt-perf's on-disk fixtures: `<cargo-target-dir>/wt-perf`.
///
/// The target dir is `cargo_target_dir`, derived from the running executable, so
/// it tracks wherever cargo actually built — the default `<workspace>/target`, a
/// `CARGO_TARGET_DIR` / `build.target-dir` override, or cargo-llvm-cov's
/// relocated dir — keeping fixtures co-located with build output and reaped by
/// `cargo clean`.
///
/// Living under `target/` means `cargo clean` reaps every fixture and each git
/// worktree keeps its own copy (worktrees don't share `target/`). That is cheap
/// for the generated `setup` fixtures — rebuilt in seconds — but a deliberate
/// cost for the multi-gigabyte imported fixture under `bench-repos/`, which
/// then re-clones per worktree and after every `cargo clean`. Relocate it with
/// cargo's own `CARGO_TARGET_DIR` if that cost bites.
pub fn wt_perf_fixture_dir() -> PathBuf {
    cargo_target_dir().join("wt-perf")
}

/// The cargo target directory containing the current executable.
///
/// Both entry points live inside it — the `wt-perf` CLI at
/// `<target>/debug/wt-perf`, the in-process benches at
/// `<target>/release/deps/<bench>` — so the closest ancestor named `debug` or
/// `release` (the profile dir) has the target dir as its parent. Reading the
/// running binary's path rather than `CARGO_TARGET_DIR` alone also honors a
/// config-file `build.target-dir` and cargo-llvm-cov's `--target-dir`: the
/// binary is physically inside whichever dir cargo used. Falls back to
/// `<workspace>/target` (from the compile-time manifest dir) if the executable
/// isn't under a recognizable profile dir.
fn cargo_target_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| target_dir_from_exe(&exe))
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .ancestors()
                .nth(3)
                .expect("wt-perf crate sits three levels below the workspace root")
                .join("target")
        })
}

/// The target dir containing `exe`: the closest ancestor named `debug` or
/// `release` (the cargo profile dir) has the target dir as its parent. `None`
/// if `exe` isn't under such a dir.
fn target_dir_from_exe(exe: &Path) -> Option<PathBuf> {
    exe.ancestors()
        .find(|p| {
            matches!(
                p.file_name().and_then(|n| n.to_str()),
                Some("debug" | "release")
            )
        })?
        .parent()
        .map(Path::to_path_buf)
}

fn imported_cache_dir() -> PathBuf {
    wt_perf_fixture_dir().join("bench-repos").join("imported")
}

fn acquire_exclusive_lock(path: &Path) -> File {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap_or_else(|error| {
        panic!(
            "failed to create fixture lock directory {}: {error}",
            path.parent().unwrap().display()
        )
    });
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .unwrap_or_else(|error| panic!("failed to open fixture lock {}: {error}", path.display()));
    file.lock_exclusive()
        .unwrap_or_else(|error| panic!("failed to lock fixture {}: {error}", path.display()));
    file
}

fn path_exists(path: &Path) -> bool {
    path.try_exists()
        .unwrap_or_else(|error| panic!("failed to inspect {}: {error}", path.display()))
}

fn repository_head(repo: &Path) -> Option<String> {
    let output = git_command()
        .args(["rev-parse", "--verify", "HEAD^{commit}"])
        .current_dir(repo)
        .output()
        .unwrap_or_else(|error| {
            panic!(
                "failed to inspect repository HEAD at {}: {error}",
                repo.display()
            )
        });
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn repository_is_on_main(repo: &Path) -> bool {
    let output = git_command()
        .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
        .current_dir(repo)
        .output()
        .unwrap_or_else(|error| {
            panic!(
                "failed to inspect repository branch at {}: {error}",
                repo.display()
            )
        });
    output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "main"
}

fn imported_source_matches(repo: &Path) -> bool {
    path_exists(repo)
        && repository_head(repo).as_deref() == Some(IMPORTED_REVISION)
        && repository_is_on_main(repo)
        && run_git_ok(repo, &["fsck", "--connectivity-only", "--no-dangling"])
}

/// Get or build the immutable source for imported benchmarks.
///
/// The revision is part of the cache path. Construction happens in a temporary
/// sibling and is atomically renamed into place after validation, so an
/// interrupted clone never looks like a usable cache entry.
fn ensure_imported_source() -> PathBuf {
    let cache_dir = imported_cache_dir();
    let source = cache_dir.join(format!("source-{IMPORTED_REVISION}"));
    let staged_source = cache_dir.join(format!("source-{IMPORTED_REVISION}.building"));
    let _build_lock =
        acquire_exclusive_lock(&cache_dir.join(format!("source-{IMPORTED_REVISION}.lock")));
    if path_exists(&staged_source) {
        remove_dir_if_exists(&staged_source);
    }

    if imported_source_matches(&source) {
        eprintln!("Using cached imported source at {}", source.display());
        return source;
    }
    if path_exists(&source) {
        eprintln!("Cached imported source is invalid; rebuilding");
        remove_dir_if_exists(&source);
    }

    let url = format!("https://github.com/{IMPORTED_CORPUS}.git");
    eprintln!(
        "Cloning {} at {} (this will take several minutes)...",
        IMPORTED_CORPUS, IMPORTED_REVISION
    );
    let mut clone = git_command();
    allow_network_transports(&mut clone);
    let output = clone
        .args(["clone", "--no-checkout", &url])
        .arg(&staged_source)
        .output()
        .unwrap_or_else(|error| panic!("failed to spawn imported clone: {error}"));
    assert!(
        output.status.success(),
        "failed to clone {}:\n{}",
        IMPORTED_CORPUS,
        String::from_utf8_lossy(&output.stderr)
    );
    run_git(&staged_source, &["checkout", "--detach", IMPORTED_REVISION]);
    run_git(&staged_source, &["branch", "-f", "main", IMPORTED_REVISION]);
    run_git(&staged_source, &["checkout", "main"]);
    assert!(
        imported_source_matches(&staged_source),
        "imported source failed validation after checkout"
    );
    std::fs::rename(&staged_source, &source).unwrap_or_else(|error| {
        panic!(
            "failed to publish imported source {}: {error}",
            source.display()
        )
    });
    eprintln!("Imported source cloned successfully");
    source
}

/// Local-clone the pinned imported source to `dest` and configure a
/// git user for fixture commits.
fn clone_imported_at(dest: &Path) {
    let source = ensure_imported_source();
    let clone_output = git_command()
        .args([
            "clone",
            "--local",
            "--single-branch",
            "--branch",
            "main",
            source.to_str().unwrap(),
            dest.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        clone_output.status.success(),
        "Failed to local-clone imported source: {}",
        String::from_utf8_lossy(&clone_output.stderr)
    );
    run_git(dest, &["config", "user.name", "Benchmark"]);
    run_git(dest, &["config", "user.email", "bench@test.com"]);
    assert_eq!(
        repository_head(dest).as_deref(),
        Some(IMPORTED_REVISION),
        "imported clone did not retain the pinned revision"
    );
    assert!(
        repository_is_on_main(dest),
        "imported clone must check out the pinned main branch"
    );
}

/// Sample up to `count` commit SHAs evenly spread across the last 5000
/// commits of `repo_path`'s current branch, newest first.
///
/// The spread reproduces the GH #461 scenario where branch divergence *depth*
/// (not count) drives cost: consumers fork branches at these points so
/// merge-base walks and merge-tree three-ways span the whole history rather
/// than a handful of tip-adjacent forks.
fn history_spread_shas(repo_path: &Path, count: usize) -> Vec<String> {
    let log_output = git_command()
        .args(["log", "--oneline", "-n", "5000", "--format=%H"])
        .current_dir(repo_path)
        .output()
        .unwrap();
    let log_str = String::from_utf8_lossy(&log_output.stdout);
    // Step over the log we actually got, not the 5000 cap: on a short history
    // (the generated fixtures have a few hundred commits) dividing by the cap
    // floors every sample onto the tip, collapsing the spread to one SHA.
    // Guard the degenerate inputs: `count == 0` would divide by zero, and
    // `count > len` would yield `step == 0`, which panics `step_by`. Both
    // `max(1)`s preserve the spread for every in-range count.
    let len = log_str.lines().count();
    let step = (len / count.max(1)).max(1);
    log_str
        .lines()
        .step_by(step)
        .take(count)
        .map(str::to_string)
        .collect()
}

/// Create `branch` at `fork`, with `commits` new commits on top of it.
///
/// The one place a fixture branch is built. What the caller varies is the pair
/// (fork point, commit count), and that pair is the whole state space:
/// `commits == 0` leaves the branch sitting exactly at `fork` — "behind" when
/// `fork` is an older commit, "identical to the tip" when it is the tip; a
/// positive count forks and advances — "ahead" from the tip, two-sided
/// "diverged" from anywhere else.
///
/// Built with plumbing (a scratch `GIT_INDEX_FILE` plus `commit-tree`), never
/// touching the working tree: on a large repo like rust-lang/rust, a
/// `git checkout` of an old fork point rewrites the whole tree and would cost
/// minutes per branch. Each commit adds one new file, so the branch's tree
/// genuinely diverges and the integration probes can't short-circuit.
fn add_branch_with_commits(repo_path: &Path, branch: &str, fork: &str, commits: usize) {
    let scratch = tempfile::tempdir().unwrap();
    let index = scratch.path().join("index");
    let mut tip = fork.to_string();
    for j in 0..commits {
        let blob_file = scratch.path().join("blob");
        std::fs::write(&blob_file, format!("// {branch} {j}\n")).unwrap();
        let blob = git_stdout(
            repo_path,
            &["hash-object", "-w", blob_file.to_str().unwrap()],
            &index,
        );
        git_stdout(repo_path, &["read-tree", &tip], &index);
        git_stdout(
            repo_path,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("100644,{blob},{}_{j}.rs", branch.replace('-', "_")),
            ],
            &index,
        );
        let tree = git_stdout(repo_path, &["write-tree"], &index);
        tip = git_stdout(
            repo_path,
            &[
                "commit-tree",
                &tree,
                "-p",
                &tip,
                "-m",
                &format!("{branch} commit {j}"),
            ],
            &index,
        );
    }
    run_git(repo_path, &["branch", branch, &tip]);
}

/// Create branches pointing at different depths in the repo's commit history.
///
/// Samples `count` commits via `history_spread_shas` and creates
/// `feature-NNN` branches with one commit on top. Older samples are genuinely
/// two-sided diverged from the default branch; `feature-000` is ahead from the
/// current tip. If a later overlay advances the default branch, it becomes
/// diverged too. This keeps the imported branches from turning into
/// incidental prune candidates.
fn add_history_spread_branches(repo_path: &Path, branchless_branches: usize) {
    let commits = history_spread_shas(repo_path, branchless_branches);
    assert_eq!(
        commits.len(),
        branchless_branches,
        "history-spread fixture needs at least {branchless_branches} commits"
    );
    for (i, commit) in commits.iter().enumerate() {
        add_branch_with_commits(repo_path, &format!("feature-{i:03}"), commit, 1);
    }
}

/// Add two-sided-diverged linked worktrees (`unmerged-wt-N`) and branchless
/// branches (`unmerged-br-NNN`) to an existing repo.
///
/// Each forks at a `history_spread_shas` point and carries its own commits on
/// top. That is the shape of real long-lived feature work: `git merge-base`
/// must walk back to the fork, and the integration probes (`merge-tree
/// --write-tree`, diff) three-way over genuinely diverged trees. None of them
/// is integrated, so against `wt step prune` they are a pure scan backdrop:
/// every probe runs and fails. Worktrees get 2 untracked files (dirty in the
/// way real worktrees are — untracked scratch, no staged state, so
/// index-restoring helpers stay safe to use).
///
/// The spread's newest sample is the default branch's own tip, so index 0 of
/// each population forks there and is strictly *ahead* rather than two-sided
/// diverged. The sole caller (`add_prune_populations`) resolves that on the
/// next line: `add_squash_merged` advances the default branch past every fork,
/// this one included. Called on its own — as the tests do — expect one
/// ahead-only member per population.
///
/// The populations are sized independently because their costs differ wildly
/// on large repos: an orphan branch is a few commits' worth of objects, while
/// a linked worktree materializes a full working tree (hundreds of MiB in the
/// pinned corpus) and pays a checkout.
fn add_diverged_backdrop(repo_path: &Path, linked_worktrees: usize, branchless_branches: usize) {
    let forks = history_spread_shas(repo_path, linked_worktrees.max(branchless_branches).max(1));

    // Each orphan branch forks at a spread point and carries 3 commits of its
    // own, so the default branch has advanced past it on one side while it
    // advanced on the other.
    for i in 0..branchless_branches {
        add_branch_with_commits(
            repo_path,
            &format!("unmerged-br-{i:03}"),
            &forks[i % forks.len()],
            3,
        );
    }

    for i in 0..linked_worktrees {
        let wt_branch = format!("unmerged-wt-{i}");
        let wt_path = linked_worktree_path(repo_path, &wt_branch);
        run_git(
            repo_path,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &wt_branch,
                wt_path.to_str().unwrap(),
                &forks[i % forks.len()],
            ],
        );
        // Targeted add — a bare `git add .` would rescan the whole tree,
        // which on rust-lang/rust costs seconds per commit.
        for j in 0..5 {
            let name = format!("{}_{j}.rs", wt_branch.replace('-', "_"));
            std::fs::write(wt_path.join(&name), format!("// {wt_branch} {j}\n")).unwrap();
            run_git(&wt_path, &["add", &name]);
            run_git(
                &wt_path,
                &["commit", "-q", "-m", &format!("{wt_branch} commit {j}")],
            );
        }
        for j in 0..2 {
            std::fs::write(wt_path.join(format!("uncommitted_{j}.txt")), "scratch\n").unwrap();
        }
    }
}

/// `git rev-parse HEAD` in `path`, trimmed. Panics on failure.
fn head_sha(path: &Path) -> String {
    capture_git(path, &["rev-parse", "HEAD"])
}

/// Append a line to a tracked file (creating it if missing). Used to make
/// working-tree edits in the generated fixture.
fn append_line(path: &Path, rel: &str, line: &str) {
    let file = path.join(rel);
    let mut content = std::fs::read_to_string(&file).unwrap_or_default();
    content.push_str(line);
    content.push('\n');
    std::fs::write(&file, content).unwrap();
}

/// Create the generated corpus with controlled worktree, branch, and
/// remote-ref populations.
///
/// This corpus exercises the full spread of `wt list` gates and tasks at once:
/// clean vs dirty working trees, merged vs ahead vs
/// diverged branches, and divergence spread across history depth. It models
/// many worktrees and branches in varied states. The main
/// worktree is available through [`FixtureRepo::path`], and linked worktrees
/// through [`FixtureRepo::worktree_path`]. Either dimension may be zero.
///
/// Worktree states cycle by index % 4:
/// 0. clean, several commits ahead of base
/// 1. unstaged modification (dirty working tree)
/// 2. staged + unstaged + untracked (full dirty mix)
/// 3. clean, sitting exactly at base
///
/// Branch states cycle by index % 4 (states 0 and 2 fork at a checkpoint that
/// slides from the oldest base commit toward the tip as the index grows, so
/// fork depth fans out across the whole history — the GH #461 deep-divergence
/// shape that drives the O(commits) `git for-each-ref %(ahead-behind)` walk):
/// 0. behind: at an older checkpoint (ancestor of base —
///    integration-positive / merged shape)
/// 1. ahead of base with its own commits (unmerged)
/// 2. diverged: a short own-commit chain forked from an older checkpoint
///    while base advanced (deep two-sided divergence)
/// 3. identical to the base tip (trees match — squash-merge shape)
fn build_generated_repo_at(
    linked_worktrees: usize,
    branchless_branches: usize,
    remote_tracking_refs: usize,
    repo: &Path,
) {
    const FILES: usize = 50;
    // Deep enough that fork points spread across history give the
    // `%(ahead-behind)` walk real commits to traverse (GH #461 shape).
    const BASE_COMMITS: usize = 200;
    // Record a checkpoint every few commits so behind/diverged branches fork
    // at many distinct depths rather than a handful of fixed points.
    const CHECKPOINT_EVERY: usize = 5;

    let repo = repo.to_path_buf();
    init_bench_repo(&repo);

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
        if c % CHECKPOINT_EVERY == 0 {
            checkpoints.push(head_sha(&repo));
        }
    }
    let base_tip = head_sha(&repo);
    // `checkpoints[0]` is the oldest (initial commit); the last is near the
    // tip. Index `i` of the branch population maps linearly across them, so behind/
    // diverged branches fork at points fanned across history depth rather than
    // a few repeated checkpoints.
    let deepest = checkpoints.len() - 1;

    // Branches without worktrees, in the documented rotation. Each state is
    // just a (fork point, own-commit count) pair — see
    // `add_branch_with_commits`, which builds every one of them with plumbing,
    // so nothing checks out and the main worktree is untouched throughout.
    for i in 0..branchless_branches {
        let name = format!("br-{i:04}");
        // The count is at least one inside this loop, so the divisor is nonzero.
        let checkpoint = checkpoints[i * deepest / branchless_branches].as_str();
        let (fork, commits) = match i % 4 {
            0 => (checkpoint, 0),                // behind
            1 => (base_tip.as_str(), 1 + i % 3), // ahead
            2 => (checkpoint, 1 + i % 3),        // diverged
            _ => (base_tip.as_str(), 0),         // identical to the tip
        };
        add_branch_with_commits(&repo, &name, fork, commits);
    }

    // Mature-repo shape: pack refs and write the commit-graph once, after every
    // branch ref exists but before the worktrees (freshly added worktrees carry
    // loose refs and uncommitted state — realistic, and keeps gc away from the
    // dirty indexes below).
    setup_fake_remote(&repo);
    add_remote_tracking_refs(remote_tracking_refs, &repo);
    run_git(&repo, &["gc", "-q"]);

    add_heterogeneous_worktrees(&repo, linked_worktrees, &base_tip);
}

/// Add linked worktrees in the canonical four-state rotation.
fn add_heterogeneous_worktrees(repo: &Path, linked_worktrees: usize, base_tip: &str) {
    // Linked worktrees are siblings named `<repo-dir>.<branch>` (worktrunk
    // convention), derived from the repo's own directory name so the path is
    // correct whether the repo is the tempdir's `repo` or a custom `setup` path.
    for j in 0..linked_worktrees {
        let branch = format!("wt-{j:04}");
        let wt = linked_worktree_path(repo, &branch);
        run_git(
            repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &branch,
                wt.to_str().unwrap(),
                base_tip,
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

/// Add `count` squash-merged worktrees (`merged-wt-N`) and `count`
/// squash-merged orphan branches (`merged-br-N`) to an existing repo.
///
/// Each branch gets its own commits, then the default branch checked out in
/// the primary worktree takes the same content as one
/// `git merge --squash` commit — integrated by content, so `wt step prune`
/// detects it via the merge-tree probes and removes it.
///
fn add_squash_merged(repo_path: &Path, count: usize) {
    let default_branch = capture_git(repo_path, &["rev-parse", "--abbrev-ref", "HEAD"]);

    // Two commits in `dir`, each adding a branch-uniquified file. The add is
    // targeted — a bare `git add .` would rescan the whole tree, which on
    // rust-lang/rust costs seconds per commit.
    let commit_branch_content = |dir: &Path, branch: &str| {
        for j in 0..2 {
            let name = format!("{}_{j}.rs", branch.replace('-', "_"));
            std::fs::write(dir.join(&name), format!("// {branch} {j}\n")).unwrap();
            run_git(dir, &["add", &name]);
            run_git(
                dir,
                &["commit", "-q", "-m", &format!("{branch} commit {j}")],
            );
        }
    };
    // Land the branch's content on the default branch as one squash commit.
    let squash_into_default = |branch: &str| {
        run_git(repo_path, &["merge", "--squash", "-q", branch]);
        run_git(
            repo_path,
            &["commit", "-q", "-m", &format!("Squash-merge {branch}")],
        );
    };

    for i in 0..count {
        let branch = format!("merged-wt-{i}");
        let wt_path = linked_worktree_path(repo_path, &branch);
        run_git(
            repo_path,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &branch,
                wt_path.to_str().unwrap(),
                "HEAD",
            ],
        );
        commit_branch_content(&wt_path, &branch);
        squash_into_default(&branch);
    }

    for i in 0..count {
        let branch = format!("merged-br-{i}");
        run_git(repo_path, &["checkout", "-q", "-b", &branch]);
        commit_branch_content(repo_path, &branch);
        run_git(repo_path, &["checkout", "-q", &default_branch]);
        squash_into_default(&branch);
    }
}

/// Add the two populations that turn a repo into a `wt step prune` workload:
/// `backdrop_pairs` two-sided-diverged worktrees and branches (`add_diverged_backdrop`
/// — the backdrop prune scans every run but never removes) and `candidate_pairs`
/// squash-merged candidate pairs (`add_squash_merged` — what prune removes).
///
/// The corpus remains one of the two canonical bases; prune is an overlay.
pub fn add_prune_populations(base_path: &Path, candidate_pairs: usize, backdrop_pairs: usize) {
    add_diverged_backdrop(base_path, backdrop_pairs, backdrop_pairs);
    add_squash_merged(base_path, candidate_pairs);
    // Candidate squash commits advance the default branch. Keep the fake
    // remote aligned so default-branch discovery sees the final state.
    setup_fake_remote(base_path);
}

/// Default populations for the imported prune overlay:
/// 12 squash-merged candidates of each kind + 24 unmerged worktrees and
/// branches layered onto the canonical imported state.
pub const IMPORTED_PRUNE_CANDIDATE_PAIRS: usize = 12;
pub const IMPORTED_PRUNE_BACKDROP_PAIRS: usize = 24;

/// Canonicalize path without Windows `\\?\` prefix.
pub fn canonicalize(path: &Path) -> std::io::Result<PathBuf> {
    dunce::canonicalize(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_simple(config: &SimpleRepoConfig) -> FixtureRepo {
        FixtureRepo::create(|repo| build_simple_repo_at(config, repo))
    }

    /// Sorted `git status --porcelain` lines for a worktree.
    ///
    /// Reads raw stdout rather than going through `capture_git`, which trims:
    /// porcelain's leading status column is significant (` M` unstaged vs
    /// `M ` staged), and trimming silently merges the two.
    fn status_lines(wt: &Path) -> Vec<String> {
        let out = git_command()
            .args(["status", "--porcelain"])
            .current_dir(wt)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git status failed in {}",
            wt.display()
        );
        let mut lines: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .collect();
        lines.sort();
        lines
    }

    /// `target_dir_from_exe` finds the cargo target dir as the parent of the
    /// closest `debug`/`release` profile dir, so fixtures track wherever cargo
    /// actually built: a relocated `CARGO_TARGET_DIR`, a bench binary under
    /// `release/deps/`, or cargo-llvm-cov's nested target. A binary outside any
    /// target dir yields `None`, so the caller uses the workspace fallback.
    #[test]
    fn target_dir_from_exe_finds_cargo_target() {
        let cases = [
            // CLI binary at <target>/debug/wt-perf
            ("/w/target/debug/wt-perf", Some("/w/target")),
            // Relocated via CARGO_TARGET_DIR / build.target-dir
            ("/tmp/tgt/debug/wt-perf", Some("/tmp/tgt")),
            // Bench binary at <target>/release/deps/<bench>
            ("/w/target/release/deps/list-abc123", Some("/w/target")),
            // cargo-llvm-cov's nested target dir
            (
                "/w/target/llvm-cov-target/debug/deps/x-1",
                Some("/w/target/llvm-cov-target"),
            ),
            // Closest profile dir wins even if an ancestor is literally "release"
            (
                "/home/release/proj/target/debug/wt-perf",
                Some("/home/release/proj/target"),
            ),
            // Installed outside any target dir → None (caller uses the fallback)
            ("/usr/local/bin/wt-perf", None),
        ];
        for (exe, expected) in cases {
            assert_eq!(
                target_dir_from_exe(Path::new(exe)),
                expected.map(PathBuf::from),
                "{exe}"
            );
        }
    }

    #[test]
    fn fixture_lock_excludes_an_independent_handle() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("fixture.lock");
        let first = acquire_exclusive_lock(&path);
        let second = OpenOptions::new().write(true).open(&path).unwrap();

        assert!(
            second.try_lock_exclusive().is_err(),
            "another process handle must not acquire a leased fixture"
        );
        FileExt::unlock(&first).unwrap();
        second.try_lock_exclusive().unwrap();
        FileExt::unlock(&second).unwrap();
    }

    #[test]
    fn imported_source_rejects_a_missing_pinned_commit_object() {
        let temp = tempfile::tempdir().unwrap();
        init_bench_repo(temp.path());
        std::fs::write(
            temp.path().join(".git/refs/heads/main"),
            format!("{IMPORTED_REVISION}\n"),
        )
        .unwrap();

        assert!(
            !imported_source_matches(temp.path()),
            "a textual ref without its commit object is not a usable clone source"
        );
    }

    /// Cache invalidation must never change the repository state presented to
    /// the command being benchmarked. In particular, deleting a real index is
    /// not a cold-cache simulation: git reads the missing index as every
    /// tracked file being staged for deletion. Refs are part of the same
    /// contract: fixture creation packs them during `git gc`, so `packed-refs`
    /// is primary storage rather than a disposable cache.
    #[test]
    fn invalidate_preserves_repository_state_and_clears_caches() {
        let fixture = create_simple(&SimpleRepoConfig {
            commits_on_main: 2,
            files: 2,
            total_worktrees: 2,
            worktree_commits_ahead: 1,
            worktree_uncommitted_files: 0,
        });
        let repo = fixture.path().to_path_buf();
        let linked = fixture.worktree_path("feature-wt-1");

        for (worktree, suffix) in [(&repo, "primary"), (&linked, "linked")] {
            let tracked = worktree.join("src/file_0.rs");
            let mut content = std::fs::read_to_string(&tracked).unwrap();
            content.push_str(&format!("\n// staged in {suffix}\n"));
            std::fs::write(&tracked, &content).unwrap();
            run_git(worktree, &["add", "src/file_0.rs"]);
            content.push_str(&format!("// unstaged in {suffix}\n"));
            std::fs::write(&tracked, content).unwrap();
            std::fs::write(
                worktree.join(format!("untracked-{suffix}.txt")),
                "untracked\n",
            )
            .unwrap();
        }

        let git_dir = resolve_git_common_dir(&repo).unwrap();
        run_git(&repo, &["commit-graph", "write", "--reachable"]);
        let commit_graph = git_dir.join("objects/info/commit-graph");
        assert!(
            commit_graph.exists(),
            "setup precondition: commit graph exists"
        );
        let cache_dir = git_dir.join("wt/cache/probe");
        std::fs::create_dir_all(&cache_dir).unwrap();
        std::fs::write(cache_dir.join("entry"), "cached\n").unwrap();
        run_git(&repo, &["config", "worktrunk.default-branch", "main"]);

        let index_path = |worktree: &Path| {
            let path = PathBuf::from(capture_git(worktree, &["rev-parse", "--git-path", "index"]));
            if path.is_absolute() {
                path
            } else {
                worktree.join(path)
            }
        };
        let primary_index = index_path(&repo);
        let linked_index = index_path(&linked);
        assert!(primary_index.exists(), "setup precondition: primary index");
        assert!(linked_index.exists(), "setup precondition: linked index");

        let primary_status = status_lines(&repo);
        let linked_status = status_lines(&linked);
        assert_eq!(
            primary_status,
            ["?? untracked-primary.txt", "MM src/file_0.rs"]
        );
        assert_eq!(
            linked_status,
            ["?? untracked-linked.txt", "MM src/file_0.rs"]
        );
        let refs = capture_git(
            &repo,
            &[
                "for-each-ref",
                "--format=%(refname) %(objectname)",
                "refs/heads",
            ],
        );
        let worktree_listing = capture_git(&repo, &["worktree", "list", "--porcelain"]);

        // Invalidate through the linked worktree to exercise common-dir
        // resolution as well as the cache policy itself.
        invalidate_caches_auto(&linked);

        assert_eq!(status_lines(&repo), primary_status);
        assert_eq!(status_lines(&linked), linked_status);
        assert_eq!(
            capture_git(
                &repo,
                &[
                    "for-each-ref",
                    "--format=%(refname) %(objectname)",
                    "refs/heads",
                ],
            ),
            refs
        );
        assert_eq!(
            capture_git(&repo, &["worktree", "list", "--porcelain"]),
            worktree_listing
        );
        assert!(primary_index.exists(), "primary index must survive");
        assert!(linked_index.exists(), "linked index must survive");
        assert!(!commit_graph.exists(), "commit graph must be cleared");
        assert!(
            !git_dir.join("wt/cache").exists(),
            "wt cache must be cleared"
        );
        assert!(
            !run_git_ok(&repo, &["config", "--get", "worktrunk.default-branch"]),
            "default-branch cache must be cleared"
        );
    }

    /// An invalid cache shape stands in for filesystem failures that are hard
    /// to induce portably. Invalidation must fail loudly rather than silently
    /// benchmark a warm cache after cleanup did not happen.
    #[test]
    #[should_panic(expected = "failed to remove cache directory")]
    fn invalidate_reports_cache_removal_errors() {
        let fixture = create_simple(&SimpleRepoConfig {
            commits_on_main: 1,
            files: 1,
            total_worktrees: 1,
            worktree_commits_ahead: 0,
            worktree_uncommitted_files: 0,
        });
        let git_dir = resolve_git_common_dir(fixture.path()).unwrap();
        std::fs::create_dir_all(git_dir.join("wt")).unwrap();
        std::fs::write(git_dir.join("wt/cache"), "not a directory\n").unwrap();

        invalidate_probe_caches(fixture.path());
    }

    /// The generated fixture's contract promises a
    /// deterministic `index % 4` rotation of branch and worktree states, and
    /// every `wt list` gate the `full` bench exercises hangs off that rotation.
    /// Nothing else pins it — the bench measures one wall time, so a generator
    /// change that collapsed (say) "diverged" into "ahead" would keep the bench
    /// green while silently measuring a different repo. Assert the states
    /// directly, via `merge-base --is-ancestor` exit codes and porcelain status.
    #[test]
    fn generated_fixture_states_follow_the_documented_rotation() {
        // Two full rotations of each 4-state cycle, so a state that collapsed
        // into its neighbour fails on both of its indices rather than one.
        const N: usize = 8;
        let fixture = FixtureRecipe::Generated {
            linked_worktrees: N,
            branchless_branches: N,
            remote_tracking_refs: 0,
        }
        .create();
        let repo = fixture.path().to_path_buf();
        let main = capture_git(&repo, &["rev-parse", "main"]);

        let refs = capture_git(
            &repo,
            &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
        );
        assert_eq!(
            refs.lines().count(),
            2 * N + 1,
            "expected {N} br-*, {N} wt-*, and main:\n{refs}"
        );

        // Branch states: 0 behind, 1 ahead, 2 diverged, 3 identical to the tip.
        let mut behind_depths = Vec::new();
        for i in 0..N {
            let branch = format!("br-{i:04}");
            let tip = capture_git(&repo, &["rev-parse", &branch]);
            let behind = run_git_ok(&repo, &["merge-base", "--is-ancestor", &branch, "main"]);
            let ahead = run_git_ok(&repo, &["merge-base", "--is-ancestor", "main", &branch]);
            match i % 4 {
                0 => {
                    assert!(
                        behind && tip != main,
                        "{branch} must be strictly behind main"
                    );
                    let depth =
                        capture_git(&repo, &["rev-list", "--count", &format!("{branch}..main")]);
                    behind_depths.push(depth.parse::<usize>().unwrap());
                }
                1 => assert!(
                    ahead && tip != main,
                    "{branch} must be strictly ahead of main"
                ),
                2 => assert!(
                    !behind && !ahead,
                    "{branch} must be two-sided diverged from main"
                ),
                _ => assert_eq!(tip, main, "{branch} must sit exactly at main's tip"),
            }
        }
        // Fork points slide from the oldest checkpoint toward the tip as the
        // index grows, so the `%(ahead-behind)` walk spans the whole history
        // rather than a handful of tip-adjacent forks (the GH #461 shape).
        assert!(
            behind_depths.windows(2).all(|w| w[0] > w[1]),
            "behind-branch fork depths must fan out across history: {behind_depths:?}"
        );

        // Worktree states: 0 clean+ahead, 1 unstaged, 2 staged+unstaged+
        // untracked, 3 clean at the tip.
        for j in 0..N {
            let branch = format!("wt-{j:04}");
            let wt = fixture.worktree_path(&branch);
            let tip = capture_git(&repo, &["rev-parse", &branch]);
            let status = status_lines(&wt);
            match j % 4 {
                0 => {
                    assert!(status.is_empty(), "{branch} must be clean: {status:?}");
                    assert!(
                        run_git_ok(&repo, &["merge-base", "--is-ancestor", "main", &branch])
                            && tip != main,
                        "{branch} must be strictly ahead of main"
                    );
                }
                1 => {
                    assert_eq!(
                        status,
                        [" M src/file_0.rs"],
                        "{branch} must be unstaged-dirty"
                    );
                    assert_eq!(tip, main, "{branch} must sit exactly at main's tip");
                }
                2 => {
                    let mut expected = vec![
                        "M  src/file_1.rs".to_string(),  // staged
                        " M src/file_2.rs".to_string(),  // unstaged
                        format!("?? untracked_{j}.txt"), // untracked
                    ];
                    expected.sort();
                    assert_eq!(status, expected, "{branch} must carry the full dirty mix");
                    assert_eq!(tip, main, "{branch} must sit exactly at main's tip");
                }
                _ => {
                    assert!(status.is_empty(), "{branch} must be clean: {status:?}");
                    assert_eq!(tip, main, "{branch} must sit exactly at main's tip");
                }
            }
        }
    }

    /// The prune fixture's load-bearing property: a squash-merged branch is
    /// integrated *by content* — `git merge-tree --write-tree main <branch>`
    /// yields main's own tree (merging it adds nothing). That's exactly the
    /// probe `wt step prune`'s integration check runs, so if this drifts, the
    /// prune benchmark stops removing anything.
    #[test]
    fn squash_merged_fixture_is_content_integrated() {
        let fixture = create_simple(&SimpleRepoConfig {
            commits_on_main: 3,
            files: 2,
            total_worktrees: 1,
            worktree_commits_ahead: 0,
            worktree_uncommitted_files: 0,
        });
        let repo_path = fixture.path().to_path_buf();

        add_squash_merged(&repo_path, 1);

        let main_tree = git_command()
            .args(["rev-parse", "main^{tree}"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        for branch in ["merged-wt-0", "merged-br-0"] {
            let merged_tree = git_command()
                .args(["merge-tree", "--write-tree", "main", branch])
                .current_dir(&repo_path)
                .output()
                .unwrap();
            assert!(merged_tree.status.success(), "merge-tree failed");
            assert_eq!(
                String::from_utf8_lossy(&merged_tree.stdout).trim(),
                String::from_utf8_lossy(&main_tree.stdout).trim(),
                "{branch} must merge into main without adding changes"
            );
        }
    }

    /// A zero-size spread is valid and must not divide by zero.
    #[test]
    fn history_spread_handles_zero_branches() {
        let fixture = create_simple(&SimpleRepoConfig {
            commits_on_main: 3,
            files: 1,
            total_worktrees: 1,
            worktree_commits_ahead: 0,
            worktree_uncommitted_files: 0,
        });
        let repo_path = fixture.path().to_path_buf();

        add_history_spread_branches(&repo_path, 0);
    }

    #[test]
    fn imported_populations_do_not_become_prune_candidates() {
        let fixture = create_simple(&SimpleRepoConfig {
            commits_on_main: 12,
            files: 2,
            total_worktrees: 1,
            worktree_commits_ahead: 0,
            worktree_uncommitted_files: 0,
        });
        let repo = fixture.path().to_path_buf();
        add_history_spread_branches(&repo, 4);
        add_imported_linked_worktrees(&repo, 4);

        let mut fork_depths = Vec::new();
        for i in 0..4 {
            let branch = format!("feature-{i:03}");
            assert_eq!(
                capture_git(&repo, &["rev-list", "--count", &format!("main..{branch}")]),
                "1",
                "{branch} must carry its own commit"
            );
            assert!(
                !run_git_ok(&repo, &["merge-base", "--is-ancestor", &branch, "main"]),
                "{branch} must not be integrated into main"
            );
            fork_depths.push(
                capture_git(&repo, &["rev-list", "--count", &format!("{branch}..main")])
                    .parse::<usize>()
                    .unwrap(),
            );
        }
        assert!(
            fork_depths.windows(2).all(|depths| depths[0] < depths[1]),
            "forks must spread down history: {fork_depths:?}"
        );

        add_prune_populations(&repo, 1, 1);
        let main_tree = capture_git(&repo, &["rev-parse", "main^{tree}"]);
        for i in 0..4 {
            let branch = format!("feature-{i:03}");
            assert!(
                !run_git_ok(&repo, &["merge-base", "--is-ancestor", &branch, "main"]),
                "{branch} became integrated after the prune overlay advanced main"
            );
            assert_ne!(
                capture_git(&repo, &["merge-tree", "--write-tree", "main", &branch]),
                main_tree,
                "{branch} became content-integrated after the prune overlay"
            );
        }
        assert!(
            !run_git_ok(&repo, &["merge-base", "--is-ancestor", "wt-0003", "main"]),
            "clean imported worktree became integrated after the prune overlay"
        );
        assert_ne!(
            capture_git(&repo, &["merge-tree", "--write-tree", "main", "wt-0003"]),
            main_tree,
            "clean imported worktree became content-integrated after the prune overlay"
        );
    }

    /// The generated fixture's population contract. Either local dimension may
    /// be zero, and the requested remote-tracking refs are additive to the
    /// `origin/main` and `origin/HEAD` pair every generated fixture carries.
    #[test]
    fn generated_fixture_preserves_each_population() {
        let refs = |repo: &Path, glob: &str| {
            capture_git(repo, &["for-each-ref", "--format=%(refname:short)", glob])
                .lines()
                .count()
        };
        // `git worktree list` always includes the main worktree itself.
        let linked = |repo: &Path| {
            capture_git(repo, &["worktree", "list", "--porcelain"])
                .lines()
                .filter(|l| l.starts_with("worktree "))
                .count()
                - 1
        };

        // Cover each local zero once and add remote refs to one fixture.
        let fixture = FixtureRecipe::Generated {
            linked_worktrees: 3,
            branchless_branches: 0,
            remote_tracking_refs: 5,
        }
        .create();
        let repo = fixture.path().to_path_buf();
        assert_eq!(refs(&repo, "refs/heads/br-*"), 0, "no branchless branches");
        assert_eq!(refs(&repo, "refs/heads/wt-*"), 3);
        assert_eq!(refs(&repo, "refs/remotes/origin/remote-only-*"), 5);
        assert_eq!(linked(&repo), 3);

        let fixture = FixtureRecipe::Generated {
            linked_worktrees: 0,
            branchless_branches: 3,
            remote_tracking_refs: 0,
        }
        .create();
        let repo = fixture.path().to_path_buf();
        assert_eq!(refs(&repo, "refs/heads/br-*"), 3);
        assert_eq!(refs(&repo, "refs/heads/wt-*"), 0, "no worktree branches");
        assert_eq!(refs(&repo, "refs/remotes/origin/remote-only-*"), 0);
        assert_eq!(linked(&repo), 0);
    }

    /// [`add_diverged_backdrop`]'s own wiring — the half of the prune fixture
    /// that isn't the squash-merged candidates. Its promise to `wt step prune`
    /// is that every member is unintegrated (so each probe runs and *fails*)
    /// and that fork points fan across history (so `merge-base` walks real
    /// depth rather than bottoming out at the tip).
    ///
    /// Deliberately built with unequal populations: the sole production caller
    /// passes `backdrop_pairs` for both, so a swap of the
    /// `(linked_worktrees, branchless_branches)`
    /// parameters is invisible there and would be caught only here.
    #[test]
    fn diverged_backdrop_is_unintegrated_and_spread_across_history() {
        let fixture = create_simple(&SimpleRepoConfig {
            commits_on_main: 40,
            files: 2,
            total_worktrees: 1,
            worktree_commits_ahead: 0,
            worktree_uncommitted_files: 0,
        });
        let repo = fixture.path().to_path_buf();
        add_diverged_backdrop(&repo, 3, 4);

        // `<branch>..main` counts what main has that the branch doesn't — the
        // fork's depth below the tip; `main..<branch>` is the branch's own work.
        let counts = |branch: &str| {
            let count = |range: String| {
                capture_git(&repo, &["rev-list", "--count", &range])
                    .parse::<usize>()
                    .unwrap()
            };
            (
                count(format!("{branch}..main")),
                count(format!("main..{branch}")),
            )
        };

        // Orphan branches: 4 of them, 3 own commits each.
        let mut depths = Vec::new();
        for i in 0..4 {
            let branch = format!("unmerged-br-{i:03}");
            let (behind, ahead) = counts(&branch);
            assert_eq!(ahead, 3, "{branch} must carry its own commits");
            assert!(
                !run_git_ok(&repo, &["merge-base", "--is-ancestor", &branch, "main"]),
                "{branch} must not be integrated — the backdrop exists to fail every probe"
            );
            depths.push(behind);
        }
        // The GH #461 shape: forks fan out down history instead of clustering
        // at the tip. Index 0 samples the tip itself, so it alone starts at 0 —
        // the sole caller's `add_squash_merged` advances main past it next.
        assert_eq!(depths[0], 0, "the newest sample is main's own tip");
        assert!(
            depths.windows(2).all(|w| w[0] < w[1]),
            "fork depths must fan out across history: {depths:?}"
        );

        // Linked worktrees: 3 of them, 5 own commits each, and dirty only via
        // untracked scratch — no staged or unstaged tracked changes, which is
        // what keeps index-restoring helpers safe to run against them.
        for i in 0..3 {
            let branch = format!("unmerged-wt-{i}");
            let (_, ahead) = counts(&branch);
            assert_eq!(ahead, 5, "{branch} must carry its own commits");
            let wt = fixture.worktree_path(&branch);
            assert_eq!(
                status_lines(&wt),
                ["?? uncommitted_0.txt", "?? uncommitted_1.txt"],
                "{branch} must be dirty only via untracked scratch"
            );
        }
        assert!(
            !fixture.worktree_path("unmerged-wt-3").exists(),
            "worktree count must not follow the branch count"
        );
    }
}
