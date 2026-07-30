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
//! // Create a typical repo with 8 total worktrees.
//! let fixture = FixtureRecipe::Typical { total_worktrees: 8 }.create();
//!
//! // Invalidate caches for cold benchmark
//! invalidate_caches_auto(fixture.path());
//! ```
//!
//! See `wt-perf --help` for CLI usage.

use fs2::FileExt;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;
use tempfile::TempDir;
use worktrunk::testing::{allow_network_transports, configure_git_cmd, isolate_subprocess_env};

const LARGE_REPOSITORY_FIXTURE_MANIFEST: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../benches/large-repository-fixture"
));
static LARGE_REPOSITORY_IDENTITY: OnceLock<LargeRepositoryIdentity> = OnceLock::new();
const LARGE_REPOSITORY_BASE_REF: &str = "refs/wt-perf/large-repository-base";
/// The history window sampled by [`FixtureRecipe::LargeRepositoryHistorySpread`].
pub const LARGE_REPOSITORY_HISTORY_SPREAD_MAX_BRANCHES: usize = 5_000;

/// The pinned identity of the corpus used for large-repository benchmarks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LargeRepositoryIdentity {
    /// Fixture-layout schema.
    pub schema: u32,
    /// Upstream `owner/repository`.
    pub corpus: String,
    /// Full pinned commit object ID.
    pub revision: String,
}

/// Read the one tracked identity shared by fixture acquisition, CI caching,
/// and benchmark result metadata.
pub fn large_repository_identity() -> &'static LargeRepositoryIdentity {
    LARGE_REPOSITORY_IDENTITY.get_or_init(|| {
        parse_large_repository_identity(LARGE_REPOSITORY_FIXTURE_MANIFEST)
            .unwrap_or_else(|error| panic!("invalid benches/large-repository-fixture: {error}"))
    })
}

fn parse_large_repository_identity(content: &str) -> Result<LargeRepositoryIdentity, String> {
    let mut schema = None;
    let mut corpus = None;
    let mut revision = None;

    for (index, line) in content.lines().enumerate() {
        let line_number = index + 1;
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("line {line_number} must be key=value"))?;
        if value.is_empty() {
            return Err(format!("line {line_number} has an empty value"));
        }
        match key {
            "schema" => {
                let parsed = value
                    .parse::<u32>()
                    .map_err(|_| format!("line {line_number} has an invalid schema"))?;
                if schema.replace(parsed).is_some() {
                    return Err("duplicate schema".to_string());
                }
            }
            "corpus" => {
                if corpus.replace(value.to_string()).is_some() {
                    return Err("duplicate corpus".to_string());
                }
            }
            "revision" => {
                if revision.replace(value.to_string()).is_some() {
                    return Err("duplicate revision".to_string());
                }
            }
            _ => return Err(format!("line {line_number} has unknown key {key:?}")),
        }
    }

    let identity = LargeRepositoryIdentity {
        schema: schema.ok_or_else(|| "missing schema".to_string())?,
        corpus: corpus.ok_or_else(|| "missing corpus".to_string())?,
        revision: revision.ok_or_else(|| "missing revision".to_string())?,
    };
    if identity.schema != 1 {
        return Err(format!("unsupported schema {}", identity.schema));
    }
    if identity.revision.len() != 40
        || !identity
            .revision
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("revision must be a 40-character hexadecimal object ID".to_string());
    }
    Ok(identity)
}

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

/// Low-level parameters for the flat synthetic repository builder.
///
/// Benchmark callers name a [`FixtureRecipe`]. Keeping this type private
/// prevents arbitrary parameter combinations from becoming an accidental
/// second fixture catalog.
#[derive(Clone, Debug)]
struct FlatRepoConfig {
    /// Number of commits on main branch
    commits_on_main: usize,
    /// Number of files in the repo
    files: usize,
    /// Number of branches (without worktrees)
    branchless_branches: usize,
    /// Commits per branch
    commits_per_branch: usize,
    /// Number of worktrees (including main)
    total_worktrees: usize,
    /// Commits ahead of main per worktree
    worktree_commits_ahead: usize,
    /// Uncommitted files per worktree
    worktree_uncommitted_files: usize,
}

impl FlatRepoConfig {
    /// Typical repo with worktrees (500 commits, 100 files).
    ///
    /// Good for skeleton rendering and general worktree benchmarks.
    const fn typical(total_worktrees: usize) -> Self {
        Self {
            commits_on_main: 500,
            files: 100,
            branchless_branches: 0,
            commits_per_branch: 0,
            total_worktrees,
            worktree_commits_ahead: 10,
            worktree_uncommitted_files: 3,
        }
    }

    /// Branch-focused config (minimal history, many branches).
    const fn minimal(
        branchless_branches: usize,
        linked_worktrees: usize,
        commits_per_branch: usize,
    ) -> Self {
        Self {
            commits_on_main: 1,
            files: 1,
            branchless_branches,
            commits_per_branch,
            total_worktrees: linked_worktrees + 1,
            worktree_commits_ahead: 0,
            worktree_uncommitted_files: 0,
        }
    }

    /// Many divergent branches (GH #461 scenario: 200 branches × 20 commits).
    const fn synthetic_divergence() -> Self {
        Self {
            commits_on_main: 100,
            files: 50,
            branchless_branches: 200,
            commits_per_branch: 20,
            total_worktrees: 1,
            worktree_commits_ahead: 0,
            worktree_uncommitted_files: 0,
        }
    }

    /// Config for testing `wt switch` interactive picker (6 worktrees with varying commits).
    const fn picker_test() -> Self {
        Self {
            commits_on_main: 3,
            files: 3,
            branchless_branches: 2, // feature-000, feature-001 (no worktree)
            commits_per_branch: 0,
            total_worktrees: 6,
            worktree_commits_ahead: 15, // feature worktree has many commits
            worktree_uncommitted_files: 1,
        }
    }
}

/// The benchmark fixture catalog.
///
/// Variants are sparse, named repository states rather than combinations of
/// independent base, shape, and storage axes. Acquisition is selected
/// separately by `wt-perf setup` only where ownership differs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixtureRecipe {
    /// Standard synthetic history, files, and feature worktrees.
    Typical { total_worktrees: usize },
    /// Minimal history and files with configurable branch/worktree populations.
    Minimal {
        branchless_branches: usize,
        linked_worktrees: usize,
    },
    /// Fixed deep-divergence branch stress.
    SyntheticDivergence,
    /// Pinned large corpus with standard feature worktrees.
    LargeRepositoryWorktrees { total_worktrees: usize },
    /// Pinned large corpus with branches spread across history depth.
    LargeRepositoryHistorySpread { branchless_branches: usize },
    /// Varied branch and worktree state rotation.
    Mixed {
        linked_worktrees: usize,
        branchless_branches: usize,
    },
    /// Prune candidates and the unintegrated scan backdrop.
    Prune {
        candidate_pairs: usize,
        backdrop_pairs: usize,
    },
    /// Fixed interactive picker debugging fixture.
    PickerTest,
}

/// Population summary printed by `wt-perf setup`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixtureSummary {
    pub total_worktrees: usize,
    pub branchless_branches: usize,
}

impl FixtureRecipe {
    /// Build an owned ephemeral fixture.
    pub fn create(self) -> FixtureRepo {
        FixtureRepo::create(|repo| {
            self.create_at(repo);
        })
    }

    /// Build an ephemeral fixture at a caller-chosen primary-worktree path.
    pub fn create_at(self, base_path: &Path) -> FixtureSummary {
        match self {
            Self::Typical { total_worktrees } => {
                assert!(total_worktrees >= 1, "a fixture has a primary worktree");
                build_flat_repo_at(&FlatRepoConfig::typical(total_worktrees), base_path);
                FixtureSummary {
                    total_worktrees,
                    branchless_branches: 0,
                }
            }
            Self::Minimal {
                branchless_branches,
                linked_worktrees,
            } => {
                build_flat_repo_at(
                    &FlatRepoConfig::minimal(branchless_branches, linked_worktrees, 0),
                    base_path,
                );
                FixtureSummary {
                    total_worktrees: linked_worktrees + 1,
                    branchless_branches,
                }
            }
            Self::SyntheticDivergence => {
                let config = FlatRepoConfig::synthetic_divergence();
                build_flat_repo_at(&config, base_path);
                FixtureSummary {
                    total_worktrees: config.total_worktrees,
                    branchless_branches: config.branchless_branches,
                }
            }
            Self::LargeRepositoryWorktrees { total_worktrees } => {
                assert!(total_worktrees >= 1, "a fixture has a primary worktree");
                clone_large_repository_at(base_path);
                add_flat_worktrees(&FlatRepoConfig::typical(total_worktrees), base_path);
                FixtureSummary {
                    total_worktrees,
                    branchless_branches: 0,
                }
            }
            Self::LargeRepositoryHistorySpread {
                branchless_branches,
            } => {
                assert!(
                    branchless_branches <= LARGE_REPOSITORY_HISTORY_SPREAD_MAX_BRANCHES,
                    "large-repository history spread supports at most \
                     {LARGE_REPOSITORY_HISTORY_SPREAD_MAX_BRANCHES} branches"
                );
                clone_large_repository_at(base_path);
                add_history_spread_branches(base_path, branchless_branches);
                FixtureSummary {
                    total_worktrees: 1,
                    branchless_branches,
                }
            }
            Self::Mixed {
                linked_worktrees,
                branchless_branches,
            } => {
                build_mixed_repo_at(linked_worktrees, branchless_branches, base_path);
                FixtureSummary {
                    total_worktrees: linked_worktrees + 1,
                    branchless_branches,
                }
            }
            Self::Prune {
                candidate_pairs,
                backdrop_pairs,
            } => {
                build_prune_repo_at(candidate_pairs, backdrop_pairs, base_path);
                FixtureSummary {
                    total_worktrees: candidate_pairs + backdrop_pairs + 1,
                    branchless_branches: candidate_pairs + backdrop_pairs,
                }
            }
            Self::PickerTest => {
                let config = FlatRepoConfig::picker_test();
                build_flat_repo_at(&config, base_path);
                FixtureSummary {
                    total_worktrees: config.total_worktrees,
                    branchless_branches: config.branchless_branches,
                }
            }
        }
    }
}

/// Add the standard linked-worktree population to an existing typical fixture.
///
/// Destructive benchmarks use this after committing scenario-specific
/// configuration on the primary worktree but before recording a candidate.
pub fn add_typical_linked_worktrees(repo_path: &Path, linked_worktrees: usize) {
    add_flat_worktrees(&FlatRepoConfig::typical(linked_worktrees + 1), repo_path);
}

/// Remove a generated fixture before rebuilding it at the same default path.
///
/// Linked worktrees live beside the primary worktree, not underneath it.
/// Resolve their exact registered paths before removing the primary so setup
/// can rebuild without globbing over unrelated siblings.
pub fn remove_fixture_for_rebuild(repo_path: &Path) {
    if !path_exists(repo_path) {
        return;
    }

    if path_exists(&repo_path.join(".git")) {
        let primary = std::fs::canonicalize(repo_path).unwrap_or_else(|error| {
            panic!(
                "failed to resolve fixture primary worktree {}: {error}",
                repo_path.display()
            )
        });
        let registered = registered_worktrees(repo_path).unwrap_or_else(|| {
            panic!(
                "refusing to remove fixture {}: worktree registrations are not an intact generated fixture",
                repo_path.display()
            )
        });
        assert_eq!(
            registered.get("main"),
            Some(&primary),
            "refusing to remove fixture {}: its primary worktree is not main",
            repo_path.display()
        );

        // Validate the complete deletion set before removing any path. A
        // manually-added or corrupted worktree may point anywhere; only the
        // sibling path derived from its branch is inside this generated
        // fixture's namespace.
        let mut linked = Vec::new();
        for (branch, worktree) in registered {
            if branch == "main" {
                continue;
            }
            let expected = linked_worktree_path(&primary, &branch);
            let expected = std::fs::canonicalize(&expected).unwrap_or_else(|error| {
                panic!(
                    "refusing to remove fixture {}: expected linked worktree {} is missing: {error}",
                    repo_path.display(),
                    expected.display()
                )
            });
            assert_eq!(
                worktree,
                expected,
                "refusing to remove fixture {}: branch {branch} is registered outside its generated path",
                repo_path.display()
            );
            linked.push(worktree);
        }

        for worktree in linked {
            remove_dir_if_exists(&worktree);
        }
    }

    remove_dir_if_exists(repo_path);
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
fn build_flat_repo_at(config: &FlatRepoConfig, base_path: &Path) {
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

    // Create branches (without worktrees)
    for i in 0..config.branchless_branches {
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

    if config.branchless_branches > 0 {
        run_git(&repo_path, &["checkout", "main"]);
    }

    add_flat_worktrees(config, &repo_path);

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
fn add_flat_worktrees(config: &FlatRepoConfig, repo_path: &Path) {
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

    // Note: `packed-refs` is intentionally NOT removed. After `build_flat_repo_at`
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

/// Rebuild every worktree's index via `git reset -q`.
///
/// Used by [`LargeRepositoryPruneFixture::acquire`] to heal a fixture whose
/// indexes were deleted by legacy `wt-perf invalidate` behavior or other
/// damage (a missing index reads as all-tracked-files-deleted and flips
/// prune's clean-worktree gate). `git reset --mixed` discards
/// staged-but-uncommitted index state, so this is safe only on fixtures whose
/// dirt is untracked files.
fn restore_worktree_indexes(repo_path: &Path) {
    let output = git_command()
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_path)
        .output()
        .unwrap();
    assert!(output.status.success(), "git worktree list failed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Parallel: each reset rebuilds one worktree's index from its own HEAD,
    // fully independent of the others. On the large-repository fixture a
    // rebuild is ~2.5 s per worktree — serially that would dominate repair.
    std::thread::scope(|s| {
        for line in stdout.lines() {
            if let Some(path) = line.strip_prefix("worktree ") {
                s.spawn(move || run_git(Path::new(path), &["reset", "-q"]));
            }
        }
    });
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
/// for the synthetic `setup` fixtures — rebuilt in seconds — but a deliberate
/// cost for the ~15 GiB large-repository fixture under `bench-repos/`, which then re-clones
/// per worktree and after every `cargo clean`. Relocate it with cargo's own
/// `CARGO_TARGET_DIR` if that cost bites.
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

fn large_repository_cache_dir() -> PathBuf {
    wt_perf_fixture_dir()
        .join("bench-repos")
        .join("large-repository")
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

fn ready_marker_matches(marker: &Path) -> bool {
    match std::fs::read_to_string(marker) {
        Ok(content) => content == LARGE_REPOSITORY_FIXTURE_MANIFEST,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => panic!(
            "failed to read fixture marker {}: {error}",
            marker.display()
        ),
    }
}

fn write_ready_marker(path: &Path) {
    let parent = path.parent().unwrap();
    let mut marker = tempfile::NamedTempFile::new_in(parent)
        .unwrap_or_else(|error| panic!("failed to create fixture ready marker: {error}"));
    marker
        .write_all(LARGE_REPOSITORY_FIXTURE_MANIFEST.as_bytes())
        .unwrap_or_else(|error| panic!("failed to write fixture ready marker: {error}"));
    marker
        .as_file()
        .sync_all()
        .unwrap_or_else(|error| panic!("failed to sync fixture ready marker: {error}"));
    marker
        .persist(path)
        .unwrap_or_else(|error| panic!("failed to publish fixture ready marker: {}", error.error));
}

fn repository_head(repo: &Path) -> Option<String> {
    let output = git_command()
        .args(["rev-parse", "HEAD"])
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

fn large_repository_source_matches(repo: &Path) -> bool {
    let marker = large_repository_cache_dir().join("source.ready");
    ready_marker_matches(&marker)
        && path_exists(repo)
        && repository_head(repo).as_deref() == Some(large_repository_identity().revision.as_str())
        && repository_is_on_main(repo)
}

/// Get or build the immutable source for large-repository benchmarks.
///
/// The build lock covers validation and construction. A ready marker is
/// published only after the exact pinned revision is checked out, so an
/// interrupted clone is rebuilt by the next caller.
fn ensure_large_repository_source() -> PathBuf {
    let cache_dir = large_repository_cache_dir();
    let source = cache_dir.join("source");
    let ready_marker = cache_dir.join("source.ready");
    let _build_lock = acquire_exclusive_lock(&cache_dir.join("source.lock"));

    if large_repository_source_matches(&source) {
        eprintln!(
            "Using cached large-repository source at {}",
            source.display()
        );
        return source;
    }
    if path_exists(&source) {
        eprintln!("Cached large-repository source does not match the fixture manifest; rebuilding");
        remove_dir_if_exists(&source);
    }
    remove_file_if_exists(&ready_marker);

    let identity = large_repository_identity();
    let url = format!("https://github.com/{}.git", identity.corpus);
    eprintln!(
        "Cloning {} at {} (this will take several minutes)...",
        identity.corpus, identity.revision
    );
    let mut clone = git_command();
    allow_network_transports(&mut clone);
    let output = clone
        .args(["clone", "--no-checkout", &url])
        .arg(&source)
        .output()
        .unwrap_or_else(|error| panic!("failed to spawn large-repository clone: {error}"));
    assert!(
        output.status.success(),
        "failed to clone {}:\n{}",
        identity.corpus,
        String::from_utf8_lossy(&output.stderr)
    );
    run_git(
        &source,
        &["checkout", "--detach", identity.revision.as_str()],
    );
    run_git(
        &source,
        &["branch", "-f", "main", identity.revision.as_str()],
    );
    run_git(&source, &["checkout", "main"]);
    assert_eq!(
        repository_head(&source).as_deref(),
        Some(identity.revision.as_str()),
        "large-repository source did not check out the pinned revision"
    );
    write_ready_marker(&ready_marker);
    eprintln!("Large-repository source cloned successfully");
    source
}

/// Local-clone the pinned large-repository source to `dest` and configure a
/// git user for fixture commits.
fn clone_large_repository_at(dest: &Path) {
    let source = ensure_large_repository_source();
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
        "Failed to local-clone large-repository source: {}",
        String::from_utf8_lossy(&clone_output.stderr)
    );
    run_git(dest, &["config", "user.name", "Benchmark"]);
    run_git(dest, &["config", "user.email", "bench@test.com"]);
    assert_eq!(
        repository_head(dest).as_deref(),
        Some(large_repository_identity().revision.as_str()),
        "large-repository clone did not retain the pinned revision"
    );
    assert!(
        repository_is_on_main(dest),
        "large-repository clone must check out the pinned main branch"
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
    // (the synthetic fixtures have a few hundred commits) dividing by the cap
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
/// `feature-NNN` branches pointing at them. None carries its own commits, so
/// every branch is an ancestor of the tip — behind it, except `feature-000`:
/// the newest sample is the tip itself, so that one sits exactly on it.
fn add_history_spread_branches(repo_path: &Path, branchless_branches: usize) {
    let commits = history_spread_shas(repo_path, branchless_branches);
    assert_eq!(
        commits.len(),
        branchless_branches,
        "history-spread fixture needs at least {branchless_branches} commits"
    );
    for (i, commit) in commits.iter().enumerate() {
        add_branch_with_commits(repo_path, &format!("feature-{i:03}"), commit, 0);
    }
}

/// Add two-sided-diverged linked worktrees (`feature-wt-N`) and branchless
/// branches (`feature-NNN`) to an existing repo.
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
            &format!("feature-{i:03}"),
            &forks[i % forks.len()],
            3,
        );
    }

    for i in 0..linked_worktrees {
        let wt_branch = format!("feature-wt-{i}");
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
/// working-tree edits in the mixed-state fixture.
fn append_line(path: &Path, rel: &str, line: &str) {
    let file = path.join(rel);
    let mut content = std::fs::read_to_string(&file).unwrap_or_default();
    content.push_str(line);
    content.push('\n');
    std::fs::write(&file, content).unwrap();
}

/// Create a repo with linked worktrees and branchless
/// branches, each in a deterministic rotation of states, for the combined
/// full-surface `wt list` benchmark (`full` in `benches/list.rs`).
///
/// Unlike the flat recipes (every worktree/branch identical), this exercises the
/// full spread of `wt list` gates and tasks at once — clean vs dirty working
/// trees, merged vs ahead vs diverged branches, *and* divergence spread across
/// history depth — the realistic shape of "a huge number of worktrees &
/// branches, all in various states". Returns an owned fixture; the main
/// worktree is available through [`FixtureRepo::path`], and linked worktrees
/// through [`FixtureRepo::worktree_path`]. Either dimension may be `0` (e.g.
/// `mixed-W-0` for a worktrees-only repo).
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
fn build_mixed_repo_at(linked_worktrees: usize, branchless_branches: usize, repo: &Path) {
    const FILES: usize = 50;
    // Deep enough that fork points spread across history give the
    // `%(ahead-behind)` walk real commits to traverse (GH #461 shape), while
    // staying far cheaper to build than the dedicated `divergent` stress
    // (`FixtureRecipe::SyntheticDivergence`, 200 branches × 20 commits).
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
    run_git(&repo, &["gc", "-q"]);

    // Linked worktrees are siblings named `<repo-dir>.<branch>` (worktrunk
    // convention), derived from the repo's own directory name so the path is
    // correct whether the repo is the tempdir's `repo` or a custom `setup` path.
    for j in 0..linked_worktrees {
        let branch = format!("wt-{j:04}");
        let wt = linked_worktree_path(&repo, &branch);
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

/// Create a repo shaped like a `wt step prune` workload at `base_path`.
///
/// `wt step prune` integration-checks every linked worktree and local branch,
/// then removes the integrated ones. Two populations drive its cost:
///
/// - **Candidates** (`candidate_pairs`): squash-merged worktrees
///   (`merged-wt-N`) and squash-merged orphan branches (`merged-br-N`). Each
///   carries its own commits whose content also landed on main as a single
///   squash commit, so it is integrated *by content* (the `merge-tree` probes),
///   not by ancestry — the post-PR-squash shape prune typically removes.
/// - **Backdrop** (`backdrop_pairs`): two-sided-diverged linked worktrees
///   and orphan branches (`add_diverged_backdrop` — forked at points spread
///   across history, with their own commits, while main advanced past them).
///   Scanned on every run, never removed — the steady state that dominates
///   scan cost, and the shape where merge-base walks and merge-tree
///   three-ways do real work rather than short-circuiting at the tip.
///
/// The main history is mature (200 commits, 100 files) so `git status` and the
/// integration probes pay realistic per-worktree costs.
fn build_prune_repo_at(candidate_pairs: usize, backdrop_pairs: usize, base_path: &Path) {
    let config = FlatRepoConfig {
        commits_on_main: 200,
        files: 100,
        branchless_branches: 0,
        commits_per_branch: 0,
        total_worktrees: 1,
        worktree_commits_ahead: 0,
        worktree_uncommitted_files: 0,
    };
    build_flat_repo_at(&config, base_path);
    add_prune_populations(base_path, candidate_pairs, backdrop_pairs);
    // The squash commits advanced main past the fake remote ref written by
    // `build_flat_repo_at`; refresh so origin/main tracks the final tip.
    setup_fake_remote(base_path);
}

/// Add `count` squash-merged worktrees (`merged-wt-N`) and `count`
/// squash-merged orphan branches (`merged-br-N`) to an existing repo.
///
/// Each branch gets its own commits, then the default branch checked out in
/// the primary worktree takes the same content as one
/// `git merge --squash` commit — integrated by content, so `wt step prune`
/// detects it via the merge-tree probes and removes it.
///
/// `round` uniquifies the committed file names. The cached large-repository
/// fixture increments it when repairing candidates consumed by a live prune;
/// reusing a round would make the squash merge empty because the content is
/// already on main. Branch names intentionally do not carry the round, so a
/// name collision fails loudly.
pub fn add_squash_merged(repo_path: &Path, count: usize, round: usize) {
    let default_branch = capture_git(repo_path, &["rev-parse", "--abbrev-ref", "HEAD"]);

    // Two commits in `dir`, each adding a round-uniquified file (the branch
    // name already carries the candidate index). The add is targeted — a bare
    // `git add .` would rescan the whole tree, which on rust-lang/rust costs
    // seconds per commit.
    let commit_branch_content = |dir: &Path, branch: &str| {
        for j in 0..2 {
            let name = format!("{}_{round}_{j}.rs", branch.replace('-', "_"));
            std::fs::write(dir.join(&name), format!("// {branch} {round}/{j}\n")).unwrap();
            run_git(dir, &["add", &name]);
            run_git(
                dir,
                &[
                    "commit",
                    "-q",
                    "-m",
                    &format!("{branch} commit {j} (round {round})"),
                ],
            );
        }
    };
    // Land the branch's content on the default branch as one squash commit.
    let squash_into_default = |branch: &str| {
        run_git(repo_path, &["merge", "--squash", "-q", branch]);
        run_git(
            repo_path,
            &[
                "commit",
                "-q",
                "-m",
                &format!("Squash-merge {branch} (round {round})"),
            ],
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
/// The base repo is the only thing the synthetic (`build_prune_repo_at`) and
/// large-repository (`create_large_repository_prune_at`) prune fixtures differ
/// in. Both layer these populations on top.
fn add_prune_populations(base_path: &Path, candidate_pairs: usize, backdrop_pairs: usize) {
    add_diverged_backdrop(base_path, backdrop_pairs, backdrop_pairs);
    add_squash_merged(base_path, candidate_pairs, 0);
}

/// Default populations for the large-repository prune fixture:
/// 12 squash-merged candidates of each kind + 24 unmerged worktrees and
/// branches → 36 linked worktrees, and a live prune that removes 24
/// candidates while keeping 72 unmerged items — the "dozens of worktrees,
/// lots removed, lots kept" shape where prune takes multiple seconds.
pub const PRUNE_LARGE_REPOSITORY_CANDIDATE_PAIRS: usize = 12;
pub const PRUNE_LARGE_REPOSITORY_BACKDROP_PAIRS: usize = 24;

/// Create a large-repository `wt step prune` workload at `base_path`.
///
/// Local-clones the pinned corpus (the first call may clone from the network)
/// and adds the same two populations as
/// `build_prune_repo_at`: squash-merged candidates of each kind
/// ([`add_squash_merged`]) against a two-sided-diverged backdrop
/// worktrees and branches forked across the last 5000 commits
/// (`add_diverged_backdrop`). This is the shape where prune's costs are
/// real — merge-base walks over deep history, `merge-tree` three-ways over
/// ~400 MiB trees, `git status` over ~60k files per worktree — and reproduces
/// the "prune takes seconds" experience that small synthetic fixtures can't
/// (their probes bottom out at subprocess-spawn cost).
///
/// Each linked worktree materializes a full working tree: ~400 MiB and ~3 s
/// per worktree, so the default populations build in minutes and take ~15 GiB.
/// Prefer [`LargeRepositoryPruneFixture::acquire`], which builds once into
/// `target/wt-perf/bench-repos` and repairs consumed candidates on later runs.
fn create_large_repository_prune_at(
    candidate_pairs: usize,
    backdrop_pairs: usize,
    base_path: &Path,
) {
    clone_large_repository_at(base_path);
    run_git(
        base_path,
        &[
            "update-ref",
            LARGE_REPOSITORY_BASE_REF,
            large_repository_identity().revision.as_str(),
        ],
    );
    add_prune_populations(base_path, candidate_pairs, backdrop_pairs);
}

/// How a cached prune fixture compares to its expected populations.
#[derive(Debug, PartialEq, Eq)]
enum PruneFixtureState {
    /// Backdrop and candidates all present.
    Intact,
    /// Backdrop intact, candidates fully consumed — a live prune ran.
    /// Repairable by re-running [`add_squash_merged`] with a fresh round.
    Consumed,
    /// Anything else (partial removal, corruption) — rebuild from scratch.
    Broken,
}

/// Classify a prune fixture (`build_prune_repo_at` /
/// `create_large_repository_prune_at` layout) against its expected populations.
///
/// Exact ref names, branch registrations, and canonical worktree paths are the
/// fixture invariants. A live prune removes exactly the `merged-*` refs and
/// registrations, which is the [`PruneFixtureState::Consumed`] signature.
/// `expected_base_revision` additionally pins the managed large-repository
/// fixture's private base ref and requires that base to remain an ancestor of
/// `main`.
fn prune_fixture_state(
    repo: &Path,
    candidate_pairs: usize,
    backdrop_pairs: usize,
    expected_base_revision: Option<&str>,
) -> PruneFixtureState {
    if !repository_is_on_main(repo) {
        return PruneFixtureState::Broken;
    }

    if let Some(revision) = expected_base_revision
        && (capture_git_oid(repo, LARGE_REPOSITORY_BASE_REF).as_deref() != Some(revision)
            || !run_git_ok(repo, &["merge-base", "--is-ancestor", revision, "main"]))
    {
        return PruneFixtureState::Broken;
    }

    let Some(actual_worktrees) = registered_worktrees(repo) else {
        return PruneFixtureState::Broken;
    };
    let actual_branches = capture_git(repo, &["for-each-ref", "--format=%(refname)", "refs/heads"])
        .lines()
        .map(str::to_string)
        .collect::<BTreeSet<_>>();

    let backdrop_branches = expected_prune_branches(0, backdrop_pairs);
    let backdrop_worktrees = expected_prune_worktrees(repo, 0, backdrop_pairs);
    let intact_branches = expected_prune_branches(candidate_pairs, backdrop_pairs);
    let intact_worktrees = expected_prune_worktrees(repo, candidate_pairs, backdrop_pairs);

    if actual_branches == intact_branches && actual_worktrees == intact_worktrees {
        PruneFixtureState::Intact
    } else if actual_branches == backdrop_branches && actual_worktrees == backdrop_worktrees {
        PruneFixtureState::Consumed
    } else {
        PruneFixtureState::Broken
    }
}

fn capture_git_oid(repo: &Path, reference: &str) -> Option<String> {
    let output = git_command()
        .args(["rev-parse", "--verify", reference])
        .current_dir(repo)
        .output()
        .unwrap_or_else(|error| {
            panic!(
                "failed to inspect {reference} in {}: {error}",
                repo.display()
            )
        });
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn registered_worktrees(repo: &Path) -> Option<BTreeMap<String, PathBuf>> {
    let output = git_command()
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo)
        .output()
        .unwrap_or_else(|error| {
            panic!(
                "failed to inspect worktree registrations in {}: {error}",
                repo.display()
            )
        });
    if !output.status.success() {
        return None;
    }

    let mut registrations = BTreeMap::new();
    for record in String::from_utf8_lossy(&output.stdout).trim().split("\n\n") {
        let path = record
            .lines()
            .find_map(|line| line.strip_prefix("worktree "))
            .map(PathBuf::from)?;
        let branch = record
            .lines()
            .find_map(|line| line.strip_prefix("branch refs/heads/"))?
            .to_string();
        let path = std::fs::canonicalize(&path).ok()?;
        if registrations.insert(branch, path).is_some() {
            return None;
        }
    }
    Some(registrations)
}

fn expected_prune_branches(candidate_pairs: usize, backdrop_pairs: usize) -> BTreeSet<String> {
    let mut branches = BTreeSet::from(["refs/heads/main".to_string()]);
    for i in 0..backdrop_pairs {
        branches.insert(format!("refs/heads/feature-{i:03}"));
        branches.insert(format!("refs/heads/feature-wt-{i}"));
    }
    for i in 0..candidate_pairs {
        branches.insert(format!("refs/heads/merged-br-{i}"));
        branches.insert(format!("refs/heads/merged-wt-{i}"));
    }
    branches
}

fn expected_prune_worktrees(
    repo: &Path,
    candidate_pairs: usize,
    backdrop_pairs: usize,
) -> BTreeMap<String, PathBuf> {
    let mut worktrees = BTreeMap::new();
    worktrees.insert(
        "main".to_string(),
        std::fs::canonicalize(repo).unwrap_or_else(|error| {
            panic!(
                "failed to resolve primary worktree {}: {error}",
                repo.display()
            )
        }),
    );
    for i in 0..backdrop_pairs {
        insert_expected_worktree(repo, &mut worktrees, format!("feature-wt-{i}"));
    }
    for i in 0..candidate_pairs {
        insert_expected_worktree(repo, &mut worktrees, format!("merged-wt-{i}"));
    }
    worktrees
}

fn insert_expected_worktree(
    repo: &Path,
    worktrees: &mut BTreeMap<String, PathBuf>,
    branch: String,
) {
    let path = linked_worktree_path(repo, &branch);
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    worktrees.insert(branch, path);
}

fn large_repository_prune_fixture_state(
    repo: &Path,
    candidate_pairs: usize,
    backdrop_pairs: usize,
) -> PruneFixtureState {
    prune_fixture_state(
        repo,
        candidate_pairs,
        backdrop_pairs,
        Some(large_repository_identity().revision.as_str()),
    )
}

/// The next [`add_squash_merged`] round for a fixture repo, derived from the
/// repo itself: each completed round leaves `merged_br_0_<round>_*.rs` files
/// on the default branch's tip tree (the squash commits survive candidate
/// removal), so the number of distinct rounds already landed IS the next
/// round index. Derived rather than stored — a sidecar counter can desync
/// from the repo (interrupted repair, hand cleanup) and turn a ~1-minute
/// repair into a name-collision panic.
fn next_squash_round(repo: &Path) -> usize {
    capture_git(repo, &["ls-tree", "--name-only", "HEAD"])
        .lines()
        .filter(|l| l.starts_with("merged_br_0_") && l.ends_with("_0.rs"))
        .count()
}

/// An exclusive lease on the mutable large-repository prune fixture.
///
/// Dropping the value releases the cross-process lock. Callers keep it alive
/// for every read or mutation of [`Self::path`].
pub struct LargeRepositoryPruneFixture {
    repo: PathBuf,
    _lock: File,
}

impl LargeRepositoryPruneFixture {
    /// Acquire the cached mutable fixture and hold its exclusive lease.
    pub fn acquire(candidate_pairs: usize, backdrop_pairs: usize) -> Self {
        acquire_large_repository_prune_fixture(candidate_pairs, backdrop_pairs)
    }

    pub fn path(&self) -> &Path {
        &self.repo
    }
}

/// Get or build the cached large-repository prune fixture.
///
/// The fixture lives at
/// `target/wt-perf/bench-repos/large-repository/prune-<candidate_pairs>-<backdrop_pairs>/repo`
/// (worktrees as siblings) so its minutes-long build is paid once, not per
/// bench run. On reuse it is validated by
/// `prune_fixture_state`:
///
/// - `Intact` → returned as-is (dry runs don't mutate it).
/// - `Consumed` — a live `wt step prune` removed the candidates — → repaired
///   in place by re-running [`add_squash_merged`] with the next round
///   (`next_squash_round`), so a live-prune measurement costs a ~1-minute
///   repair, not a full rebuild.
/// - `Broken` (interrupted prune or build, corruption) → wiped and rebuilt.
///
/// Worktree indexes missing from a legacy cache invalidation or other damage
/// are healed first with `restore_worktree_indexes` — without an index, `git
/// status` reports every tracked file as a staged deletion and prune's
/// clean-worktree gate silently drops the worktree candidates. Safe here
/// because the fixture's only dirt is untracked files.
///
/// The returned lease holds an exclusive lock across validation, repair,
/// rebuild, and caller use. A ready marker is published only after a complete
/// build and must match the tracked corpus identity on every reuse.
fn acquire_large_repository_prune_fixture(
    candidate_pairs: usize,
    backdrop_pairs: usize,
) -> LargeRepositoryPruneFixture {
    let base = large_repository_cache_dir();
    let cache_dir = base.join(format!("prune-{candidate_pairs}-{backdrop_pairs}"));
    let repo = cache_dir.join("repo");
    let ready_marker = cache_dir.join("ready");
    let lock = acquire_exclusive_lock(
        &base.join(format!("prune-{candidate_pairs}-{backdrop_pairs}.lock")),
    );

    if ready_marker_matches(&ready_marker) {
        let worktrees_dir = repo.join(".git/worktrees");
        let linked_index_missing = match std::fs::read_dir(&worktrees_dir) {
            Ok(mut entries) => entries.any(|entry| {
                let entry = entry.unwrap_or_else(|error| {
                    panic!(
                        "failed to inspect worktree metadata in {}: {error}",
                        worktrees_dir.display()
                    )
                });
                !path_exists(&entry.path().join("index"))
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => panic!(
                "failed to read worktree metadata {}: {error}",
                worktrees_dir.display()
            ),
        };
        let index_missing = !path_exists(&repo.join(".git/index")) || linked_index_missing;
        if index_missing {
            eprintln!("Restoring invalidated worktree indexes...");
            restore_worktree_indexes(&repo);
        }
        match large_repository_prune_fixture_state(&repo, candidate_pairs, backdrop_pairs) {
            PruneFixtureState::Intact => {
                eprintln!("Using cached prune fixture at {}", repo.display());
                return LargeRepositoryPruneFixture { repo, _lock: lock };
            }
            PruneFixtureState::Consumed => {
                let round = next_squash_round(&repo);
                eprintln!(
                    "Re-creating {candidate_pairs} consumed squash-merged candidate pairs (round {round})..."
                );
                add_squash_merged(&repo, candidate_pairs, round);
                assert_eq!(
                    large_repository_prune_fixture_state(&repo, candidate_pairs, backdrop_pairs),
                    PruneFixtureState::Intact,
                    "repaired large-repository prune fixture did not match its recipe"
                );
                return LargeRepositoryPruneFixture { repo, _lock: lock };
            }
            PruneFixtureState::Broken => {
                eprintln!("Cached prune fixture unusable, rebuilding...");
            }
        }
    } else if path_exists(&cache_dir) {
        eprintln!("Cached prune fixture does not match the fixture manifest, rebuilding...");
    }

    eprintln!(
        "Building large-repository prune fixture: {} linked worktrees (one-time, cached)...",
        candidate_pairs + backdrop_pairs
    );
    // Clear remnants unconditionally: an interrupted build or rebuild can
    // leave sibling worktree dirs without `repo`, and `git worktree add`
    // fails on an existing non-empty destination.
    if path_exists(&cache_dir) {
        remove_dir_if_exists(&cache_dir);
    }
    std::fs::create_dir_all(&cache_dir).unwrap();
    create_large_repository_prune_at(candidate_pairs, backdrop_pairs, &repo);
    write_ready_marker(&ready_marker);
    LargeRepositoryPruneFixture { repo, _lock: lock }
}

/// Canonicalize path without Windows `\\?\` prefix.
pub fn canonicalize(path: &Path) -> std::io::Result<PathBuf> {
    dunce::canonicalize(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_flat(config: &FlatRepoConfig) -> FixtureRepo {
        FixtureRepo::create(|repo| build_flat_repo_at(config, repo))
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
    fn large_repository_manifest_has_one_strict_identity() {
        assert_eq!(
            parse_large_repository_identity(
                "schema=1\ncorpus=example/project\nrevision=0123456789abcdef0123456789abcdef01234567\n"
            ),
            Ok(LargeRepositoryIdentity {
                schema: 1,
                corpus: "example/project".to_string(),
                revision: "0123456789abcdef0123456789abcdef01234567".to_string(),
            })
        );
        assert_eq!(
            parse_large_repository_identity(
                "schema=1\ncorpus=example/project\ncorpus=other/project\nrevision=0123456789abcdef0123456789abcdef01234567\n"
            ),
            Err("duplicate corpus".to_string())
        );
        assert_eq!(
            parse_large_repository_identity("schema=1\ncorpus=example/project\nrevision=short\n"),
            Err("revision must be a 40-character hexadecimal object ID".to_string())
        );
        assert_eq!(
            large_repository_identity(),
            &LargeRepositoryIdentity {
                schema: 1,
                corpus: "rust-lang/rust".to_string(),
                revision: "be3d26db984c6f96335faca1f254dc04873cb1c1".to_string(),
            }
        );
    }

    #[test]
    fn fixture_ready_marker_matches_only_the_tracked_identity() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("ready");

        assert!(!ready_marker_matches(&marker));
        write_ready_marker(&marker);
        assert!(ready_marker_matches(&marker));

        std::fs::write(
            &marker,
            LARGE_REPOSITORY_FIXTURE_MANIFEST.replace("schema=1", "schema=2"),
        )
        .unwrap();
        assert!(!ready_marker_matches(&marker));
    }

    #[test]
    #[should_panic(expected = "failed to read fixture marker")]
    fn fixture_ready_marker_reports_read_errors() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("ready");
        std::fs::create_dir(&marker).unwrap();

        ready_marker_matches(&marker);
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
    fn fixture_rebuild_refuses_registered_worktrees_outside_its_namespace() {
        let fixture = FixtureRecipe::Minimal {
            branchless_branches: 0,
            linked_worktrees: 1,
        }
        .create();
        let external_root = tempfile::tempdir().unwrap();
        let external = external_root.path().join("manual-worktree");
        run_git(
            fixture.path(),
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "manual",
                external.to_str().unwrap(),
            ],
        );
        std::fs::write(external.join("sentinel"), "keep").unwrap();

        let result = std::panic::catch_unwind(|| remove_fixture_for_rebuild(fixture.path()));
        assert!(
            result.is_err(),
            "rebuild must fail rather than delete a registered external worktree"
        );
        assert!(
            fixture.path().is_dir(),
            "primary must survive failed validation"
        );
        assert!(
            fixture.worktree_path("feature-wt-1").is_dir(),
            "generated linked worktrees must survive failed validation"
        );
        assert_eq!(
            std::fs::read_to_string(external.join("sentinel")).unwrap(),
            "keep"
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
        let fixture = create_flat(&FlatRepoConfig {
            commits_on_main: 2,
            files: 2,
            branchless_branches: 0,
            commits_per_branch: 0,
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
        let fixture = FixtureRecipe::Typical { total_worktrees: 1 }.create();
        let git_dir = resolve_git_common_dir(fixture.path()).unwrap();
        std::fs::create_dir_all(git_dir.join("wt")).unwrap();
        std::fs::write(git_dir.join("wt/cache"), "not a directory\n").unwrap();

        invalidate_probe_caches(fixture.path());
    }

    /// The `full` fixture's contract: [`FixtureRecipe::Mixed`] promises a
    /// deterministic `index % 4` rotation of branch and worktree states, and
    /// every `wt list` gate the `full` bench exercises hangs off that rotation.
    /// Nothing else pins it — the bench measures one wall time, so a generator
    /// change that collapsed (say) "diverged" into "ahead" would keep the bench
    /// green while silently measuring a different repo. Assert the states
    /// directly, via `merge-base --is-ancestor` exit codes and porcelain status.
    #[test]
    fn mixed_fixture_states_follow_the_documented_rotation() {
        // Two full rotations of each 4-state cycle, so a state that collapsed
        // into its neighbour fails on both of its indices rather than one.
        const N: usize = 8;
        let fixture = FixtureRecipe::Mixed {
            linked_worktrees: N,
            branchless_branches: N,
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
    /// prune benchmark stops removing anything. Round 1 re-creation must keep
    /// the property (unique file content per round).
    #[test]
    fn squash_merged_fixture_is_content_integrated() {
        let fixture = create_flat(&FlatRepoConfig {
            commits_on_main: 3,
            files: 2,
            branchless_branches: 0,
            commits_per_branch: 0,
            total_worktrees: 1,
            worktree_commits_ahead: 0,
            worktree_uncommitted_files: 0,
        });
        let repo_path = fixture.path().to_path_buf();

        for round in 0..2 {
            add_squash_merged(&repo_path, 1, round);

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
                assert!(
                    merged_tree.status.success(),
                    "merge-tree failed (round {round})"
                );
                assert_eq!(
                    String::from_utf8_lossy(&merged_tree.stdout).trim(),
                    String::from_utf8_lossy(&main_tree.stdout).trim(),
                    "{branch} must merge into main without adding changes (round {round})"
                );
            }

            // Simulate the live benchmark's per-iteration cleanup before the
            // next round re-creates the candidates.
            let wt_path = fixture.worktree_path("merged-wt-0");
            run_git(
                &repo_path,
                &["worktree", "remove", "--force", wt_path.to_str().unwrap()],
            );
            run_git(&repo_path, &["branch", "-D", "merged-wt-0", "merged-br-0"]);
        }
    }

    /// The classifier that decides whether the cached large-repository fixture
    /// is reusable, repairable, or must be rebuilt
    /// ([`LargeRepositoryPruneFixture::acquire`]).
    /// Exercised on the synthetic fixture, which shares the exact layout.
    #[test]
    fn prune_fixture_state_classifies_lifecycle() {
        let fixture = FixtureRecipe::Prune {
            candidate_pairs: 1,
            backdrop_pairs: 2,
        }
        .create();
        let repo_path = fixture.path().to_path_buf();

        assert_eq!(
            prune_fixture_state(&repo_path, 1, 2, None),
            PruneFixtureState::Intact
        );
        let base = capture_git(&repo_path, &["rev-parse", "main~1"]);
        run_git(
            &repo_path,
            &["update-ref", LARGE_REPOSITORY_BASE_REF, &base],
        );
        assert_eq!(
            prune_fixture_state(&repo_path, 1, 2, Some(&base)),
            PruneFixtureState::Intact
        );
        assert_eq!(
            prune_fixture_state(&repo_path, 1, 2, Some(&head_sha(&repo_path))),
            PruneFixtureState::Broken,
            "the managed fixture base ref must match its declared identity"
        );
        run_git(&repo_path, &["update-ref", "-d", LARGE_REPOSITORY_BASE_REF]);
        run_git(&repo_path, &["checkout", "-q", "-b", "unexpected-primary"]);
        assert_eq!(
            prune_fixture_state(&repo_path, 1, 2, None),
            PruneFixtureState::Broken,
            "the primary worktree must remain on main"
        );
        run_git(&repo_path, &["checkout", "-q", "main"]);
        run_git(&repo_path, &["branch", "-D", "unexpected-primary"]);

        // Wrong expected populations don't match this repo.
        assert_eq!(
            prune_fixture_state(&repo_path, 2, 2, None),
            PruneFixtureState::Broken
        );

        // Partial consumption (worktree candidate gone, branches still there)
        // is Broken — an interrupted live prune needs a rebuild.
        let wt_path = fixture.worktree_path("merged-wt-0");
        run_git(
            &repo_path,
            &["worktree", "remove", "--force", wt_path.to_str().unwrap()],
        );
        assert_eq!(
            prune_fixture_state(&repo_path, 1, 2, None),
            PruneFixtureState::Broken
        );

        // Full consumption — exactly what a live prune leaves behind.
        run_git(&repo_path, &["branch", "-D", "merged-wt-0", "merged-br-0"]);
        assert_eq!(
            prune_fixture_state(&repo_path, 1, 2, None),
            PruneFixtureState::Consumed
        );

        // Repair restores Intact, with the round derived from the repo
        // itself (round 0's squash commits survive candidate removal).
        assert_eq!(next_squash_round(&repo_path), 1);
        add_squash_merged(&repo_path, 1, next_squash_round(&repo_path));
        assert_eq!(next_squash_round(&repo_path), 2);
        assert_eq!(
            prune_fixture_state(&repo_path, 1, 2, None),
            PruneFixtureState::Intact
        );

        // A missing backdrop branch is Broken.
        run_git(&repo_path, &["branch", "-D", "feature-000"]);
        assert_eq!(
            prune_fixture_state(&repo_path, 1, 2, None),
            PruneFixtureState::Broken
        );
    }

    #[test]
    fn prune_fixture_state_rejects_a_same_count_population_substitution() {
        let fixture = FixtureRecipe::Prune {
            candidate_pairs: 1,
            backdrop_pairs: 2,
        }
        .create();
        let repo = fixture.path();

        // Counts alone are insufficient validation for the mutable cached
        // fixture: replacing an expected backdrop ref with another ref under
        // the same prefix changes the measured workload without changing any
        // of the classifier's totals.
        run_git(repo, &["branch", "-m", "feature-000", "feature-substitute"]);
        assert_eq!(
            prune_fixture_state(repo, 1, 2, None),
            PruneFixtureState::Broken
        );

        run_git(repo, &["branch", "-m", "feature-substitute", "feature-000"]);
        let original = fixture.worktree_path("feature-wt-0");
        let moved = fixture.root().join("moved-feature-wt-0");
        run_git(
            repo,
            &[
                "worktree",
                "move",
                original.to_str().unwrap(),
                moved.to_str().unwrap(),
            ],
        );
        assert_eq!(
            prune_fixture_state(repo, 1, 2, None),
            PruneFixtureState::Broken,
            "registered worktrees must use the recipe's canonical paths"
        );
    }

    /// A zero-size spread is valid and must not divide by zero.
    #[test]
    fn history_spread_handles_zero_branches() {
        let fixture = create_flat(&FlatRepoConfig {
            commits_on_main: 3,
            files: 1,
            branchless_branches: 0,
            commits_per_branch: 0,
            total_worktrees: 1,
            worktree_commits_ahead: 0,
            worktree_uncommitted_files: 0,
        });
        let repo_path = fixture.path().to_path_buf();

        add_history_spread_branches(&repo_path, 0);
    }

    #[test]
    #[should_panic(expected = "supports at most 5000 branches")]
    fn history_spread_recipe_rejects_counts_beyond_its_window_before_acquisition() {
        let root = tempfile::tempdir().unwrap();
        FixtureRecipe::LargeRepositoryHistorySpread {
            branchless_branches: LARGE_REPOSITORY_HISTORY_SPREAD_MAX_BRANCHES + 1,
        }
        .create_at(&root.path().join("repo"));
    }

    /// The `mixed` fixture's second documented contract: either dimension may
    /// be `0` (`wt-perf setup mixed 3 0` is a worktrees-only repo). The branch
    /// loop divides by `branches` to fan fork points across history, so a zero
    /// there is a divide-by-zero the instant the body runs — it is safe only
    /// because `0..0` never enters, which is exactly the kind of guarantee a
    /// later "defensive" `branches.max(1)` would quietly break. Assert the
    /// resulting populations, not merely that nothing panicked, so such a fix
    /// fails here instead of silently adding a branch nobody asked for.
    #[test]
    fn mixed_fixture_allows_either_dimension_to_be_zero() {
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

        // The two dimensions share no state, so covering each zero once spans
        // the contract — a both-zero repo just skips both loops.
        let fixture = FixtureRecipe::Mixed {
            linked_worktrees: 3,
            branchless_branches: 0,
        }
        .create();
        let repo = fixture.path().to_path_buf();
        assert_eq!(refs(&repo, "refs/heads/br-*"), 0, "no branchless branches");
        assert_eq!(refs(&repo, "refs/heads/wt-*"), 3);
        assert_eq!(linked(&repo), 3);

        let fixture = FixtureRecipe::Mixed {
            linked_worktrees: 0,
            branchless_branches: 3,
        }
        .create();
        let repo = fixture.path().to_path_buf();
        assert_eq!(refs(&repo, "refs/heads/br-*"), 3);
        assert_eq!(refs(&repo, "refs/heads/wt-*"), 0, "no worktree branches");
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
        let fixture = create_flat(&FlatRepoConfig {
            commits_on_main: 40,
            files: 2,
            branchless_branches: 0,
            commits_per_branch: 0,
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
            let branch = format!("feature-{i:03}");
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
            let branch = format!("feature-wt-{i}");
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
            !fixture.worktree_path("feature-wt-3").exists(),
            "worktree count must not follow the branch count"
        );
    }
}
