// Many helper functions are conditionally used based on platform (#[cfg(not(windows))]).
// Allow dead_code at the module level to avoid warnings for platform-specific helpers.
#![allow(dead_code)]

//! # Test Utilities for worktrunk
//!
//! This module provides test harnesses for testing the worktrunk CLI tool.
//!
//! ## TestRepo
//!
//! The `TestRepo` struct creates isolated git repositories in temporary directories
//! with deterministic timestamps and configuration. Each test gets a fresh repo
//! that is automatically cleaned up when the test ends.
//!
//! ## Fixture-Based Initialization
//!
//! To improve test performance, `TestRepo::new()` copies from a pre-initialized
//! template stored in `tests/fixtures/template-repo/`. The template contains a
//! `_git` directory (renamed from `.git` so it can be committed) which gets
//! copied and renamed to `.git` for each test. This avoids spawning `git init`
//! for every test, saving ~10ms per test.
//!
//! ## Environment Isolation
//!
//! Git commands are run with isolated environments using `Command::env()` to ensure:
//! - No interference from global git config
//! - Deterministic commit timestamps
//! - Consistent locale settings
//! - No cross-test contamination
//! - Thread-safe execution (no global state mutation)
//!
//! ## Path Canonicalization
//!
//! Paths are canonicalized to handle platform differences (especially macOS symlinks
//! like /var -> /private/var). This ensures snapshot filters work correctly.
//!
//! On Windows, `std::fs::canonicalize()` returns verbatim paths (`\\?\C:\...`) which
//! git cannot handle. We use `normalize_path()` to strip these prefixes while
//! preserving the symlink resolution behavior needed on macOS.

pub mod list_snapshots;
// Progressive output tests use PTY and are Unix-only for now
#[cfg(unix)]
pub mod progressive_output;
// Shell integration tests are Unix-only for now (Windows support planned)
#[cfg(all(unix, feature = "shell-integration-tests"))]
pub mod shell;

/// Block SIGTTIN and SIGTTOU signals to prevent test processes from being
/// stopped when PTY operations interact with terminal control in background
/// process groups.
///
/// This is needed when running tests in environments like Codex where the test
/// process may be in the background process group of a controlling terminal.
/// PTY operations (via `portable_pty`) can trigger these signals, causing the
/// process to be stopped rather than continuing execution.
///
/// Signal masks are per-thread, so this must be called on each thread that
/// performs PTY operations. It's idempotent within a thread (safe to call
/// multiple times on the same thread).
///
/// **Preferred usage**: Use the `pty_safe` rstest fixture instead of calling directly:
/// ```ignore
/// use rstest::rstest;
/// use crate::common::pty_safe;
///
/// #[rstest]
/// fn test_something(_pty_safe: ()) {
///     // PTY operations here won't cause SIGTTIN/SIGTTOU stops
/// }
/// ```
#[cfg(unix)]
pub fn ignore_tty_signals() {
    use std::cell::Cell;
    thread_local! {
        static TTY_SIGNALS_BLOCKED: Cell<bool> = const { Cell::new(false) };
    }
    TTY_SIGNALS_BLOCKED.with(|blocked| {
        if blocked.get() {
            return;
        }
        use nix::sys::signal::{SigSet, SigmaskHow, Signal, pthread_sigmask};
        let mut mask = SigSet::empty();
        mask.add(Signal::SIGTTIN);
        mask.add(Signal::SIGTTOU);
        // Block these signals in the current thread's signal mask.
        // Fail fast if this doesn't work - silent failure would cause flaky tests.
        pthread_sigmask(SigmaskHow::SIG_BLOCK, Some(&mask), None)
            .expect("failed to block SIGTTIN/SIGTTOU signals");
        blocked.set(true);
    });
}

/// Rstest fixture that blocks SIGTTIN/SIGTTOU signals before each test.
///
/// Use this for any test that performs PTY operations to prevent the test
/// from being stopped when running in background process groups (e.g., Codex).
///
/// # Example
/// ```ignore
/// use rstest::rstest;
/// use crate::common::pty_safe;
///
/// #[rstest]
/// fn test_pty_interaction(_pty_safe: ()) {
///     // PTY operations here are safe from SIGTTIN/SIGTTOU stops
/// }
/// ```
#[cfg(unix)]
#[rstest::fixture]
pub fn pty_safe() {
    ignore_tty_signals();
}

/// Basic TestRepo fixture - creates a fresh git repository.
///
/// Use with `#[rstest]` to inject a new repo into tests:
/// ```ignore
/// use rstest::rstest;
/// use crate::common::repo;
///
/// #[rstest]
/// fn test_something(repo: TestRepo) {
///     // repo is a fresh TestRepo
/// }
///
/// #[rstest]
/// fn test_mutating(mut repo: TestRepo) {
///     repo.add_worktree("feature");
/// }
/// ```
#[rstest::fixture]
pub fn repo() -> TestRepo {
    TestRepo::new()
}

/// Temporary directory for use as fake home directory in tests.
///
/// Use this for tests that need to manipulate shell config files (~/.zshrc, ~/.bashrc, etc.)
/// or other home directory content. The directory is automatically cleaned up when dropped.
///
/// # Example
/// ```ignore
/// #[rstest]
/// fn test_shell_config(repo: TestRepo, temp_home: TempDir) {
///     let zshrc = temp_home.path().join(".zshrc");
///     fs::write(&zshrc, "# config").unwrap();
///     // test with temp_home as HOME
/// }
/// ```
#[rstest::fixture]
pub fn temp_home() -> TempDir {
    TempDir::new().unwrap()
}

/// Repo with remote tracking set up.
///
/// Builds on the `repo` fixture, adding a "remote" for the default branch.
/// Use `#[from(repo_with_remote)]` in rstest:
/// ```ignore
/// #[rstest]
/// fn test_push(#[from(repo_with_remote)] repo: TestRepo) {
///     // repo has remote tracking configured
/// }
/// ```
#[rstest::fixture]
pub fn repo_with_remote(mut repo: TestRepo) -> TestRepo {
    repo.setup_remote("main");
    repo
}

/// Repo with a separate worktree for the main branch.
///
/// Many merge/push tests need the main branch in a separate worktree so the
/// primary worktree can be on a feature branch. This fixture sets that up.
///
/// Use `#[from(repo_with_main_worktree)]` in rstest:
/// ```ignore
/// #[rstest]
/// fn test_merge(#[from(repo_with_main_worktree)] mut repo: TestRepo) {
///     let feature_wt = repo.add_worktree("feature");
///     // main is already in a separate worktree
/// }
/// ```
#[rstest::fixture]
pub fn repo_with_main_worktree(repo: TestRepo) -> TestRepo {
    repo.add_main_worktree();
    repo
}

/// Repo with main worktree and a feature branch with one commit.
///
/// Builds on `repo_with_main_worktree`, adding a "feature" worktree with a
/// single commit. Access the feature worktree path via `repo.worktrees["feature"]`.
///
/// Use directly or with `#[from(repo_with_feature_worktree)]` in rstest:
/// ```ignore
/// #[rstest]
/// fn test_merge(mut repo_with_feature_worktree: TestRepo) {
///     let repo = &mut repo_with_feature_worktree;
///     let feature_wt = &repo.worktrees["feature"];
///     // feature has one commit, ready to merge
/// }
/// ```
#[rstest::fixture]
pub fn repo_with_feature_worktree(mut repo_with_main_worktree: TestRepo) -> TestRepo {
    repo_with_main_worktree.add_worktree_with_commit(
        "feature",
        "feature.txt",
        "feature content",
        "Add feature file",
    );
    repo_with_main_worktree
}

/// Repo with remote, main worktree, and a feature branch with one commit.
///
/// Combines `repo_with_remote` with a main worktree and feature worktree setup.
/// Access the feature worktree path via `repo.worktrees["feature"]`.
///
/// Use for tests that need remote tracking AND a feature branch ready to merge/push.
/// ```ignore
/// #[rstest]
/// fn test_push(mut repo_with_remote_and_feature: TestRepo) {
///     let repo = &mut repo_with_remote_and_feature;
///     let feature_wt = &repo.worktrees["feature"];
///     // Has remote, main worktree, and feature with one commit
/// }
/// ```
#[rstest::fixture]
pub fn repo_with_remote_and_feature(mut repo_with_remote: TestRepo) -> TestRepo {
    repo_with_remote.add_main_worktree();
    repo_with_remote.add_worktree_with_commit(
        "feature",
        "feature.txt",
        "feature content",
        "Add feature file",
    );
    repo_with_remote
}

/// Repo with primary worktree on a non-default branch and main in separate worktree.
///
/// Switches the primary worktree to "develop" branch, then creates a worktree
/// for the default branch (main). This tests scenarios where the user's primary
/// checkout is not on the default branch.
///
/// Use for merge/switch tests that need to verify behavior when primary != default.
/// ```ignore
/// #[rstest]
/// fn test_merge_primary_not_default(mut repo_with_alternate_primary: TestRepo) {
///     let repo = &mut repo_with_alternate_primary;
///     // Primary is on "develop", main is in repo.main-wt
///     let feature_wt = repo.add_worktree_with_commit("feature", ...);
/// }
/// ```
#[rstest::fixture]
pub fn repo_with_alternate_primary(repo: TestRepo) -> TestRepo {
    repo.switch_primary_to("develop");
    repo.add_main_worktree();
    repo
}

/// Repo with main worktree and a feature branch with two commits.
///
/// Builds on `repo_with_main_worktree`, adding a "feature" worktree with two
/// commits (file1.txt and file2.txt). Useful for testing squash merges.
/// Access the feature worktree path via `repo.worktrees["feature"]`.
///
/// ```ignore
/// #[rstest]
/// fn test_squash(mut repo_with_multi_commit_feature: TestRepo) {
///     let repo = &mut repo_with_multi_commit_feature;
///     let feature_wt = &repo.worktrees["feature"];
///     // feature has 2 commits, ready to squash-merge
/// }
/// ```
#[rstest::fixture]
pub fn repo_with_multi_commit_feature(mut repo_with_main_worktree: TestRepo) -> TestRepo {
    let feature_wt = repo_with_main_worktree.add_worktree("feature");
    repo_with_main_worktree.commit_in_worktree(
        &feature_wt,
        "file1.txt",
        "content 1",
        "feat: add file 1",
    );
    repo_with_main_worktree.commit_in_worktree(
        &feature_wt,
        "file2.txt",
        "content 2",
        "feat: add file 2",
    );
    repo_with_main_worktree
}

/// Merge test setup with a single commit on feature branch.
///
/// Creates a repo with:
/// - A worktree for main at `repo.main-wt`
/// - A feature worktree with one commit adding `feature.txt`
///
/// Returns `(repo, feature_worktree_path)`.
///
/// # Example
/// ```ignore
/// #[rstest]
/// fn test_merge(merge_scenario: (TestRepo, PathBuf)) {
///     let (repo, feature_wt) = merge_scenario;
///     // feature_wt has one commit ready to merge
/// }
/// ```
#[rstest::fixture]
pub fn merge_scenario(mut repo: TestRepo) -> (TestRepo, PathBuf) {
    // Create a worktree for main
    let main_wt = repo.root_path().parent().unwrap().join("repo.main-wt");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["worktree", "add", main_wt.to_str().unwrap(), "main"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    // Create a feature worktree and make a commit
    let feature_wt = repo.add_worktree("feature");
    std::fs::write(feature_wt.join("feature.txt"), "feature content").unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "feature.txt"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add feature file"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    (repo, feature_wt)
}

/// Merge test setup with multiple commits on feature branch.
///
/// Creates a repo with:
/// - A worktree for main at `repo.main-wt`
/// - A feature worktree with two commits: `file1.txt` and `file2.txt`
///
/// Returns `(repo, feature_worktree_path)`.
///
/// # Example
/// ```ignore
/// #[rstest]
/// fn test_squash(merge_scenario_multi_commit: (TestRepo, PathBuf)) {
///     let (repo, feature_wt) = merge_scenario_multi_commit;
///     // feature_wt has two commits ready to squash-merge
/// }
/// ```
#[rstest::fixture]
pub fn merge_scenario_multi_commit(mut repo: TestRepo) -> (TestRepo, PathBuf) {
    // Create a worktree for main
    let main_wt = repo.root_path().parent().unwrap().join("repo.main-wt");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["worktree", "add", main_wt.to_str().unwrap(), "main"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    // Create a feature worktree and make multiple commits
    let feature_wt = repo.add_worktree("feature");

    std::fs::write(feature_wt.join("file1.txt"), "content 1").unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "file1.txt"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "feat: add file 1"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    std::fs::write(feature_wt.join("file2.txt"), "content 2").unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "file2.txt"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "feat: add file 2"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    (repo, feature_wt)
}

/// Returns a PTY system with a guard that restores the TTY foreground pgrp on drop.
///
/// Use this instead of `portable_pty::native_pty_system()` directly to ensure:
/// 1. PTY tests work in background process groups (signals blocked)
/// 2. SIGTTIN/SIGTTOU are blocked to prevent test processes from being stopped
///
/// NOTE: PTY tests are behind the `shell-integration-tests` feature because they can
/// trigger a nextest bug where its InputHandler cleanup receives SIGTTOU. This is NOT
/// caused by our code - investigation confirmed our tests don't change the terminal's
/// foreground process group. The issue is in nextest's teardown path (InputHandlerImpl::restore
/// calls tcsetattr while nextest isn't in the foreground pgrp). Workaround: run with
/// NEXTEST_NO_INPUT_HANDLER=1. See CLAUDE.md for details.
#[cfg(unix)]
pub fn native_pty_system() -> Box<dyn portable_pty::PtySystem> {
    ignore_tty_signals();
    portable_pty::native_pty_system()
}

/// Open a PTY pair with default size (48 rows x 200 cols).
///
/// Most PTY tests use this standard size. Returns the master/slave pair.
#[cfg(unix)]
pub fn open_pty() -> portable_pty::PtyPair {
    open_pty_with_size(48, 200)
}

/// Open a PTY pair with specified size.
#[cfg(unix)]
pub fn open_pty_with_size(rows: u16, cols: u16) -> portable_pty::PtyPair {
    native_pty_system()
        .openpty(portable_pty::PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap()
}

use insta_cmd::get_cargo_bin;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;
use worktrunk::config::sanitize_branch_name;
use worktrunk::path::to_posix_path;

/// Path to the fixture template repo (relative to crate root).
/// Contains `_git/` (renamed .git) and `gitconfig`.
fn fixture_template_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/template-repo")
}

/// Copy the fixture template to create a new test repo.
///
/// The fixture contains a git repo with one initial commit (on `main` branch).
/// Copies `_git/` to `.git/`, `file.txt`, and `gitconfig` to `test-gitconfig`.
/// Uses `cp -r` which benchmarks faster than native Rust fs operations.
fn copy_fixture_template(dest: &Path) {
    let template = fixture_template_path();

    // Create repo subdirectory
    let repo_path = dest.join("repo");
    std::fs::create_dir(&repo_path).unwrap();

    // Copy _git to repo/.git (suppress stderr for socket file warnings)
    let output = Command::new("cp")
        .args(["-r", "--"])
        .arg(template.join("_git"))
        .arg(repo_path.join(".git"))
        .stderr(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Failed to copy template _git directory"
    );

    // Copy file.txt (part of the initial commit)
    std::fs::copy(template.join("file.txt"), repo_path.join("file.txt")).unwrap();

    // Copy .gitattributes (forces LF line endings on all platforms)
    std::fs::copy(
        template.join(".gitattributes"),
        repo_path.join(".gitattributes"),
    )
    .unwrap();

    // Copy gitconfig
    std::fs::copy(template.join("gitconfig"), dest.join("test-gitconfig")).unwrap();
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

/// The epoch used for deterministic timestamps in tests (2025-01-01T00:00:00Z).
/// Use this when creating test data with timestamps (cache entries, etc.).
pub const TEST_EPOCH: u64 = 1735776000;

/// Null device path, platform-appropriate.
/// Use this for GIT_CONFIG_SYSTEM to disable system config in tests.
#[cfg(windows)]
const NULL_DEVICE: &str = "NUL";
#[cfg(not(windows))]
const NULL_DEVICE: &str = "/dev/null";

/// Create a `wt` CLI command with standardized test environment settings.
///
/// The command has the following guarantees:
/// - All host `GIT_*` and `WORKTRUNK_*` variables are cleared
/// - Color output is forced (`CLICOLOR_FORCE=1`) so ANSI styling appears in snapshots
/// - Terminal width set to 150 columns (`COLUMNS=150`)
pub fn wt_command() -> Command {
    let mut cmd = Command::new(get_cargo_bin("wt"));
    configure_cli_command(&mut cmd);
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
        "fish" => {
            // Fish doesn't set _CLAP_COMPLETE_INDEX - it appends the current token
            // as the last argument, so the completion handler uses args.len() - 1
        }
        _ => {}
    }
}

/// Configure an existing command with the standardized worktrunk CLI environment.
///
/// This helper mirrors the environment preparation performed by `wt_command`
/// and is intended for cases where tests need to construct the command manually
/// (e.g., to execute shell pipelines).
pub fn configure_cli_command(cmd: &mut Command) {
    for (key, _) in std::env::vars() {
        if key.starts_with("GIT_") || key.starts_with("WORKTRUNK_") {
            cmd.env_remove(&key);
        }
    }
    // Set to non-existent path to prevent loading user's real config.
    // Tests that need config should use TestRepo::clean_cli_env() which overrides this.
    // Note: env_remove above may cause insta-cmd to capture empty values in snapshots,
    // but correctness (isolating from host WORKTRUNK_* vars) trumps snapshot aesthetics.
    cmd.env("WORKTRUNK_CONFIG_PATH", "/nonexistent/test/config.toml");
    cmd.env("CLICOLOR_FORCE", "1");
    cmd.env("SOURCE_DATE_EPOCH", TEST_EPOCH.to_string());
    cmd.env("COLUMNS", "150");

    // Pass through LLVM coverage profiling environment for subprocess coverage collection.
    // When running under cargo-llvm-cov, spawned binaries need LLVM_PROFILE_FILE to record
    // their coverage data. Without this, integration test coverage isn't captured.
    for key in [
        "LLVM_PROFILE_FILE",
        "CARGO_LLVM_COV",
        "CARGO_LLVM_COV_TARGET_DIR",
    ] {
        if let Ok(val) = std::env::var(key) {
            cmd.env(key, val);
        }
    }
}

/// Configure a PTY CommandBuilder with isolated environment for testing.
///
/// This is the PTY equivalent of `configure_cli_command()`. It:
/// 1. Clears all inherited environment variables
/// 2. Sets minimal required vars (HOME, PATH)
/// 3. Passes through LLVM coverage profiling vars so subprocess coverage works
///
/// Call this early in PTY test setup, then add any test-specific env vars after.
pub fn configure_pty_command(cmd: &mut portable_pty::CommandBuilder) {
    // Clear inherited environment for test isolation
    cmd.env_clear();

    // Minimal environment for shells/binaries to function
    cmd.env(
        "HOME",
        home::home_dir().unwrap().to_string_lossy().to_string(),
    );
    cmd.env(
        "PATH",
        std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string()),
    );

    // Pass through LLVM coverage profiling environment for subprocess coverage.
    // Without this, spawned binaries can't write coverage data.
    pass_coverage_env_to_pty_cmd(cmd);
}

/// Pass through LLVM coverage profiling environment to a portable_pty::CommandBuilder.
///
/// PTY tests use `cmd.env_clear()` for isolation, which removes LLVM_PROFILE_FILE.
/// Without this, spawned binaries can't write coverage data.
///
/// Use `configure_pty_command()` for the full setup, or call this directly if you
/// need custom env_clear handling (e.g., shell-specific env vars).
pub fn pass_coverage_env_to_pty_cmd(cmd: &mut portable_pty::CommandBuilder) {
    for key in [
        "LLVM_PROFILE_FILE",
        "CARGO_LLVM_COV",
        "CARGO_LLVM_COV_TARGET_DIR",
    ] {
        if let Ok(val) = std::env::var(key) {
            cmd.env(key, val);
        }
    }
}

/// Create a CommandBuilder for running a shell in PTY tests.
///
/// Handles all shell-specific setup:
/// - env_clear + HOME + PATH (with optional bin_dir prefix)
/// - Shell-specific env vars (ZDOTDIR for zsh)
/// - Shell-specific isolation flags (--norc, --no-rcs, --no-config)
/// - Coverage passthrough
///
/// Returns a CommandBuilder ready for `.arg("-c")` and `.arg(&script)`.
#[cfg(unix)]
pub fn shell_command(
    shell: &str,
    bin_dir: Option<&std::path::Path>,
) -> portable_pty::CommandBuilder {
    let mut cmd = portable_pty::CommandBuilder::new(shell);
    cmd.env_clear();

    cmd.env(
        "HOME",
        home::home_dir().unwrap().to_string_lossy().to_string(),
    );

    let path = match bin_dir {
        Some(dir) => format!(
            "{}:{}",
            dir.display(),
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string())
        ),
        None => std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string()),
    };
    cmd.env("PATH", path);

    // Shell-specific setup
    match shell {
        "zsh" => {
            cmd.env("ZDOTDIR", "/dev/null");
            cmd.arg("--no-rcs");
            cmd.arg("-o");
            cmd.arg("NO_GLOBAL_RCS");
            cmd.arg("-o");
            cmd.arg("NO_RCS");
        }
        "bash" => {
            cmd.arg("--norc");
            cmd.arg("--noprofile");
        }
        "fish" => {
            cmd.arg("--no-config");
        }
        _ => {}
    }

    pass_coverage_env_to_pty_cmd(&mut cmd);
    cmd
}

/// Set home environment variables for commands that rely on isolated temp homes.
///
/// Sets both Unix (`HOME`, `XDG_CONFIG_HOME`) and Windows (`USERPROFILE`) variables
/// so the `home` crate can find the temp home directory on all platforms.
pub fn set_temp_home_env(cmd: &mut Command, home: &Path) {
    cmd.env("HOME", home);
    cmd.env("XDG_CONFIG_HOME", home.join(".config"));
    // Windows: the `home` crate uses USERPROFILE for home_dir()
    cmd.env("USERPROFILE", home);
    // Windows: etcetera uses APPDATA for config_dir() (AppData\Roaming)
    // Map it to .config to match Unix XDG_CONFIG_HOME behavior
    cmd.env("APPDATA", home.join(".config"));
}

pub struct TestRepo {
    temp_dir: TempDir, // Must keep to ensure cleanup on drop
    root: PathBuf,
    pub worktrees: HashMap<String, PathBuf>,
    remote: Option<PathBuf>, // Path to bare remote repo if created
    /// Isolated config file for this test (prevents pollution of user's config)
    test_config_path: PathBuf,
    /// Git config file with test settings (advice disabled, etc.)
    git_config_path: PathBuf,
    /// Path to mock bin directory for gh/glab commands
    mock_bin_path: Option<PathBuf>,
}

impl TestRepo {
    /// Create a new test repository with isolated git environment.
    ///
    /// The repo is initialized on `main` branch with one initial commit.
    /// Uses a fixture template for fast initialization - copies a pre-initialized
    /// git repo from `tests/fixtures/template-repo/` instead of running `git init`.
    /// This saves ~10ms per test by avoiding process spawns.
    ///
    /// Also sets up mock gh/glab commands that appear authenticated to prevent
    /// CI status hints from appearing in test output.
    pub fn new() -> Self {
        let temp_dir = TempDir::new().unwrap();

        // Copy from fixture template (includes initial commit)
        copy_fixture_template(temp_dir.path());

        // Canonicalize to resolve symlinks (important on macOS where /var is symlink to /private/var)
        let root = canonicalize(&temp_dir.path().join("repo")).unwrap();

        // Create isolated config path for this test
        let test_config_path = temp_dir.path().join("test-config.toml");
        let git_config_path = temp_dir.path().join("test-gitconfig");

        let mut repo = Self {
            temp_dir,
            root,
            worktrees: HashMap::new(),
            remote: None,
            test_config_path,
            git_config_path,
            mock_bin_path: None,
        };

        // Mock gh/glab as authenticated to prevent CI hints in test output
        repo.setup_mock_gh();

        repo
    }

    /// Create an empty test repository (no commits, no branches).
    ///
    /// Use this for tests that specifically need to test behavior in an
    /// uninitialized repo. Most tests should use `new()` instead.
    pub fn empty() -> Self {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        let root = canonicalize(&root).unwrap();

        let test_config_path = temp_dir.path().join("test-config.toml");
        let git_config_path = temp_dir.path().join("test-gitconfig");

        // Write gitconfig
        std::fs::write(
            &git_config_path,
            "[user]\n\tname = Test User\n\temail = test@example.com\n\
             [advice]\n\tmergeConflict = false\n\tresolveConflict = false\n\
             [init]\n\tdefaultBranch = main\n",
        )
        .unwrap();

        let repo = Self {
            temp_dir,
            root,
            worktrees: HashMap::new(),
            remote: None,
            test_config_path,
            git_config_path,
            mock_bin_path: None,
        };

        // Run git init (can't avoid this for empty repos)
        let mut cmd = Command::new("git");
        repo.configure_git_cmd(&mut cmd);
        cmd.args(["init", "-q"])
            .current_dir(&repo.root)
            .output()
            .unwrap();

        repo
    }

    /// Configure a git command with isolated environment
    ///
    /// This sets environment variables only for the specific command,
    /// ensuring thread-safety and test isolation.
    pub fn configure_git_cmd(&self, cmd: &mut Command) {
        // Use test git config file with advice settings disabled
        cmd.env("GIT_CONFIG_GLOBAL", &self.git_config_path);
        cmd.env("GIT_CONFIG_SYSTEM", NULL_DEVICE);
        cmd.env("GIT_AUTHOR_DATE", "2025-01-01T00:00:00Z");
        cmd.env("GIT_COMMITTER_DATE", "2025-01-01T00:00:00Z");
        cmd.env("LC_ALL", "C");
        cmd.env("LANG", "C");
        cmd.env("SOURCE_DATE_EPOCH", TEST_EPOCH.to_string());
    }

    /// Get standard test environment variables as a vector
    ///
    /// This is useful for PTY tests and other cases where you need environment variables
    /// as a vector rather than setting them on a Command.
    #[cfg_attr(windows, allow(dead_code))] // Used only by unix PTY tests
    pub fn test_env_vars(&self) -> Vec<(String, String)> {
        vec![
            ("CLICOLOR_FORCE".to_string(), "1".to_string()),
            ("COLUMNS".to_string(), "150".to_string()),
            (
                "GIT_CONFIG_GLOBAL".to_string(),
                self.git_config_path.display().to_string(),
            ),
            ("GIT_CONFIG_SYSTEM".to_string(), NULL_DEVICE.to_string()),
            (
                "GIT_AUTHOR_DATE".to_string(),
                "2025-01-01T00:00:00Z".to_string(),
            ),
            (
                "GIT_COMMITTER_DATE".to_string(),
                "2025-01-01T00:00:00Z".to_string(),
            ),
            ("LC_ALL".to_string(), "C".to_string()),
            ("LANG".to_string(), "C".to_string()),
            ("SOURCE_DATE_EPOCH".to_string(), TEST_EPOCH.to_string()),
            (
                "WORKTRUNK_CONFIG_PATH".to_string(),
                self.test_config_path().display().to_string(),
            ),
        ]
    }

    /// Create a configured git command with args and current_dir set
    ///
    /// This is a convenience wrapper around configure_git_cmd that reduces boilerplate.
    /// Returns a Command ready to execute with .output(), .status(), etc.
    ///
    /// # Example
    /// ```ignore
    /// // Before:
    /// let mut cmd = Command::new("git");
    /// self.configure_git_cmd(&mut cmd);
    /// cmd.args(["add", "."]).current_dir(&self.root).output()?;
    ///
    /// // After:
    /// self.git_command(&["add", "."]).output()?;
    /// ```
    pub fn git_command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        cmd.args(args);
        cmd.current_dir(&self.root);
        cmd
    }

    /// Configure command for CLI tests with isolated environment.
    ///
    /// Sets `WORKTRUNK_CONFIG_PATH`, `HOME`, and mock gh/glab commands.
    pub fn clean_cli_env(&self, cmd: &mut Command) {
        configure_cli_command(cmd);
        self.configure_git_cmd(cmd);
        cmd.env("WORKTRUNK_CONFIG_PATH", &self.test_config_path);
        set_temp_home_env(cmd, self.home_path());
        self.configure_mock_commands(cmd);
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
    /// ├── test-config.toml   # WORKTRUNK_CONFIG_PATH target
    /// └── test-gitconfig     # GIT_CONFIG_GLOBAL target
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
        self.clean_cli_env(&mut cmd);
        cmd.current_dir(self.root_path());
        cmd
    }

    /// Get the root path of the repository
    pub fn root_path(&self) -> &Path {
        &self.root
    }

    /// Get the path to the isolated test config file
    ///
    /// This config path is automatically set via WORKTRUNK_CONFIG_PATH when using
    /// `clean_cli_env()`, ensuring tests don't pollute the user's real config.
    pub fn test_config_path(&self) -> &Path {
        &self.test_config_path
    }

    /// Write project-specific config (`.config/wt.toml`) under the repo root.
    pub fn write_project_config(&self, contents: &str) {
        let config_dir = self.root_path().join(".config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("wt.toml"), contents).unwrap();
    }

    /// Overwrite the isolated WORKTRUNK_CONFIG_PATH used during tests.
    pub fn write_test_config(&self, contents: &str) {
        std::fs::write(&self.test_config_path, contents).unwrap();
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

        self.git_command(&["add", "."]).output().unwrap();

        self.git_command(&["commit", "-m", message])
            .output()
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

        self.git_command(&["add", "."]).output().unwrap();

        self.git_command(&["commit", "-m", message])
            .output()
            .unwrap();
    }

    /// Create a commit with a specific age relative to SOURCE_DATE_EPOCH
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

        self.git_command(&["add", "."]).output().unwrap();

        // Create commit with custom timestamp
        let mut cmd = Command::new("git");
        cmd.env("GIT_CONFIG_GLOBAL", &self.git_config_path);
        cmd.env("GIT_CONFIG_SYSTEM", NULL_DEVICE);
        cmd.env("GIT_AUTHOR_DATE", &timestamp);
        cmd.env("GIT_COMMITTER_DATE", &timestamp);
        cmd.env("LC_ALL", "C");
        cmd.env("LANG", "C");
        cmd.args(["commit", "-m", message])
            .current_dir(&self.root)
            .output()
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

        let mut cmd = Command::new("git");
        cmd.env("GIT_CONFIG_GLOBAL", &self.git_config_path);
        cmd.env("GIT_CONFIG_SYSTEM", NULL_DEVICE);
        cmd.env("GIT_AUTHOR_DATE", &timestamp);
        cmd.env("GIT_COMMITTER_DATE", &timestamp);
        cmd.env("LC_ALL", "C");
        cmd.env("LANG", "C");
        cmd.args(["commit", "-m", message])
            .current_dir(dir)
            .output()
            .unwrap();
    }

    /// Add a worktree with the given name and branch
    ///
    /// The worktree path follows the default template format: `repo.{branch}`
    /// (sanitized, with slashes replaced by dashes).
    pub fn add_worktree(&mut self, branch: &str) -> PathBuf {
        let safe_branch = sanitize_branch_name(branch);
        // Use default template path format: ../{{ main_worktree }}.{{ branch }}
        // From {temp_dir}/repo, this resolves to {temp_dir}/repo.{branch}
        let worktree_path = self.temp_dir.path().join(format!("repo.{}", safe_branch));

        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        let output = cmd
            .args([
                "worktree",
                "add",
                "-b",
                branch,
                worktree_path.to_str().unwrap(),
            ])
            .current_dir(&self.root)
            .output()
            .unwrap();

        if !output.status.success() {
            panic!(
                "Failed to add worktree:\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        // Canonicalize worktree path to match what git returns
        let canonical_path = canonicalize(&worktree_path).unwrap();
        // Use branch as key (consistent with path generation)
        self.worktrees
            .insert(branch.to_string(), canonical_path.clone());
        canonical_path
    }

    /// Creates a worktree for the main branch (required for merge operations)
    ///
    /// This is a convenience method that creates a worktree for the main branch
    /// in the standard location expected by merge tests. Returns the path to the
    /// created worktree.
    pub fn add_main_worktree(&self) -> PathBuf {
        let main_wt = self.root_path().parent().unwrap().join("repo.main-wt");
        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        cmd.args(["worktree", "add", main_wt.to_str().unwrap(), "main"])
            .current_dir(self.root_path())
            .output()
            .unwrap();
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

        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        cmd.args(["add", filename])
            .current_dir(&worktree_path)
            .output()
            .unwrap();

        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        cmd.args(["commit", "-m", message])
            .current_dir(&worktree_path)
            .output()
            .unwrap();

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

        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        cmd.args(["add", filename])
            .current_dir(worktree_path)
            .output()
            .unwrap();

        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        cmd.args(["commit", "-m", message])
            .current_dir(worktree_path)
            .output()
            .unwrap();
    }

    /// Creates a branch without a worktree.
    ///
    /// This creates a local branch pointing to HEAD without checking it out.
    /// Useful for testing branch listing without creating worktrees.
    pub fn create_branch(&self, branch_name: &str) {
        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        cmd.args(["branch", branch_name])
            .current_dir(self.root_path())
            .output()
            .unwrap();
    }

    /// Pushes a branch to origin.
    ///
    /// Creates a remote tracking branch on origin. Requires `setup_remote()`
    /// to have been called first.
    pub fn push_branch(&self, branch_name: &str) {
        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        cmd.args(["push", "origin", branch_name])
            .current_dir(self.root_path())
            .output()
            .unwrap();
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
        // Get current commit SHA
        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        let output = cmd
            .args(["rev-parse", "HEAD"])
            .current_dir(path)
            .output()
            .unwrap();

        let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();

        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        cmd.args(["checkout", "--detach", &sha])
            .current_dir(path)
            .output()
            .unwrap();
    }

    /// Lock a worktree with an optional reason
    pub fn lock_worktree(&self, name: &str, reason: Option<&str>) {
        let worktree_path = self.worktree_path(name);

        let mut args = vec!["worktree", "lock"];
        if let Some(r) = reason {
            args.push("--reason");
            args.push(r);
        }
        args.push(worktree_path.to_str().unwrap());

        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        cmd.args(&args).current_dir(&self.root).output().unwrap();
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
    pub fn setup_custom_remote(&mut self, remote_name: &str, default_branch: &str) {
        // Create bare remote repository
        let remote_path = self.temp_dir.path().join(format!("{}.git", remote_name));
        std::fs::create_dir(&remote_path).unwrap();

        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        cmd.args(["init", "--bare", "--initial-branch", default_branch])
            .current_dir(&remote_path)
            .output()
            .unwrap();

        // Canonicalize remote path
        let remote_path = canonicalize(&remote_path).unwrap();

        // Add as remote
        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        cmd.args(["remote", "add", remote_name, remote_path.to_str().unwrap()])
            .current_dir(&self.root)
            .output()
            .unwrap();

        // Push current branch to remote
        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        cmd.args(["push", "-u", remote_name, default_branch])
            .current_dir(&self.root)
            .output()
            .unwrap();

        // Set remote/HEAD to point to the default branch
        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        cmd.args(["remote", "set-head", remote_name, default_branch])
            .current_dir(&self.root)
            .output()
            .unwrap_or_else(|_| panic!("Failed to set {}/HEAD", remote_name));

        self.remote = Some(remote_path);
    }

    /// Clear the local origin/HEAD reference
    ///
    /// This forces git to not have a cached default branch, useful for testing
    /// the fallback path that queries the remote.
    pub fn clear_origin_head(&self) {
        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        cmd.args(["remote", "set-head", "origin", "--delete"])
            .current_dir(&self.root)
            .output()
            .unwrap();
    }

    /// Check if origin/HEAD is set
    pub fn has_origin_head(&self) -> bool {
        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        let output = cmd
            .args(["rev-parse", "--abbrev-ref", "origin/HEAD"])
            .current_dir(&self.root)
            .output()
            .unwrap();
        output.status.success()
    }

    /// Switch the primary worktree to a different branch
    ///
    /// Creates a new branch and switches to it in the primary worktree.
    /// This is useful for testing scenarios where the primary worktree is not on the main branch.
    pub fn switch_primary_to(&self, branch: &str) {
        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        cmd.args(["switch", "-c", branch])
            .current_dir(&self.root)
            .output()
            .unwrap_or_else(|_| panic!("Failed to create {} branch", branch));
    }

    /// Get the current branch of the primary worktree
    ///
    /// Returns the name of the current branch, or panics if HEAD is detached.
    pub fn current_branch(&self) -> String {
        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        let output = cmd
            .args(["branch", "--show-current"])
            .current_dir(&self.root)
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// Setup mock `gh` and `glab` commands that return immediately without network calls
    ///
    /// Creates a mock bin directory with fake gh/glab scripts. After calling this,
    /// use `configure_mock_commands()` to add the mock bin to PATH for your commands.
    ///
    /// The mock gh returns:
    /// - `gh auth status`: exits successfully (0)
    /// - `gh pr list`: returns empty JSON array (no PRs found)
    /// - `gh run list`: returns empty JSON array (no runs found)
    ///
    /// This prevents CI detection from blocking tests with network calls.
    pub fn setup_mock_gh(&mut self) {
        // Delegate to setup_mock_gh_with_ci_data with empty arrays
        self.setup_mock_gh_with_ci_data("[]", "[]");
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

        // Create mock gh script - installed but not authenticated
        let gh_script = mock_bin.join("gh");
        std::fs::write(
            &gh_script,
            r#"#!/bin/sh
# Mock gh: installed but not authenticated

case "$1" in
    --version)
        echo "gh version 2.0.0 (mock)"
        exit 0
        ;;
    auth)
        # gh auth status - fail (not authenticated)
        exit 1
        ;;
    *)
        exit 1
        ;;
esac
"#,
        )
        .unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&gh_script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        // Create Windows batch file version of gh mock (unauthenticated)
        #[cfg(windows)]
        {
            let gh_cmd = mock_bin.join("gh.cmd");
            // Use goto-based structure for reliable exit codes on Windows.
            // Single-line `if ... exit /b N` can have inconsistent behavior
            // when scripts are invoked via `cmd /c`.
            std::fs::write(
                &gh_cmd,
                r#"@echo off
if "%1"=="--version" goto version
if "%1"=="auth" goto auth
goto fail

:version
echo gh version 2.0.0 (mock)
exit /b 0

:auth
exit /b 1

:fail
exit /b 1
"#,
            )
            .unwrap();
        }

        // Create mock glab script - installed but not authenticated
        let glab_script = mock_bin.join("glab");
        std::fs::write(
            &glab_script,
            r#"#!/bin/sh
# Mock glab: installed but not authenticated

case "$1" in
    --version)
        echo "glab version 1.0.0 (mock)"
        exit 0
        ;;
    auth)
        # glab auth status - fail (not authenticated)
        exit 1
        ;;
    *)
        exit 1
        ;;
esac
"#,
        )
        .unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&glab_script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        // Create Windows batch file version of glab mock (unauthenticated)
        #[cfg(windows)]
        {
            let glab_cmd = mock_bin.join("glab.cmd");
            // Use goto-based structure for reliable exit codes on Windows.
            std::fs::write(
                &glab_cmd,
                r#"@echo off
if "%1"=="--version" goto version
if "%1"=="auth" goto auth
goto fail

:version
echo glab version 1.0.0 (mock)
exit /b 0

:auth
exit /b 1

:fail
exit /b 1
"#,
            )
            .unwrap();
        }

        self.mock_bin_path = Some(mock_bin);
    }

    /// Setup mock `gh` that returns configurable PR/CI data
    ///
    /// Use this for testing CI status parsing code. The mock returns JSON data
    /// for `gh pr list` and `gh run list` commands.
    ///
    /// # Arguments
    /// * `pr_json` - JSON string to return for `gh pr list --json ...`
    /// * `run_json` - JSON string to return for `gh run list --json ...`
    pub fn setup_mock_gh_with_ci_data(&mut self, pr_json: &str, run_json: &str) {
        let mock_bin = self.temp_dir.path().join("mock-bin");
        std::fs::create_dir_all(&mock_bin).unwrap();

        // Write JSON files to be read by the script
        let pr_json_file = mock_bin.join("pr_data.json");
        let run_json_file = mock_bin.join("run_data.json");
        std::fs::write(&pr_json_file, pr_json).unwrap();
        std::fs::write(&run_json_file, run_json).unwrap();

        // Create mock gh script that returns JSON data
        let gh_script = mock_bin.join("gh");
        std::fs::write(
            &gh_script,
            format!(
                r#"#!/bin/sh
# Mock gh command that returns configured JSON data

case "$1" in
    --version)
        echo "gh version 2.0.0 (mock)"
        exit 0
        ;;
    auth)
        # gh auth status - succeed immediately
        exit 0
        ;;
    pr)
        # gh pr list - return PR data from file
        cat "{pr_json}"
        exit 0
        ;;
    run)
        # gh run list - return run data from file
        cat "{run_json}"
        exit 0
        ;;
    *)
        exit 1
        ;;
esac
"#,
                pr_json = pr_json_file.display(),
                run_json = run_json_file.display(),
            ),
        )
        .unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&gh_script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        // Create Windows batch file versions of gh mock (.bat and .cmd)
        // Both are needed because different resolution methods may prefer different extensions.
        // Use %~dp0 (directory containing the batch file) for reliable relative paths.
        #[cfg(windows)]
        {
            let batch_content = r#"@echo off
if "%1"=="--version" goto version
if "%1"=="auth" goto auth
if "%1"=="pr" goto pr
if "%1"=="run" goto run
goto fail

:version
echo gh version 2.0.0 (mock)
exit /b 0

:auth
exit /b 0

:pr
type "%~dp0pr_data.json"
exit /b 0

:run
type "%~dp0run_data.json"
exit /b 0

:fail
exit /b 1
"#;
            std::fs::write(mock_bin.join("gh.cmd"), batch_content).unwrap();
            std::fs::write(mock_bin.join("gh.bat"), batch_content).unwrap();
        }

        // Create mock glab script (fails immediately - no GitLab support in this mock)
        let glab_script = mock_bin.join("glab");
        std::fs::write(
            &glab_script,
            r#"#!/bin/sh
# Mock glab command that fails fast
exit 1
"#,
        )
        .unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&glab_script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        #[cfg(windows)]
        {
            let glab_content = "@echo off\nexit /b 1\n";
            std::fs::write(mock_bin.join("glab.cmd"), glab_content).unwrap();
            std::fs::write(mock_bin.join("glab.bat"), glab_content).unwrap();
        }

        self.mock_bin_path = Some(mock_bin);
    }

    /// Configure a command to use mock gh/glab commands
    ///
    /// Must call `setup_mock_gh()` first. Prepends the mock bin directory to PATH
    /// so gh/glab commands are intercepted.
    ///
    /// On Windows, this also removes directories containing real gh.exe/glab.exe
    /// from PATH and sets PATHEXT to prefer .BAT/.CMD before .EXE. This ensures
    /// our mock .bat/.cmd scripts are found instead of any real gh.exe.
    ///
    /// Metadata redactions keep PATH private in snapshots, so we can reuse the
    /// caller's PATH instead of a hardcoded minimal list.
    pub fn configure_mock_commands(&self, cmd: &mut Command) {
        if let Some(mock_bin) = &self.mock_bin_path {
            let mut paths: Vec<PathBuf> = std::env::var_os("PATH")
                .as_deref()
                .map(|p| std::env::split_paths(p).collect())
                .unwrap_or_default();

            // On Windows, Rust's Command::new looks for executables with .exe extension.
            // We need a gh.exe in mock_bin, but creating real executables is complex.
            // Instead, create a gh.bat (which Windows will execute for "gh" if .bat
            // comes before .exe in PATHEXT) and modify PATHEXT accordingly.
            // Also remove directories containing real gh.exe from PATH.
            #[cfg(windows)]
            {
                paths.retain(|dir| !dir.join("gh.exe").exists() && !dir.join("glab.exe").exists());
                // Put .BAT before .EXE so our gh.bat is found
                cmd.env(
                    "PATHEXT",
                    ".BAT;.CMD;.COM;.EXE;.VBS;.VBE;.JS;.JSE;.WSF;.WSH;.MSC",
                );
            }

            paths.insert(0, mock_bin.clone());
            let new_path = std::env::join_paths(&paths).unwrap();
            cmd.env("PATH", new_path);
        }
    }

    /// Set a marker for a branch with proper escaping for git config keys.
    ///
    /// This handles the branch name escaping required for git config keys,
    /// which only allow alphanumeric, `-`, and `.` characters.
    /// Markers are stored as JSON with a timestamp.
    pub fn set_marker(&self, branch: &str, marker: &str) {
        let escaped = worktrunk::git::escape_branch_for_config(branch);
        let config_key = format!("worktrunk.marker.{}", escaped);
        let json_value = format!(r#"{{"marker":"{}","set_at":{}}}"#, marker, TEST_EPOCH);
        self.git_command(&["config", &config_key, &json_value])
            .output()
            .unwrap();
    }
}

/// Add standard env var redactions to insta settings
///
/// These redact volatile metadata captured by insta-cmd in the `info` block.
/// Called by all snapshot settings helpers for consistency.
pub fn add_standard_env_redactions(settings: &mut insta::Settings) {
    settings.add_redaction(".env.GIT_CONFIG_GLOBAL", "[TEST_GIT_CONFIG]");
    settings.add_redaction(".env.WORKTRUNK_CONFIG_PATH", "[TEST_CONFIG]");
    settings.add_redaction(".env.HOME", "[TEST_HOME]");
    // Windows: the `home` crate uses USERPROFILE for home_dir()
    settings.add_redaction(".env.USERPROFILE", "[TEST_HOME]");
    settings.add_redaction(".env.XDG_CONFIG_HOME", "[TEST_CONFIG_HOME]");
    // Windows: etcetera uses APPDATA for config_dir()
    settings.add_redaction(".env.APPDATA", "[TEST_CONFIG_HOME]");
    settings.add_redaction(".env.PATH", "[PATH]");
}

/// Create configured insta Settings for snapshot tests
///
/// This extracts the common settings configuration while allowing the
/// `assert_cmd_snapshot!` macro to remain in test files for correct module path capture.
pub fn setup_snapshot_settings(repo: &TestRepo) -> insta::Settings {
    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    // Normalize project root path (for test fixtures)
    // This must come before repo path filter to avoid partial matches
    let project_root = std::env::var("CARGO_MANIFEST_DIR")
        .ok()
        .and_then(|p| canonicalize(std::path::Path::new(&p)).ok());
    if let Some(root) = project_root {
        settings.add_filter(&regex::escape(root.to_str().unwrap()), "[PROJECT_ROOT]");
    }

    // Normalize paths (canonicalize for macOS /var -> /private/var symlink)
    let root_canonical =
        canonicalize(repo.root_path()).unwrap_or_else(|_| repo.root_path().to_path_buf());
    let root_str = root_canonical.to_str().unwrap();
    // Add both backslash and forward-slash versions for Windows compatibility
    // (output may have either format depending on when filters are applied)
    settings.add_filter(&regex::escape(root_str), "[REPO]");
    settings.add_filter(&regex::escape(&root_str.replace('\\', "/")), "[REPO]");
    // Also add POSIX-style path for Git Bash (C:\foo\bar -> /c/foo/bar)
    settings.add_filter(&regex::escape(&to_posix_path(root_str)), "[REPO]");

    // In tests, HOME is set to the temp directory containing the repo. Commands being tested
    // see HOME=temp_dir, so format_path_for_display() outputs ~/repo instead of the full path.
    // The repo is always at {temp_dir}/repo, so we hardcode ~/repo for the filter.
    // IMPORTANT: Add worktree pattern FIRST so it matches before the plain ~/repo pattern.
    // Filters are applied in order, so ~/repo.feature is replaced before ~/repo would match it.
    settings.add_filter(r"~/repo(\.[a-zA-Z0-9_-]+)", "[REPO]$1");
    settings.add_filter(r"~/repo", "[REPO]");

    // Also handle the case where the real home contains the temp directory (Windows/macOS)
    // Note: canonicalize home_dir too, since on Windows home::home_dir() may return a short path
    // (C:\Users\RUNNER~1) while dunce::canonicalize returns the long path (C:\Users\runneradmin).
    if let Some(home) = home::home_dir().and_then(|h| canonicalize(&h).ok())
        && let Ok(relative) = root_canonical.strip_prefix(&home)
    {
        let tilde_path = format!("~/{}", relative.display()).replace('\\', "/");
        // Match exact repo path
        settings.add_filter(&regex::escape(&tilde_path), "[REPO]");
        // Match worktree paths
        let tilde_worktree_pattern = format!(r"{}(\.[a-zA-Z0-9_-]+)", regex::escape(&tilde_path));
        settings.add_filter(&tilde_worktree_pattern, "[REPO]$1");
    }

    for (name, path) in &repo.worktrees {
        let canonical = canonicalize(path).unwrap_or_else(|_| path.clone());
        let path_str = canonical.to_str().unwrap();
        let replacement = format!("[WORKTREE_{}]", name.to_uppercase().replace('-', "_"));
        settings.add_filter(&regex::escape(path_str), &replacement);
        settings.add_filter(&regex::escape(&path_str.replace('\\', "/")), &replacement);
        // Also add POSIX-style path for Git Bash (C:\foo\bar -> /c/foo/bar)
        settings.add_filter(&regex::escape(&to_posix_path(path_str)), &replacement);

        // Also add tilde-prefixed worktree path filter for Windows
        if let Some(home) = home::home_dir().and_then(|h| canonicalize(&h).ok())
            && let Ok(relative) = canonical.strip_prefix(&home)
        {
            let tilde_path = format!("~/{}", relative.display()).replace('\\', "/");
            settings.add_filter(&regex::escape(&tilde_path), &replacement);
        }
    }

    // Normalize backslashes for Windows compatibility
    settings.add_filter(r"\\", "/");

    // Windows fallback: use a regex pattern to catch tilde-prefixed Windows temp paths.
    // This handles cases where path formats differ between home::home_dir() and the actual
    // paths used in commands. MUST come after backslash normalization so paths have forward slashes.
    // Pattern: ~/AppData/Local/Temp/.tmpXXXXXX/repo (where XXXXXX varies)
    settings.add_filter(r"~/AppData/Local/Temp/\.tmp[^/]+/repo", "[REPO]");
    // Windows fallback for POSIX-style paths from Git Bash (used in hook template expansion).
    // Pattern: /c/Users/.../Temp/.tmpXXXXXX/repo and worktrees like /c/.../repo.feature-test
    settings.add_filter(
        r"/[a-z]/Users/[^/]+/AppData/Local/Temp/\.tmp[^/]+/repo(\.[a-zA-Z0-9_/-]+)?",
        "[REPO]$1",
    );

    // Normalize WORKTRUNK_CONFIG_PATH temp paths in stdout/stderr output
    // (metadata is handled via redactions below)
    // IMPORTANT: These specific filters must come BEFORE the generic [PROJECT_ID] filters
    settings.add_filter(r".*/\.tmp[^/]+/test-config\.toml", "[TEST_CONFIG]");

    // Normalize GIT_CONFIG_GLOBAL temp paths
    settings.add_filter(r".*/\.tmp[^/]+/test-gitconfig", "[TEST_GIT_CONFIG]");

    // Normalize temp directory paths in project identifiers (approval prompts)
    // Example: /private/var/folders/wf/.../T/.tmpABC123/origin -> [PROJECT_ID]
    // Note: [^)'\s\x1b]+ stops at ), ', whitespace, or ANSI escape to avoid matching too much
    settings.add_filter(
        r"/private/var/folders/[^/]+/[^/]+/T/\.[^/]+/[^)'\s\x1b]+",
        "[PROJECT_ID]",
    );
    // Linux: /tmp/.tmpXXXXXX/path -> [PROJECT_ID]
    settings.add_filter(r"/tmp/\.tmp[^/]+/[^)'\s\x1b]+", "[PROJECT_ID]");
    // Windows: C:/Users/user/AppData/Local/Temp/.tmpXXXXXX/path -> [PROJECT_ID]
    // Handles Windows temp paths with drive letters (after backslash normalization)
    settings.add_filter(
        r"[A-Z]:/Users/[^/]+/AppData/Local/Temp/\.tmp[^/]+/[^)'\s\x1b]+",
        "[PROJECT_ID]",
    );

    // Generic tilde-prefixed paths that aren't repo or worktree paths.
    // On CI, HOME is a temp directory, so paths under HOME become ~/something.
    // This catches paths like ~/wrong-path that don't follow the repo naming convention.
    // MUST come AFTER specific ~/repo patterns so they match first.
    settings.add_filter(r"~/[a-zA-Z0-9_-]+", "[PROJECT_ID]");

    // Normalize HOME temp directory in snapshots (stdout/stderr content)
    // Matches any temp directory path (without trailing filename)
    // Examples:
    //   macOS: HOME: /var/folders/.../T/.tmpXXX
    //   Linux: HOME: /tmp/.tmpXXX
    //   Windows: HOME: C:\Users\...\Temp\.tmpXXX (after backslash normalization)
    settings.add_filter(r"HOME: .*/\.tmp[^/\s]+", "HOME: [TEST_HOME]");

    add_standard_env_redactions(&mut settings);

    // Normalize timestamps in log filenames (format: YYYYMMDD-HHMMSS)
    // Match: post-start-NAME-SHA-HHMMSS.log
    settings.add_filter(
        r"post-start-[^-]+-[0-9a-f]{7,40}-\d{6}\.log",
        "post-start-[NAME]-[TIMESTAMP].log",
    );

    // Filter out Git hint messages that vary across Git versions
    // These hints appear during rebase conflicts and can differ between versions
    // Pattern matches lines with gutter formatting + "hint:" + message + newline
    // The gutter is: ESC[107m (bright white bg) ESC[0m followed by spaces
    settings.add_filter(r"(?m)^\x1b\[107m \x1b\[0m {1,2}hint:.*\n", "");

    // Normalize Git error message format differences across versions
    // Older Git (< 2.43): "Could not apply SHA... # commit message"
    // Newer Git (>= 2.43): "Could not apply SHA... commit message"
    // Add the "# " prefix to newer Git output for consistency with snapshots
    // Match if followed by a letter/character (not "#")
    settings.add_filter(
        r"(Could not apply [0-9a-f]{7,40}\.\.\.) ([A-Za-z])",
        "$1 # $2",
    );

    // Normalize OS-specific error messages in gutter output
    // Ubuntu may produce "Broken pipe (os error 32)" instead of the expected error
    // when capturing stderr from shell commands due to timing/buffering differences
    settings.add_filter(r"Broken pipe \(os error 32\)", "Error: connection refused");

    // Filter out PowerShell lines on Windows - these appear only on Windows
    // and would cause snapshot mismatches with Unix snapshots
    settings.add_filter(r"(?m)^.*[Pp]owershell.*\n", "");

    // Normalize Windows executable extension in help output
    // On Windows, clap shows "wt.exe" instead of "wt"
    settings.add_filter(r"wt\.exe", "wt");

    // Remove trailing ANSI reset codes at end of lines for cross-platform consistency
    // Windows terminal strips these trailing resets that Unix includes
    settings.add_filter(r"\x1b\[0m$", "");
    settings.add_filter(r"\x1b\[0m\n", "\n");

    settings
}

/// Create configured insta Settings for snapshot tests with a temporary home directory
///
/// This extends `setup_snapshot_settings` by adding a filter for the temporary home directory.
/// Use this for tests that need both a TestRepo and a temporary home (for user config testing).
pub fn setup_snapshot_settings_with_home(repo: &TestRepo, temp_home: &TempDir) -> insta::Settings {
    let mut settings = setup_snapshot_settings(repo);
    settings.add_filter(
        &regex::escape(&temp_home.path().to_string_lossy()),
        "[TEMP_HOME]",
    );
    settings
}

/// Create configured insta Settings for snapshot tests with only a temporary home directory
///
/// Use this for tests that don't need a TestRepo but do need a temporary home directory
/// (e.g., shell configuration tests, config init tests).
pub fn setup_home_snapshot_settings(temp_home: &TempDir) -> insta::Settings {
    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    settings.add_filter(
        &regex::escape(&temp_home.path().to_string_lossy()),
        "[TEMP_HOME]",
    );
    settings.add_filter(r"\\", "/");
    // Filter out PowerShell lines on Windows - these appear only on Windows
    // and would cause snapshot mismatches with Unix snapshots
    settings.add_filter(r"(?m)^.*[Pp]owershell.*\n", "");
    // Normalize Windows executable extension in help output
    settings.add_filter(r"wt\.exe", "wt");

    add_standard_env_redactions(&mut settings);

    settings
}

/// Create configured insta Settings for snapshot tests with a temp directory
///
/// Use this for tests that don't use TestRepo but need temp path redaction and
/// standard env var redactions (e.g., bare repository tests).
pub fn setup_temp_snapshot_settings(temp_path: &std::path::Path) -> insta::Settings {
    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    // Filter temp paths in output
    settings.add_filter(&regex::escape(temp_path.to_str().unwrap()), "[TEMP]");
    settings.add_filter(r"\\", "/");
    // Normalize Windows executable extension in help output
    settings.add_filter(r"wt\.exe", "wt");

    add_standard_env_redactions(&mut settings);

    settings
}

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
/// * `global_flags` - Optional global flags to pass before the subcommand (e.g., &["--internal"])
pub fn make_snapshot_cmd_with_global_flags(
    repo: &TestRepo,
    subcommand: &str,
    args: &[&str],
    cwd: Option<&Path>,
    global_flags: &[&str],
) -> Command {
    let mut cmd = Command::new(insta_cmd::get_cargo_bin("wt"));
    repo.clean_cli_env(&mut cmd);
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
    let repo = worktrunk::git::Repository::at(worktree_path);
    repo.git_common_dir().unwrap()
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
/// ```
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

/// Wait for a file to exist, polling with exponential backoff.
/// Use this instead of fixed sleeps for background commands to avoid flaky tests.
pub fn wait_for_file(path: &Path, timeout: std::time::Duration) {
    let start = std::time::Instant::now();
    let mut attempt = 0;
    while start.elapsed() < timeout {
        if path.exists() {
            return;
        }
        exponential_sleep(attempt);
        attempt += 1;
    }
    panic!(
        "File was not created within {:?}: {}",
        timeout,
        path.display()
    );
}

/// Wait for a directory to contain at least `expected_count` files with a given extension.
pub fn wait_for_file_count(
    dir: &Path,
    extension: &str,
    expected_count: usize,
    timeout: std::time::Duration,
) {
    let start = std::time::Instant::now();
    let mut attempt = 0;
    while start.elapsed() < timeout {
        if let Ok(entries) = std::fs::read_dir(dir) {
            let count = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some(extension))
                .count();
            if count >= expected_count {
                return;
            }
        }
        exponential_sleep(attempt);
        attempt += 1;
    }
    panic!(
        "Expected {} .{} files in {:?} within {:?}",
        expected_count, extension, dir, timeout
    );
}

/// Wait for a file to have non-empty content, polling with exponential backoff.
/// Use when a background process creates a file but may not have finished writing.
pub fn wait_for_file_content(path: &Path, timeout: std::time::Duration) {
    let start = std::time::Instant::now();
    let mut attempt = 0;
    while start.elapsed() < timeout {
        if std::fs::metadata(path).is_ok_and(|m| m.len() > 0) {
            return;
        }
        exponential_sleep(attempt);
        attempt += 1;
    }
    panic!(
        "File remained empty within {:?}: {}",
        timeout,
        path.display()
    );
}

/// Wait for a file to have at least `expected_lines` lines, polling with exponential backoff.
/// Use when a background process writes multiple lines sequentially.
pub fn wait_for_file_lines(path: &Path, expected_lines: usize, timeout: std::time::Duration) {
    let start = std::time::Instant::now();
    let mut attempt = 0;
    while start.elapsed() < timeout {
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
        timeout,
        actual,
        path.display()
    );
}

/// Wait for a file to contain valid JSON, polling with exponential backoff.
/// Use when a background process writes JSON that may be partially written.
pub fn wait_for_valid_json(path: &Path, timeout: std::time::Duration) -> serde_json::Value {
    let start = std::time::Instant::now();
    let mut attempt = 0;
    let mut last_error = String::new();
    while start.elapsed() < timeout {
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
        timeout,
        path.display(),
        last_error
    );
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
    use rstest::rstest;

    #[test]
    fn test_unix_to_iso8601() {
        // 2025-01-01T00:00:00Z
        assert_eq!(unix_to_iso8601(1735689600), "2025-01-01T00:00:00Z");
        // 2025-01-02T00:00:00Z (SOURCE_DATE_EPOCH)
        assert_eq!(unix_to_iso8601(1735776000), "2025-01-02T00:00:00Z");
        // 2024-12-31T00:00:00Z (one day before 2025-01-01)
        assert_eq!(unix_to_iso8601(1735603200), "2024-12-31T00:00:00Z");
        // Unix epoch
        assert_eq!(unix_to_iso8601(0), "1970-01-01T00:00:00Z");
        // Leap year: 2024-02-29
        assert_eq!(unix_to_iso8601(1709164800), "2024-02-29T00:00:00Z");
    }

    #[rstest]
    fn test_commit_with_age(repo: TestRepo) {
        // TestRepo::new() already includes one initial commit from fixture

        // Create commits with specific ages
        repo.commit_with_age("One hour ago", HOUR);
        repo.commit_with_age("One day ago", DAY);
        repo.commit_with_age("One week ago", WEEK);
        repo.commit_with_age("Ten minutes ago", 10 * MINUTE);

        // Verify commits were created (1 from fixture + 4 = 5 commits)
        let output = repo.git_command(&["log", "--oneline"]).output().unwrap();
        let log = String::from_utf8_lossy(&output.stdout);
        assert_eq!(log.lines().count(), 5);
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
}
