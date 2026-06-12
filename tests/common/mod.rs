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
    // Bind insta snapshot filters for this test thread, leaking the guard:
    // rstest has no teardown, and the guard can't ride along in TestRepo
    // (insta is a dev-dependency, invisible to the lib crate defining
    // TestRepo; the guard is also !Send by design). Under nextest (process
    // per test) the leak ends with the process. Under libtest the binding
    // persists on the reused worker thread and bleeds into later tests on
    // it — so a test must never rely on settings it didn't bind itself,
    // and a missing redaction can be masked here but exposed under nextest.
    // `test_no_host_specific_paths_in_snapshots` (snapshot_formatting_guard)
    // enforces the observable invariant: no host-specific paths in committed
    // snapshots.
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

/// Canonicalize a `temp_home` for use as a base when building paths that
/// production code will compare against the exported HOME.
///
/// `set_temp_home_env` exports HOME via `dunce::canonicalize`, so paths the
/// command later prints (e.g. the nushell vendor-autoload dir) only tilde-
/// shorten in `format_path_for_display` when they share that canonical prefix.
/// Two platform pitfalls make a bare `temp_home.path()` wrong:
///
/// - **macOS**: the temp dir lives under `/var`, a symlink to `/private/var`.
///   Canonicalizing resolves it to match the exported HOME, so the printed path
///   shortens to `~/...` instead of the full temp path.
/// - **Windows**: `std::fs::canonicalize` returns a `\\?\` verbatim path, where
///   the forward slashes in a later `.join("a/b/c")` are treated as literal
///   filename characters rather than separators — breaking both the file write
///   and `.exists()`. `dunce::canonicalize` returns an ordinary path.
pub fn canonical_temp_home(temp_home: &TempDir) -> std::path::PathBuf {
    dunce::canonicalize(temp_home.path()).unwrap()
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
/// Without this, an instrumented child writes `default_*.profraw` into its cwd
/// (i.e. the repo root for tests that don't override it) instead of the path
/// `cargo llvm-cov` chose. The non-PTY equivalent lives inside
/// `worktrunk::testing::isolate_subprocess_env`; both paths share
/// [`worktrunk::testing::default_llvm_profile_file`] for the
/// inherit-or-temp-dir resolution.
///
/// Use `configure_pty_command()` for the full setup, or call this directly if you
/// need custom env_clear handling (e.g., shell-specific env vars).
pub fn pass_coverage_env_to_pty_cmd(cmd: &mut portable_pty::CommandBuilder) {
    cmd.env(
        "LLVM_PROFILE_FILE",
        worktrunk::testing::default_llvm_profile_file(),
    );
    for key in worktrunk::testing::COVERAGE_ENV_VARS {
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
    settings.add_redaction(
        ".env.WORKTRUNK_PROJECT_CONFIG_PATH",
        "[TEST_PROJECT_CONFIG]",
    );
    settings.add_redaction(".env.WORKTRUNK_APPROVALS_PATH", "[TEST_APPROVALS]");
    settings.add_redaction(".env.WORKTRUNK_DIRECTIVE_CD_FILE", "[DIRECTIVE_CD_FILE]");
    settings.add_redaction(
        ".env.WORKTRUNK_DIRECTIVE_EXEC_FILE",
        "[DIRECTIVE_EXEC_FILE]",
    );
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
    // Nushell vendor-autoload override (temp path pinned by shell-integration tests)
    settings.add_redaction(
        ".env.WORKTRUNK_TEST_NU_VENDOR_AUTOLOAD_DIR",
        "[TEST_NU_VENDOR_AUTOLOAD]",
    );
    // OpenCode config directory (platform-independent override for tests)
    settings.add_redaction(".env.OPENCODE_CONFIG_DIR", "[TEST_OPENCODE_CONFIG]");
    // `wt config show --full` tests inject WORKTRUNK_TEST_LATEST_VERSION = the
    // current crate version (so the version-check line reads "Up to date"), which
    // would otherwise churn this `info` block on every release bump. Redact any
    // semver-shaped value to [VERSION]; the "error" sentinel test passes a
    // non-semver value and is left intact.
    settings.add_dynamic_redaction(".env.WORKTRUNK_TEST_LATEST_VERSION", |value, _path| {
        let is_semver = value.as_str().is_some_and(|s| {
            let mut parts = s.split('.');
            (0..3).all(|_| parts.next().is_some_and(|p| p.parse::<u32>().is_ok()))
                && parts.next().is_none()
        });
        if is_semver {
            insta::internals::Content::from("[VERSION]")
        } else {
            value
        }
    });

    // Redact cargo-llvm-cov env from snapshot info blocks. `LLVM_PROFILE_FILE`
    // is set on every test subprocess by `isolate_subprocess_env` (#2730) —
    // its `<temp_dir>/wt-test-profraw/cov-%m_%p.profraw` value is platform-
    // and host-specific and would leak into snapshots regenerated on any
    // other machine. CARGO_LLVM_COV* propagate by the same path under
    // `cargo llvm-cov`. `add_redaction` is the right hammer here:
    // `add_filter` substitutes only on the captured snapshot content, not
    // the YAML info/env block where these entries live.
    settings.add_redaction(".env.LLVM_PROFILE_FILE", "[LLVM_PROFILE_FILE]");
    settings.add_redaction(".env.CARGO_LLVM_COV", "[CARGO_LLVM_COV]");
    settings.add_redaction(
        ".env.CARGO_LLVM_COV_TARGET_DIR",
        "[CARGO_LLVM_COV_TARGET_DIR]",
    );

    // Drop empty-valued entries from the recorded `env:` block so it depends
    // only on what the test affirmatively sets — identically on every
    // contributor's machine.
    //
    // insta-cmd records `Command::env_remove` calls as `KEY: ""` (it
    // serializes `get_envs()`, which yields `None` for removals, rendered via
    // `unwrap_or("")`), so a removal is indistinguishable from a deliberate
    // set-to-empty — and the recorded line is false either way: the child sees
    // no variable, not an empty one. Worse, `isolate_subprocess_env` scrubs
    // every `GIT_*` / `WORKTRUNK_*` key it finds in the *parent* environment,
    // so which markers appear is a property of the host: CI has `GIT_EDITOR`,
    // a contributor's box might have `GIT_PAGER`, neither, or both. insta
    // never *compares* the `info:` block (only stdout/stderr/exit), so this
    // only churns regenerated snapshots from machine to machine — but that
    // churn is exactly what makes contributors think they have to match their
    // local environment to CI. Normalizing it away here means they don't.
    //
    // This runs as its own pass after the per-key `.env.*` redactions above,
    // so affirmatively-set vars (config paths → `[TEST_CONFIG]`, coverage →
    // `[LLVM_PROFILE_FILE]`, etc.) already hold non-empty placeholders and are
    // kept.
    //
    // Caveat: a test that affirmatively sets a var to `""` as its subject
    // (e.g. `test_list_config_env_override_validation_failure` setting
    // `WORKTRUNK_WORKTREE_PATH=""` to trigger the "worktree-path cannot be
    // empty" warning) loses that header line too — the recorded YAML can't
    // tell it apart from a removal. Harmless: the test body still asserts the
    // warning, and insta never compares the `env:` block.
    settings.add_dynamic_redaction(".env", |content, _path| {
        drop_empty_env_entries(content)
    });
}

/// Drop empty-valued entries from an insta-cmd `env` map node. These are
/// usually `Command::env_remove` markers — chiefly the
/// [`isolate_subprocess_env`](worktrunk::testing) scrub, whose key set depends
/// on the host environment; removing them makes the recorded block
/// host-independent. An affirmatively-set empty value is indistinguishable
/// from a removal in the recorded YAML and is dropped too — see the caveat on
/// [`add_standard_env_redactions`].
fn drop_empty_env_entries(content: insta::internals::Content) -> insta::internals::Content {
    use insta::internals::Content;

    let Content::Map(entries) = content else {
        return content;
    };
    let kept = entries
        .into_iter()
        .filter(|(_, value)| value.as_str() != Some(""))
        .collect();
    Content::Map(kept)
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
        let root_str = root.to_str().unwrap();
        // Raw (backslashes on Windows) and forward-slash forms. Worktrunk normalizes
        // paths for display (`invocation_path`, `to_slash_lossy`), so output can use
        // either form depending on the code path.
        settings.add_filter(&regex::escape(root_str), "[PROJECT_ROOT]");
        let root_str_normalized = root_str.replace('\\', "/");
        if root_str_normalized != root_str {
            settings.add_filter(&regex::escape(&root_str_normalized), "[PROJECT_ROOT]");
        }
    }

    // Normalize llvm-cov-target to target for coverage builds (cargo-llvm-cov)
    settings.add_filter(r"/target/llvm-cov-target/", "/target/");

    // Normalize cargo-affected's instrumented build dir to target/. cargo-affected
    // routes its instrumented build to target/affected/build/ to avoid invalidating
    // the project's normal target/ — see max-sixty/cargo-affected#12.
    settings.add_filter(r"/target/affected/build/", "/target/");

    // Normalize cross-target build dirs (target/<triple>/) to target/ when tests
    // run via `cargo nextest run --target <triple>` — used by the nightly
    // `release-target` matrix (musl, intel-darwin). Anchored on the vendor
    // field of a Rust target triple to avoid matching unrelated subdirs.
    settings.add_filter(
        r"/target/[a-z0-9_]+-(?:unknown|apple|pc|wasi)-[a-z0-9_-]+/",
        "/target/",
    );

    // Deliberately no global `\\` → `/` normalization here: it corrupts
    // intentional backslashes (JSON `\u001b` ANSI escapes, shell line
    // continuations) and worktrunk already emits forward-slash paths via
    // `path_slash`. If a test produces a raw Windows path, add a specific
    // filter for it in `add_repo_and_worktree_path_filters`.
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
    // Raw backslash form (Windows) + forward-slash form (all platforms) + Git Bash POSIX form.
    // The forward-slash form also handles Unix since `root_str_normalized == root_str` there.
    settings.add_filter(&regex::escape(root_str), "_REPO_");
    settings.add_filter(&regex::escape(&root_str_normalized), "_REPO_");
    settings.add_filter(&regex::escape(&to_posix_path(root_str)), "_REPO_");

    // Filters rewrite snapshot *content* only; the structured `info` block
    // insta-cmd records is reachable solely via redactions. A test that
    // passes a repo path as a CLI argument (`wt -C <root> list`) would
    // otherwise bake the per-test temp path into the snapshot's `args:`
    // block. Mirror the body filters: root-prefixed args become `_REPO_…`.
    // No POSIX form here — args are built in-process from `root_path()`.
    let arg_prefixes = [root_str.to_string(), root_str_normalized.clone()];
    settings.add_dynamic_redaction(".args[]", move |value, _path| {
        if let Some(arg) = value.as_str() {
            for prefix in &arg_prefixes {
                if let Some(suffix) = arg.strip_prefix(prefix.as_str()) {
                    return insta::internals::Content::from(format!(
                        "_REPO_{}",
                        suffix.replace('\\', "/")
                    ));
                }
            }
        }
        value
    });

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
        // Raw backslash form (Windows), forward-slash form, and Git Bash POSIX form.
        settings.add_filter(&regex::escape(path_str), &replacement);
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
}

/// Match the parent path of a `test-*` config under the test tempdir, in
/// either the absolute (`/var/folders/.../.tmp.../`) or tilde
/// (`~/`, `~/.tmp.../`) form. `format_path_for_display` produces both shapes
/// — macOS keeps the absolute form (canonicalized HOME `/private/var/...`
/// doesn't prefix the uncanonicalized config path), Linux strips to a tilde
/// (HOME == tempdir, prefix matches).
const TEST_PATH_PREFIX: &str =
    r"'?(?:~(?:/\.tmp[^/\\']+)?|(?:[A-Z]:)?[/\\][^\s']+[/\\]\.tmp[^/\\']+)[/\\]";

fn add_temp_path_placeholder_filters(settings: &mut insta::Settings) {
    settings.add_filter(
        &format!(r"{TEST_PATH_PREFIX}test-config\.toml\.new'?"),
        "[TEST_CONFIG_NEW]",
    );
    settings.add_filter(
        &format!(r"{TEST_PATH_PREFIX}test-config\.toml'?"),
        "[TEST_CONFIG]",
    );
    settings.add_filter(
        &format!(r"{TEST_PATH_PREFIX}test-approvals\.toml'?"),
        "[TEST_APPROVALS]",
    );
    settings.add_filter(
        r"(?:[A-Z]:)?/[^\s]+/\.tmp[^/]+/test-gitconfig",
        "[TEST_GIT_CONFIG]",
    );
}

/// Strip ANSI codes immediately wrapping a path-redaction placeholder so a
/// `<bold>{path}</>` source collapses to a clean `[PLACEHOLDER]` in snapshots.
///
/// Targeted: only placeholders that name a redacted path — not value
/// placeholders (`[VERSION]`, `[HASH]`, `[BUILD_MODE]`, `[BINARY_PATH]`) where
/// bold is meaningful styling we want to assert.
///
/// Insta filters apply in insertion order, so this must run *after* every
/// in-setup substitution that establishes one of these placeholders. Filters
/// added by tests on top of `setup_snapshot_settings*` are past this point —
/// they must consume ANSI inline via [`add_path_placeholder_filter`].
fn add_placeholder_ansi_strip_filter(settings: &mut insta::Settings) {
    settings.add_filter(
        r"(?:\x1b\[\d+m)+(\[(?:TEST_(?:CONFIG(?:_NEW)?|APPROVALS|GIT_CONFIG)|PROJECT_ID|TEMP(?:_HOME)?)\])(?:\x1b\[\d+m)+",
        "$1",
    );
}

/// Add a filter substituting `path_pattern` → `placeholder`, consuming any
/// ANSI codes immediately wrapping the path. Use this for test-specific path
/// redactions that need to survive a `<bold>` source — the late strip pass in
/// `setup_snapshot_settings*` runs before test-level filters get a chance.
pub fn add_path_placeholder_filter(
    settings: &mut insta::Settings,
    path_pattern: &str,
    placeholder: &str,
) {
    settings.add_filter(
        &format!(r"(?:\x1b\[\d+m)*{path_pattern}(?:\x1b\[\d+m)*"),
        placeholder,
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

/// Catch tempfile::tempdir() paths under non-standard OS temp directories.
///
/// `add_project_id_filters` has hardcoded patterns for standard temp locations
/// (/tmp, /var/folders, C:/Users/.../AppData/Local/Temp). CI may use a different
/// TEMP (e.g., D:\tmp for faster I/O on Windows). This filter uses the runtime
/// temp directory to catch those paths.
fn add_os_temp_dir_filter(settings: &mut insta::Settings) {
    let temp_dir = std::env::temp_dir();
    let temp_dir_str = temp_dir.to_string_lossy().replace('\\', "/");
    let temp_dir_str = temp_dir_str.trim_end_matches('/');

    let canonical = canonicalize(&temp_dir).unwrap_or_else(|_| temp_dir.clone());
    let canonical_str = canonical.to_string_lossy().replace('\\', "/");
    let canonical_str = canonical_str.trim_end_matches('/');

    // Canonical (longer) path first so it matches before the shorter one
    // (e.g., /private/var/folders/... before /var/folders/... on macOS).
    settings.add_filter(
        &format!(
            r"'?{}/\.tmp[^/']+/[^)'\s\x1b]+'?",
            regex::escape(canonical_str)
        ),
        "[PROJECT_ID]",
    );
    if canonical_str != temp_dir_str {
        settings.add_filter(
            &format!(
                r"'?{}/\.tmp[^/']+/[^)'\s\x1b]+'?",
                regex::escape(temp_dir_str)
            ),
            "[PROJECT_ID]",
        );
    }
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
    setup_snapshot_settings_for_paths_with_home(repo.root_path(), &HashMap::new(), None)
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
    add_os_temp_dir_filter(&mut settings);
    add_project_id_filters(&mut settings);

    add_standard_env_redactions(&mut settings);

    // Normalize timestamps in log filenames (format: YYYYMMDD-HHMMSS)
    // Match: post-start-NAME-SHA-HHMMSS.log
    settings.add_filter(
        r"post-start-[^-]+-[0-9a-f]{7,40}-\d{6}\.log",
        "post-start-[NAME]-[TIMESTAMP].log",
    );

    add_remove_stats_byte_filter(&mut settings);

    // Normalize the platform shell basename so cross-platform snapshots match.
    // `wt step {commit,squash} --dry-run` renders the LLM shell invocation,
    // which is `sh` on Unix and `bash.exe` on Windows (Git Bash). No `\b`: the
    // syntax-highlighted output puts an ANSI code (`...m`) immediately before
    // `bash`, and `m` is a word char so `\bbash` wouldn't see a word boundary.
    settings.add_filter(r"bash\.exe", "sh");

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

    // Collapse build-mode (debug|release) so snapshots survive both cargo's
    // debug builds and crane/release builds (notably the nightly nix-flake
    // sandbox). The pattern is anchored on `/target/.../wt` so it matches
    // the bin path in "Invoked as:" / "Binary invoked as:" / diagnostic
    // hint output without touching unrelated `target/` paths.
    settings.add_filter(r"/target/(debug|release)/wt", "/target/[BUILD_MODE]/wt");

    // Normalize shell probe binary paths
    // Shell probe reports the actual binary location which varies by system
    // Format: "is binary at <bold>PATH</>, not function"
    settings.add_filter(
        r"(is binary at \x1b\[1m)[^\x1b]+(/wt|/wt\.exe)(\x1b\[22m)",
        "${1}[BINARY_PATH]$2$3",
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

    // Last so every placeholder established above (e.g. [TEST_CONFIG],
    // [PROJECT_ID], [TEMP_HOME], [HASH]) is in place when we strip styling
    // wrappers around it.
    add_placeholder_ansi_strip_filter(&mut settings);

    settings
}

/// Create configured insta Settings for snapshot tests with a temporary home directory
///
/// This extends `setup_snapshot_settings` by adding a filter for the temporary home directory.
/// Use this for tests that need both a TestRepo and a temporary home (for user config testing).
///
/// IMPORTANT: The temp_home filter is added BEFORE the generic [PROJECT_ID] filters.
/// Otherwise, paths like /tmp/.tmpXXX/.config/worktrunk/config.toml would match [PROJECT_ID] first.
pub fn setup_snapshot_settings_with_home(repo: &TestRepo, temp_home: &TempDir) -> insta::Settings {
    setup_snapshot_settings_for_paths_with_home(
        repo.root_path(),
        &HashMap::new(),
        Some(temp_home.path()),
    )
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
    // Filter out PowerShell lines (see main filter in setup_snapshot_settings_for_paths_with_home for details)
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
    add_placeholder_ansi_strip_filter(&mut settings);

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
    add_remove_stats_byte_filter(&mut settings);
    add_placeholder_ansi_strip_filter(&mut settings);

    settings
}

/// Normalize byte counts inside the `(N files · X UNIT)` stats parenthetical
/// emitted by `wt remove --foreground`.
///
/// The walk in `remove_dir_with_progress` hits the renamed worktree's `.git`
/// pointer file, whose content is the gitdir's absolute path — so the byte
/// total is sensitive to the temp-dir prefix (macOS `/var/folders/...` vs
/// Linux `/tmp/...` vs Windows). The literal `(...files · X UNIT)` shape is
/// unique enough to leave the deterministic copy-ignored summary
/// (`Copied N files · X B` — no surrounding parens) untouched, so the regex
/// doesn't depend on ANSI styling and works in both colored and `NO_COLOR`
/// test environments.
fn add_remove_stats_byte_filter(settings: &mut insta::Settings) {
    settings.add_filter(
        r"(\(\d+ files? · )\d+(?:\.\d+)? (B|KiB|MiB|GiB|TiB)",
        "${1}[BYTES] $2",
    );
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
/// Note: CRLF normalization is done eagerly in PTY exec functions, not here.
pub fn add_pty_filters(settings: &mut insta::Settings) {
    // macOS PTYs emit ^D (literal caret-D) followed by backspaces (0x08)
    // when EOF is signaled. Linux PTYs don't. Strip these for consistency.
    settings.add_filter(r"\^D\x08+", "");
}

/// Add filters for binary paths (target/debug/wt) in PTY output.
///
/// Test binaries are run from the cargo target directory, which varies.
pub fn add_pty_binary_path_filters(settings: &mut insta::Settings) {
    // Match paths ending in target/.../{debug,release}/wt — covers the
    // default layout (`target/debug/wt`), cargo-llvm-cov (`llvm-cov-target/`),
    // cargo-affected (`affected/build/`, max-sixty/cargo-affected#12), and
    // cross-target builds (e.g. `target/x86_64-unknown-linux-musl/debug/wt`
    // from the nightly `release-target` matrix).
    //
    // Include the literal `[BUILD_MODE]` placeholder so this filter still
    // collapses to `[BIN]` when the prelude's `target/(debug|release)/wt`
    // → `target/[BUILD_MODE]/wt` rewrite has already run.
    settings.add_filter(
        r"[^\s]+/target/(?:[^/\s]+/)*(?:debug|release|\[BUILD_MODE\])/wt",
        "[BIN]",
    );
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;
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

    /// Regression: a `<bold>{path}</>` warning has to land on the same snapshot
    /// regardless of whether `format_path_for_display` returned the absolute
    /// (macOS) or tilde (Linux) form. The path-substitution filter has to
    /// catch both alternatives, and the late ANSI-strip pass has to remove the
    /// styling wrappers around the resulting placeholder.
    #[test]
    fn placeholder_strip_collapses_styled_paths_cross_platform() {
        let mut settings = insta::Settings::new();
        add_temp_path_placeholder_filters(&mut settings);
        add_placeholder_ansi_strip_filter(&mut settings);

        // macOS: format_path_for_display falls through to the absolute form
        // because HOME is canonicalized (/private/var) but the path isn't.
        let macos = "▲ \x1b[1m/var/folders/abc/T/.tmpXYZ/test-config.toml\x1b[22m failed";
        // Linux CI: HOME == tempdir, so the prefix strips to a clean tilde
        // form with no `.tmp...` segment between `~` and the filename.
        let linux_home_eq_tempdir = "▲ \x1b[1m~/test-config.toml\x1b[22m failed";
        // Linux dev box: HOME != tempdir but tempdir lives under HOME, so the
        // tilde form keeps the `.tmpXYZ/` segment.
        let linux_home_above_tempdir = "▲ \x1b[1m~/.tmpXYZ/test-config.toml\x1b[22m failed";

        settings.bind(|| {
            assert_snapshot!(macos, @"▲ [TEST_CONFIG] failed");
            assert_snapshot!(linux_home_eq_tempdir, @"▲ [TEST_CONFIG] failed");
            assert_snapshot!(linux_home_above_tempdir, @"▲ [TEST_CONFIG] failed");
        });
    }

    /// The `env:` redaction drops every empty-valued entry (`KEY: ""` — how
    /// insta-cmd records `env_remove`, including the host-dependent
    /// `GIT_*`/`WORKTRUNK_*` scrub and the unconditional `NO_COLOR`/`SHELL`/
    /// `PSModulePath` scrubs) while keeping everything a test affirmatively
    /// sets to a non-empty value. A non-map node passes through.
    #[test]
    fn drop_empty_env_entries_normalizes_env_block() {
        use insta::internals::Content;

        let env = Content::Map(vec![
            // Host-dependent scrub markers — dropped.
            (Content::from("GIT_EDITOR"), Content::from("")),
            (Content::from("GIT_PAGER"), Content::from("")),
            (Content::from("WORKTRUNK_LEAKED"), Content::from("")),
            // Affirmatively set vars — kept.
            (Content::from("CLICOLOR_FORCE"), Content::from("1")),
            (Content::from("LANG"), Content::from("C")),
            // A scrub-prefixed var the test set to a real (redacted) value — kept.
            (
                Content::from("WORKTRUNK_CONFIG_PATH"),
                Content::from("[TEST_CONFIG]"),
            ),
            // A scrub-prefixed var set to "0", not removed — kept.
            (
                Content::from("WORKTRUNK_TEST_CLAUDE_INSTALLED"),
                Content::from("0"),
            ),
            // Unconditional, explicitly-named scrub — dropped like any removal.
            (Content::from("NO_COLOR"), Content::from("")),
        ]);

        let Content::Map(kept) = drop_empty_env_entries(env) else {
            panic!("expected a map back");
        };
        let keys: Vec<&str> = kept.iter().filter_map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "CLICOLOR_FORCE",
                "LANG",
                "WORKTRUNK_CONFIG_PATH",
                "WORKTRUNK_TEST_CLAUDE_INSTALLED",
            ]
        );

        // Non-map nodes pass through untouched.
        let scalar = Content::from("not a map");
        assert_eq!(drop_empty_env_entries(scalar.clone()), scalar);
    }
}
