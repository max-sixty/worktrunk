// Many helper functions are conditionally used based on platform (#[cfg(not(windows))]).
// Allow dead_code at the module level to avoid warnings for platform-specific helpers.
#![allow(dead_code)]

// Re-export from worktrunk::testing so integration tests can keep using
// `crate::common::TestRepo`, `crate::common::wt_bin`, etc.
pub use worktrunk::testing::mock_commands;
pub use worktrunk::testing::*;

pub mod list_snapshots;
// Progressive output tests use PTY and are Unix-only for now
#[cfg(unix)]
pub mod progressive_output;
// PTY execution helpers - cross-platform (uses portable_pty with ConPTY on Windows)
#[cfg(feature = "shell-integration-tests")]
pub mod pty;
// Shell integration tests - cross-platform with PTY support
#[cfg(feature = "shell-integration-tests")]
pub mod shell;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use worktrunk::path::to_posix_path;

// =============================================================================
// Signal handling (for PTY tests)
// =============================================================================

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

// =============================================================================
// rstest fixtures
// =============================================================================

/// Basic TestRepo fixture - creates a fresh git repository from the standard fixture.
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
    let repo = TestRepo::standard();
    // Bind insta snapshot filters for this test thread. `mem::forget` intentionally
    // leaks the scope guard so settings persist without storing the guard in TestRepo.
    // Safe: each test sets its own settings, and thread-locals are cleaned up on exit.
    let guard =
        setup_snapshot_settings_for_paths(repo.root_path(), &repo.worktrees).bind_to_scope();
    std::mem::forget(guard);
    repo
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

/// Repo with default branch available for merge operations.
///
/// The primary worktree is already on main, so no separate worktree is needed.
/// This fixture exists for compatibility with tests that expect it.
///
/// Use `#[from(repo_with_main_worktree)]` in rstest:
/// ```ignore
/// #[rstest]
/// fn test_merge(#[from(repo_with_main_worktree)] mut repo: TestRepo) {
///     let feature_wt = repo.add_worktree("feature");
///     // primary is on main, ready for merge
/// }
/// ```
#[rstest::fixture]
pub fn repo_with_main_worktree(repo: TestRepo) -> TestRepo {
    // Primary is already on main - no separate worktree needed
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

/// Repo with remote and a feature branch with one commit.
///
/// Combines `repo_with_remote` with a feature worktree setup.
/// Access the feature worktree path via `repo.worktrees["feature"]`.
///
/// Use for tests that need remote tracking AND a feature branch ready to merge/push.
/// ```ignore
/// #[rstest]
/// fn test_push(mut repo_with_remote_and_feature: TestRepo) {
///     let repo = &mut repo_with_remote_and_feature;
///     let feature_wt = &repo.worktrees["feature"];
///     // Has remote and feature with one commit
/// }
/// ```
#[rstest::fixture]
pub fn repo_with_remote_and_feature(mut repo_with_remote: TestRepo) -> TestRepo {
    // Primary is already on main - no separate worktree needed
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
/// - Primary worktree on main (unchanged)
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
    // Create a feature worktree and make a commit
    // Primary stays on main - no need for separate main worktree
    let feature_wt = repo.add_worktree("feature");
    std::fs::write(feature_wt.join("feature.txt"), "feature content").unwrap();
    repo.run_git_in(&feature_wt, &["add", "feature.txt"]);
    repo.run_git_in(&feature_wt, &["commit", "-m", "Add feature file"]);

    (repo, feature_wt)
}

/// Merge test setup with multiple commits on feature branch.
///
/// Creates a repo with:
/// - Primary worktree on main (unchanged)
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
    // Create a feature worktree and make multiple commits
    // Primary stays on main - no need for separate main worktree
    let feature_wt = repo.add_worktree("feature");
    repo.commit_in_worktree(&feature_wt, "file1.txt", "content 1", "feat: add file 1");
    repo.commit_in_worktree(&feature_wt, "file2.txt", "content 2", "feat: add file 2");

    (repo, feature_wt)
}

// =============================================================================
// PTY functions
// =============================================================================

/// Returns a PTY system with platform-appropriate setup.
///
/// On Unix, this blocks SIGTTIN/SIGTTOU signals to prevent test processes from
/// being stopped when PTY operations interact with terminal control.
///
/// On Windows, this returns the native ConPTY system directly.
///
/// Use this instead of `portable_pty::native_pty_system()` directly to ensure
/// PTY tests work correctly across platforms.
///
/// NOTE: PTY tests are behind the `shell-integration-tests` feature because they can
/// trigger a nextest bug where its InputHandler cleanup receives SIGTTOU. This happens
/// when tests spawn interactive shells (zsh -ic, bash -ic) which take control of the
/// foreground process group. See https://github.com/nextest-rs/nextest/issues/2878
/// Workaround: run with NEXTEST_NO_INPUT_HANDLER=1. See CLAUDE.md for details.
pub fn native_pty_system() -> Box<dyn portable_pty::PtySystem> {
    #[cfg(unix)]
    ignore_tty_signals();
    portable_pty::native_pty_system()
}

/// Open a PTY pair with default size (48 rows x 200 cols).
///
/// Most PTY tests use this standard size. Returns the master/slave pair.
pub fn open_pty() -> portable_pty::PtyPair {
    open_pty_with_size(48, 200)
}

/// Open a PTY pair with specified size.
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
    let home_dir = home::home_dir().unwrap().to_string_lossy().to_string();
    cmd.env("HOME", &home_dir);
    cmd.env(
        "PATH",
        std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string()),
    );

    // Windows-specific env vars required for processes to run
    #[cfg(windows)]
    {
        // USERPROFILE is Windows equivalent of HOME
        cmd.env("USERPROFILE", &home_dir);

        // SystemRoot is critical - many DLLs and system components need this
        if let Ok(val) = std::env::var("SystemRoot") {
            cmd.env("SystemRoot", &val);
            cmd.env("windir", &val); // Alias used by some programs
        }

        // SystemDrive (usually C:)
        if let Ok(val) = std::env::var("SystemDrive") {
            cmd.env("SystemDrive", val);
        }

        // TEMP/TMP directories
        if let Ok(val) = std::env::var("TEMP") {
            cmd.env("TEMP", &val);
            cmd.env("TMP", val);
        }

        // COMSPEC (cmd.exe path) - needed by some programs
        if let Ok(val) = std::env::var("COMSPEC") {
            cmd.env("COMSPEC", val);
        }

        // PSModulePath for PowerShell
        if let Ok(val) = std::env::var("PSModulePath") {
            cmd.env("PSModulePath", val);
        }
    }

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

// =============================================================================
// Snapshot settings functions (insta)
// =============================================================================

/// Add standard env var redactions to insta settings
///
/// These redact volatile metadata captured by insta-cmd in the `info` block.
/// Called by all snapshot settings helpers for consistency.
pub fn add_standard_env_redactions(settings: &mut insta::Settings) {
    settings.add_redaction(".env.GIT_CONFIG_GLOBAL", "[TEST_GIT_CONFIG]");
    settings.add_redaction(".env.WORKTRUNK_CONFIG_PATH", "[TEST_CONFIG]");
    settings.add_redaction(".env.WORKTRUNK_SYSTEM_CONFIG_PATH", "[TEST_SYSTEM_CONFIG]");
    settings.add_redaction(".env.WORKTRUNK_APPROVALS_PATH", "[TEST_APPROVALS]");
    settings.add_redaction(".env.WORKTRUNK_DIRECTIVE_FILE", "[DIRECTIVE_FILE]");
    settings.add_redaction(".env.HOME", "[TEST_HOME]");
    // Windows: the `home` crate uses USERPROFILE for home_dir()
    settings.add_redaction(".env.USERPROFILE", "[TEST_HOME]");
    settings.add_redaction(".env.XDG_CONFIG_HOME", "[TEST_CONFIG_HOME]");
    // Windows: etcetera uses APPDATA for config_dir()
    settings.add_redaction(".env.APPDATA", "[TEST_CONFIG_HOME]");
    settings.add_redaction(".env.PATH", "[PATH]");
    settings.add_redaction(".env.PWD", "[PWD]");
    // Mock commands directory (temp path for mock gh/glab binaries)
    settings.add_redaction(".env.MOCK_CONFIG_DIR", "[MOCK_CONFIG_DIR]");
    // OpenCode config directory (platform-independent override for tests)
    settings.add_redaction(".env.OPENCODE_CONFIG_DIR", "[TEST_OPENCODE_CONFIG]");
}

fn canonical_home_dir() -> Option<PathBuf> {
    home::home_dir().and_then(|path| canonicalize(&path).ok())
}

fn add_snapshot_path_prelude_filters(settings: &mut insta::Settings) {
    // Normalize project root path (for test fixtures)
    // This must come before repo path filter to avoid partial matches
    let project_root = std::env::var("CARGO_MANIFEST_DIR")
        .ok()
        .and_then(|path| canonicalize(std::path::Path::new(&path)).ok());
    if let Some(root) = project_root {
        settings.add_filter(&regex::escape(root.to_str().unwrap()), "[PROJECT_ROOT]");
    }

    // Normalize llvm-cov-target to target for coverage builds (cargo-llvm-cov)
    settings.add_filter(r"/target/llvm-cov-target/", "/target/");

    // Normalize backslashes FIRST so all subsequent path filters only need forward-slash versions.
    // This must come before any path replacement filters.
    settings.add_filter(r"\\", "/");
}

fn add_repo_and_worktree_path_filters(
    settings: &mut insta::Settings,
    root: &Path,
    worktrees: &HashMap<String, PathBuf>,
) {
    // Normalize paths (canonicalize for macOS /var -> /private/var symlink)
    let root_canonical = canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let root_str = root_canonical.to_str().unwrap();
    let root_str_normalized = root_str.replace('\\', "/");
    settings.add_filter(&regex::escape(&root_str_normalized), "_REPO_");
    // Also add POSIX-style path for Git Bash (C:\foo\bar -> /c/foo/bar)
    settings.add_filter(&regex::escape(&to_posix_path(root_str)), "_REPO_");

    // In tests, HOME is set to the temp directory containing the repo. Commands being tested
    // see HOME=temp_dir, so format_path_for_display() outputs ~/repo instead of the full path.
    // The repo is always at {temp_dir}/repo, so we hardcode ~/repo for the filter.
    // The optional suffix matches worktree paths like ~/repo.feature
    settings.add_filter(r"~/repo(\.[a-zA-Z0-9_-]+)?", "_REPO_$1");

    let home_dir = canonical_home_dir();

    // Also handle the case where the real home contains the temp directory (Windows/macOS)
    if let Some(home) = home_dir.as_ref()
        && let Ok(relative) = root_canonical.strip_prefix(home)
    {
        let tilde_path = format!("~/{}", relative.display()).replace('\\', "/");
        settings.add_filter(&regex::escape(&tilde_path), "_REPO_");
        let tilde_worktree_pattern = format!(r"{}(\.[a-zA-Z0-9_-]+)", regex::escape(&tilde_path));
        settings.add_filter(&tilde_worktree_pattern, "_REPO_$1");
    }

    for (name, path) in worktrees {
        let canonical = canonicalize(path).unwrap_or_else(|_| path.clone());
        let path_str = canonical.to_str().unwrap();
        let replacement = format!("_WORKTREE_{}_", name.to_uppercase().replace('-', "_"));
        let path_str_normalized = path_str.replace('\\', "/");
        settings.add_filter(&regex::escape(&path_str_normalized), &replacement);
        settings.add_filter(&regex::escape(&to_posix_path(path_str)), &replacement);

        if let Some(home) = home_dir.as_ref()
            && let Ok(relative) = canonical.strip_prefix(home)
        {
            let tilde_path = format!("~/{}", relative.display()).replace('\\', "/");
            settings.add_filter(&regex::escape(&tilde_path), &replacement);
        }
    }

    // Windows fallback: use a regex pattern to catch tilde-prefixed Windows temp paths.
    settings.add_filter(r"~/AppData/Local/Temp/\.tmp[^/]+/repo", "_REPO_");
    // Windows fallback for POSIX-style paths from Git Bash (used in hook template expansion).
    settings.add_filter(
        r"/[a-z]/Users/[^/]+/AppData/Local/Temp/\.tmp[^/]+/repo(\.[a-zA-Z0-9_/-]+)?",
        "_REPO_$1",
    );
}

fn add_placeholder_cleanup_filters(settings: &mut insta::Settings) {
    // Final cleanup: strip any remaining quotes around placeholders.
    settings.add_filter(
        r"'(?:\x1b\[[0-9;]*m)*(_(?:REPO|WORKTREE_[A-Z0-9_]+)_(?:\.[a-zA-Z0-9_.-]+)?(?:/[^']*)?)(?:\x1b\[[0-9;]*m)*'",
        "$1",
    );

    // Also strip quotes around bracket placeholders like [PROJECT_ID]
    settings.add_filter(
        r"'(?:\x1b\[[0-9;]*m)*(\[[A-Z_]+\])(?:\x1b\[[0-9;]*m)*'",
        "$1",
    );
    settings.add_filter(
        r"'(_(?:REPO|WORKTREE_[A-Z0-9_]+)_(?:\.[a-zA-Z0-9_-]+)?/[^']+)'",
        "$1",
    );
    settings.add_filter(r"(diff --git )a/(_(?:REPO|WORKTREE_[A-Z0-9_]+)_)", "$1a$2");
    settings.add_filter(r" b/(_(?:REPO|WORKTREE_[A-Z0-9_]+)_)", " b$1");
    settings.add_filter(r"(--- )a/(_(?:REPO|WORKTREE_[A-Z0-9_]+)_)", "$1a$2");
    settings.add_filter(r"(\+\+\+ )b/(_(?:REPO|WORKTREE_[A-Z0-9_]+)_)", "$1b$2");

    settings.add_filter(
        r"(\x1b\[1m)(_(?:REPO|WORKTREE_[A-Z0-9_]+)_/[^\s]+) b(_(?:REPO|WORKTREE_[A-Z0-9_]+)_/[^\s]+)",
        "$1diff --git a$2 b$3",
    );
    settings.add_filter(
        r"(\x1b\[0m) +--git a(_(?:REPO|WORKTREE_[A-Z0-9_]+)_/)",
        "$1 \x1b[1mdiff --git a$2",
    );
    settings.add_filter(r"(--- )(_(?:REPO|WORKTREE_[A-Z0-9_]+)_/)", "$1a$2");
    settings.add_filter(r"(\+\+\+ )(_(?:REPO|WORKTREE_[A-Z0-9_]+)_/)", "$1b$2");
    settings.add_filter(
        r"(\x1b\[1m)(_(?:REPO|WORKTREE_[A-Z0-9_]+)_/[^\x1b]+\.toml)(\x1b\[m)",
        "$1--- a$2$3",
    );

    // Normalize syntax highlighting around placeholders.
    settings.add_filter(
        r"\x1b\[2m \x1b\[0m\x1b\[2m(?:\x1b\[32m)?(_(?:REPO|WORKTREE_[A-Z0-9_]+)_(?:\.[a-zA-Z0-9_-]+)?)(?:\x1b\[0m)?\x1b\[2m \x1b\[0m",
        "\x1b[2m $1 \x1b[0m",
    );
    settings.add_filter(
        r"(?:\x1b\[\d+m)*\x1b\[32m(_(?:REPO|WORKTREE_[A-Z0-9_]+)_(?:/[^\x1b\s]+)?)(?:\x1b\[\d+m)*",
        "$1",
    );
}

fn add_temp_path_placeholder_filters(settings: &mut insta::Settings) {
    settings.add_filter(
        r"'?(?:[A-Z]:)?[/\\][^\s']+[/\\]\.tmp[^/\\']+[/\\]test-config\.toml\.new'?",
        "[TEST_CONFIG_NEW]",
    );
    settings.add_filter(
        r"'?(?:[A-Z]:)?[/\\][^\s']+[/\\]\.tmp[^/\\']+[/\\]test-config\.toml'?",
        "[TEST_CONFIG]",
    );
    settings.add_filter(
        r"'?(?:[A-Z]:)?[/\\][^\s']+[/\\]\.tmp[^/\\']+[/\\]test-approvals\.toml'?",
        "[TEST_APPROVALS]",
    );
    settings.add_filter(
        r"(?:\x1b\[\d+m)+(\[TEST_(?:CONFIG(?:_NEW)?|APPROVALS)\])(?:\x1b\[\d+m)+",
        "$1",
    );
    settings.add_filter(
        r"(?:[A-Z]:)?/[^\s]+/\.tmp[^/]+/test-gitconfig",
        "[TEST_GIT_CONFIG]",
    );
}

fn add_temp_home_filters(settings: &mut insta::Settings, temp_home: &Path) {
    // Get both the original path and the canonicalized path - they may differ on Windows
    // due to short path names (e.g., RUNNER~1 vs runneradmin) or other normalization.
    let temp_home_original = temp_home.to_string_lossy().replace('\\', "/");
    let temp_home_canonical = canonicalize(temp_home).unwrap_or_else(|_| temp_home.to_path_buf());
    let temp_home_str = temp_home_canonical.to_string_lossy().replace('\\', "/");

    if temp_home_str.contains(':') {
        settings.add_filter(
            &format!("'{}", regex::escape(&temp_home_str)),
            "'[TEMP_HOME]",
        );
        if temp_home_original != temp_home_str {
            settings.add_filter(
                &format!("'{}", regex::escape(&temp_home_original)),
                "'[TEMP_HOME]",
            );
        }
    }
    settings.add_filter(&regex::escape(&temp_home_str), "[TEMP_HOME]");
    if temp_home_original != temp_home_str {
        settings.add_filter(&regex::escape(&temp_home_original), "[TEMP_HOME]");
    }

    if temp_home_str.starts_with("/private/") {
        let without_private = &temp_home_str["/private".len()..];
        settings.add_filter(&regex::escape(without_private), "[TEMP_HOME]");
    }

    settings.add_filter(r"( )(?:\x1b\[[0-9;]*m)+('?)(\[TEMP_HOME\]/)", "$1$2$3");
    settings.add_filter(r"(\[TEMP_HOME\]/[^\x1b\s]+)(?:\x1b\[[0-9;]*m)+", "$1");
    settings.add_filter(r"'\[TEMP_HOME\](/[^']+)'", "[TEMP_HOME]$1");

    settings.add_filter(r"(diff --git )a/(\[TEMP_HOME\])", "$1a$2");
    settings.add_filter(r" b/(\[TEMP_HOME\])", " b$1");
    settings.add_filter(r"(--- )a/(\[TEMP_HOME\])", "$1a$2");
    settings.add_filter(r"(\+\+\+ )b/(\[TEMP_HOME\])", "$1b$2");

    settings.add_filter(
        r"(diff --git )(\[TEMP_HOME\]/[^\s]+) (\[TEMP_HOME\]/)",
        "$1a$2 b$3",
    );
    settings.add_filter(
        r"(\x1b\[1m)(\[TEMP_HOME\]/[^\s]+) b(\[TEMP_HOME\]/[^\s]+)",
        "$1diff --git a$2 b$3",
    );
    settings.add_filter(
        r"(\x1b\[0m) +--git a(\[TEMP_HOME\]/)",
        "$1 \x1b[1mdiff --git a$2",
    );
    settings.add_filter(r"(--- )a/(\[TEMP_HOME\]/)", "$1a$2");
    settings.add_filter(r"(--- )(\[TEMP_HOME\]/)", "$1a$2");
    settings.add_filter(r"(\+\+\+ )b/(\[TEMP_HOME\]/)", "$1b$2");
    settings.add_filter(r"(\+\+\+ )(\[TEMP_HOME\]/)", "$1b$2");
    settings.add_filter(
        r"(\x1b\[1m)(\[TEMP_HOME\]/[^\s\x1b]+\.toml)(\x1b\[m|\n|$)",
        "$1--- a$2$3",
    );
}

fn add_project_id_filters(settings: &mut insta::Settings) {
    settings.add_filter(
        r"/private/var/folders/[^/]+/[^/]+/T/\.[^/]+/[^)'\s\x1b]+",
        "[PROJECT_ID]",
    );
    settings.add_filter(
        r"/var/folders/[^/]+/[^/]+/T/\.[^/]+/[^)'\s\x1b]+",
        "[PROJECT_ID]",
    );
    settings.add_filter(
        r"/private/tmp/(?:[^/]+/)*\.tmp[^/]+/[^)'\s\x1b]+",
        "[PROJECT_ID]",
    );
    settings.add_filter(r"/tmp/(?:[^/]+/)*\.tmp[^/]+/[^)'\s\x1b]+", "[PROJECT_ID]");
    settings.add_filter(
        r"[A-Z]:/Users/[^/]+/AppData/Local/Temp/\.tmp[^/]+/[^)'\s\x1b]+",
        "[PROJECT_ID]",
    );
    settings.add_filter(
        r"'[A-Z]:/Users/[^/]+/AppData/Local/Temp/\.tmp[^/]+/[^']+'",
        "[PROJECT_ID]",
    );
    settings.add_filter(r"~/([a-zA-Z0-9_-]+)", "_PARENT_/$1");
    settings.add_filter(r"'\[PROJECT_ID\]'", "[PROJECT_ID]");
    settings.add_filter(r"HOME: .*/\.tmp[^/\s]+", "HOME: [TEST_HOME]");
}

/// Create configured insta Settings for snapshot tests
///
/// This extracts the common settings configuration while allowing the
/// `assert_cmd_snapshot!` macro to remain in test files for correct module path capture.
pub fn setup_snapshot_settings(repo: &TestRepo) -> insta::Settings {
    setup_snapshot_settings_impl(repo.root_path(), None)
}

/// Internal implementation that optionally includes temp_home filter.
/// The temp_home filter MUST be added before PROJECT_ID filters to take precedence.
fn setup_snapshot_settings_impl(root: &Path, temp_home: Option<&Path>) -> insta::Settings {
    let worktrees = HashMap::new(); // Caller doesn't need worktree filters
    setup_snapshot_settings_for_paths_with_home(root, &worktrees, temp_home)
}

/// Full snapshot settings - path filters AND ANSI cleanup.
/// Use this with `settings.bind()` for assert_cmd_snapshot! tests.
/// Clones current settings (which may already have minimal path filters from TestRepo).
pub fn setup_snapshot_settings_for_paths(
    root: &Path,
    worktrees: &HashMap<String, PathBuf>,
) -> insta::Settings {
    setup_snapshot_settings_for_paths_with_home(root, worktrees, None)
}

/// Internal implementation with optional temp_home support.
///
/// When `temp_home` is provided, we create fresh settings rather than cloning current settings.
/// This is critical because TestRepo's snapshot guard may have already added PROJECT_ID filters,
/// and cloning would inherit those filters which would be applied BEFORE our TEMP_HOME filter.
fn setup_snapshot_settings_for_paths_with_home(
    root: &Path,
    worktrees: &HashMap<String, PathBuf>,
    temp_home: Option<&Path>,
) -> insta::Settings {
    // When temp_home is provided, start fresh to ensure TEMP_HOME filter is applied before
    // any inherited PROJECT_ID filters. Otherwise, clone current settings for consistency.
    let mut settings = if temp_home.is_some() {
        insta::Settings::new()
    } else {
        insta::Settings::clone_current()
    };
    settings.set_snapshot_path("../snapshots");
    add_snapshot_path_prelude_filters(&mut settings);
    add_repo_and_worktree_path_filters(&mut settings, root, worktrees);
    add_placeholder_cleanup_filters(&mut settings);
    add_temp_path_placeholder_filters(&mut settings);
    if let Some(temp_home) = temp_home {
        add_temp_home_filters(&mut settings, temp_home);
    }
    add_project_id_filters(&mut settings);

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

    // Normalize shell "command not found" errors across platforms
    // - macOS: "sh: nonexistent-command: command not found"
    // - Windows Git Bash: "/usr/bin/bash: line 1: nonexistent-command: command not found"
    // - Linux (dash): "sh: 1: nonexistent-command: not found"
    // Normalize to a consistent format
    settings.add_filter(
        r"(?:/usr/bin/bash: line \d+|sh(?:: line \d+)?|bash)(?:: \d+)?: ([^:]+): (?:command )?not found",
        "sh: $1: command not found",
    );

    // Filter out PowerShell lines that differ between Windows and Unix.
    // On Windows, PowerShell profile paths use Documents\PowerShell\... while Unix uses
    // ~/.config/powershell/..., and PowerShell scanning is auto-enabled on Windows.
    // These targeted patterns strip platform-dependent output without affecting:
    // - "Detected shell: powershell" diagnostics (no colon after powershell)
    // - Clap help/error messages listing available shells
    // ANSI codes can appear between "powershell" and ":" in styled output (e.g.,
    // "\x1b[1mpowershell\x1b[22m:"), so we allow optional escape sequences in the match.
    settings.add_filter(r"(?m)^.*[Pp]owershell(?:\x1b\[[0-9;]*m)*:.*\n", ""); // status: "○ powershell: ..."
    settings.add_filter(r"(?m)^.*No .*powershell.* shell extension.*\n", ""); // uninstall hints
    settings.add_filter(r"(?m)^.*shell init powershell.*\n", ""); // gutter config content
    settings.add_filter(r"(?m)^.*for powershell .*\n", ""); // install success lines

    // Normalize Windows executable extension in help output
    // On Windows, clap shows "wt.exe" instead of "wt"
    settings.add_filter(r"wt\.exe", "wt");

    // Normalize version strings in `wt config show` OTHER section
    // wt version can be: v0.8.5, v0.8.5-2-gabcdef, v0.8.5-dirty, or bare git hash (b9ffe83)
    // Format: "○ wt: <bold>VERSION</>" on its own line
    settings.add_filter(
        r"(wt: \x1b\[1m)(?:v[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9]+-g[0-9a-f]+)?(?:-dirty)?|[0-9a-f]{7,40}(?:-dirty)?)",
        "${1}[VERSION]",
    );
    // git version format: "○ git: <bold>VERSION</>" (e.g., "2.47.1")
    settings.add_filter(
        r"(git: \x1b\[1m)[0-9]+\.[0-9]+\.[0-9]+[^\x1b]*",
        "${1}[VERSION]",
    );
    // Version check: "Up to date (<bold>VERSION</>)" or "current: VERSION)"
    // version_str() can be: v0.8.5, v0.8.5-2-gabcdef, v0.8.5-dirty, 0.8.5, or bare hash (8465a1f)
    settings.add_filter(
        r"(current: |Up to date \(\x1b\[1m)(?:v?[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9]+-g[0-9a-f]+)?(?:-dirty)?|[0-9a-f]{7,40}(?:-dirty)?)",
        "${1}[VERSION]",
    );

    // Normalize project root paths in "Binary invoked as:" debug output
    // Tests run cargo which produces paths like /path/to/worktrunk/target/debug/wt
    // Normalize to [PROJECT_ROOT]/target/debug/wt for deterministic snapshots
    settings.add_filter(
        r"(Binary invoked as: \x1b\[1m)[^\x1b]+/target/(debug|release)/wt(\x1b\[22m)",
        "${1}[PROJECT_ROOT]/target/$2/wt$3",
    );

    // Normalize shell probe binary paths
    // Shell probe reports the actual binary location which varies by system
    // Format: "is binary at <bold>PATH</>, not function"
    settings.add_filter(
        r"(is binary at \x1b\[1m)[^\x1b]+(/wt|/wt\.exe)(\x1b\[22m)",
        "${1}[BINARY_PATH]$2$3",
    );

    // Remove trailing ANSI reset codes at end of lines for cross-platform consistency
    // Windows terminal strips these trailing resets that Unix includes
    settings.add_filter(r"\x1b\[0m$", "");
    settings.add_filter(r"\x1b\[0m\n", "\n");

    // Normalize tree-sitter bash syntax highlighting differences between platforms.
    // On Linux, tree-sitter-bash may parse paths as "string" tokens (green: [32m),
    // while on macOS the same paths are just dimmed (no color). This causes snapshot
    // mismatches when the same code produces different ANSI sequences.
    // Strip green color from _REPO_ placeholders and normalize the surrounding sequences.
    // Pattern: [2m [0m[2m[32m_REPO_...[0m[2m [0m[2m  ->  [2m _REPO_... [0m[2m
    settings.add_filter(
        r"\x1b\[2m \x1b\[0m\x1b\[2m\x1b\[32m(_REPO_[^\x1b]*)\x1b\[0m\x1b\[2m \x1b\[0m\x1b\[2m",
        "\x1b[2m $1 \x1b[0m\x1b[2m",
    );

    // Normalize commit hashes throughout output.
    // Git on Windows produces different tree hashes due to filemode handling, causing
    // commit hashes to differ between platforms. Redact to [HASH] for consistency.
    //
    // Pattern 1: "Squashed @ <hash>" and "Committed @ <hash>" messages
    // Format: "Squashed @ " + optional dim code + 7-char hex hash + optional reset
    settings.add_filter(
        r"(Squashed|Committed) @ (?:\x1b\[2m)?[a-f0-9]{7}(?:\x1b\[22m)?",
        "$1 @ [HASH]",
    );
    // Pattern 2: "Merging/Pushing N commit(s) to branch @ <hash>" messages
    // Format: "@ " + dim code + 7-char hex hash + reset
    settings.add_filter(r"@ \x1b\[2m[a-f0-9]{7}\x1b\[22m", "@ \x1b[2m[HASH]\x1b[22m");
    // Pattern 3: Git log style "* <hash> message" lines
    // Format: "* " + yellow code + 7-char hex hash + reset
    settings.add_filter(r"\* \x1b\[33m[a-f0-9]{7}\x1b\[m", "* \x1b[33m[HASH]\x1b[m");

    // Filter out cargo-llvm-cov env variables from snapshot YAML headers.
    // These are only present during coverage runs and cause snapshot mismatches.
    // Note: YAML indentation in the info.env section is 4 spaces.
    settings.add_filter(r#"    CARGO_LLVM_COV: "1"\n"#, "");
    settings.add_filter(r#"    CARGO_LLVM_COV_TARGET_DIR: "[^"]+"\n"#, "");
    settings.add_filter(r#"    LLVM_PROFILE_FILE: "[^"]+"\n"#, "");

    settings
}

/// Create configured insta Settings for snapshot tests with a temporary home directory
///
/// This extends `setup_snapshot_settings` by adding a filter for the temporary home directory.
/// Use this for tests that need both a TestRepo and a temporary home (for user config testing).
///
/// IMPORTANT: The temp_home filter is passed to setup_snapshot_settings_impl so it gets added
/// BEFORE the generic [PROJECT_ID] filters. Otherwise, paths like /tmp/.tmpXXX/.config/worktrunk/config.toml
/// would match [PROJECT_ID] first.
pub fn setup_snapshot_settings_with_home(repo: &TestRepo, temp_home: &TempDir) -> insta::Settings {
    setup_snapshot_settings_impl(repo.root_path(), Some(temp_home.path()))
}

/// Create configured insta Settings for snapshot tests with only a temporary home directory
///
/// Use this for tests that don't need a TestRepo but do need a temporary home directory
/// (e.g., shell configuration tests, config init tests).
pub fn setup_home_snapshot_settings(temp_home: &TempDir) -> insta::Settings {
    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    // Canonicalize to match paths in output (macOS /var -> /private/var)
    let canonical_home =
        canonicalize(temp_home.path()).unwrap_or_else(|_| temp_home.path().to_path_buf());
    settings.add_filter(
        &regex::escape(&canonical_home.to_string_lossy()),
        "[TEMP_HOME]",
    );
    settings.add_filter(r"\\", "/");
    // Filter out PowerShell lines (see main filter in setup_snapshot_settings_impl for details)
    settings.add_filter(r"(?m)^.*[Pp]owershell(?:\x1b\[[0-9;]*m)*:.*\n", "");
    settings.add_filter(r"(?m)^.*No .*powershell.* shell extension.*\n", "");
    settings.add_filter(r"(?m)^.*shell init powershell.*\n", "");
    settings.add_filter(r"(?m)^.*for powershell .*\n", "");
    // Normalize Windows executable extension in help output
    settings.add_filter(r"wt\.exe", "wt");
    // Normalize git "not a git repository" messages across environments.
    // Local:     "fatal: not a git repository (or any parent up to mount point /)\n
    //             Stopping at filesystem boundary (GIT_DISCOVERY_ACROSS_FILESYSTEM not set)."
    // CI/Docker: "fatal: not a git repository (or any of the parent directories): .git"
    settings.add_filter(
        r"fatal: not a git repository \(or any[^\n]*(?:\n[^\n]*filesystem boundary[^\n]*)?",
        "fatal: not a git repository [GIT_DISCOVERY_MSG]",
    );
    // Normalize thread IDs in panic messages (vary across runs)
    settings.add_filter(r"thread '([^']+)' \(\d+\)", "thread '$1'");
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

    // Filter temp paths in output — multiple forms needed for cross-platform:
    // 1. Canonical path (macOS: /private/tmp needs the canonical /private form)
    // 2. Raw path as provided
    // 3. Regex matching the unique temp dir name with any prefix (Windows:
    //    format_path_for_display replaces $HOME with ~, producing ~/AppData/...
    //    which doesn't match the raw path. Match by unique dir name instead.)
    if let Ok(canonical) = dunce::canonicalize(temp_path) {
        let canonical_str = canonical.to_str().unwrap();
        let temp_str = temp_path.to_str().unwrap();
        if canonical_str != temp_str {
            settings.add_filter(&regex::escape(canonical_str), "[TEMP]");
        }
    }
    settings.add_filter(&regex::escape(temp_path.to_str().unwrap()), "[TEMP]");
    // Match the unique temp dir name with any path prefix (handles ~/AppData/... on Windows)
    if let Some(dir_name) = temp_path.file_name().and_then(|n| n.to_str()) {
        // Consume optional leading quote from shell_escape (format_path_for_display
        // wraps non-home paths in single quotes on Windows).
        let pattern = format!(r"'?[^\s]*{}", regex::escape(dir_name));
        settings.add_filter(&pattern, "[TEMP]");
    }
    settings.add_filter(r"\\", "/");
    // Clean up trailing shell-escape quote after [TEMP] replacement — the leading
    // quote is consumed by the dir-name regex, but the trailing one remains after
    // the file name (e.g., [TEMP]/test-config.toml' → [TEMP]/test-config.toml).
    settings.add_filter(r"(\[TEMP\]/[^\s]*)'", "$1");
    // Normalize Windows executable extension in help output
    settings.add_filter(r"wt\.exe", "wt");

    add_standard_env_redactions(&mut settings);

    settings
}

// =============================================================================
// PTY Test Filters
// =============================================================================
//
// PTY-based tests (shell wrappers, approval prompts, TUI picker) capture output
// from pseudo-terminals. This output has platform-specific artifacts that need
// normalization for stable snapshots.
//
// These filters consolidate patterns that were previously scattered across
// individual `normalize_*` functions in each test file. Using insta filters
// instead of custom normalization functions:
// - Reduces code duplication
// - Ensures consistent normalization across all PTY tests
// - Makes it easier to add new normalizations in one place
//
// Usage:
//   let mut settings = insta::Settings::clone_current();
//   add_pty_filters(&mut settings);
//   settings.bind(|| {
//       assert_snapshot!(output);
//   });

/// Add filters for PTY-specific artifacts that vary between platforms.
///
/// This handles:
/// - macOS PTY control sequences (^D followed by backspaces)
/// - Leading ANSI reset codes that vary between macOS and Linux
///
/// Note: CRLF normalization is done eagerly in PTY exec functions, not here.
pub fn add_pty_filters(settings: &mut insta::Settings) {
    // macOS PTYs emit ^D (literal caret-D) followed by backspaces (0x08)
    // when EOF is signaled. Linux PTYs don't. Strip these for consistency.
    settings.add_filter(r"\^D\x08+", "");

    // Remove redundant leading reset codes per line.
    // macOS and Linux PTYs generate ANSI codes slightly differently.
    // This handles lines that start with ESC[0m (reset).
    settings.add_filter(r"(?m)^\x1b\[0m", "");
}

/// Add filters for binary paths (target/debug/wt) in PTY output.
///
/// Test binaries are run from the cargo target directory, which varies.
pub fn add_pty_binary_path_filters(settings: &mut insta::Settings) {
    // Match paths ending in target/debug/wt or target/release/wt
    // Also handles llvm-cov-target used by cargo-llvm-cov
    settings.add_filter(
        r"[^\s]+/target/(?:llvm-cov-target/)?(?:debug|release)/wt",
        "[BIN]",
    );
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    fn test_commit_with_age(repo: TestRepo) {
        // TestRepo::standard() already includes one initial commit from fixture

        // Create commits with specific ages
        repo.commit_with_age("One hour ago", HOUR);
        repo.commit_with_age("One day ago", DAY);
        repo.commit_with_age("One week ago", WEEK);
        repo.commit_with_age("Ten minutes ago", 10 * MINUTE);

        // Verify commits were created (1 from fixture + 4 = 5 commits)
        let output = repo.git_command().args(["log", "--oneline"]).run().unwrap();
        let log = String::from_utf8_lossy(&output.stdout);
        assert_eq!(log.lines().count(), 5);
    }
}
