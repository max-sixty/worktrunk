//! Shared test fixtures for worktrunk unit and integration tests.
//!
//! This module is `#[doc(hidden)] pub` so both library (`src/`) and binary
//! (`src/commands/`) unit tests, as well as integration tests (`tests/`), can
//! use it. Integration tests import via `worktrunk::testing::TestRepo`.
//!
//! ## TestRepo
//!
//! The `TestRepo` struct creates isolated git repositories in temporary directories
//! with deterministic timestamps and configuration. Each test gets a fresh repo
//! that is automatically cleaned up when the test ends.
//!
//! ## Constructors
//!
//! - [`TestRepo::new()`] — lightweight: `git init` + identity. For unit tests.
//! - [`TestRepo::with_initial_commit()`] — lightweight + one commit.
//! - [`TestRepo::bare()`] — bare repository (`git init --bare`). No working tree.
//! - [`TestRepo::at(path)`](TestRepo::at) — repo at a caller-specified path.
//!   For tests needing multiple repos in a shared directory.
//! - [`TestRepo::bare_at(path)`](TestRepo::bare_at) — bare repo at a
//!   caller-specified path (e.g. the `project/.git` clone layout).
//! - [`TestRepo::standard()`] — copies pre-built fixture with remote + worktrees.
//!   For integration tests (used by the `repo()` rstest fixture).
//! - [`TestRepo::standard_main_only()`] — copies the same pinned main history and
//!   remote without linked worktrees. For tests that shape their own topology.
//! - [`TestRepo::empty()`] — `git init` with no commits, no branches.
//!
//! ## Environment Isolation
//!
//! No `git` the suite runs reads the developer's `~/.gitconfig`. The
//! guarantee is `shell_exec::HERMETIC_TEST_GIT_ENV` — the deny pair pointing
//! `GIT_CONFIG_GLOBAL` and `GIT_CONFIG_SYSTEM` at a path that does not exist,
//! plus `user.useConfigOnly` through `GIT_CONFIG_COUNT` so a git with no
//! identity fails rather than guessing one from the host — applied to every
//! child at its spawn site. For the git that *production* code spawns while a
//! test drives it in-process, the spawn site is `Cmd`, and the harness has no
//! per-command hook there; instead the fixture constructors latch
//! `shell_exec::enable_hermetic_test_env`, and `Cmd` applies the floor to
//! every child while the latch is set. (A test cannot set its own process
//! environment instead — under `cargo test` tests are parallel threads, and
//! `std::env::set_var` beside them is the race that makes it `unsafe`; the
//! atomic latch is sound from any thread.)
//!
//! The rest applies the same floor at the spawn sites the harness does own:
//!
//! - [`git_test_env`] adds the test identity, pinned dates, and locale per
//!   command, which the floor leaves to the per-command layers.
//! - [`isolate_subprocess_env`] scrubs the host's `GIT_*` from `wt` children
//!   and re-applies the floor explicitly, so a subprocess denies host config
//!   just as this process does.
//!
//! On top of that isolation the helpers pin commit timestamps, locale, and
//! terminal width, all per command — no test mutates process-global state.

pub mod mock_commands;
pub mod mock_stub;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::sanitize_branch_name;
use crate::git::Repository;
use crate::shell_exec::{self, Cmd, INHERITED_GIT_PATH_VARS};
use path_slash::PathExt;

use self::mock_commands::{MockConfig, MockResponse, tea_api_include_stderr};

/// Path to the `wt` binary built by Cargo, pinned against concurrent builds.
///
/// Resolved at runtime from `CARGO_BIN_EXE_wt`, which cargo and nextest both
/// provide to integration-test processes, naming the binary the same
/// invocation just built — so the path can never be stale. Unit-test targets
/// get neither this variable nor a compile-time `option_env!` value (cargo
/// sets `CARGO_BIN_EXE_<name>` only for integration tests and benches), so
/// code that spawns `wt` — `mock_commands` included — belongs in integration
/// tests.
///
/// The returned path is not `CARGO_BIN_EXE_wt` itself but a hardlink to it
/// under `target/<profile>/wt-test-bin/<key>/` (see [`pin_test_binary`]):
/// cargo uplifts `target/debug/wt` by removing the path and recreating it, so
/// a concurrent `cargo build` in the same tree leaves the uplifted path absent
/// for a fraction of a millisecond per rebuild, failing whatever spawn is in
/// flight with `NotFound`. The hardlink keeps the observed binary's inode
/// alive whatever a concurrent cargo does to the uplifted path. Pinned once
/// per test process; every spawn helper routes through here, so no test
/// spawns the unlinkable path directly (`test_wt_spawns_are_pinned`).
///
/// Panics when the variable is absent (the test binary run outside a cargo
/// runner) rather than deriving a path that could name a stale binary.
pub fn wt_bin() -> PathBuf {
    static PINNED: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    PINNED
        .get_or_init(|| {
            let uplifted = PathBuf::from(
                std::env::var("CARGO_BIN_EXE_wt")
                    .expect("CARGO_BIN_EXE_wt not set — only available during `cargo test`"),
            );
            pin_test_binary(&uplifted)
        })
        .clone()
}

/// Pin `src` where a concurrent cargo can't unlink it, returning the pinned
/// path.
///
/// Hardlinks `src` into a sibling `wt-test-bin/<mtime>-<len>/` directory named
/// by the observed binary's identity, keeping `src`'s basename. The hardlink
/// pins the inode: cargo's uplift only unlinks the *path*, so the pinned entry
/// keeps serving the observed binary through any number of concurrent
/// rebuilds. An entry's marginal disk cost is near zero — where cargo uplifts
/// by hardlink (Linux) the pin shares the `deps/` artifact's inode outright,
/// and where it uplifts by copy-on-write clone (macOS/APFS) the pin keeps the
/// clone, whose blocks stay shared with that artifact (measured: cloning the
/// 70 MB binary consumes 8 KB) — the bytes belong to `deps/`, which cargo
/// already retains. Everything under `wt-test-bin/` dies with `cargo clean`,
/// so nothing sweeps it; a sweeper could unlink a generation another live
/// suite pinned, re-creating the very window this exists to close.
///
/// Two runs observing the same binary converge on the same entry (`link`
/// returning `AlreadyExists` is success — the key names the content); a run
/// observing a rebuilt binary creates a new entry beside the old one, exactly
/// as it would have spawned the rebuilt binary before pinning existed. A
/// `NotFound` from `stat` or `link` is the uplift window itself — the binary
/// is absent for well under a millisecond — so it's polled through rather than
/// surfaced (bounded; a genuinely missing binary still panics, with the path).
pub fn pin_test_binary(src: &Path) -> PathBuf {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match try_pin(src) {
            Ok(pinned) => return pinned,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "binary to pin stayed absent for 10s: {}",
                    src.display()
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(e) => panic!("failed to pin test binary {}: {e}", src.display()),
        }
    }
}

/// One pin attempt. `NotFound` means `src` was observed mid-uplift (or the
/// pin directory's ancestors vanished under a `cargo clean`); the caller
/// retries those.
fn try_pin(src: &Path) -> std::io::Result<PathBuf> {
    let meta = std::fs::metadata(src)?;
    let mtime = meta
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = src
        .parent()
        .expect("binary path has a parent")
        .join("wt-test-bin")
        .join(format!("{mtime:x}-{:x}", meta.len()));
    let pinned = dir.join(src.file_name().expect("binary path has a file name"));
    if pinned.exists() {
        return Ok(pinned);
    }
    std::fs::create_dir_all(&dir)?;
    match std::fs::hard_link(src, &pinned) {
        // A concurrent test process pinned the same key first; the key names
        // the content, so its entry is ours.
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => Err(e),
        _ => Ok(pinned),
    }
}

use tempfile::TempDir;

/// Bump when [`build_standard_fixture`] changes, so stale templates under
/// `target/` are abandoned rather than reused.
const STANDARD_FIXTURE_VERSION: u32 = 1;

/// Bump when the main-only transformation in
/// [`build_standard_main_only_fixture`] changes. Its cache key also includes
/// [`STANDARD_FIXTURE_VERSION`] because the derived template copies that
/// fixture as its source.
const STANDARD_MAIN_ONLY_FIXTURE_VERSION: u32 = 1;

/// Timestamp for the template's commits, author and committer alike.
///
/// The `-08:00` offset looks arbitrary but is load-bearing: commit SHAs
/// derive from it, and the fixture's SHAs (`05a4a45`, `1b87d47`, ...) appear
/// in ~170 committed snapshots. This value reproduces them exactly; the
/// guard is `test_standard_fixture_template_reproduces_pinned_shas`.
const STANDARD_FIXTURE_COMMIT_DATE: &str = "2025-01-01T00:00:00-08:00";

/// Worktree info returned from fixture copy.
struct FixtureWorktrees {
    worktrees: HashMap<String, PathBuf>,
    remote: PathBuf,
}

/// Get-or-create the standard fixture template under `target/<profile>/`.
///
/// The template is built by real git once per target directory, then copied
/// per test ([`copy_standard_fixture`]): building per test would cost
/// ~300 ms under nextest's process-per-test model, copying costs ~20 ms.
/// `cargo clean` removes the template with everything else.
fn standard_fixture_template() -> PathBuf {
    let mut base = std::env::current_exe().expect("failed to get test executable path");
    base.pop(); // test binary name
    base.pop(); // deps/
    let fixtures = base.join("wt-test-fixtures");
    let dir = fixtures.join(format!("standard-v{STANDARD_FIXTURE_VERSION}"));
    if !dir.exists() {
        claim_template(&fixtures, &dir, build_standard_fixture);
    }
    dir
}

/// Get-or-create the standard fixture's main-only topology.
///
/// Picker tests build every linked-worktree topology themselves. Copying this
/// template avoids immediately tearing down the standard fixture's three
/// linked worktrees with six serial Git subprocesses in every test.
fn standard_main_only_fixture_template() -> PathBuf {
    let mut base = std::env::current_exe().expect("failed to get test executable path");
    base.pop(); // test binary name
    base.pop(); // deps/
    let fixtures = base.join("wt-test-fixtures");
    let dir = fixtures.join(format!(
        "standard-v{STANDARD_FIXTURE_VERSION}-main-only-v{STANDARD_MAIN_ONLY_FIXTURE_VERSION}"
    ));
    if !dir.exists() {
        claim_template(&fixtures, &dir, build_standard_main_only_fixture);
    }
    dir
}

/// Build into a private scratch directory under `parent`, then rename it to
/// `dir` — the atomic claim that makes concurrent cold-cache builders race
/// benignly. `TempDir::new_in` names the scratch uniquely per call, so
/// builders can't collide on it whether they're separate nextest processes
/// or threads of one `cargo test` process; the first rename into place wins
/// and losers discard their build.
fn claim_template(parent: &Path, dir: &Path, build: impl FnOnce(&Path)) {
    std::fs::create_dir_all(parent).unwrap();
    let scratch = TempDir::new_in(parent).expect("create template build dir");
    build(scratch.path());

    // Disarm auto-cleanup: from here the scratch either becomes `dir` or is
    // removed explicitly on losing the race.
    let scratch = scratch.keep();
    match std::fs::rename(&scratch, dir) {
        Ok(()) => {}
        Err(_) if dir.exists() => {
            // Another builder won the race; use its template.
            std::fs::remove_dir_all(&scratch).ok();
        }
        Err(e) => panic!(
            "failed to move template into place at {}: {e}",
            dir.display()
        ),
    }
}

/// Build the standard fixture at `root`: `repo/` on `main` with one commit,
/// a bare `origin.git` remote (init + push, so the template records no
/// back-pointing remote), and three feature worktrees with one commit each.
fn build_standard_fixture(root: &Path) {
    let git = |dir: &Path| {
        configure_git_env(Cmd::new("git"))
            .env("GIT_AUTHOR_DATE", STANDARD_FIXTURE_COMMIT_DATE)
            .env("GIT_COMMITTER_DATE", STANDARD_FIXTURE_COMMIT_DATE)
            .current_dir(dir)
    };
    let run = |cmd: Cmd, what: &str| {
        let output = cmd.run().unwrap();
        check_git_status(&output, what);
    };

    let repo = root.join("repo");
    run(git(root).args(["init", "-q", "-b", "main", "repo"]), "init");
    std::fs::write(repo.join("file.txt"), "initial content\n").unwrap();
    std::fs::write(repo.join(".gitattributes"), "* text=auto eol=lf\n").unwrap();
    run(git(&repo).args(["add", "-A"]), "add -A");
    run(
        git(&repo).args(["commit", "-q", "-m", "Initial commit"]),
        "commit",
    );

    run(
        git(root).args(["init", "-q", "--bare", "-b", "main", "origin.git"]),
        "init --bare origin.git",
    );
    run(
        git(&repo).args(["remote", "add", "origin", "../origin.git"]),
        "remote add",
    );
    run(
        git(&repo).args(["push", "-q", "-u", "origin", "main"]),
        "push -u origin main",
    );

    for branch in ["feature-a", "feature-b", "feature-c"] {
        let worktree = root.join(format!("repo.{branch}"));
        run(
            git(&repo).args([
                "worktree",
                "add",
                "-q",
                "-b",
                branch,
                &format!("../repo.{branch}"),
            ]),
            "worktree add",
        );
        let file = format!("{branch}.txt");
        std::fs::write(worktree.join(&file), format!("{branch} content\n")).unwrap();
        run(git(&worktree).args(["add", &file]), "add");
        run(
            git(&worktree).args(["commit", "-q", "-m", &format!("Add {branch} file")]),
            "commit",
        );
    }
}

/// Build the main-only template from the standard fixture once per target
/// directory. Deriving it with real Git preserves the pinned commit, remote,
/// tracking refs, attributes, and config while removing linked-worktree
/// administration through Git's supported path.
fn build_standard_main_only_fixture(root: &Path) {
    let fixture = copy_standard_fixture(root);
    let git = |dir: &Path| configure_git_env(Cmd::new("git")).current_dir(dir);
    let run = |cmd: Cmd, what: &str| {
        let output = cmd.run().unwrap();
        check_git_status(&output, what);
    };
    let repo = root.join("repo");

    for branch in ["feature-a", "feature-b", "feature-c"] {
        let worktree = fixture
            .worktrees
            .get(branch)
            .expect("standard fixture worktree");
        let worktree = worktree.to_string_lossy();
        run(
            git(&repo).args(["worktree", "remove", "--force", worktree.as_ref()]),
            "worktree remove",
        );
        run(
            git(&repo).args(["branch", "-D", branch]),
            "delete fixture branch",
        );
    }
}

/// Copy a directory tree without spawning a platform-specific copy command.
fn copy_dir_recursive(src: &Path, dest: &Path) {
    std::fs::create_dir_all(dest).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let file_type = entry.file_type().unwrap();
        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dest_path);
        } else if file_type.is_file() {
            std::fs::copy(&src_path, &dest_path).unwrap();
        }
        // Skip symlinks, sockets, etc (shouldn't be in fixtures).
    }
}

/// Copy the standard fixture template to create a new test repo with
/// worktrees and remote.
///
/// Pure Rust recursive copy - 2.5x faster than spawning cp/robocopy.
/// Benchmarked at 21ms vs 53ms per fixture copy on macOS.
fn copy_standard_fixture(dest: &Path) -> FixtureWorktrees {
    shell_exec::enable_hermetic_test_env();
    copy_dir_recursive(&standard_fixture_template(), dest);

    // Canonicalize dest for worktrees map (on macOS /var -> /private/var)
    let canonical_dest = canonicalize(dest).unwrap();

    // The template's worktree links are absolute paths into the template;
    // point each copy's links at its own location. Use absolute paths,
    // matching `git worktree add`, so `git worktree list` does not interpret
    // relative paths from the wrong base on some Git builds.
    for wt in ["feature-a", "feature-b", "feature-c"] {
        let worktree_path = canonical_dest.join(format!("repo.{wt}"));
        let worktree_gitdir = worktree_path.join(".git").to_slash_lossy().into_owned();
        let main_worktree_gitdir = canonical_dest
            .join("repo")
            .join(".git")
            .join("worktrees")
            .join(format!("repo.{wt}"))
            .to_slash_lossy()
            .into_owned();

        std::fs::write(
            dest.join(format!("repo.{wt}/.git")),
            format!("gitdir: {main_worktree_gitdir}\n"),
        )
        .unwrap();
        std::fs::write(
            dest.join(format!("repo/.git/worktrees/repo.{wt}/gitdir")),
            format!("{worktree_gitdir}\n"),
        )
        .unwrap();
    }

    // Build worktrees map using canonical paths
    let mut worktrees = HashMap::new();
    for wt in ["feature-a", "feature-b", "feature-c"] {
        worktrees.insert(wt.to_string(), canonical_dest.join(format!("repo.{wt}")));
    }

    let remote = canonical_dest.join("origin.git");

    FixtureWorktrees { worktrees, remote }
}

/// Copy the pinned standard history with only its primary worktree and remote.
fn copy_standard_main_only_fixture(dest: &Path) -> PathBuf {
    shell_exec::enable_hermetic_test_env();
    copy_dir_recursive(&standard_main_only_fixture_template(), dest);
    canonicalize(dest).unwrap().join("origin.git")
}

/// The identity every test commit is authored and committed under.
///
/// Reaches a harness-built `git` through [`git_test_env`] and an in-process one
/// through [`LOCAL_TEST_CONFIG`], which repeats these two values because a
/// config file cannot interpolate a constant.
const TEST_IDENTITY_NAME: &str = "Test User";
const TEST_IDENTITY_EMAIL: &str = "test@example.com";

/// Settings written into every test repo's own config by
/// [`write_local_test_config`].
///
/// A unit test driving the library in-process — `Repository::run_command` and
/// everything layered on it — gets a `git` carrying the *test process's*
/// environment, so none of [`configure_git_env`]'s per-command isolation
/// applies: no `GIT_ALLOW_PROTOCOL`, no identity, no pinned dates. The
/// repo's own config is the one layer such a command still reads, so whatever
/// must hold for it lives here.
///
/// What it does *not* have to carry is host-config denial or the floor's
/// settings. The hermetic latch (`shell_exec::enable_hermetic_test_env`)
/// puts both on every `Cmd` child, so an in-process git resolves those
/// rather than the developer's — see the Git Config Isolation section of
/// `tests/CLAUDE.md`.
///
/// `protocol.allow = never` with a `file` exception is the config spelling of
/// `GIT_ALLOW_PROTOCOL=file` (see [`GIT_ALLOWED_PROTOCOLS`] for why the suite
/// stays off the wire). The identity is required rather than convenient: the
/// hermetic floor sets `user.useConfigOnly`, and deliberately carries no name or
/// email, so a repo without a local identity fails its commit instead of
/// authoring one from the host's username and hostname.
const LOCAL_TEST_CONFIG: &str = r#"[user]
	name = Test User
	email = test@example.com
[protocol]
	allow = never
[protocol "file"]
	allow = always
"#;

/// Append [`LOCAL_TEST_CONFIG`] to `repo`'s own config file.
///
/// Linked worktrees share the common dir's config, so this reaches every
/// worktree a test later adds.
fn write_local_test_config(repo: &Repository) {
    let path = repo.git_common_dir().join("config");
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap_or_else(|e| panic!("failed to open {} for append: {e}", path.display()));
    std::io::Write::write_all(&mut file, LOCAL_TEST_CONFIG.as_bytes())
        .unwrap_or_else(|e| panic!("failed to write {}: {e}", path.display()));
}

/// Canonicalize a path without Windows verbatim prefix (`\\?\`).
///
/// On Windows, `std::fs::canonicalize()` returns verbatim paths like `\\?\C:\...`
/// which git cannot handle. The `dunce` crate strips this prefix when safe.
/// On Unix, this is equivalent to `std::fs::canonicalize()`.
pub fn canonicalize(path: &Path) -> std::io::Result<PathBuf> {
    dunce::canonicalize(path)
}

/// Time constants for `commit_with_age()` - use as `5 * MINUTE`, `2 * HOUR`, etc.
pub const MINUTE: i64 = 60;
pub const HOUR: i64 = 60 * MINUTE;
pub const DAY: i64 = 24 * HOUR;
pub const WEEK: i64 = 7 * DAY;

/// The epoch used for deterministic timestamps in tests (2025-01-02T00:00:00Z).
/// Use this when creating test data with timestamps (cache entries, etc.).
pub const TEST_EPOCH: u64 = 1735776000;

/// Default timeout for background hook/command completion.
/// Generous to avoid flakiness under CI load; exponential backoff means fast tests when things work.
const BG_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Value for `GIT_ALLOW_PROTOCOL` on every git process a test spawns, whether
/// directly or as a child of `wt`: local paths and `file://` only. A git a test
/// starts in-process carries none of this environment and is denied by
/// [`LOCAL_TEST_CONFIG`] instead; between them the suite cannot reach the wire.
///
/// Tests point remotes at real hosts (`https://github.com/test-owner/…`) to
/// drive forge detection, and `Repository::default_branch()` is allowed to
/// fall through to `git ls-remote` when neither the worktrunk cache nor
/// `origin/HEAD` resolves. So a test that never meant to do network work
/// makes an unbounded connect to whatever host the URL names. Nothing in that
/// path has a timeout: an unanswered SYN costs ~127 s per address on Linux
/// (`tcp_syn_retries=6`) and a host with several A/AAAA records is tried in
/// turn, which is how a 2-second test reaches nextest's 180 s limit.
///
/// Denying the transport turns that into an immediate `transport 'https' not
/// allowed`; detection then falls back to local inference, leaving output
/// unchanged. The adjacent `GIT_TERMINAL_PROMPT=0` doesn't subsume this: it
/// only suppresses the credential prompt the host's 401 triggers, so the
/// request has already gone out by the time it applies.
const GIT_ALLOWED_PROTOCOLS: &str = "file";

/// Restore git's default protocol set on a command built by
/// [`configure_git_cmd`], for a caller whose job is to fetch a fixture from
/// upstream — the large-repository benchmark corpus. Grep for this to enumerate
/// everything that may reach the wire; tests are not among them.
pub fn allow_network_transports(cmd: &mut Command) {
    cmd.env_remove("GIT_ALLOW_PROTOCOL");
}

/// Determinism knobs every isolated wt subprocess needs, whatever it's
/// attached to.
///
/// A test child's environment is three layers, each with one home: this
/// baseline, the fixture's paths ([`pty_env_vars`]), and whatever the child's
/// transport needs — [`configure_cli_command`] for a piped child,
/// [`PTY_TEST_ENV_VARS`] for one on a terminal. A knob both transports need
/// belongs here, so adding it once reaches every path.
///
/// `TERM` is transport-level rather than baseline, which is why it's absent
/// here: a piped child gets `TERM=alacritty` so hyperlink detection has
/// something to key on, while a PTY child needs a `TERM` with real terminfo —
/// macOS CI carries no alacritty entry, and skim fails without one.
pub const STATIC_TEST_ENV_VARS: &[(&str, &str)] = &[
    ("CLICOLOR_FORCE", "1"),
    // Deny network git transports (see GIT_ALLOWED_PROTOCOLS)
    ("GIT_ALLOW_PROTOCOL", GIT_ALLOWED_PROTOCOLS),
    // Terminal width for PTY tests. configure_cli_command() overrides to 500 for longer paths.
    ("COLUMNS", "150"),
    // Deterministic locale settings
    ("LC_ALL", "C"),
    ("LANG", "C"),
    // Skip URL health checks to avoid flaky tests from random local processes
    ("WORKTRUNK_TEST_SKIP_URL_HEALTH_CHECK", "1"),
    // Disable delayed streaming for deterministic output across platforms.
    // Without this, slow CI triggers progress messages that don't appear on faster systems.
    ("WORKTRUNK_TEST_DELAYED_STREAM_MS", "-1"),
    // Give the `--reap` probes (`git::reap::probe_timeout`) a load-proof
    // bound. At the production 5s, a probe spawn stalling under suite load
    // trips the timeout, whose fail-safe empty result turns "reap the child"
    // into "No processes to reap" — a load-dependent outcome.
    ("WORKTRUNK_TEST_PROBE_TIMEOUT_MS", "60000"),
    // Treat shells as not installed by default so the "Skipped …; rc not found"
    // filter in scan_shell_configs is deterministic across hosts. Tests that need
    // a shell to count as installed (e.g., to assert the Skipped path) set "1".
    // Nushell uses the historical env-var name `_ENV` (see Shell::is_installed).
    ("WORKTRUNK_TEST_BASH_INSTALLED", "0"),
    ("WORKTRUNK_TEST_ZSH_INSTALLED", "0"),
    ("WORKTRUNK_TEST_FISH_INSTALLED", "0"),
    ("WORKTRUNK_TEST_NUSHELL_ENV", "0"),
    ("WORKTRUNK_TEST_POWERSHELL_INSTALLED", "0"),
    // Disable the process-tree shell walk (see `shell::ancestor_shell`): the
    // real ancestry of a spawned test wt is the test harness → nextest →
    // cargo → the developer's or CI runner's shell, which would leak into
    // shell-detection results nondeterministically. Empty = "no shell
    // ancestor found", so tests drive detection via SHELL. Tests exercising
    // the walk set a shell name instead.
    ("WORKTRUNK_TEST_PARENT_SHELL", ""),
    // Disable PowerShell auto-detection (PSModulePath / SHELL signal).
    // Iteration is unconditional (matches the other shells); this var only
    // controls `allow_create` via `should_auto_configure_powershell()` so we
    // don't write a profile in tests that aren't asserting that path.
    ("WORKTRUNK_TEST_POWERSHELL_ENV", "0"),
];

/// Determinism knobs for a child whose stderr is a terminal, layered over
/// [`STATIC_TEST_ENV_VARS`].
///
/// Output that appears only once an operation runs past a threshold is a
/// function of machine load rather than of behavior, and a PTY test captures
/// the raw byte stream — so it keeps every in-place redraw frame a terminal
/// would have erased, elapsed-second counter and all. These pin such output to
/// one state, off, so a snapshot records what the command did rather than how
/// fast the machine was. A piped child needs none of them: the TTY half of
/// each gate is already false there, which is why they'd only add noise to the
/// `env:` block every `assert_cmd_snapshot!` records.
pub const PTY_TEST_ENV_VARS: &[(&str, &str)] = &[
    // The `Progress` and `Watchdog` spinners (src/progress.rs). Gates the
    // render, not the counters.
    ("WORKTRUNK_TEST_SPINNERS", "0"),
];

/// The paths an isolated wt subprocess is pointed at — everything in its
/// environment that varies per fixture. See [`pty_env_vars`].
pub struct TestEnvPaths<'a> {
    /// `HOME`, with `XDG_CONFIG_HOME` beneath it.
    pub home: &'a Path,
    /// `WORKTRUNK_CONFIG_PATH`.
    pub wt_config: &'a Path,
    /// `WORKTRUNK_APPROVALS_PATH`.
    pub approvals: &'a Path,
}

/// Every environment variable a PTY-spawned wt subprocess needs: the
/// [`STATIC_TEST_ENV_VARS`] and [`PTY_TEST_ENV_VARS`] baselines, plus `paths`.
///
/// A PTY child is spawned through `portable_pty::CommandBuilder`, which takes
/// variables one at a time rather than a configured [`Command`] — so its
/// environment has to exist as a value, which is what separates this from
/// [`configure_cli_command`]. Every fixture that spawns one builds it here, so
/// a new variable reaches all of them.
pub fn pty_env_vars(paths: TestEnvPaths<'_>) -> Vec<(String, String)> {
    // Baselines, then the git isolation set ([`git_test_env`] — its
    // LC_ALL/LANG/GIT_ALLOW_PROTOCOL repeat STATIC_TEST_ENV_VARS entries
    // with identical values; the later pair wins, harmlessly), then the
    // fixture's paths.
    let mut vars: Vec<(String, String)> = STATIC_TEST_ENV_VARS
        .iter()
        .chain(PTY_TEST_ENV_VARS)
        .map(|&(k, v)| (k.to_string(), v.to_string()))
        .collect();
    // A PTY child is `env_clear`ed, so the hermetic floor reaches it only if
    // carried across by hand — every other transport gets it from the `Cmd`
    // latch or `isolate_subprocess_env`.
    vars.extend(
        shell_exec::HERMETIC_TEST_GIT_ENV
            .iter()
            .map(|&(k, v)| (k.to_string(), v.to_string())),
    );
    vars.extend(git_test_env().into_iter().map(|(k, v)| (k.to_string(), v)));

    vars.extend(
        [
            ("HOME", paths.home.display().to_string()),
            (
                "XDG_CONFIG_HOME",
                paths.home.join(".config").display().to_string(),
            ),
            (
                "WORKTRUNK_CONFIG_PATH",
                paths.wt_config.display().to_string(),
            ),
            (
                "WORKTRUNK_SYSTEM_CONFIG_PATH",
                DEFAULT_ISOLATED_SYSTEM_CONFIG.to_string(),
            ),
            (
                "WORKTRUNK_APPROVALS_PATH",
                paths.approvals.display().to_string(),
            ),
        ]
        .map(|(key, value)| (key.to_string(), value)),
    );

    vars
}

/// Default user-config path for isolated subprocesses — points at a
/// nonexistent file so wt treats it as "no config." Callers can override
/// via the `user_config` parameter to [`isolate_subprocess_env`].
pub const DEFAULT_ISOLATED_USER_CONFIG: &str = "/nonexistent/wt/config.toml";

/// Default approvals path for isolated subprocesses — nonexistent file.
const DEFAULT_ISOLATED_APPROVALS: &str = "/nonexistent/wt/approvals.toml";

/// Default system-config path for isolated subprocesses. Uses the real
/// XDG location so tests verify wt's XDG lookup path is exercised; the
/// file doesn't actually exist on test/CI machines.
const DEFAULT_ISOLATED_SYSTEM_CONFIG: &str = "/etc/xdg/worktrunk/config.toml";

/// `cargo llvm-cov` / `cargo affected` coverage env vars that propagate
/// verbatim from parent to test subprocesses. `LLVM_PROFILE_FILE` is *not*
/// in this list — it's resolved by [`default_llvm_profile_file`] so an
/// instrumented child writing to its cwd is impossible even when nothing's
/// inherited. Mirrored at the PTY call sites in `tests/common/`.
pub const COVERAGE_ENV_VARS: &[&str] = &["CARGO_LLVM_COV", "CARGO_LLVM_COV_TARGET_DIR"];

/// Resolve the `LLVM_PROFILE_FILE` value to set on test subprocesses.
///
/// Returns the inherited value when the parent is running under
/// `cargo llvm-cov` (so coverage data lands where the runner expects). When
/// nothing is inherited, returns a per-binary, per-pid path under the system
/// temp dir so an instrumented child can't fall back to writing
/// `default_<hash>_<pid>.profraw` into the subprocess's cwd. That cwd is the
/// test worktree for any `wt list` snapshot that spawns a mock, and a stray
/// profraw there flips `wt list` to "1 with changes" and flakes the snapshot.
///
/// The `%m` and `%p` placeholders are expanded by the LLVM runtime in the
/// instrumented child; uninstrumented children ignore the env var entirely.
pub fn default_llvm_profile_file() -> std::ffi::OsString {
    default_llvm_profile_file_with(std::env::var_os("LLVM_PROFILE_FILE"))
}

/// Inner form of [`default_llvm_profile_file`] that takes the inherited value
/// as a parameter instead of reading [`std::env::var_os`]. CI always runs
/// under `cargo llvm-cov`, so the production caller always takes the
/// inherited branch — this split lets a unit test exercise the fallback
/// without mutating process env (which races with parallel tests).
fn default_llvm_profile_file_with(inherited: Option<std::ffi::OsString>) -> std::ffi::OsString {
    if let Some(inherited) = inherited {
        return inherited;
    }
    let dir = std::env::temp_dir().join("wt-test-profraw");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("cov-%m_%p.profraw").into_os_string()
}

/// Prepare a subprocess to run with a clean wt environment.
///
/// Strips every `GIT_*` and `WORKTRUNK_*` from the parent env, plus
/// `NO_COLOR` / `FORCE_HYPERLINK` / `SHELL` / `PSModulePath`; re-applies the
/// hermetic floor (`shell_exec::HERMETIC_TEST_GIT_ENV`), then points the three
/// `WORKTRUNK_*_PATH` env vars at known locations:
///
/// - `WORKTRUNK_CONFIG_PATH` ← `user_config` (or [`DEFAULT_ISOLATED_USER_CONFIG`])
/// - `WORKTRUNK_SYSTEM_CONFIG_PATH` ← real XDG location (typically nonexistent on CI)
/// - `WORKTRUNK_APPROVALS_PATH` ← nonexistent file
///
/// Also sets `LLVM_PROFILE_FILE` via [`default_llvm_profile_file`] and
/// re-applies [`COVERAGE_ENV_VARS`] from the parent so an instrumented child
/// writes its `.profraw` to the path `cargo llvm-cov` chose (or to a temp-dir
/// fallback when nothing's inherited), not to a `default_*.profraw` in the
/// child's cwd. The re-apply is defensive: a caller that did `env_clear()`
/// before us would otherwise drop the inherited values.
///
/// Shared by [`configure_cli_command`] (test-side, layers on test
/// determinism: forced colors, fixed timestamps, log level, etc.) and
/// bench callers (no extra layer — benches want realism). The two
/// genuinely diverge in the *additions*, but the isolation baseline is
/// identical.
///
/// Use on any wt subprocess that must not see host context. For `git`
/// subprocesses, use [`configure_git_cmd`] instead.
pub fn isolate_subprocess_env(cmd: &mut Command, user_config: Option<&Path>) {
    isolate_subprocess_env_from(cmd, user_config, std::env::vars().map(|(k, _)| k));
}

/// Inner form of [`isolate_subprocess_env`] that takes the parent-env keys
/// as an iterator instead of reading [`std::env::vars`]. Lets tests exercise
/// the GIT_*/WORKTRUNK_* scrub branch with synthetic input — `set_var` is
/// `unsafe` and races with parallel tests, so we don't mutate process env.
fn isolate_subprocess_env_from<I>(cmd: &mut Command, user_config: Option<&Path>, env_keys: I)
where
    I: IntoIterator<Item = String>,
{
    for key in env_keys {
        if key.starts_with("GIT_") || key.starts_with("WORKTRUNK_") {
            cmd.env_remove(&key);
        }
    }
    // The hermetic floor, restated explicitly now that every inherited
    // `GIT_*` is gone — the subprocess must deny the host's git config just
    // as this process does.
    for (key, val) in shell_exec::HERMETIC_TEST_GIT_ENV {
        cmd.env(key, val);
    }
    cmd.env_remove("NO_COLOR");
    // Overrides the OSC 8 probe, so an inherited value changes whether `wt
    // list` links its CI cell and shortens its URL cell to `:port`. The
    // statusline links unconditionally and is unaffected.
    cmd.env_remove("FORCE_HYPERLINK");
    cmd.env_remove("SHELL");
    // PSModulePath being inherited triggers false PowerShell detection on
    // CI environments where PowerShell Core is installed but not in use.
    cmd.env_remove("PSModulePath");

    cmd.env(
        "WORKTRUNK_CONFIG_PATH",
        user_config.unwrap_or(Path::new(DEFAULT_ISOLATED_USER_CONFIG)),
    );
    cmd.env(
        "WORKTRUNK_SYSTEM_CONFIG_PATH",
        DEFAULT_ISOLATED_SYSTEM_CONFIG,
    );
    cmd.env("WORKTRUNK_APPROVALS_PATH", DEFAULT_ISOLATED_APPROVALS);

    // Always set LLVM_PROFILE_FILE — to the inherited value under coverage,
    // or to a temp-dir default otherwise — so an instrumented child never
    // falls back to writing `default_*.profraw` into its cwd. See
    // [`default_llvm_profile_file`] for the rationale.
    cmd.env("LLVM_PROFILE_FILE", default_llvm_profile_file());
    for key in COVERAGE_ENV_VARS {
        if let Ok(val) = std::env::var(key) {
            cmd.env(key, val);
        }
    }
}

/// `env_remove` the [`INHERITED_GIT_PATH_VARS`] from `cmd`. Call this on
/// any `git` subprocess spawned with an explicit `current_dir`, so an
/// inherited relative `GIT_DIR=.git` (from a git alias, hook, etc.)
/// doesn't redirect discovery away from the path you set.
///
/// Strictly defensive when used downstream of [`isolate_subprocess_env`]
/// (which already stripped these). Required when there's no upstream
/// scrub — e.g. wt-perf shells out to `git` directly without a `wt` parent.
pub fn scrub_git_path_vars(cmd: &mut Command) {
    for var in INHERITED_GIT_PATH_VARS {
        cmd.env_remove(var);
    }
}

/// Root for every temp directory the test fixtures create: one subdirectory of
/// the system temp dir rather than hundreds of siblings directly inside it.
///
/// Entries in the shared temp root are cheap to ignore but expensive to *walk*,
/// and `git::recover::recover_from_path` read_dirs every existing ancestor of a
/// deleted CWD until a repo claims it — reaching the shared dir itself when
/// nothing nearer does. This sub-root can't shorten that worst-case walk; what
/// it does is keep the suite's own churn from growing the shared dir every
/// run, the accumulation that made the walk slow (see `isolated_test_cwd`).
///
/// Where that root sits carries no isolation weight: the fixtures read no
/// git config outside themselves wherever they live, so a conditional
/// `includeIf "gitdir:<home>"` in the developer's config can't reach them.
/// Only the ancestor-walk cost above argues for one location over another.
///
/// The name is two characters because a unix socket path can't exceed
/// `sun_path` (104 bytes on macOS, including the NUL). macOS's per-user
/// `$TMPDIR` is 56 canonicalized characters, and
/// `test_copy_ignored_skips_non_regular_files` binds a listener at
/// `<fixture>/repo/target/test.sock` — 89 bytes before this directory exists at
/// all. The name and its slash come out of the 14 bytes that were spare;
/// `worktrunk-tests` (16 with its slash) would overflow them by two.
///
/// Created on first use and left in place; what goes inside it are `TempDir`s
/// that remove themselves on drop, so it stays near-empty between runs.
pub fn test_temp_root() -> &'static Path {
    static ROOT: std::sync::LazyLock<PathBuf> = std::sync::LazyLock::new(|| {
        let root = std::env::temp_dir().join("wt");
        std::fs::create_dir_all(&root).expect("create test temp root");
        root
    });
    ROOT.as_path()
}

/// Create a temp directory under [`test_temp_root`].
///
/// The fixtures' replacement for `TempDir::new()` / `tempfile::tempdir()`.
pub fn test_tempdir() -> TempDir {
    TempDir::new_in(test_temp_root()).expect("create test temp dir")
}

/// A single fixed empty directory used as the default `current_dir` for
/// [`wt_command`], created on first use and shared by every test process.
///
/// The dir is outside any git repository and has no `.config/wt.toml`, so `wt`
/// invocations spawned through [`wt_command`] don't pick up the test process's
/// inherited CWD (which is typically the worktrunk repo root, with its own
/// `.config/wt.toml` and git history). Nothing writes into it — a test that
/// needs to write uses a `TestRepo`.
///
/// Deliberately *not* a `TempDir`. Statics aren't dropped at process exit, so a
/// `TempDir` here is never cleaned up, and nextest runs one process per test:
/// that leaked one empty directory per test — ~700 per integration-suite run —
/// into a temp root nothing reliably sweeps (macOS clears it only at boot).
/// Hundreds of thousands of stale entries cost nothing to ignore but are
/// expensive to enumerate, and `git::recover::recover_from_path` reads every
/// ancestor directory of a deleted CWD — the measured cost is in
/// `tests/CLAUDE.md` → Profiling the Suite. One fixed directory never grows.
fn isolated_test_cwd() -> &'static Path {
    static ISOLATED_CWD: std::sync::LazyLock<PathBuf> = std::sync::LazyLock::new(|| {
        let dir = test_temp_root().join("isolated-cwd");
        std::fs::create_dir_all(&dir).expect("create isolated test cwd");
        dir
    });
    ISOLATED_CWD.as_path()
}

/// Create a `wt` CLI command with standardized test environment settings.
///
/// The command has the following guarantees:
/// - All host `GIT_*` and `WORKTRUNK_*` variables are cleared
/// - Color output is forced (`CLICOLOR_FORCE=1`) so ANSI styling appears in snapshots
/// - Terminal width set to 150 columns (`COLUMNS=150`)
/// - `current_dir` defaults to one fixed empty directory shared by every test
///   process (not a git repo, no project config), so `wt` doesn't pick up worktrunk's own
///   `.config/wt.toml` or detect a git repo from the test process's inherited
///   CWD. Tests that need a specific CWD must override via
///   `cmd.current_dir(...)`; `repo.wt_command()` does so automatically.
#[must_use]
pub fn wt_command() -> Command {
    let mut cmd = Command::new(wt_bin());
    configure_cli_command(&mut cmd);
    cmd.current_dir(isolated_test_cwd());
    cmd
}

/// Create a `wt` invocation configured like shell-driven completions (`COMPLETE=bash`).
///
/// `words` should match the shell's `COMP_WORDS` array, e.g. `["wt", "switch", ""]`.
pub fn wt_completion_command(words: &[&str]) -> Command {
    assert!(
        matches!(words.first(), Some(&"wt")),
        "completion words must include command name as the first element"
    );

    let mut cmd = wt_command();
    configure_completion_invocation(&mut cmd, words);
    cmd
}

/// Configure an existing command to mimic shell completion environment.
pub fn configure_completion_invocation(cmd: &mut Command, words: &[&str]) {
    configure_completion_invocation_for_shell(cmd, words, "bash");
}

/// Configure an existing command to mimic shell completion environment for a specific shell.
///
/// This matches how each shell actually invokes completions (per clap_complete's
/// registration scripts). Tests should match real behavior to catch shell-specific bugs.
///
/// Note: We use newline as IFS for all shells to simplify test parsing. The actual
/// shells use different separators (bash: vertical tab, zsh/fish: newline), but IFS
/// only affects output parsing, not completion logic. Shell-specific completion bugs
/// are caught by the index calculation differences (fish vs bash/zsh).
pub fn configure_completion_invocation_for_shell(cmd: &mut Command, words: &[&str], shell: &str) {
    cmd.arg("--");
    cmd.args(words);
    cmd.env("COMPLETE", shell);
    cmd.env("_CLAP_IFS", "\n"); // Use newline for test parsing simplicity

    // Shell-specific environment setup - only set what affects completion logic
    match shell {
        "bash" | "zsh" => {
            // Bash and Zsh set the cursor index via environment variable
            let index = words.len().saturating_sub(1);
            cmd.env("_CLAP_COMPLETE_INDEX", index.to_string());
        }
        "fish" | "nu" => {
            // Fish and Nushell don't set _CLAP_COMPLETE_INDEX - they append the
            // current token as the last argument, so the completion handler uses
            // args.len() - 1
        }
        _ => {}
    }
}

/// Configure an existing command with the standardized worktrunk CLI environment.
///
/// This helper mirrors the environment preparation performed by `wt_command`
/// and is intended for cases where tests need to construct the command manually
/// (e.g., to execute shell pipelines).
///
/// Layers on top of [`isolate_subprocess_env`] (the env-strip + path
/// baseline that benches use directly): tests additionally pin
/// timestamps, locale, mock-command flags, and wide COLUMNS for
/// deterministic snapshots, where benches want realism.
///
/// ## Related: `TestRepo::test_env_vars()`
///
/// PTY tests use `test_env_vars()` which returns env vars as a Vec. Both functions
/// share common variables via `STATIC_TEST_ENV_VARS`. Key differences:
/// - This function uses COLUMNS=500 (wider for long macOS paths in error messages)
/// - `test_env_vars()` uses COLUMNS=150 (narrower for PTY snapshot consistency)
/// - This function sets TERM=alacritty; PTY tests don't (skim needs valid terminfo)
/// - This function clears host GIT_*/WORKTRUNK_* vars; PTY tests start with clean env
pub fn configure_cli_command(cmd: &mut Command) {
    // Strip host context and set baseline `WORKTRUNK_*_PATH` defaults
    // (all pointing at nonexistent or XDG paths). Tests needing real
    // config should override after this call (e.g. via
    // `TestRepo::configure_wt_cmd`). `WORKTRUNK_PROJECT_CONFIG_PATH` is
    // intentionally not set so tests pick up `.config/wt.toml` in their
    // own test repo via the default lookup; host leakage is prevented
    // by the env-strip above.
    isolate_subprocess_env(cmd, None);
    cmd.env("WORKTRUNK_TEST_EPOCH", TEST_EPOCH.to_string());
    // Do not inherit the host's RUST_LOG: the flag baseline and `RUST_LOG`
    // merge via the env-wins-when-set contract enforced by
    // `tracing_subscriber::EnvFilter` (see `logging::init`), so a blanket
    // host `RUST_LOG=warn` would cap `-vv` tests at Warn and starve `trace.log`
    // of debug-level `[wt-trace]` records. Tests that need warn-level output
    // can opt in after command construction.
    cmd.env_remove("RUST_LOG");
    // Treat Claude as not installed by default (tests can override with "1")
    cmd.env("WORKTRUNK_TEST_CLAUDE_INSTALLED", "0");
    // Treat Codex as not installed by default (tests can override with "1")
    cmd.env("WORKTRUNK_TEST_CODEX_INSTALLED", "0");
    // Treat OpenCode as not installed by default (tests can override with "1")
    cmd.env("WORKTRUNK_TEST_OPENCODE_INSTALLED", "0");
    // Treat Gemini as not installed by default (tests can override with "1")
    cmd.env("WORKTRUNK_TEST_GEMINI_INSTALLED", "0");

    // Apply shared static env vars (see STATIC_TEST_ENV_VARS)
    for &(key, value) in STATIC_TEST_ENV_VARS {
        cmd.env(key, value);
    }

    // Override COLUMNS to 500 (wider than STATIC_TEST_ENV_VARS default) for long paths.
    // macOS temp paths (~80 chars) are much longer than Linux (~10 chars),
    // so error messages containing paths need room to avoid platform-specific line breaks.
    cmd.env("COLUMNS", "500");
    // Set consistent terminal type for hyperlink detection via supports-hyperlinks crate.
    // Not in STATIC_TEST_ENV_VARS because PTY tests need a TERM with valid terminfo.
    cmd.env("TERM", "alacritty");

    // LLVM coverage env propagation lives in `isolate_subprocess_env` so bench
    // callers get the same defense; nothing extra needed here.
}

/// The environment for a directly-spawned test `git`: deterministic identity,
/// timestamps and locale, no terminal prompts, no network transports
/// (`GIT_ALLOWED_PROTOCOLS`). Host-config denial is not here — that is the
/// hermetic floor's job (`shell_exec::HERMETIC_TEST_GIT_ENV`), which every
/// consumer of this set applies through its own transport.
///
/// Single home for these settings — [`configure_git_cmd`] (Command),
/// [`configure_git_env`] (`Cmd`), and [`pty_env_vars`] (PTY) all
/// consume it, so the three spellings cannot drift.
///
/// The identity is here rather than in the hermetic floor because the floor
/// carries only the denial and the two `-c` settings; identity already has
/// this per-command home, and a second copy in the floor could only drift
/// from it. A harness-built `git` needs one because it commits into repos it
/// has just created, before `LOCAL_TEST_CONFIG` reaches their local config.
pub fn git_test_env() -> [(&'static str, String); 11] {
    [
        ("GIT_AUTHOR_NAME", TEST_IDENTITY_NAME.to_string()),
        ("GIT_AUTHOR_EMAIL", TEST_IDENTITY_EMAIL.to_string()),
        ("GIT_COMMITTER_NAME", TEST_IDENTITY_NAME.to_string()),
        ("GIT_COMMITTER_EMAIL", TEST_IDENTITY_EMAIL.to_string()),
        ("GIT_AUTHOR_DATE", "2025-01-01T00:00:00Z".to_string()),
        ("GIT_COMMITTER_DATE", "2025-01-01T00:00:00Z".to_string()),
        ("LC_ALL", "C".to_string()),
        ("LANG", "C".to_string()),
        ("WORKTRUNK_TEST_EPOCH", TEST_EPOCH.to_string()),
        ("GIT_TERMINAL_PROMPT", "0".to_string()),
        ("GIT_ALLOW_PROTOCOL", GIT_ALLOWED_PROTOCOLS.to_string()),
    ]
}

/// Configure a git command with isolated environment for testing.
///
/// Applies [`git_test_env`].
pub fn configure_git_cmd(cmd: &mut Command) {
    shell_exec::enable_hermetic_test_env();
    // Defensive: every existing caller is downstream of `configure_cli_command`
    // (which already stripped these via `isolate_subprocess_env`), but a future
    // test that spawns `git` from an unprepared parent shouldn't be vulnerable
    // to an inherited relative `GIT_DIR` redirecting discovery.
    scrub_git_path_vars(cmd);
    // The floor by hand — a plain `Command` child doesn't pass through the
    // `Cmd` latch.
    for (key, val) in shell_exec::HERMETIC_TEST_GIT_ENV {
        cmd.env(key, val);
    }
    for (key, value) in git_test_env() {
        cmd.env(key, value);
    }
}

/// Configure a `Cmd`-based git command with isolated environment for testing.
///
/// This is the `Cmd` equivalent of [`configure_git_cmd`]. Use this when building
/// git commands via the builder pattern (`Cmd::new("git")`).
pub fn configure_git_env(cmd: Cmd) -> Cmd {
    shell_exec::enable_hermetic_test_env();
    // Defensive `GIT_*` path-var strip — see `configure_git_cmd` for rationale.
    let cmd = INHERITED_GIT_PATH_VARS
        .iter()
        .fold(cmd, |acc, var| acc.env_remove(var));
    git_test_env()
        .into_iter()
        .fold(cmd, |acc, (key, value)| acc.env(key, value))
}

/// Shared interface for test repository fixtures.
///
/// Provides `configure_git_cmd()` (for `Command`), `git_command()` (returns `Cmd`),
/// and `run_git_in()` with consistent environment isolation.
pub trait TestRepoBase {
    /// Configure a git command with isolated environment.
    fn configure_git_cmd(&self, cmd: &mut Command) {
        configure_git_cmd(cmd);
    }

    /// Create a git command for the given directory.
    fn git_command(&self, dir: &Path) -> Cmd {
        configure_git_env(Cmd::new("git")).current_dir(dir)
    }

    /// Run a git command in a specific directory, panicking on failure.
    fn run_git_in(&self, dir: &Path, args: &[&str]) {
        let output = self
            .git_command(dir)
            .args(args.iter().copied())
            .run()
            .unwrap();
        check_git_status(&output, &args.join(" "));
    }

    /// Create a commit in the specified directory.
    ///
    /// Creates or overwrites `file.txt` with the message content, stages it, and commits.
    fn commit_in(&self, dir: &Path, message: &str) {
        std::fs::write(dir.join("file.txt"), message).unwrap();
        self.run_git_in(dir, &["add", "file.txt"]);

        let output = self
            .git_command(dir)
            .args(["commit", "-m", message])
            .run()
            .unwrap();

        if !output.status.success() {
            panic!(
                "Failed to commit:\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

/// Create a temporary CD directive file for a shell-integration test.
///
/// The guard must be kept alive for the duration of the test — dropping it
/// removes the directory and file.
pub fn directive_file() -> (PathBuf, TempDir) {
    let dir = directive_temp_dir();
    let cd_path = create_empty(dir.path().join("cd"));
    (cd_path, dir)
}

/// A private directory for a test's directive files.
///
/// Why not `NamedTempFile::new()` (which is what these helpers used to call):
/// `tempfile` retries a name collision only when it surfaces as
/// `AlreadyExists`, and on Windows `create_new` against a name already held by
/// a *directory* — or by a file in delete-pending state — returns
/// `PermissionDenied` instead, which it hands straight back. A full
/// suite run leaves the shared temp directory full of `.tmpXXXXXX` entries
/// (every `TestRepo` creates one), and under that load the call failed ~1% of
/// the time on Windows CI: `failed to create cd temp file: … Os { code: 5 …
/// "Access is denied." }`. `TempDir::new` isn't exposed to it — a directory
/// collision surfaces as `AlreadyExists`, which `tempfile` retries — and the
/// files inside carry fixed names, which nothing else can collide with.
fn directive_temp_dir() -> TempDir {
    TempDir::new().expect("failed to create directive temp dir")
}

/// Create `path` as an empty file and return it.
fn create_empty(path: PathBuf) -> PathBuf {
    std::fs::File::create(&path).expect("failed to create a directive file in its own temp dir");
    path
}

/// Configure a Command with the shell wrapper's CD directive file.
pub fn configure_directive_file(cmd: &mut Command, cd_path: &Path) {
    cmd.env("WORKTRUNK_DIRECTIVE_CD_FILE", cd_path);
}

/// Set home environment variables for commands that rely on isolated temp homes.
///
/// Sets both Unix (`HOME`, `XDG_CONFIG_HOME`) and Windows (`USERPROFILE`) variables
/// so the `home` crate can find the temp home directory on all platforms.
///
/// Canonicalizes the path on macOS to handle `/var` → `/private/var` symlinks.
/// This ensures `format_path_for_display()` can correctly convert paths to `~/...`.
pub fn set_temp_home_env(cmd: &mut Command, home: &Path) {
    // Canonicalize to resolve macOS symlinks (/var -> /private/var)
    // This ensures paths match when format_path_for_display() compares against HOME
    let home = canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    cmd.env("HOME", &home);
    cmd.env("XDG_CONFIG_HOME", home.join(".config"));
    // Windows: the `home` crate uses USERPROFILE for home_dir()
    cmd.env("USERPROFILE", &home);
    // Windows: etcetera uses APPDATA for config_dir() (AppData\Roaming)
    // Map it to .config to match Unix XDG_CONFIG_HOME behavior
    cmd.env("APPDATA", home.join(".config"));
    // OpenCode: override config dir to avoid platform-specific dirs::config_dir() differences
    // (Linux: ~/.config, macOS: ~/Library/Application Support, Windows: AppData\Roaming)
    cmd.env("OPENCODE_CONFIG_DIR", home.join("opencode-config"));
    // Claude Code: pin the config dir to the temp home's `.claude` so detection
    // matches the setup helpers and stays hermetic against an ambient
    // CLAUDE_CONFIG_DIR inherited from the test runner's environment.
    cmd.env("CLAUDE_CONFIG_DIR", home.join(".claude"));
}

/// Override `WORKTRUNK_CONFIG_PATH` to point to the XDG-derived user config path
/// under `home`. Use this after `set_temp_home_env` in tests that write user
/// config at the XDG path and need `config create`/`config show` to find it.
pub fn set_xdg_config_path(cmd: &mut Command, home: &Path) {
    let home = canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    cmd.env(
        "WORKTRUNK_CONFIG_PATH",
        home.join(".config").join("worktrunk").join("config.toml"),
    );
}

/// Check that a git command succeeded, panicking with diagnostics if not.
///
/// Use this after `git_command().run()` to ensure the command succeeded.
///
/// # Example
/// ```ignore
/// let output = repo.git_command().args(["add", "."]).current_dir(&dir).run().unwrap();
/// check_git_status(&output, "add");
/// ```
pub fn check_git_status(output: &std::process::Output, cmd_desc: &str) {
    if !output.status.success() {
        panic!(
            "git {} failed:\nstdout: {}\nstderr: {}",
            cmd_desc,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// The isolated per-test config files a [`TestRepo`] carries, both rooted in
/// one temp directory. (There is no git config among them: the suite's git
/// isolation is environment, carrying no per-test state.)
struct TestConfigPaths {
    wt: PathBuf,
    approvals: PathBuf,
}

impl TestConfigPaths {
    fn in_dir(dir: &Path) -> Self {
        Self {
            wt: dir.join("test-config.toml"),
            approvals: dir.join("test-approvals.toml"),
        }
    }
}

pub struct TestRepo {
    temp_dir: TempDir, // Must keep to ensure cleanup on drop
    root: PathBuf,
    /// Repository handle for direct library API access (unit tests).
    pub repo: Repository,
    pub worktrees: HashMap<String, PathBuf>,
    remote: Option<PathBuf>, // Path to bare remote repo if created
    /// Isolated config file for this test (prevents pollution of user's config)
    test_config_path: PathBuf,
    /// Isolated approvals file for this test (prevents pollution of user's approvals)
    test_approvals_path: PathBuf,
    /// Path to mock bin directory for gh/glab commands
    mock_bin_path: Option<PathBuf>,
    /// Whether Claude CLI should be treated as installed
    claude_installed: bool,
    /// Whether Codex CLI should be treated as installed
    codex_installed: bool,
    /// Whether OpenCode CLI should be treated as installed
    opencode_installed: bool,
    /// Whether Gemini CLI should be treated as installed
    gemini_installed: bool,
    /// Whether to drop the `WORKTRUNK_TEST_*_INSTALLED` overrides so
    /// `is_*_available()` exercises its real `which::which` PATH lookup
    detect_clis_via_path: bool,
}

impl TestRepo {
    /// Create a lightweight test repo with `git init -b main` and test identity.
    ///
    /// For unit tests that need a real `.git` directory. Uses env-isolated
    /// git commands for deterministic behavior.
    ///
    /// For integration tests needing a full fixture (remote, worktrees, mock
    /// commands), use [`standard()`](Self::standard) instead.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self::init_repo(&["init", "-b", "main"])
    }

    /// Create a repo with one initial commit on `main`.
    ///
    /// Equivalent to `new()` followed by creating a file and committing it.
    /// Use this when tests need a non-empty repo (e.g. for branching or
    /// worktree operations that require at least one commit).
    pub fn with_initial_commit() -> Self {
        let test = Self::new();
        std::fs::write(test.path().join("file.txt"), "hello").unwrap();
        test.run_git(&["add", "."]);
        test.run_git(&["commit", "-m", "init"]);
        test
    }

    /// Create a bare repository (`git init --bare`).
    ///
    /// Bare repos have no working tree — useful for testing error paths
    /// and bare-repo-specific behavior (e.g., hint fallback to `wt list`).
    pub fn bare() -> Self {
        Self::init_repo(&["init", "--bare", "-b", "main"])
    }

    /// Path to the repository working directory.
    ///
    /// Alias for [`root_path()`](Self::root_path) for backward compatibility
    /// with unit tests.
    pub fn path(&self) -> &Path {
        self.root_path()
    }

    /// Create a test repository from the standard fixture.
    ///
    /// The repo includes:
    /// - Main branch with one initial commit
    /// - Remote (origin) bare repository
    /// - Three feature worktrees (feature-a, feature-b, feature-c) each with one commit
    ///
    /// Copies the once-per-target-dir template ([`standard_fixture_template`])
    /// instead of running git commands, for fast initialization.
    ///
    /// Also sets up mock gh/glab commands that appear authenticated to prevent
    /// CI status hints from appearing in test output.
    pub fn standard() -> Self {
        let temp_dir = test_tempdir();

        // Copy from standard fixture (includes worktrees and remote)
        let fixture = copy_standard_fixture(temp_dir.path());

        let paths = TestConfigPaths::in_dir(temp_dir.path());
        let root = temp_dir.path().join("repo");

        let mut repo = Self::assemble(temp_dir, &root, paths);
        repo.worktrees = fixture.worktrees;
        repo.remote = Some(fixture.remote);

        // Mock gh/glab as authenticated to prevent CI hints in test output
        repo.setup_mock_gh();

        repo
    }

    /// Create a test repository with the standard fixture's pinned main
    /// history and remote, but no linked worktrees.
    ///
    /// Use when a test constructs its own worktree topology. Unlike
    /// [`standard()`](Self::standard), this does not install forge mocks:
    /// callers that exercise forge commands should install strict mocks for
    /// the route they expect.
    pub fn standard_main_only() -> Self {
        let temp_dir = test_tempdir();
        let remote = copy_standard_main_only_fixture(temp_dir.path());

        let paths = TestConfigPaths::in_dir(temp_dir.path());
        let root = temp_dir.path().join("repo");
        let mut repo = Self::assemble(temp_dir, &root, paths);
        repo.remote = Some(remote);
        repo
    }

    /// Create a repo at a caller-specified path with identity configured.
    ///
    /// Unlike [`new()`](Self::new), this does not own the repo's parent
    /// directory — the caller manages its lifetime (e.g., via their own
    /// `TempDir`). A separate internal tempdir holds config files.
    ///
    /// Use for tests that need multiple repos in a shared directory
    /// (e.g., sibling worktrees, multi-repo recovery tests).
    pub fn at(path: &Path) -> Self {
        Self::at_with(path, &["init", "-b", "main", "--quiet"])
    }

    /// [`at()`](Self::at) for a bare repository (`git init --bare`).
    ///
    /// Use for caller-shaped bare layouts, e.g. the `git clone --bare <url>
    /// project/.git` pattern where the bare dir sits inside a project
    /// directory the test owns.
    pub fn bare_at(path: &Path) -> Self {
        Self::at_with(path, &["init", "--bare", "-b", "main", "--quiet"])
    }

    /// Shared initializer for [`at()`](Self::at) and [`bare_at()`](Self::bare_at):
    /// runs `git init` with the given arguments at a caller-managed path.
    fn at_with(path: &Path, git_args: &[&str]) -> Self {
        std::fs::create_dir_all(path).unwrap();

        let config_dir = test_tempdir();
        let paths = TestConfigPaths::in_dir(config_dir.path());

        configure_git_env(Cmd::new("git"))
            .args(git_args.iter().copied())
            .current_dir(path)
            .run()
            .unwrap();

        Self::assemble(config_dir, path, paths)
    }

    /// Create an empty test repository (no commits, no branches).
    ///
    /// Use this for tests that specifically need to test behavior in an
    /// uninitialized repo. Most tests should use `new()` instead.
    pub fn empty() -> Self {
        Self::init_repo(&["init", "-q", "-b", "main"])
    }

    /// Shared initializer for `new()`, `bare()`, and `empty()`: makes a tempdir
    /// and runs `git init` with the given arguments inside it.
    fn init_repo(git_args: &[&str]) -> Self {
        shell_exec::enable_hermetic_test_env();
        let temp_dir = test_tempdir();
        let root = temp_dir.path().join("repo");
        std::fs::create_dir(&root).unwrap();

        let paths = TestConfigPaths::in_dir(temp_dir.path());

        configure_git_env(Cmd::new("git"))
            .args(git_args.iter().copied())
            .current_dir(&root)
            .run()
            .unwrap();

        Self::assemble(temp_dir, &root, paths)
    }

    /// Assemble a `TestRepo` around a git repo that already exists at `root`.
    ///
    /// Every constructor lands here, which is what lets
    /// [`write_local_test_config`] be unconditional: a repo built by a future
    /// constructor carries those settings by construction rather than by
    /// remembering to ask for them.
    fn assemble(temp_dir: TempDir, root: &Path, paths: TestConfigPaths) -> Self {
        // Canonicalize to resolve symlinks (on macOS /var is a symlink to /private/var)
        let root = canonicalize(root).unwrap();
        let repo = Repository::at(&root).unwrap();
        write_local_test_config(&repo);

        Self {
            temp_dir,
            root,
            repo,
            worktrees: HashMap::new(),
            remote: None,
            test_config_path: paths.wt,
            test_approvals_path: paths.approvals,
            mock_bin_path: None,
            claude_installed: false,
            codex_installed: false,
            opencode_installed: false,
            gemini_installed: false,
            detect_clis_via_path: false,
        }
    }

    /// Configure a git command with isolated environment
    ///
    /// This sets environment variables only for the specific command,
    /// ensuring thread-safety and test isolation.
    pub fn configure_git_cmd(&self, cmd: &mut Command) {
        configure_git_cmd(cmd);
    }

    /// This repo's environment for a PTY-spawned wt subprocess.
    ///
    /// Thin wrapper over [`pty_env_vars`], which documents the layering and is
    /// where a new variable goes. Command-based tests use
    /// [`configure_cli_command`] instead.
    #[cfg_attr(windows, allow(dead_code))] // Used only by unix PTY tests
    pub fn test_env_vars(&self) -> Vec<(String, String)> {
        pty_env_vars(TestEnvPaths {
            home: self.home_path(),
            wt_config: self.test_config_path(),
            approvals: self.test_approvals_path(),
        })
    }

    /// Configure shell integration for test environment.
    ///
    /// Writes the shell config line to `.zshrc` in the test home directory.
    /// Call this before tests that need shell integration to appear configured.
    /// The test should also include `SHELL=/bin/zsh` in its env vars.
    #[cfg_attr(windows, allow(dead_code))] // Used only by unix PTY tests
    pub fn configure_shell_integration(&self) {
        let zshrc_path = self.home_path().join(".zshrc");
        std::fs::write(
            &zshrc_path,
            "if command -v wt >/dev/null 2>&1; then eval \"$(command wt config shell init zsh)\"; fi\n",
        )
        .expect("Failed to write .zshrc for test");
    }

    /// Create a `git` command pre-configured for this test repo.
    ///
    /// Returns an isolated `Cmd` with test-specific git config.
    /// Chain `.args()` to add arguments, then `.run()` to execute.
    ///
    /// # Example
    /// ```ignore
    /// repo.git_command()
    ///     .args(["status", "--porcelain"])
    ///     .run()?;
    /// ```
    #[must_use]
    pub fn git_command(&self) -> Cmd {
        configure_git_env(Cmd::new("git")).current_dir(&self.root)
    }

    /// Run a git command in the repo root, panicking on failure.
    ///
    /// Thin wrapper around `git_command()` that runs the command and checks status.
    pub fn run_git(&self, args: &[&str]) {
        let output = self.git_command().args(args.iter().copied()).run().unwrap();
        check_git_status(&output, &args.join(" "));
    }

    /// Create many branches at `HEAD` in a single `git update-ref --stdin`
    /// spawn, panicking on failure.
    ///
    /// A fixture that needs N branches to cross a threshold cares about the
    /// ref count, not about how the refs got there. Looping `git branch`
    /// turns that into N process spawns, and integration tests run at full
    /// core parallelism — so a fixture loop is multiplied by every other test
    /// running beside it. One spawn keeps the fixture's cost proportional to
    /// what it is actually setting up.
    pub fn create_branches(&self, names: &[String]) {
        let stdin: String = names
            .iter()
            .map(|n| format!("create refs/heads/{n} HEAD\n"))
            .collect();
        let output = self
            .git_command()
            .args(["update-ref", "--stdin"])
            .stdin_bytes(stdin)
            .run()
            .unwrap();
        check_git_status(&output, "update-ref --stdin");
    }

    /// Run a git command in a specific directory, panicking on failure.
    ///
    /// Thin wrapper around `git_command()` that runs in `dir` and checks status.
    pub fn run_git_in(&self, dir: &Path, args: &[&str]) {
        let output = self
            .git_command()
            .args(args.iter().copied())
            .current_dir(dir)
            .run()
            .unwrap();
        check_git_status(&output, &args.join(" "));
    }

    /// Run a git command and return stdout as a trimmed string.
    ///
    /// Thin wrapper around `git_command()` for commands that return output.
    pub fn git_output(&self, args: &[&str]) -> String {
        let output = self.git_command().args(args.iter().copied()).run().unwrap();
        check_git_status(&output, &args.join(" "));
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// Remove fixture worktrees to get a clean state for tests.
    ///
    /// The standard fixture includes worktrees for feature-a, feature-b, feature-c.
    /// Call this method in tests that need a specific worktree state. Also clears
    /// the worktrees map so `add_worktree` can recreate them if needed.
    pub fn remove_fixture_worktrees(&mut self) {
        for branch in &["feature-a", "feature-b", "feature-c"] {
            let worktree_path = self
                .root_path()
                .parent()
                .unwrap()
                .join(format!("repo.{}", branch));
            if worktree_path.exists() {
                let _ = self
                    .git_command()
                    .args([
                        "worktree",
                        "remove",
                        "--force",
                        worktree_path.to_str().unwrap(),
                    ])
                    .run();
            }
            // Delete the branch after removing the worktree
            let _ = self.git_command().args(["branch", "-D", branch]).run();
            // Remove from worktrees map so add_worktree() can recreate if needed
            self.worktrees.remove(*branch);
        }
    }

    /// Stage all changes in a directory.
    pub fn stage_all(&self, dir: &Path) {
        self.run_git_in(dir, &["add", "."]);
    }

    /// Get the HEAD commit SHA.
    pub fn head_sha(&self) -> String {
        let output = self
            .git_command()
            .args(["rev-parse", "HEAD"])
            .run()
            .unwrap();
        check_git_status(&output, "rev-parse HEAD");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// Get the HEAD commit SHA in a specific directory.
    pub fn head_sha_in(&self, dir: &Path) -> String {
        let output = self
            .git_command()
            .args(["rev-parse", "HEAD"])
            .current_dir(dir)
            .run()
            .unwrap();
        check_git_status(&output, "rev-parse HEAD");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// Configure command for CLI tests with isolated environment.
    ///
    /// Sets `WORKTRUNK_CONFIG_PATH`, `HOME`, and mock gh/glab commands.
    ///
    /// **Internal helper** - used by `wt_command()` and `make_snapshot_cmd()`.
    /// Tests should use `repo.wt_command()` instead of calling this directly.
    pub fn configure_wt_cmd(&self, cmd: &mut Command) {
        configure_cli_command(cmd);
        self.configure_git_cmd(cmd);
        cmd.env("WORKTRUNK_CONFIG_PATH", &self.test_config_path);
        cmd.env(
            "WORKTRUNK_SYSTEM_CONFIG_PATH",
            "/etc/xdg/worktrunk/config.toml",
        );
        cmd.env("WORKTRUNK_APPROVALS_PATH", &self.test_approvals_path);
        set_temp_home_env(cmd, self.home_path());
        self.configure_mock_commands(cmd);
    }

    /// Create a `wt` command pre-configured for this test repo.
    ///
    /// This is the preferred way to run wt commands in tests. The returned
    /// Command is isolated from the host environment (no WORKTRUNK_* leakage,
    /// no GIT_* interference) and configured with the test repo's config.
    ///
    /// # Example
    /// ```ignore
    /// let output = repo.wt_command()
    ///     .args(["switch", "--create", "feature"])
    ///     .output()?;
    /// ```
    #[must_use]
    pub fn wt_command(&self) -> Command {
        let mut cmd = Command::new(wt_bin());
        self.configure_wt_cmd(&mut cmd);
        cmd.current_dir(self.root_path());
        cmd
    }

    /// Get the isolated HOME directory for this test.
    ///
    /// This is the temp directory containing the repo and can be used to set up
    /// user config files before running commands:
    /// - `.zshrc`, `.bashrc` - shell integration config
    /// - `.config/worktrunk/config.toml` - user config (note: overridden by WORKTRUNK_CONFIG_PATH)
    ///
    /// The directory structure is:
    /// ```text
    /// home_path()/
    /// ├── repo/              # The git repository (root_path())
    /// └── test-config.toml   # WORKTRUNK_CONFIG_PATH target
    /// ```
    pub fn home_path(&self) -> &Path {
        self.temp_dir.path()
    }

    /// Prepare a `wt` command configured for shell completions within this repo.
    pub fn completion_cmd(&self, words: &[&str]) -> Command {
        self.completion_cmd_for_shell(words, "bash")
    }

    /// Prepare a `wt` command configured for shell completions for a specific shell.
    pub fn completion_cmd_for_shell(&self, words: &[&str], shell: &str) -> Command {
        let mut cmd = wt_command();
        configure_completion_invocation_for_shell(&mut cmd, words, shell);
        self.configure_wt_cmd(&mut cmd);
        cmd.current_dir(self.root_path());
        cmd
    }

    /// Get the root path of the repository
    pub fn root_path(&self) -> &Path {
        &self.root
    }

    /// Get the mock bin directory path (for custom mock setups)
    pub fn mock_bin_path(&self) -> Option<&Path> {
        self.mock_bin_path.as_deref()
    }

    /// Get the path to the bare remote repository, if created.
    pub fn remote_path(&self) -> Option<&Path> {
        self.remote.as_deref()
    }

    /// Get the project identifier (canonical path) for this test repo.
    ///
    /// Returns the full canonical path of the repository. The standard fixture uses a local
    /// path remote (`../origin.git`) which doesn't parse as a proper git URL, causing
    /// worktrunk to fall back to the full canonical path.
    ///
    /// Use with TOML literal strings (single quotes) to avoid backslash escaping:
    /// ```ignore
    /// format!(r#"[projects.'{}']"#, repo.project_id())
    /// ```
    pub fn project_id(&self) -> String {
        dunce::canonicalize(&self.root)
            .unwrap_or_else(|_| self.root.clone())
            .to_str()
            .unwrap_or("")
            .to_string()
    }

    /// Get the path to the isolated test config file
    ///
    /// This config path is automatically set via WORKTRUNK_CONFIG_PATH when using
    /// `configure_wt_cmd()`, ensuring tests don't pollute the user's real config.
    pub fn test_config_path(&self) -> &Path {
        &self.test_config_path
    }

    /// Get the path to the isolated test approvals file
    ///
    /// This approvals path is automatically set via WORKTRUNK_APPROVALS_PATH when using
    /// `configure_wt_cmd()`, ensuring tests don't pollute the user's real approvals.
    pub fn test_approvals_path(&self) -> &Path {
        &self.test_approvals_path
    }

    /// Write project-specific config (`.config/wt.toml`) under the repo root.
    pub fn write_project_config(&self, contents: &str) {
        let config_dir = self.root_path().join(".config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("wt.toml"), contents).unwrap();
    }

    /// Overwrite the isolated WORKTRUNK_CONFIG_PATH used during tests.
    ///
    /// Automatically prepends `skip-commit-generation-prompt = true` to prevent
    /// interactive prompts from appearing in test output.
    pub fn write_test_config(&self, contents: &str) {
        let full_contents = format!("skip-commit-generation-prompt = true\n{}", contents);
        std::fs::write(&self.test_config_path, full_contents).unwrap();
    }

    /// Write approved commands to the isolated WORKTRUNK_APPROVALS_PATH.
    pub fn write_test_approvals(&self, contents: &str) {
        std::fs::write(&self.test_approvals_path, contents).unwrap();
    }

    /// Get the path to a named worktree
    pub fn worktree_path(&self, name: &str) -> &Path {
        self.worktrees
            .get(name)
            .unwrap_or_else(|| panic!("Worktree '{}' not found", name))
    }

    /// Create a commit with the given message
    pub fn commit(&self, message: &str) {
        // Create a file to ensure there's something to commit
        let file_path = self.root.join("file.txt");
        std::fs::write(&file_path, message).unwrap();

        self.git_command().args(["add", "."]).run().unwrap();

        self.git_command()
            .args(["commit", "-m", message])
            .run()
            .unwrap();
    }

    /// Create a commit with a custom message (useful for testing malicious messages)
    pub fn commit_with_message(&self, message: &str) {
        // Create file with message-derived name for deterministic commits
        // Use first 16 chars of message (sanitized) as filename
        let sanitized: String = message
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
            .take(16)
            .collect();
        let file_path = self.root.join(format!("file-{}.txt", sanitized));
        std::fs::write(&file_path, message).unwrap();

        self.git_command().args(["add", "."]).run().unwrap();

        self.git_command()
            .args(["commit", "-m", message])
            .run()
            .unwrap();
    }

    /// Create a commit with a specific age relative to TEST_EPOCH
    ///
    /// This allows creating commits that display specific relative ages
    /// in the Age column (e.g., "10m", "1h", "1d").
    ///
    /// # Arguments
    /// * `message` - The commit message
    /// * `age_seconds` - How many seconds ago the commit should appear
    ///
    /// # Example
    /// ```ignore
    /// repo.commit_with_age("Initial commit", 86400);  // Shows "1d"
    /// repo.commit_with_age("Fix bug", 3600);          // Shows "1h"
    /// repo.commit_with_age("Add feature", 600);       // Shows "10m"
    /// ```
    pub fn commit_with_age(&self, message: &str, age_seconds: i64) {
        let commit_time = TEST_EPOCH as i64 - age_seconds;
        // Use ISO 8601 format for consistent behavior across git versions
        let timestamp = unix_to_iso8601(commit_time);

        // Use file.txt like commit() does - allows multiple commits to the same file
        let file_path = self.root.join("file.txt");
        std::fs::write(&file_path, message).unwrap();

        self.git_command().args(["add", "."]).run().unwrap();

        // Create commit with custom timestamp
        self.git_command()
            .env("GIT_AUTHOR_DATE", &timestamp)
            .env("GIT_COMMITTER_DATE", &timestamp)
            .args(["commit", "-m", message])
            .run()
            .unwrap();
    }

    /// Commit already-staged changes with a specific age
    ///
    /// This does NOT create or modify any files - it only commits staged changes.
    /// Use this when you've already staged specific files and want clean diffs
    /// (no spurious file.txt changes).
    ///
    /// # Example
    /// ```ignore
    /// std::fs::write(wt.join("feature.rs"), "...").unwrap();
    /// run_git(&repo, &["add", "feature.rs"], &wt);
    /// repo.commit_staged_with_age("Add feature", 2 * HOUR, &wt);
    /// ```
    pub fn commit_staged_with_age(&self, message: &str, age_seconds: i64, dir: &Path) {
        let commit_time = TEST_EPOCH as i64 - age_seconds;
        let timestamp = unix_to_iso8601(commit_time);

        self.git_command()
            .env("GIT_AUTHOR_DATE", &timestamp)
            .env("GIT_COMMITTER_DATE", &timestamp)
            .args(["commit", "-m", message])
            .current_dir(dir)
            .run()
            .unwrap();
    }

    /// Add a worktree with the given name and branch
    ///
    /// The worktree path follows the default template format: `repo.{branch}`
    /// (sanitized, with slashes replaced by dashes).
    ///
    /// If the worktree already exists (from the standard fixture), returns its path
    /// without creating a new one.
    pub fn add_worktree(&mut self, branch: &str) -> PathBuf {
        // If worktree already exists (from fixture), just return its path
        if let Some(path) = self.worktrees.get(branch) {
            return path.clone();
        }

        let safe_branch = sanitize_branch_name(branch);
        // Use default template path format: ../{{ repo }}.{{ branch }}
        // From {temp_dir}/repo, this resolves to {temp_dir}/repo.{branch}
        let worktree_path = self.temp_dir.path().join(format!("repo.{}", safe_branch));
        let worktree_str = worktree_path.to_str().unwrap();

        self.run_git(&["worktree", "add", "-b", branch, worktree_str]);

        // Canonicalize worktree path to match what git returns
        let canonical_path = canonicalize(&worktree_path).unwrap();
        // Use branch as key (consistent with path generation)
        self.worktrees
            .insert(branch.to_string(), canonical_path.clone());
        canonical_path
    }

    /// Creates a worktree at a custom path (for testing nested worktrees).
    ///
    /// Unlike `add_worktree`, this places the worktree at the specified path
    /// rather than using the default sibling layout.
    pub fn add_worktree_at_path(&mut self, branch: &str, path: &Path) -> PathBuf {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let path_str = path.to_str().unwrap();
        self.run_git(&["worktree", "add", "-b", branch, path_str]);

        let canonical_path = canonicalize(path).unwrap();
        self.worktrees
            .insert(branch.to_string(), canonical_path.clone());
        canonical_path
    }

    /// Creates a worktree for the default branch (required for merge operations)
    ///
    /// This is a convenience method that creates a worktree for the default branch
    /// in the standard location expected by merge tests. Returns the path to the
    /// created worktree.
    ///
    /// If the primary worktree is currently on "main", this method detaches HEAD
    /// first so the worktree can be created.
    pub fn add_main_worktree(&self) -> PathBuf {
        // If primary is on main, detach HEAD first so we can create a worktree for it
        if self.current_branch() == "main" {
            self.detach_head();
        }

        let main_wt = self.root_path().parent().unwrap().join("repo.main-wt");
        let main_wt_str = main_wt.to_str().unwrap();
        self.run_git(&["worktree", "add", main_wt_str, "main"]);
        main_wt
    }

    /// Creates a worktree with a file and commits it.
    ///
    /// This is a convenience method that combines the common pattern of:
    /// 1. Creating a worktree for a new branch
    /// 2. Writing a file to it
    /// 3. Staging and committing the file
    ///
    /// # Example
    /// ```ignore
    /// let feature_wt = repo.add_worktree_with_commit(
    ///     "feature",
    ///     "feature.txt",
    ///     "feature content",
    ///     "Add feature file",
    /// );
    /// ```
    pub fn add_worktree_with_commit(
        &mut self,
        branch: &str,
        filename: &str,
        content: &str,
        message: &str,
    ) -> PathBuf {
        let worktree_path = self.add_worktree(branch);
        std::fs::write(worktree_path.join(filename), content).unwrap();
        self.run_git_in(&worktree_path, &["add", filename]);
        self.run_git_in(&worktree_path, &["commit", "-m", message]);
        worktree_path
    }

    /// Shorthand: adds a "feature" worktree with a canonical commit.
    ///
    /// Equivalent to:
    /// ```ignore
    /// repo.add_worktree_with_commit("feature", "feature.txt", "feature content", "Add feature file")
    /// ```
    ///
    /// Returns the path to the feature worktree.
    pub fn add_feature(&mut self) -> PathBuf {
        self.add_worktree_with_commit(
            "feature",
            "feature.txt",
            "feature content",
            "Add feature file",
        )
    }

    /// Adds a commit to an existing worktree.
    ///
    /// This writes a file, stages it, and commits it in the specified worktree.
    /// Useful for tests that need multiple commits in the same worktree.
    ///
    /// # Arguments
    /// * `worktree_path` - Path to the existing worktree
    /// * `filename` - Name of the file to create/modify
    /// * `content` - Content to write to the file
    /// * `message` - Commit message
    ///
    /// # Example
    /// ```ignore
    /// let feature_wt = repo.add_worktree("feature");
    /// repo.commit_in_worktree(&feature_wt, "file1.txt", "content 1", "feat: add file 1");
    /// repo.commit_in_worktree(&feature_wt, "file2.txt", "content 2", "feat: add file 2");
    /// ```
    pub fn commit_in_worktree(
        &self,
        worktree_path: &Path,
        filename: &str,
        content: &str,
        message: &str,
    ) {
        std::fs::write(worktree_path.join(filename), content).unwrap();
        self.run_git_in(worktree_path, &["add", filename]);
        self.run_git_in(worktree_path, &["commit", "-m", message]);
    }

    /// Creates a branch without a worktree.
    ///
    /// This creates a local branch pointing to HEAD without checking it out.
    /// Useful for testing branch listing without creating worktrees.
    pub fn create_branch(&self, branch_name: &str) {
        self.run_git(&["branch", branch_name]);
    }

    /// Pushes a branch to origin.
    ///
    /// Creates a remote tracking branch on origin. Requires `setup_remote()`
    /// to have been called first.
    pub fn push_branch(&self, branch_name: &str) {
        self.run_git(&["push", "origin", branch_name]);
    }

    /// Detach HEAD in the main repository
    pub fn detach_head(&self) {
        self.detach_head_at(&self.root);
    }

    /// Detach HEAD in a specific worktree
    pub fn detach_head_in_worktree(&self, name: &str) {
        let worktree_path = self.worktree_path(name);
        self.detach_head_at(worktree_path);
    }

    fn detach_head_at(&self, path: &Path) {
        let sha = self.head_sha_in(path);
        self.run_git_in(path, &["checkout", "--detach", &sha]);
    }

    /// Lock a worktree with an optional reason
    pub fn lock_worktree(&self, name: &str, reason: Option<&str>) {
        let worktree_path = self.worktree_path(name);
        let worktree_str = worktree_path.to_str().unwrap();

        match reason {
            Some(r) => self.run_git(&["worktree", "lock", "--reason", r, worktree_str]),
            None => self.run_git(&["worktree", "lock", worktree_str]),
        }
    }

    /// Create a bare remote repository and set it as origin
    ///
    /// This creates a bare git repository in the temp directory and configures
    /// it as the 'origin' remote. The remote will have the same default branch
    /// as the local repository (main).
    pub fn setup_remote(&mut self, default_branch: &str) {
        self.setup_custom_remote("origin", default_branch);
    }

    /// Create a bare remote repository with a custom name
    ///
    /// This creates a bare git repository in the temp directory and configures
    /// it with the specified remote name. The remote will have the same default
    /// branch as the local repository.
    ///
    /// If the remote already exists (from fixture), this is a no-op.
    pub fn setup_custom_remote(&mut self, remote_name: &str, default_branch: &str) {
        // If origin remote already exists (from fixture), just ensure HEAD is set
        if remote_name == "origin" && self.remote.is_some() {
            // Set origin/HEAD (fixture may not have this set)
            self.run_git(&["remote", "set-head", "origin", default_branch]);
            return;
        }

        // Create bare remote repository
        let remote_path = self.temp_dir.path().join(format!("{}.git", remote_name));
        if remote_path.exists() {
            // Remote directory already exists, just use it
            self.remote = Some(canonicalize(&remote_path).unwrap());
            return;
        }
        std::fs::create_dir(&remote_path).unwrap();

        self.run_git_in(
            &remote_path,
            &["init", "--bare", "--initial-branch", default_branch],
        );

        // Canonicalize remote path
        let remote_path = canonicalize(&remote_path).unwrap();
        let remote_path_str = remote_path.to_str().unwrap();

        // Add as remote, push, and set HEAD
        self.run_git(&["remote", "add", remote_name, remote_path_str]);
        self.run_git(&["push", "-u", remote_name, default_branch]);
        self.run_git(&["remote", "set-head", remote_name, default_branch]);

        self.remote = Some(remote_path);
    }

    /// Clear the local origin/HEAD reference
    ///
    /// This forces git to not have a cached default branch, useful for testing
    /// the fallback path that queries the remote.
    pub fn clear_origin_head(&self) {
        self.run_git(&["remote", "set-head", "origin", "--delete"]);
    }

    /// Check if origin/HEAD is set
    pub fn has_origin_head(&self) -> bool {
        self.git_command()
            .args(["rev-parse", "--abbrev-ref", "origin/HEAD"])
            .run()
            .unwrap()
            .status
            .success()
    }

    /// Switch the primary worktree to a different branch
    ///
    /// Creates a new branch and switches to it in the primary worktree.
    /// This is useful for testing scenarios where the primary worktree is not on the default branch.
    pub fn switch_primary_to(&self, branch: &str) {
        self.run_git(&["switch", "-c", branch]);
    }

    /// Get the current branch of the primary worktree
    ///
    /// Returns the name of the current branch, or panics if HEAD is detached.
    pub fn current_branch(&self) -> String {
        self.git_output(&["branch", "--show-current"])
    }

    /// Setup mock `gh` and `glab` commands that return immediately without network calls
    ///
    /// Creates a mock bin directory with fake gh/glab scripts. After calling this,
    /// use `configure_mock_commands()` to add the mock bin to PATH for your commands.
    ///
    /// The mock gh returns:
    /// - `gh auth status`: exits successfully (0)
    /// - `gh pr list`: returns empty JSON array (no PRs found)
    ///
    /// This prevents CI detection from blocking tests with network calls.
    pub fn setup_mock_gh(&mut self) {
        self.setup_mock_gh_with_ci_data("[]");
    }

    /// Setup mock `gh` and `glab` commands that show "installed but not authenticated"
    ///
    /// Use this for `wt config show` tests that need deterministic BINARIES output.
    /// Creates mocks where:
    /// - `gh --version`: succeeds (installed)
    /// - `gh auth status`: fails (not authenticated)
    /// - `glab --version`: succeeds (installed)
    /// - `glab auth status`: fails (not authenticated)
    pub fn setup_mock_ci_tools_unauthenticated(&mut self) {
        let mock_bin = self.temp_dir.path().join("mock-bin");
        std::fs::create_dir_all(&mock_bin).unwrap();

        // gh: installed but not authenticated
        MockConfig::new("gh")
            .version("gh version 2.0.0 (mock)")
            .command("auth", MockResponse::exit(1))
            .write(&mock_bin);

        // glab: installed but not authenticated
        MockConfig::new("glab")
            .version("glab version 1.0.0 (mock)")
            .command("auth", MockResponse::exit(1))
            .write(&mock_bin);

        // claude: not installed (don't create mock - which::which won't find it)

        self.mock_bin_path = Some(mock_bin);
    }

    /// Add a mock `tea` (installed, no login) to the existing mock bin.
    ///
    /// Call this after `setup_mock_ci_tools_unauthenticated()` to make
    /// `wt config show --full` against a Gitea remote deterministic — the
    /// Gitea diagnostics row only depends on `tea`'s state.
    pub fn setup_mock_tea_installed(&mut self) {
        let mock_bin = self
            .mock_bin_path
            .as_ref()
            .expect("call setup_mock_ci_tools_unauthenticated() first");
        MockConfig::new("tea")
            .version("tea version development (mock)")
            .write(mock_bin);
    }

    /// Add a mock `az` (installed and authenticated) whose `az extension list`
    /// reports `extensions_json` to the existing mock bin.
    ///
    /// Call this after `setup_mock_ci_tools_unauthenticated()` to make
    /// `wt config show --full` against an Azure DevOps remote deterministic —
    /// the Azure diagnostics rows depend on `az`'s state and on whether the
    /// `azure-devops` extension is among the installed ones.
    pub fn setup_mock_az_with_extensions(&mut self, extensions_json: &str) {
        let mock_bin = self
            .mock_bin_path
            .as_ref()
            .expect("call setup_mock_ci_tools_unauthenticated() first");
        std::fs::write(mock_bin.join("az_extensions.json"), extensions_json).unwrap();
        MockConfig::new("az")
            .version("azure-cli 2.60.0 (mock)")
            .command("account show", MockResponse::exit(0))
            .command("extension list", MockResponse::file("az_extensions.json"))
            .command("_default", MockResponse::exit(1))
            .write(mock_bin);
    }

    /// Setup mock `claude` CLI as installed
    ///
    /// Call this after setup_mock_ci_tools_unauthenticated() to simulate
    /// Claude Code being available on the system.
    pub fn setup_mock_claude_installed(&mut self) {
        // Mark Claude as installed for test environment
        self.claude_installed = true;
    }

    /// Setup mock `codex` CLI as installed
    ///
    /// Call this after setup_mock_ci_tools_unauthenticated() to simulate
    /// Codex being available on the system.
    pub fn setup_mock_codex_installed(&mut self) {
        self.codex_installed = true;
    }

    /// Setup the worktrunk plugin as installed in Claude Code
    ///
    /// Creates the installed_plugins.json file in the temp home directory.
    /// The temp_home must already be set up (via set_temp_home_env on the command).
    pub fn setup_plugin_installed(temp_home: &std::path::Path) {
        let plugins_dir = temp_home.join(".claude/plugins");
        std::fs::create_dir_all(&plugins_dir).unwrap();
        std::fs::write(
            plugins_dir.join("installed_plugins.json"),
            r#"{"version":2,"plugins":{"worktrunk@worktrunk":[{"scope":"user"}]}}"#,
        )
        .unwrap();
    }

    /// Setup the statusline as configured in Claude Code settings
    ///
    /// Creates the settings.json file with the wt statusline command.
    /// The temp_home must already be set up (via set_temp_home_env on the command).
    pub fn setup_statusline_configured(temp_home: &std::path::Path) {
        let claude_dir = temp_home.join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(
            claude_dir.join("settings.json"),
            r#"{"statusLine":{"type":"command","command":"wt list statusline --format=claude-code"}}"#,
        )
        .unwrap();
    }

    /// Setup mock `opencode` CLI as installed
    ///
    /// Call this to simulate OpenCode being available on the system.
    pub fn setup_mock_opencode_installed(&mut self) {
        self.opencode_installed = true;
    }

    /// Setup the worktrunk plugin as installed in OpenCode
    ///
    /// Creates the worktrunk.ts plugin file in the OpenCode config directory.
    /// Uses `opencode-config/plugins/` under temp_home, which aligns with the
    /// `OPENCODE_CONFIG_DIR` env var set in `configure_wt_cmd()` and install/uninstall tests.
    pub fn setup_opencode_plugin_installed(temp_home: &std::path::Path) {
        let plugins_dir = temp_home.join("opencode-config/plugins");
        std::fs::create_dir_all(&plugins_dir).unwrap();
        std::fs::write(
            plugins_dir.join("worktrunk.ts"),
            include_str!("../../dev/opencode-plugin.ts"),
        )
        .unwrap();
    }

    /// Setup mock `gemini` CLI as installed
    ///
    /// Call this to simulate Gemini CLI being available on the system.
    pub fn setup_mock_gemini_installed(&mut self) {
        self.gemini_installed = true;
    }

    /// Make `claude`, `codex`, `opencode`, and `gemini` resolvable on `PATH`.
    ///
    /// The `setup_mock_*_installed` helpers force detection through the
    /// `WORKTRUNK_TEST_*_INSTALLED` env overrides, so the `which::which`
    /// lookup inside each `is_*_available()` never runs under test. This
    /// helper instead drops those overrides and prepends real mock
    /// executables, exercising the production PATH-detection path for all
    /// four AI CLIs at once. Call `setup_mock_ci_tools_unauthenticated()`
    /// first to create the mock bin directory.
    pub fn setup_mock_clis_on_path(&mut self) {
        let mock_bin = self
            .mock_bin_path
            .as_ref()
            .expect("call setup_mock_ci_tools_unauthenticated() first");
        // `wt config show` only `which`-detects these CLIs (never runs
        // them), so the mocks need no command behavior.
        for cli in ["claude", "codex", "opencode", "gemini"] {
            MockConfig::new(cli).write(mock_bin);
        }
        self.detect_clis_via_path = true;
    }

    /// Setup the worktrunk extension as installed in Gemini CLI
    ///
    /// `gemini extensions install` clones the extension into
    /// `~/.gemini/extensions/<name>/`; this writes the resulting
    /// `gemini-extension.json` under the temp home directory (which must
    /// already be set up via `set_temp_home_env` on the command).
    pub fn setup_gemini_extension_installed(temp_home: &std::path::Path) {
        let extension_dir = temp_home.join(".gemini/extensions/worktrunk");
        std::fs::create_dir_all(&extension_dir).unwrap();
        std::fs::write(
            extension_dir.join("gemini-extension.json"),
            include_str!("../../gemini-extension.json"),
        )
        .unwrap();
    }

    /// Setup mock `claude` CLI with plugin subcommand support
    ///
    /// Creates a mock claude binary that handles `plugin marketplace`,
    /// `plugin install`, and `plugin uninstall` commands. Must call
    /// `setup_mock_ci_tools_unauthenticated()` first to create the mock bin directory.
    pub fn setup_mock_claude_with_plugins(&mut self) {
        let mock_bin = self
            .mock_bin_path
            .as_ref()
            .expect("call setup_mock_ci_tools_unauthenticated() first");

        MockConfig::new("claude")
            .command("plugin marketplace", MockResponse::exit(0))
            .command("plugin install", MockResponse::exit(0))
            .command("plugin uninstall", MockResponse::exit(0))
            .write(mock_bin);

        self.claude_installed = true;
    }

    /// Setup mock `codex` CLI with plugin marketplace support
    ///
    /// Creates a mock codex binary that handles `plugin marketplace add`,
    /// and `plugin marketplace remove` commands. Must call
    /// `setup_mock_ci_tools_unauthenticated()` first to create the mock bin
    /// directory.
    pub fn setup_mock_codex_with_plugins(&mut self) {
        let mock_bin = self
            .mock_bin_path
            .as_ref()
            .expect("call setup_mock_ci_tools_unauthenticated() first");

        MockConfig::new("codex")
            .command("plugin marketplace add", MockResponse::exit(0))
            .command("plugin marketplace remove", MockResponse::exit(0))
            .write(mock_bin);

        self.codex_installed = true;
    }

    /// Setup mock `claude` CLI where plugin commands fail
    ///
    /// Creates a mock claude binary where `plugin marketplace`, `plugin install`,
    /// and `plugin uninstall` all exit with code 1 and print an error.
    /// Must call `setup_mock_ci_tools_unauthenticated()` first.
    pub fn setup_mock_claude_with_plugins_failing(&mut self) {
        let mock_bin = self
            .mock_bin_path
            .as_ref()
            .expect("call setup_mock_ci_tools_unauthenticated() first");

        MockConfig::new("claude")
            .command(
                "plugin marketplace",
                MockResponse::exit(1).with_stderr("error: network timeout\n"),
            )
            .command(
                "plugin install",
                MockResponse::exit(1).with_stderr("error: install failed\n"),
            )
            .command(
                "plugin uninstall",
                MockResponse::exit(1).with_stderr("error: uninstall failed\n"),
            )
            .write(mock_bin);

        self.claude_installed = true;
    }

    /// Setup mock `codex` CLI where marketplace commands fail
    ///
    /// Must call `setup_mock_ci_tools_unauthenticated()` first.
    pub fn setup_mock_codex_with_plugins_failing(&mut self) {
        let mock_bin = self
            .mock_bin_path
            .as_ref()
            .expect("call setup_mock_ci_tools_unauthenticated() first");

        MockConfig::new("codex")
            .command(
                "plugin marketplace add",
                MockResponse::exit(1).with_stderr("error: marketplace add failed\n"),
            )
            .command(
                "plugin marketplace remove",
                MockResponse::exit(1).with_stderr("error: marketplace remove failed\n"),
            )
            .write(mock_bin);

        self.codex_installed = true;
    }

    /// Setup mock `gh` that returns configurable PR/CI data
    ///
    /// Use this for testing CI status parsing code. The mock returns JSON data
    /// for `gh pr list`.
    ///
    /// # Arguments
    /// * `pr_json` - JSON string to return for `gh pr list --json ...`
    pub fn setup_mock_gh_with_ci_data(&mut self, pr_json: &str) {
        let mock_bin = self.temp_dir.path().join("mock-bin");
        std::fs::create_dir_all(&mock_bin).unwrap();

        std::fs::write(mock_bin.join("pr_data.json"), pr_json).unwrap();

        MockConfig::new("gh")
            .version("gh version 2.0.0 (mock)")
            .command("auth", MockResponse::exit(0))
            .command("pr", MockResponse::file("pr_data.json"))
            .write(&mock_bin);

        // Configure glab mock (fails - no GitLab support)
        MockConfig::new("glab")
            .command("_default", MockResponse::exit(1))
            .write(&mock_bin);

        self.mock_bin_path = Some(mock_bin);
    }

    /// Setup mock `glab` that returns configurable MR/CI data for GitLab
    ///
    /// Use this for testing GitLab CI status parsing code. The mock handles the
    /// two-step MR resolution process:
    /// - `glab mr list` returns basic MR info (iid, sha, conflicts, etc.)
    /// - `glab mr view <iid>` returns full MR info including head_pipeline
    ///
    /// # Arguments
    /// * `mr_json` - JSON string for MR data. Should include an `iid` field and
    ///   optionally `head_pipeline`. This data is used for both `mr list` and
    ///   `mr view` responses.
    /// * `project_id` - Optional project ID to return from `glab repo view`
    ///
    /// # Note
    /// The mock automatically handles the compound command matching:
    /// - "mr list" → returns MR list data
    /// - "mr view" → returns same data (works because glab mr view returns same fields)
    pub fn setup_mock_glab_with_ci_data(&mut self, mr_json: &str, project_id: Option<u64>) {
        let mock_bin = self.temp_dir.path().join("mock-bin");
        std::fs::create_dir_all(&mock_bin).unwrap();

        // Parse the MR JSON to create separate list and view responses
        // mr list needs: iid (for two-step lookup), sha, has_conflicts, detailed_merge_status, source_project_id, web_url
        // mr view needs: sha, has_conflicts, detailed_merge_status, head_pipeline, pipeline, web_url
        //
        // Since we provide the same JSON for both, we need to ensure iid is present.
        // The actual glab mr list doesn't return head_pipeline, but our mock can return
        // it harmlessly - the code will ignore it and do a second lookup.

        // Write JSON data files - same data for list (array) and view (single object)
        std::fs::write(mock_bin.join("mr_list_data.json"), mr_json).unwrap();

        // For mr view, create separate files for each MR by iid
        // This allows triple-matching "mr view <iid>" to return the correct MR
        let mut mock_config = MockConfig::new("glab")
            .version("glab version 1.0.0 (mock)")
            .command("auth", MockResponse::exit(0))
            .command("mr list", MockResponse::file("mr_list_data.json"));

        // Parse MR array and create iid-specific view commands
        // Triple match: "mr view 1" matches before "mr view" (see `mock_stub`)
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(mr_json)
            && let Some(arr) = parsed.as_array()
        {
            for mr in arr {
                if let Some(iid) = mr.get("iid").and_then(|v| v.as_u64()) {
                    let filename = format!("mr_view_{}.json", iid);
                    let json = serde_json::to_string(mr).unwrap_or_default();
                    std::fs::write(mock_bin.join(&filename), json).unwrap();
                    mock_config = mock_config
                        .command(&format!("mr view {}", iid), MockResponse::file(&filename));
                }
            }
        }

        // Build project ID response
        let project_id_response = match project_id {
            Some(id) => format!(r#"{{"id": {}}}"#, id),
            None => r#"{"error": "not found"}"#.to_string(),
        };

        // Configure glab mock with compound command matching
        // "mr view <iid>" is matched before "mr view" (see `mock_stub` triple matching)
        mock_config
            .command("repo", MockResponse::output(&project_id_response))
            .command("ci", MockResponse::output("[]"))
            .write(&mock_bin);

        // Configure gh mock (fails - no GitHub support)
        MockConfig::new("gh")
            .command("_default", MockResponse::exit(1))
            .write(&mock_bin);

        self.mock_bin_path = Some(mock_bin);
    }

    /// Setup mock glab where mr list succeeds but mr view fails.
    ///
    /// Use this to test the error path when `glab mr view` fails after finding an MR.
    /// The mock returns the MR from mr list but exits with error for mr view.
    pub fn setup_mock_glab_with_failing_mr_view(&mut self, mr_json: &str, project_id: Option<u64>) {
        let mock_bin = self.temp_dir.path().join("mock-bin");
        std::fs::create_dir_all(&mock_bin).unwrap();

        std::fs::write(mock_bin.join("mr_list_data.json"), mr_json).unwrap();

        let project_id_response = match project_id {
            Some(id) => format!(r#"{{"id": {}}}"#, id),
            None => r#"{"error": "not found"}"#.to_string(),
        };

        // glab mock: mr list succeeds, but NO mr view commands registered
        // (falls back to exit code 1)
        MockConfig::new("glab")
            .version("glab version 1.0.0 (mock)")
            .command("auth", MockResponse::exit(0))
            .command("mr list", MockResponse::file("mr_list_data.json"))
            // No "mr view" commands - will fall back to default exit code 1
            .command("repo", MockResponse::output(&project_id_response))
            .command("ci", MockResponse::output("[]"))
            .write(&mock_bin);

        MockConfig::new("gh")
            .command("_default", MockResponse::exit(1))
            .write(&mock_bin);

        self.mock_bin_path = Some(mock_bin);
    }

    /// Set up mock glab with no MRs and a successful `ci list` pipeline.
    ///
    /// Used to test `detect_gitlab_pipeline`'s success path: a branch
    /// pipeline with no MR renders the bare `#` colored by pipeline status.
    pub fn setup_mock_glab_with_pipeline(&mut self, pipeline_json: &str, project_id: Option<u64>) {
        let mock_bin = self.temp_dir.path().join("mock-bin");
        std::fs::create_dir_all(&mock_bin).unwrap();

        let project_id_response = match project_id {
            Some(id) => format!(r#"{{"id": {}}}"#, id),
            None => r#"{"error": "not found"}"#.to_string(),
        };

        // glab mock: mr list returns empty (no MRs), ci list returns the pipeline
        MockConfig::new("glab")
            .version("glab version 1.0.0 (mock)")
            .command("auth", MockResponse::exit(0))
            .command("mr list", MockResponse::output("[]")) // No MRs - triggers ci list fallback
            .command("repo", MockResponse::output(&project_id_response))
            .command("ci", MockResponse::output(pipeline_json))
            .write(&mock_bin);

        MockConfig::new("gh")
            .command("_default", MockResponse::exit(1))
            .write(&mock_bin);

        self.mock_bin_path = Some(mock_bin);
    }

    /// Set up mock glab that returns a rate limit error on `ci list`.
    ///
    /// Used to test the `is_retriable_error` path in `detect_gitlab_pipeline`.
    /// MR list returns empty (no MRs), so the code falls through to pipeline detection
    /// which then hits the rate limit error.
    pub fn setup_mock_glab_with_ci_rate_limit(&mut self, project_id: Option<u64>) {
        let mock_bin = self.temp_dir.path().join("mock-bin");
        std::fs::create_dir_all(&mock_bin).unwrap();

        let project_id_response = match project_id {
            Some(id) => format!(r#"{{"id": {}}}"#, id),
            None => r#"{"error": "not found"}"#.to_string(),
        };

        // glab mock: mr list returns empty (no MRs), ci list fails with rate limit
        MockConfig::new("glab")
            .version("glab version 1.0.0 (mock)")
            .command("auth", MockResponse::exit(0))
            .command("mr list", MockResponse::output("[]")) // No MRs - triggers ci list fallback
            .command("repo", MockResponse::output(&project_id_response))
            .command(
                "ci",
                MockResponse::stderr("API rate limit exceeded").with_exit_code(1),
            )
            .write(&mock_bin);

        MockConfig::new("gh")
            .command("_default", MockResponse::exit(1))
            .write(&mock_bin);

        self.mock_bin_path = Some(mock_bin);
    }

    /// Setup mock `az` that returns configurable PR/pipeline data for Azure DevOps.
    ///
    /// Use this for testing Azure DevOps CI status parsing. The mock handles:
    /// - `az repos pr list` → returns `pr_list_json` (array of PR entries)
    /// - `az pipelines runs list` → returns `runs_json` (array of pipeline runs)
    ///
    /// `az repos pr list` is queried per branch (with `--source-branch`), so the
    /// same `pr_list_json` is returned for every branch — mirroring how the
    /// `glab mr list` mock behaves.
    ///
    /// # Arguments
    /// * `pr_list_json` - JSON for `az repos pr list --output json`. Each entry
    ///   should include `pullRequestId`, optionally `mergeStatus`,
    ///   `lastMergeSourceCommit.commitId`, and `repository.{name,project.name}`.
    /// * `runs_json` - JSON for `az pipelines runs list --output json`. Each entry
    ///   should include `id`, optionally `status`, `result`, and `sourceVersion`.
    pub fn setup_mock_az_with_ci_data(&mut self, pr_list_json: &str, runs_json: &str) {
        let mock_bin = self.temp_dir.path().join("mock-bin");
        std::fs::create_dir_all(&mock_bin).unwrap();

        std::fs::write(mock_bin.join("az_pr_list.json"), pr_list_json).unwrap();
        std::fs::write(mock_bin.join("az_runs.json"), runs_json).unwrap();

        MockConfig::new("az")
            .version("azure-cli 2.60.0 (mock)")
            .command("account", MockResponse::exit(0))
            .command("repos pr list", MockResponse::file("az_pr_list.json"))
            .command("pipelines runs list", MockResponse::file("az_runs.json"))
            .command("_default", MockResponse::exit(1))
            .write(&mock_bin);

        // gh/glab mocks fail — platform detection is URL-based, but keep these
        // present-but-useless so a real gh/glab on PATH can't interfere.
        MockConfig::new("gh")
            .command("_default", MockResponse::exit(1))
            .write(&mock_bin);
        MockConfig::new("glab")
            .command("_default", MockResponse::exit(1))
            .write(&mock_bin);

        self.mock_bin_path = Some(mock_bin);
    }

    /// Setup mock `az` where `az repos pr list` and/or `az pipelines runs list`
    /// fail with the given stderr (exit code 1).
    ///
    /// Used to exercise the `is_retriable_error` branches in `detect_azure_pr`
    /// and `detect_azure_pipeline`. A `None` argument makes that command return
    /// an empty JSON array instead of failing.
    pub fn setup_mock_az_with_detection_errors(
        &mut self,
        pr_list_stderr: Option<&str>,
        runs_stderr: Option<&str>,
    ) {
        let mock_bin = self.temp_dir.path().join("mock-bin");
        std::fs::create_dir_all(&mock_bin).unwrap();

        let pr_list_response = match pr_list_stderr {
            Some(stderr) => MockResponse::stderr(stderr).with_exit_code(1),
            None => MockResponse::output("[]"),
        };
        let runs_response = match runs_stderr {
            Some(stderr) => MockResponse::stderr(stderr).with_exit_code(1),
            None => MockResponse::output("[]"),
        };

        MockConfig::new("az")
            .version("azure-cli 2.60.0 (mock)")
            .command("account", MockResponse::exit(0))
            .command("repos pr list", pr_list_response)
            .command("pipelines runs list", runs_response)
            .command("_default", MockResponse::exit(1))
            .write(&mock_bin);

        MockConfig::new("gh")
            .command("_default", MockResponse::exit(1))
            .write(&mock_bin);
        MockConfig::new("glab")
            .command("_default", MockResponse::exit(1))
            .write(&mock_bin);

        self.mock_bin_path = Some(mock_bin);
    }

    /// Setup mock `tea` that returns configurable Gitea PR / commit-status
    /// responses.
    ///
    /// Use this for testing Gitea CI status parsing. The mock handles:
    /// - `tea api --include repos/{owner}/{repo}/pulls?state=open` → `pulls`
    /// - `tea api --include repos/{owner}/{repo}/commits/{head_sha}/status` →
    ///   `status`
    ///
    /// `owner`/`repo_name`/`head_sha` are needed because the mock playback matches the
    /// invocation's leading arguments verbatim, and `tea api --include <path>`
    /// passes the whole API path as a single argument — so the exact path
    /// string must be registered.
    ///
    /// # Arguments
    /// * `owner`, `repo_name` - the Gitea repo the test's remote points at.
    /// * `head_sha` - the SHA used for the `commits/{sha}/status` lookup
    ///   (the feature branch's HEAD; also the PR head SHA in `pulls`).
    /// * `pulls` - `(HTTP status, body)` for `tea api .../pulls`. On 200 the
    ///   body is a JSON array whose entries carry `mergeable`, `html_url`, and
    ///   `head.{ref,sha,repo.owner.login}`; on a 4xx/5xx it is whatever the
    ///   server sent instead, which the backend reads only for error text.
    /// * `status` - `(HTTP status, body)` for
    ///   `tea api .../commits/{sha}/status`, the 200 body carrying `state` and
    ///   `total_count`.
    ///
    /// The status is what the backend classifies on, so it is a parameter
    /// rather than inferred from the body — an `APIError` served with a 200 is
    /// a Gitea API change, and the tests say which they mean.
    pub fn setup_mock_tea_with_ci_data(
        &mut self,
        owner: &str,
        repo_name: &str,
        head_sha: &str,
        pulls: (&str, &str),
        status: (&str, &str),
    ) {
        let mock_bin = self.temp_dir.path().join("mock-bin");
        std::fs::create_dir_all(&mock_bin).unwrap();

        let (pulls_status, pulls_json) = pulls;
        let (status_status, status_json) = status;
        std::fs::write(mock_bin.join("tea_pulls.json"), pulls_json).unwrap();
        std::fs::write(mock_bin.join("tea_status.json"), status_json).unwrap();

        // Keep `&limit=20` in sync with `MAX_PRS_TO_FETCH` in
        // `src/commands/list/ci_status/mod.rs`.
        let pulls_path =
            format!("api --include repos/{owner}/{repo_name}/pulls?state=open&limit=20");
        let status_path =
            format!("api --include repos/{owner}/{repo_name}/commits/{head_sha}/status");

        MockConfig::new("tea")
            .version("tea version development (mock)")
            .command(
                &pulls_path,
                MockResponse::file("tea_pulls.json")
                    .with_stderr(&tea_api_include_stderr(pulls_status)),
            )
            .command(
                &status_path,
                MockResponse::file("tea_status.json")
                    .with_stderr(&tea_api_include_stderr(status_status)),
            )
            .command("_default", MockResponse::exit(1))
            .write(&mock_bin);

        // Other CI tools fail — platform detection is URL-based, but keep these
        // present-but-useless so a real tool on PATH can't interfere.
        for tool in ["gh", "glab", "az"] {
            MockConfig::new(tool)
                .command("_default", MockResponse::exit(1))
                .write(&mock_bin);
        }

        self.mock_bin_path = Some(mock_bin);
    }

    /// Setup mock `tea` where every `tea api` call fails with the given stderr
    /// (exit code 1).
    ///
    /// Used to exercise the `is_retriable_error` branch in `detect_gitea_pr`.
    pub fn setup_mock_tea_with_detection_error(&mut self, stderr: &str) {
        let mock_bin = self.temp_dir.path().join("mock-bin");
        std::fs::create_dir_all(&mock_bin).unwrap();

        MockConfig::new("tea")
            .version("tea version development (mock)")
            .command("api", MockResponse::stderr(stderr).with_exit_code(1))
            .command("_default", MockResponse::exit(1))
            .write(&mock_bin);

        for tool in ["gh", "glab", "az"] {
            MockConfig::new(tool)
                .command("_default", MockResponse::exit(1))
                .write(&mock_bin);
        }

        self.mock_bin_path = Some(mock_bin);
    }

    /// Setup mock `tea` for the Gitea commit-status fallback (no PR): the
    /// `pulls` list returns `[]`, and the `commits/{head_sha}/status` lookup
    /// fails with `stderr` (exit code 1). Assumes the repo's remote points at
    /// `owner/test-repo`.
    ///
    /// Used to exercise the `is_retriable_error` branch in
    /// `fetch_combined_status`.
    pub fn setup_mock_tea_commit_status_error(&mut self, head_sha: &str, stderr: &str) {
        let mock_bin = self.temp_dir.path().join("mock-bin");
        std::fs::create_dir_all(&mock_bin).unwrap();

        std::fs::write(mock_bin.join("tea_pulls.json"), "[]").unwrap();

        MockConfig::new("tea")
            .version("tea version development (mock)")
            .command(
                // Keep `&limit=20` in sync with `MAX_PRS_TO_FETCH` in
                // `src/commands/list/ci_status/mod.rs`.
                "api --include repos/owner/test-repo/pulls?state=open&limit=20",
                MockResponse::file("tea_pulls.json").with_stderr(&tea_api_include_stderr("200 OK")),
            )
            .command(
                // `tea` itself failing: no response, so no status line.
                &format!("api --include repos/owner/test-repo/commits/{head_sha}/status"),
                MockResponse::stderr(stderr).with_exit_code(1),
            )
            .command("_default", MockResponse::exit(1))
            .write(&mock_bin);

        for tool in ["gh", "glab", "az"] {
            MockConfig::new(tool)
                .command("_default", MockResponse::exit(1))
                .write(&mock_bin);
        }

        self.mock_bin_path = Some(mock_bin);
    }

    /// Configure a command to use mock gh/glab commands
    ///
    /// Must call `setup_mock_gh()` first. Prepends the mock bin directory to PATH
    /// so gh/glab commands are intercepted.
    ///
    /// On Windows, the mock commands have .exe files (see `mock_commands`) so they're
    /// found directly by CreateProcessW without needing PATHEXT manipulation.
    ///
    /// Metadata redactions keep PATH private in snapshots, so we can reuse the
    /// caller's PATH instead of a hardcoded minimal list.
    pub fn configure_mock_commands(&self, cmd: &mut Command) {
        if let Some(mock_bin) = &self.mock_bin_path {
            // Tell the mock playback where to find config files directly, avoiding PATH search
            cmd.env("WORKTRUNK_TEST_MOCK_CONFIG_DIR", mock_bin);

            // On Windows, env vars are case-insensitive but Rust stores them
            // case-sensitively. Find the actual PATH variable name to avoid
            // creating a duplicate with different case.
            let (path_var_name, current_path) = std::env::vars_os()
                .find(|(k, _)| k.eq_ignore_ascii_case("PATH"))
                .map(|(k, v)| (k.to_string_lossy().into_owned(), Some(v)))
                .unwrap_or(("PATH".to_string(), None));

            let mut paths: Vec<PathBuf> = current_path
                .as_deref()
                .map(|p| std::env::split_paths(p).collect())
                .unwrap_or_default();

            // Prepend mock bin to PATH so our mocks are found first
            paths.insert(0, mock_bin.clone());
            let new_path = std::env::join_paths(&paths).unwrap();
            cmd.env(&path_var_name, new_path);
        }

        // AI CLI detection. `setup_mock_clis_on_path()` drops the
        // `WORKTRUNK_TEST_*_INSTALLED` overrides so `is_*_available()`
        // exercises its real `which::which` PATH lookup against the mock
        // executables prepended above. Otherwise each override forces
        // detection on for the CLIs whose `setup_mock_*_installed()` ran.
        if self.detect_clis_via_path {
            for var in [
                "WORKTRUNK_TEST_CLAUDE_INSTALLED",
                "WORKTRUNK_TEST_CODEX_INSTALLED",
                "WORKTRUNK_TEST_OPENCODE_INSTALLED",
                "WORKTRUNK_TEST_GEMINI_INSTALLED",
            ] {
                cmd.env_remove(var);
            }
        } else {
            if self.claude_installed {
                cmd.env("WORKTRUNK_TEST_CLAUDE_INSTALLED", "1");
            }
            if self.codex_installed {
                cmd.env("WORKTRUNK_TEST_CODEX_INSTALLED", "1");
            }
            if self.opencode_installed {
                cmd.env("WORKTRUNK_TEST_OPENCODE_INSTALLED", "1");
            }
            if self.gemini_installed {
                cmd.env("WORKTRUNK_TEST_GEMINI_INSTALLED", "1");
            }
        }
    }

    /// Set a marker for a branch.
    ///
    /// Markers are stored as JSON with a timestamp in `worktrunk.state.<branch>.marker`.
    pub fn set_marker(&self, branch: &str, marker: &str) {
        let config_key = format!("worktrunk.state.{branch}.marker");
        let json_value = format!(r#"{{"marker":"{}","set_at":{}}}"#, marker, TEST_EPOCH);
        self.git_command()
            .args(["config", &config_key, &json_value])
            .run()
            .unwrap();
    }
}

impl TestRepoBase for TestRepo {}

/// Helper to create a bare repository test setup.
///
/// Bare repositories are useful for testing scenarios where you need worktrees
/// for the default branch (which isn't possible with normal repos since the
/// main worktree already has it checked out).
///
/// A thin composition over [`TestRepo::bare()`], so construction routes
/// through `TestRepo::assemble` and inherits everything it guarantees
/// (canonicalized root, `LOCAL_TEST_CONFIG`). What it adds is the bare
/// fixture style: worktrees created *inside* the repo directory (via the
/// `worktree-path` template), and `wt` commands that keep the terminal's
/// plain output (`configure_wt_cmd` strips `CLICOLOR_FORCE`, sets no temp
/// home, and leaves the working directory to the caller) — the style the
/// bare-repo snapshot suite is written against.
pub struct BareRepoTest {
    repo: TestRepo,
}

impl BareRepoTest {
    /// Create a new bare repository test setup.
    ///
    /// The bare repo is created at `temp_dir/repo` with worktrees configured
    /// to be created as subdirectories (e.g., `repo/main`, `repo/feature`).
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let repo = TestRepo::bare();
        // Template {{ branch }} creates worktrees as subdirectories: repo/main, repo/feature
        std::fs::write(
            repo.test_config_path(),
            "worktree-path = \"{{ branch }}\"\n",
        )
        .unwrap();
        Self { repo }
    }

    /// Get the path to the bare repository.
    pub fn bare_repo_path(&self) -> &Path {
        self.repo.path()
    }

    /// Get the path to the test config file.
    pub fn config_path(&self) -> &Path {
        self.repo.test_config_path()
    }

    /// Get the temp directory path.
    pub fn temp_path(&self) -> &Path {
        self.repo.home_path()
    }

    /// Create a worktree from the bare repository.
    ///
    /// Worktrees are created inside the bare repo directory: repo/main, repo/feature
    pub fn create_worktree(&self, branch: &str, worktree_name: &str) -> PathBuf {
        let worktree_path = self.repo.path().join(worktree_name);

        let output = self
            .git_command(self.repo.path())
            .args([
                "worktree",
                "add",
                "-b",
                branch,
                worktree_path.to_str().unwrap(),
            ])
            .run()
            .unwrap();

        if !output.status.success() {
            panic!(
                "Failed to create worktree:\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        canonicalize(&worktree_path).unwrap()
    }

    /// Configure a wt command with test environment.
    pub fn configure_wt_cmd(&self, cmd: &mut Command) {
        self.configure_git_cmd(cmd);
        cmd.env("WORKTRUNK_CONFIG_PATH", self.repo.test_config_path())
            .env(
                "WORKTRUNK_SYSTEM_CONFIG_PATH",
                "/etc/xdg/worktrunk/config.toml",
            )
            .env("WORKTRUNK_APPROVALS_PATH", self.repo.test_approvals_path())
            .env_remove("NO_COLOR")
            .env_remove("CLICOLOR_FORCE");
    }

    /// Create a pre-configured wt command.
    pub fn wt_command(&self) -> Command {
        let mut cmd = wt_command();
        self.configure_wt_cmd(&mut cmd);
        cmd
    }
}

impl TestRepoBase for BareRepoTest {}

/// Create a configured Command for snapshot testing
///
/// This extracts the common command setup while allowing the test file
/// to call the macro with the correct module path for snapshot naming.
///
/// # Arguments
/// * `repo` - The test repository
/// * `subcommand` - The subcommand to run (e.g., "switch", "remove")
/// * `args` - Arguments to pass after the subcommand
/// * `cwd` - Optional working directory (defaults to repo root)
/// * `global_flags` - Optional global flags to pass before the subcommand (e.g., &["--verbose"])
pub fn make_snapshot_cmd_with_global_flags(
    repo: &TestRepo,
    subcommand: &str,
    args: &[&str],
    cwd: Option<&Path>,
    global_flags: &[&str],
) -> Command {
    let mut cmd = Command::new(wt_bin());
    repo.configure_wt_cmd(&mut cmd);
    cmd.args(global_flags)
        .arg(subcommand)
        .args(args)
        .current_dir(cwd.unwrap_or(repo.root_path()));
    cmd
}

/// Create a configured Command for snapshot testing
///
/// This extracts the common command setup while allowing the test file
/// to call the macro with the correct module path for snapshot naming.
pub fn make_snapshot_cmd(
    repo: &TestRepo,
    subcommand: &str,
    args: &[&str],
    cwd: Option<&Path>,
) -> Command {
    make_snapshot_cmd_with_global_flags(repo, subcommand, args, cwd, &[])
}

/// Resolve the git common directory (shared across all worktrees)
///
/// This is where centralized logs and other shared data are stored.
/// For linked worktrees, this returns the primary worktree's `.git/` directory.
/// For the primary worktree, this returns the `.git/` directory.
///
/// # Arguments
/// * `worktree_path` - Path to any worktree root
///
/// # Returns
/// The common git directory path
pub fn resolve_git_common_dir(worktree_path: &Path) -> PathBuf {
    let repo = Repository::at(worktree_path).unwrap();
    repo.git_common_dir().to_path_buf()
}

/// Validates ANSI escape sequences for the specific nested reset pattern that causes color leaks
///
/// Checks for the pattern: color code wrapping content that contains its own color codes with resets.
/// This causes the outer color to leak when the inner reset is encountered.
///
/// Example of the leak pattern:
/// ```text
/// \x1b[36mOuter text (\x1b[32minner\x1b[0m more)\x1b[0m
///                             ^^^^ This reset kills the cyan!
///                                  "more)" appears without cyan
/// ```
///
/// # Example
/// ```ignore
/// // Good - no nesting, proper closure
/// let output = "\x1b[36mtext\x1b[0m (stats)";
/// assert!(validate_ansi_codes(output).is_empty());
///
/// // Bad - nested reset breaks outer style
/// let output = "\x1b[36mtext (\x1b[32mnested\x1b[0m more)\x1b[0m";
/// let warnings = validate_ansi_codes(output);
/// assert!(!warnings.is_empty());
/// ```
pub fn validate_ansi_codes(text: &str) -> Vec<String> {
    let mut warnings = Vec::new();

    // Look for the specific pattern: color + content + color + content + reset + non-whitespace + reset
    // This indicates an outer style wrapping content with inner styles
    // We look for actual text (not just whitespace) between resets
    let nested_pattern = regex::Regex::new(
        r"(\x1b\[[0-9;]+m)([^\x1b]+)(\x1b\[[0-9;]+m)([^\x1b]*?)(\x1b\[0m)(\s*[^\s\x1b]+)(\x1b\[0m)",
    )
    .unwrap();

    for cap in nested_pattern.captures_iter(text) {
        let content_after_reset = cap[6].trim();

        // Only warn if there's actual content after the inner reset
        // (not just punctuation or whitespace)
        if !content_after_reset.is_empty()
            && content_after_reset.chars().any(|c| c.is_alphanumeric())
        {
            warnings.push(format!(
                "Nested color reset detected: content '{}' appears after inner reset but before outer reset - it will lose the outer color",
                content_after_reset
            ));
        }
    }

    warnings
}

// ============================================================================
// Timing utilities for background command tests
// ============================================================================

/// Configuration for exponential backoff polling.
///
/// Default: 10ms → 20ms → 40ms → ... → 500ms max, 5s timeout.
#[derive(Debug, Clone)]
pub struct ExponentialBackoff {
    /// Initial sleep duration in milliseconds
    pub initial_ms: u64,
    /// Maximum sleep duration in milliseconds
    pub max_ms: u64,
    /// Total timeout
    #[cfg_attr(windows, allow(dead_code))] // Used only by unix PTY tests
    pub timeout: std::time::Duration,
}

impl Default for ExponentialBackoff {
    fn default() -> Self {
        Self {
            initial_ms: 10,
            max_ms: 500,
            timeout: std::time::Duration::from_secs(5),
        }
    }
}

impl ExponentialBackoff {
    /// Sleep for the appropriate duration based on attempt number.
    pub fn sleep(&self, attempt: u32) {
        let ms = (self.initial_ms * (1u64 << attempt.min(20))).min(self.max_ms);
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

/// Poll with exponential backoff: 10ms → 20ms → 40ms → ... → 500ms max.
/// Fast initial checks catch quick completions; backs off to reduce CPU on slow CI.
fn exponential_sleep(attempt: u32) {
    ExponentialBackoff::default().sleep(attempt);
}

/// Fixed window for an **absence** assertion, proving that something did
/// *not* happen (a hook that must not fire, a marker that must not appear).
///
/// The polarity of the assertion decides the tool. A **presence** assertion
/// waits for an event that *will* happen: poll with [`wait_for_file`] and
/// friends, which return the instant the event lands and tolerate a slow CI
/// runner via a generous timeout. An **absence** assertion has no event to
/// wait for, so polling can't help: the only option is to wait long enough
/// that the thing would have happened if it were going to, then assert it
/// didn't. 500ms is the floor; a starved background process needs a wide
/// margin for the window to be conclusive.
///
/// Use this constant rather than a bare `Duration::from_millis(500)` so
/// absence sleeps are greppable and self-documenting. Never pair it with a
/// presence assertion in the same test: a fixed sleep before a "did happen"
/// check is the flaky pattern this constant exists to keep out of the
/// assertion path. When the absence is *structural* (the event is gated on a
/// condition the test never sets up, so it can't fire at all), no window is
/// needed: poll the positive precondition instead and the absence holds by
/// construction.
pub const SLEEP_FOR_ABSENCE_CHECK: std::time::Duration = std::time::Duration::from_millis(500);

/// True when a worktree's contents have been removed — either the path
/// is gone, or it's an empty placeholder directory.
///
/// After the instant-removal path renames the worktree into trash, the
/// original path can linger as an empty placeholder until the background
/// shell's `sleep 1 && rmdir` runs (the placeholder keeps `$PWD` valid
/// for shells like Nushell that validate it). The `rmdir` silences
/// errors with `2>/dev/null` and only removes empty directories, so
/// under load — or when any stray file (e.g., `.DS_Store`) lands in the
/// placeholder — the path can remain indefinitely. Production doesn't
/// care (empty placeholder is harmless); tests that do a strict
/// `!path.exists()` check would flake.
fn worktree_contents_removed(path: &Path) -> bool {
    // Single `read_dir` avoids a TOCTOU race between `exists()` and the
    // background process removing the placeholder.
    match path.read_dir() {
        Ok(mut entries) => entries.next().is_none(), // empty placeholder
        Err(_) => true,                              // already gone (NotFound or other)
    }
}

/// Assert that a worktree's contents have been removed.
///
/// Mirrors `worktree_contents_removed`: the path must be either gone
/// or an empty placeholder.
pub fn assert_worktree_removed(path: &Path) {
    assert!(
        worktree_contents_removed(path),
        "Worktree contents should be removed (empty placeholder OK): {}",
        path.display()
    );
}

/// Poll until a worktree's contents have been removed.
///
/// Prefer this over `wait_for(..., || !path.exists())` when removal
/// goes through the instant-removal path (`wt merge`, `wt remove`),
/// which can leave an empty placeholder directory. See
/// `worktree_contents_removed`.
pub fn wait_for_worktree_removed(path: &Path) {
    wait_for(
        &format!("worktree contents removed: {}", path.display()),
        || worktree_contents_removed(path),
    );
}

/// Wait for a file to exist, polling with exponential backoff.
/// Use this instead of fixed sleeps for background commands to avoid flaky tests.
pub fn wait_for_file(path: &Path) {
    let start = std::time::Instant::now();
    let mut attempt = 0;
    while start.elapsed() < BG_TIMEOUT {
        if path.exists() {
            return;
        }
        exponential_sleep(attempt);
        attempt += 1;
    }
    panic!(
        "File was not created within {:?}: {}",
        BG_TIMEOUT,
        path.display()
    );
}

/// Wait for a directory tree to contain at least `expected_count` files with a given extension.
///
/// Walks recursively — used to count hook log files which live in nested
/// `{branch}/{source}/{hook-type}/{name}.log` subtrees under `.git/wt/logs/`.
pub fn wait_for_file_count(dir: &Path, extension: &str, expected_count: usize) {
    let start = std::time::Instant::now();
    let mut attempt = 0;
    while start.elapsed() < BG_TIMEOUT {
        if count_files_recursive(dir, extension) >= expected_count {
            return;
        }
        exponential_sleep(attempt);
        attempt += 1;
    }
    panic!(
        "Expected {} .{} files in {:?} within {:?}",
        expected_count, extension, dir, BG_TIMEOUT
    );
}

fn count_files_recursive(dir: &Path, extension: &str) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut count = 0;
    for entry in entries.filter_map(|e| e.ok()) {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            count += count_files_recursive(&path, extension);
        } else if path.extension().and_then(|s| s.to_str()) == Some(extension) {
            count += 1;
        }
    }
    count
}

/// Wait for a file to have non-empty content, polling with exponential backoff.
/// Use when a background process creates a file but may not have finished writing.
pub fn wait_for_file_content(path: &Path) {
    let start = std::time::Instant::now();
    let mut attempt = 0;
    while start.elapsed() < BG_TIMEOUT {
        if std::fs::metadata(path).is_ok_and(|m| m.len() > 0) {
            return;
        }
        exponential_sleep(attempt);
        attempt += 1;
    }
    panic!(
        "File remained empty within {:?}: {}",
        BG_TIMEOUT,
        path.display()
    );
}

/// Wait for a file to have at least `expected_lines` lines, polling with exponential backoff.
/// Use when a background process writes multiple lines sequentially.
pub fn wait_for_file_lines(path: &Path, expected_lines: usize) {
    let start = std::time::Instant::now();
    let mut attempt = 0;
    while start.elapsed() < BG_TIMEOUT {
        if let Ok(content) = std::fs::read_to_string(path) {
            let line_count = content.lines().count();
            if line_count >= expected_lines {
                return;
            }
        }
        exponential_sleep(attempt);
        attempt += 1;
    }
    let actual = std::fs::read_to_string(path)
        .map(|c| c.lines().count())
        .unwrap_or(0);
    panic!(
        "File did not reach {} lines within {:?} (got {}): {}",
        expected_lines,
        BG_TIMEOUT,
        actual,
        path.display()
    );
}

/// Wait for a file to contain valid JSON, polling with exponential backoff.
/// Use when a background process writes JSON that may be partially written.
pub fn wait_for_valid_json(path: &Path) -> serde_json::Value {
    let start = std::time::Instant::now();
    let mut attempt = 0;
    let mut last_error = String::new();
    while start.elapsed() < BG_TIMEOUT {
        if let Ok(content) = std::fs::read_to_string(path) {
            match serde_json::from_str(&content) {
                Ok(json) => return json,
                Err(e) => last_error = format!("{e} (content: {content})"),
            }
        }
        exponential_sleep(attempt);
        attempt += 1;
    }
    panic!(
        "File did not contain valid JSON within {:?}: {}\nLast error: {}",
        BG_TIMEOUT,
        path.display(),
        last_error
    );
}

/// Poll until a condition is met, with exponential backoff.
///
/// Use this instead of fixed sleeps for any condition that may take time to become true.
/// Fast initial checks (10ms) catch quick completions; backs off to reduce CPU on slow CI.
///
/// # Arguments
/// * `description` - Human-readable description for the panic message if timeout is reached
/// * `check` - Closure that returns `true` when the condition is met
///
/// # Example
/// ```ignore
/// // Wait for git to detect file changes (handles "racy git" timing issues)
/// wait_for("git to detect dirty working tree", || {
///     repo.git_command()
///         .args(["status", "--porcelain"])
///         .run()
///         .map(|o| !o.stdout.is_empty())
///         .unwrap_or(false)
/// });
/// ```
pub fn wait_for(description: &str, mut check: impl FnMut() -> bool) {
    let start = std::time::Instant::now();
    let mut attempt = 0;
    while start.elapsed() < BG_TIMEOUT {
        if check() {
            return;
        }
        exponential_sleep(attempt);
        attempt += 1;
    }
    panic!("Condition not met within {:?}: {}", BG_TIMEOUT, description);
}

/// Convert Unix timestamp to ISO 8601 format for consistent git date handling
///
/// Git interprets `@timestamp` format inconsistently across versions and platforms.
/// Using ISO 8601 format ensures deterministic commit SHAs across all environments.
fn unix_to_iso8601(timestamp: i64) -> String {
    // Calculate date components from Unix timestamp
    let days_since_epoch = timestamp / 86400;
    let seconds_in_day = timestamp % 86400;

    let hours = seconds_in_day / 3600;
    let minutes = (seconds_in_day % 3600) / 60;
    let seconds = seconds_in_day % 60;

    // Calculate year, month, day from days since Unix epoch (1970-01-01)
    // Simplified algorithm: account for leap years
    let mut year = 1970i64;
    let mut remaining_days = days_since_epoch;

    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }

    let days_in_months: [i64; 12] = if is_leap_year(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1;
    for &days in &days_in_months {
        if remaining_days < days {
            break;
        }
        remaining_days -= days;
        month += 1;
    }

    let day = remaining_days + 1; // Days are 1-indexed

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hours, minutes, seconds
    )
}

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A harness-built `git` reaches local paths and nothing else, so no test
    /// can make an unbounded connect to whatever host a fixture URL names
    /// (see [`GIT_ALLOWED_PROTOCOLS`]).
    #[test]
    fn test_harness_git_allows_local_paths_only() {
        let repo = TestRepo::with_initial_commit();
        let ls_remote = |url: &str| {
            repo.git_command()
                .args(["ls-remote", "--symref", url, "HEAD"])
                .run()
                .unwrap()
        };

        let local = ls_remote(&path_slash::PathExt::to_slash_lossy(repo.root_path()));
        let local_stderr = String::from_utf8_lossy(&local.stderr);
        assert!(
            local.status.success(),
            "local-path remote must still resolve: {local_stderr}"
        );

        let remote = ls_remote("https://github.com/test-owner/test-repo.git");
        let stderr = String::from_utf8_lossy(&remote.stderr);
        assert!(
            stderr.contains("transport 'https' not allowed"),
            "https transport must be refused, got: {stderr}"
        );

        // The opt-out has to clear the var, not narrow it: a value that still
        // named a protocol list would silently keep the fixture clone offline,
        // and the daily benchmark run is the only thing that would notice.
        let mut opted_out = Command::new("git");
        configure_git_cmd(&mut opted_out);
        allow_network_transports(&mut opted_out);
        assert!(
            opted_out
                .get_envs()
                .any(|(k, v)| k == "GIT_ALLOW_PROTOCOL" && v.is_none())
        );
    }

    /// The env deny above rides on `Cmd`s the harness builds. A unit test that
    /// drives the library directly — `Repository::run_command` and everything
    /// layered on it — spawns git with the *test process's* environment, which
    /// carries no `GIT_ALLOW_PROTOCOL`, so that deny has to be in the repo's
    /// own config to reach it (see [`LOCAL_TEST_CONFIG`]).
    ///
    /// That environment includes the host locale, so the assertions can't
    /// match git's message text (`transport 'https' not allowed` translates —
    /// e.g. `Übertragungsart 'https' nicht erlaubt`). What is locale-stable is
    /// the URL: a refusal never names the host, while a real connect attempt
    /// interpolates it verbatim (`konnte nicht auf 'https://127.0.0.1:1/…'
    /// zugreifen: Failed to connect…`) — port 1 keeps that pre-fix path
    /// offline and instant.
    #[test]
    fn test_in_process_git_allows_local_paths_only() {
        let repo = TestRepo::with_initial_commit();
        let ls_remote = |url: &str| {
            repo.repo
                .current_worktree()
                .run_command_output(&["ls-remote", "--symref", url, "HEAD"])
                .unwrap()
        };

        // `protocol.allow = never` would take local remotes down with the
        // network ones without the `file` exception beside it.
        let local = ls_remote(&path_slash::PathExt::to_slash_lossy(repo.root_path()));
        let local_stderr = String::from_utf8_lossy(&local.stderr);
        assert!(
            local.status.success(),
            "local-path remote must still resolve: {local_stderr}"
        );

        let remote = ls_remote("https://127.0.0.1:1/test-owner/test-repo.git");
        let stderr = String::from_utf8_lossy(&remote.stderr);
        assert!(
            !remote.status.success(),
            "https transport must be refused in-process"
        );
        assert!(
            !stderr.contains("127.0.0.1"),
            "git walked to the wire instead of refusing the transport: {stderr}"
        );
    }

    /// Both outcomes of the template claim: an uncontested build renames the
    /// scratch into place, and a builder that loses the race — the final dir
    /// appears while it builds, the exact timing two cold-cache builders
    /// produce — keeps the winner's template and removes its own scratch.
    #[test]
    fn test_claim_template_winner_and_loser() {
        let parent = TempDir::new().unwrap();
        let dir = parent.path().join("template-v1");

        claim_template(parent.path(), &dir, |scratch| {
            std::fs::write(scratch.join("marker"), "winner").unwrap();
        });
        assert_eq!(
            std::fs::read_to_string(dir.join("marker")).unwrap(),
            "winner"
        );

        // Losing builder: `dir` exists by the time it renames.
        let dir2 = parent.path().join("template-v2");
        claim_template(parent.path(), &dir2, |scratch| {
            std::fs::create_dir(&dir2).unwrap();
            std::fs::write(dir2.join("marker"), "winner").unwrap();
            std::fs::write(scratch.join("marker"), "loser").unwrap();
        });
        assert_eq!(
            std::fs::read_to_string(dir2.join("marker")).unwrap(),
            "winner"
        );

        // Only the two templates remain — both scratch dirs are gone.
        let entries: Vec<_> = std::fs::read_dir(parent.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        let mut sorted = entries.clone();
        sorted.sort();
        assert_eq!(
            sorted,
            ["template-v1", "template-v2"],
            "scratch dirs must not linger"
        );
    }

    /// The template must reproduce the exact commits the committed snapshots
    /// were recorded against: ~170 snapshots embed these SHAs in short form
    /// (via `wt list` output and mock CI payloads). If this fails after
    /// editing [`build_standard_fixture`], the fixture's history changed:
    /// bump [`STANDARD_FIXTURE_VERSION`], update these SHAs, and regenerate
    /// the affected snapshots.
    #[test]
    fn test_standard_fixture_template_reproduces_pinned_shas() {
        let repo = standard_fixture_template().join("repo");
        let rev_parse = |repo: &Path, rev: &str| {
            let output = configure_git_env(Cmd::new("git"))
                .args(["rev-parse", rev])
                .current_dir(repo)
                .run()
                .unwrap();
            String::from_utf8(output.stdout).unwrap().trim().to_string()
        };
        assert_eq!(
            rev_parse(&repo, "main"),
            "05a4a45d0b981dad5c27db59dca482836d59f89e"
        );
        assert_eq!(
            rev_parse(&repo, "feature-a"),
            "1b87d4731ea707905d15a726e193531c20affa14"
        );
        assert_eq!(
            rev_parse(&repo, "feature-b"),
            "f62940fcec424585adf98625e722fdf990810614"
        );
        assert_eq!(
            rev_parse(&repo, "feature-c"),
            "345c7c93ad7c3d8f5b08380898d78e024019599c"
        );

        let main_only = standard_main_only_fixture_template().join("repo");
        assert_eq!(
            rev_parse(&main_only, "main"),
            "05a4a45d0b981dad5c27db59dca482836d59f89e"
        );
        let branches = configure_git_env(Cmd::new("git"))
            .args(["branch", "--format=%(refname:short)"])
            .current_dir(&main_only)
            .run()
            .unwrap();
        assert_eq!(
            String::from_utf8(branches.stdout).unwrap().trim(),
            "main",
            "main-only fixture must not retain the standard linked-worktree branches"
        );
    }

    #[test]
    fn test_unix_to_iso8601() {
        // 2025-01-01T00:00:00Z
        assert_eq!(unix_to_iso8601(1735689600), "2025-01-01T00:00:00Z");
        // 2025-01-02T00:00:00Z (TEST_EPOCH)
        assert_eq!(unix_to_iso8601(1735776000), "2025-01-02T00:00:00Z");
        // 2024-12-31T00:00:00Z (one day before 2025-01-01)
        assert_eq!(unix_to_iso8601(1735603200), "2024-12-31T00:00:00Z");
        // Unix epoch
        assert_eq!(unix_to_iso8601(0), "1970-01-01T00:00:00Z");
        // Leap year: 2024-02-29
        assert_eq!(unix_to_iso8601(1709164800), "2024-02-29T00:00:00Z");
    }

    #[test]
    fn test_validate_ansi_codes_no_leak() {
        // Good - no nesting
        let output = "\x1b[36mtext\x1b[0m (stats)";
        assert!(validate_ansi_codes(output).is_empty());

        // Good - nested but closes properly
        let output = "\x1b[36mtext\x1b[0m (\x1b[32mnested\x1b[0m)";
        assert!(validate_ansi_codes(output).is_empty());
    }

    #[test]
    fn test_validate_ansi_codes_detects_leak() {
        // Bad - nested reset breaks outer style
        let output = "\x1b[36mtext (\x1b[32mnested\x1b[0m more)\x1b[0m";
        let warnings = validate_ansi_codes(output);
        assert!(!warnings.is_empty());
        assert!(warnings[0].contains("more"));
    }

    #[test]
    fn test_validate_ansi_codes_ignores_punctuation() {
        // Punctuation after reset is acceptable (not a leak we care about)
        let output = "\x1b[36mtext (\x1b[32mnested\x1b[0m)\x1b[0m";
        let warnings = validate_ansi_codes(output);
        // Should not warn about ")" since it's just punctuation
        assert!(warnings.is_empty() || !warnings[0].contains("loses"));
    }

    /// A PTY child is the one transport that inherits nothing, so the floor
    /// is carried by hand — by `configure_pty_command` (the PTY choke point in
    /// `tests/common`, pinned by its own test there) and by this vector, which
    /// declares a PTY `wt` child's complete environment and so carries the
    /// floor itself rather than leaning on the transport. Losing the copy here
    /// would break that contract silently: the settings only quiet advice and
    /// refuse a guessed identity, so no PTY assertion would notice.
    #[test]
    fn pty_env_vars_carry_the_git_config_floor() {
        let dir = Path::new("/tmp");
        let vars = pty_env_vars(TestEnvPaths {
            home: dir,
            wt_config: dir,
            approvals: dir,
        });
        let keys: Vec<&str> = vars.iter().map(|(k, _)| k.as_str()).collect();

        // The vector must be complete on its own — every member present,
        // the numbered settings as much as the deny pair.
        for (var, _) in shell_exec::HERMETIC_TEST_GIT_ENV {
            assert!(keys.contains(&var), "{var} missing: {keys:?}");
        }
    }

    #[test]
    fn isolate_subprocess_env_scrubs_git_and_worktrunk_keys() {
        let mut cmd = Command::new("true");
        let synthetic_env = [
            "GIT_DIR".to_string(),
            "GIT_AUTHOR_DATE".to_string(),
            "GIT_CONFIG_GLOBAL".to_string(),
            "GIT_CONFIG_SYSTEM".to_string(),
            "GIT_CONFIG_COUNT".to_string(),
            "GIT_CONFIG_KEY_0".to_string(),
            "GIT_CONFIG_VALUE_0".to_string(),
            "WORKTRUNK_CONFIG_PATH".to_string(),
            "WORKTRUNK_HISTORY".to_string(),
            "PATH".to_string(),
            "HOME".to_string(),
            "GIT".to_string(),       // No underscore — should not match
            "WORKTRUNK".to_string(), // No underscore — should not match
        ];
        isolate_subprocess_env_from(&mut cmd, None, synthetic_env);

        let removed: HashMap<String, Option<String>> = cmd
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();

        // Scrubbed: GIT_*, WORKTRUNK_* (overwritten path vars also appear here),
        // NO_COLOR, FORCE_HYPERLINK, SHELL, PSModulePath.
        assert_eq!(removed.get("GIT_DIR"), Some(&None));
        assert_eq!(removed.get("GIT_AUTHOR_DATE"), Some(&None));
        assert_eq!(removed.get("WORKTRUNK_HISTORY"), Some(&None));
        assert_eq!(removed.get("NO_COLOR"), Some(&None));
        assert_eq!(removed.get("FORCE_HYPERLINK"), Some(&None));

        // Not scrubbed: vars that don't match either prefix.
        assert!(!removed.contains_key("PATH"));
        assert!(!removed.contains_key("HOME"));
        // The `GIT_CONFIG_*` family is scrubbed like the rest of `GIT_*`, then
        // re-set to the hermetic floor's values — a host-exported member can't
        // reach the child, and the child still denies `~/.gitconfig`. The
        // numbered members matter as much as the deny pair: drop them and the
        // child keeps the denial but loses every setting it was denying *for*.
        for (var, val) in shell_exec::HERMETIC_TEST_GIT_ENV {
            assert_eq!(
                removed.get(var),
                Some(&Some(val.to_string())),
                "{var} should be re-set to the floor value"
            );
        }
        // No underscore — prefix check requires `GIT_`/`WORKTRUNK_`.
        assert!(!removed.contains_key("GIT"));
        assert!(!removed.contains_key("WORKTRUNK"));

        // Path vars get set explicitly to known values.
        assert_eq!(
            removed.get("WORKTRUNK_CONFIG_PATH"),
            Some(&Some(DEFAULT_ISOLATED_USER_CONFIG.to_string()))
        );
        assert_eq!(
            removed.get("WORKTRUNK_SYSTEM_CONFIG_PATH"),
            Some(&Some(DEFAULT_ISOLATED_SYSTEM_CONFIG.to_string()))
        );
        assert_eq!(
            removed.get("WORKTRUNK_APPROVALS_PATH"),
            Some(&Some(DEFAULT_ISOLATED_APPROVALS.to_string()))
        );
    }

    #[test]
    fn configure_cli_command_scrubs_host_rust_log() {
        let mut cmd = Command::new("true");
        configure_cli_command(&mut cmd);

        let rust_log = cmd
            .get_envs()
            .find(|(key, _)| key.to_string_lossy() == "RUST_LOG")
            .map(|(_, value)| value);

        assert!(
            matches!(rust_log, Some(None)),
            "RUST_LOG should be explicitly removed from CLI test children"
        );
    }

    /// The isolation the whole suite rests on, asserted where it is weakest.
    ///
    /// `Repository::run_command` builds a plain `Cmd::new("git")` with no
    /// `GIT_CONFIG_*` of its own, so what its child resolves is whatever env
    /// the spawn site gives it — which the hermetic latch pins to the floor
    /// for every `Cmd` child. Resolving it through the production API fails loudly if the layer
    /// goes missing, instead of leaving the suite to read the developer's
    /// `~/.gitconfig` and pass or fail on its contents.
    #[test]
    fn in_process_git_reads_only_the_hermetic_config() {
        let repo = TestRepo::with_initial_commit();

        // Every setting resolves, and every one of them comes from the
        // environment rather than a file on the host.
        let floor = repo
            .repo
            .run_command(&["config", "--list", "--show-scope"])
            .unwrap();
        let from_env: Vec<&str> = floor
            .lines()
            .filter_map(|line| line.strip_prefix("command\t"))
            .collect();
        insta::assert_snapshot!(from_env.join("\n"), @r"
        user.useconfigonly=true
        rerere.enabled=false
        ");

        // Nothing outside the fixture contributes. The only scopes a resolved
        // setting may carry are `command` (the environment floor above) and
        // `local` (the fixture's own config); a host `~/.gitconfig` or a
        // system file reaching this git would surface as `global` or `system`.
        let outside: Vec<&str> = floor
            .lines()
            .filter(|line| !line.starts_with("command\t") && !line.starts_with("local\t"))
            .collect();
        assert!(
            outside.is_empty(),
            "config resolved from outside the fixture: {outside:#?}"
        );

        // Identity comes from the fixture's own local config, so a commit made
        // through the production API is authored the same way on every machine
        // — and `useConfigOnly` in the floor means a fixture that forgot one
        // errors rather than borrowing the host's username.
        std::fs::write(repo.path().join("second.txt"), "second").unwrap();
        repo.repo.run_command(&["add", "second.txt"]).unwrap();
        repo.repo.run_command(&["commit", "-m", "second"]).unwrap();
        let author = repo
            .repo
            .run_command(&["log", "-1", "--format=%an <%ae>"])
            .unwrap();
        insta::assert_snapshot!(author.trim(), @"Test User <test@example.com>");
    }

    #[test]
    fn default_llvm_profile_file_with_inherited_value_returns_it_verbatim() {
        let inherited = std::ffi::OsString::from("/cov/expected-%p.profraw");
        let resolved = default_llvm_profile_file_with(Some(inherited.clone()));
        assert_eq!(resolved, inherited);
    }

    #[test]
    fn default_llvm_profile_file_falls_back_to_temp_dir_when_uninherited() {
        let resolved = default_llvm_profile_file_with(None);
        let expected_dir = std::env::temp_dir().join("wt-test-profraw");
        // The returned path is `<temp>/wt-test-profraw/cov-%m_%p.profraw`. We
        // assert the parent dir lives under temp and the file name carries the
        // LLVM templating placeholders — the runtime expands those in the
        // instrumented child, so the literal `%m_%p` in this string is correct.
        let resolved_path = std::path::PathBuf::from(&resolved);
        assert_eq!(resolved_path.parent(), Some(expected_dir.as_path()));
        assert_eq!(
            resolved_path.file_name().and_then(|n| n.to_str()),
            Some("cov-%m_%p.profraw"),
        );
        // The fallback creates the dir if absent.
        assert!(expected_dir.is_dir());
    }
}
