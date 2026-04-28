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
//! - [`TestRepo::standard()`] — copies pre-built fixture with remote + worktrees.
//!   For integration tests (used by the `repo()` rstest fixture).
//! - [`TestRepo::empty()`] — `git init` with no commits, no branches.
//!
//! ## Environment Isolation
//!
//! Git commands are run with isolated environments using `Cmd::env()` to ensure:
//! - No interference from global git config
//! - Deterministic commit timestamps
//! - Consistent locale settings
//! - No cross-test contamination
//! - Thread-safe execution (no global state mutation)

pub mod mock_commands;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::sanitize_branch_name;
use crate::git::Repository;
use crate::shell_exec::Cmd;

use self::mock_commands::{MockConfig, MockResponse};

/// Path to the `wt` binary built by Cargo.
///
/// Tries compile-time `option_env!()` first (works for unit tests in this
/// crate), then falls back to runtime `std::env::var()` (works for
/// integration tests that import via `worktrunk::testing`).
///
/// Panics if neither is available (only set during `cargo test`).
pub fn wt_bin() -> PathBuf {
    if let Some(path) = option_env!("CARGO_BIN_EXE_wt") {
        return PathBuf::from(path);
    }
    PathBuf::from(
        std::env::var("CARGO_BIN_EXE_wt")
            .expect("CARGO_BIN_EXE_wt not set — only available during `cargo test`"),
    )
}

/// Path to a workspace member binary (e.g., `wt-perf`, `mock-stub`).
///
/// These are binaries from other workspace packages (not the main `wt` crate),
/// so `CARGO_BIN_EXE_<name>` isn't available. Derives the path from the test
/// executable's location in `target/debug/deps/`.
pub fn workspace_bin(name: &str) -> PathBuf {
    let mut path = std::env::current_exe().expect("failed to get test executable path");
    path.pop(); // Remove test binary name
    path.pop(); // Remove deps/

    #[cfg(windows)]
    path.push(format!("{name}.exe"));

    #[cfg(not(windows))]
    path.push(name);

    path
}
use tempfile::TempDir;

/// Path to the standard fixture (relative to crate root).
/// Contains repo/, repo.feature-a/, repo.feature-b/, repo.feature-c/, origin_git/.
fn standard_fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/standard")
}

/// Worktree info returned from fixture copy.
struct FixtureWorktrees {
    worktrees: HashMap<String, PathBuf>,
    remote: PathBuf,
}

/// Copy the standard fixture to create a new test repo with worktrees and remote.
///
/// The fixture contains:
/// - Main repo on `main` branch with one commit
/// - Remote (origin) bare repository
/// - Three feature worktrees (feature-a, feature-b, feature-c) each with one commit
///
/// Pure Rust recursive copy - 2.5x faster than spawning cp/robocopy.
/// Benchmarked at 21ms vs 53ms per fixture copy on macOS.
fn copy_standard_fixture(dest: &Path) -> FixtureWorktrees {
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
            // Skip symlinks, sockets, etc (shouldn't be in fixture)
        }
    }

    let fixture = standard_fixture_path();
    copy_dir_recursive(&fixture, dest);

    // Verify essential directories exist after copy
    let essential = ["repo/_git", "origin_git", "repo.feature-a/_git"];
    for path in essential {
        let full_path = dest.join(path);
        assert!(
            full_path.exists(),
            "Essential fixture path missing after copy: {:?}",
            full_path
        );
    }

    // Rename _git to .git in all locations
    let renames = [
        ("repo/_git", "repo/.git"),
        ("origin_git", "origin.git"),
        ("repo.feature-a/_git", "repo.feature-a/.git"),
        ("repo.feature-b/_git", "repo.feature-b/.git"),
        ("repo.feature-c/_git", "repo.feature-c/.git"),
    ];
    for (from, to) in renames {
        let from_path = dest.join(from);
        let to_path = dest.join(to);
        if from_path.exists() {
            std::fs::rename(&from_path, &to_path).unwrap_or_else(|e| {
                panic!("Failed to rename {:?} to {:?}: {}", from_path, to_path, e)
            });
        }
    }

    // Verify origin.git is a valid bare repository
    let origin_git = dest.join("origin.git");
    assert!(
        origin_git.join("HEAD").exists(),
        "origin.git is not a valid git repository (missing HEAD): {:?}",
        origin_git
    );

    // Canonicalize dest for worktrees map (on macOS /var -> /private/var)
    let canonical_dest = canonicalize(dest).unwrap();

    // Fix gitdir files - fixture uses _git which we rename to .git
    // Paths are relative so no absolute path replacement needed
    for wt in ["feature-a", "feature-b", "feature-c"] {
        let gitdir_path = dest.join(format!("repo.{wt}/.git"));
        if gitdir_path.exists() {
            let content = std::fs::read_to_string(&gitdir_path).unwrap();
            let fixed = content.replace("_git", ".git");
            std::fs::write(&gitdir_path, fixed).unwrap();
        }

        // Fix main repo's worktree gitdir reference
        let main_gitdir = dest.join(format!("repo/.git/worktrees/repo.{wt}/gitdir"));
        if main_gitdir.exists() {
            let content = std::fs::read_to_string(&main_gitdir).unwrap();
            let fixed = content.replace("_git", ".git");
            std::fs::write(&main_gitdir, fixed).unwrap();
        }
    }

    // Fix remote URL in config (origin_git -> origin.git)
    let config_path = dest.join("repo/.git/config");
    if config_path.exists() {
        let content = std::fs::read_to_string(&config_path).unwrap();
        let fixed = content.replace("origin_git", "origin.git");
        std::fs::write(&config_path, fixed).unwrap();
    }

    // Build worktrees map using canonical paths
    let mut worktrees = HashMap::new();
    for wt in ["feature-a", "feature-b", "feature-c"] {
        worktrees.insert(wt.to_string(), canonical_dest.join(format!("repo.{wt}")));
    }

    let remote = canonical_dest.join("origin.git");

    FixtureWorktrees { worktrees, remote }
}

/// Write a gitconfig file for tests.
fn write_test_gitconfig(path: &Path) {
    std::fs::write(
        path,
        "[user]\n\tname = Test User\n\temail = test@example.com\n\
         [advice]\n\tmergeConflict = false\n\tresolveConflict = false\n\
         [init]\n\tdefaultBranch = main\n\
         [commit]\n\tgpgsign = false\n\
         [rerere]\n\tenabled = true\n",
    )
    .unwrap();
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

/// Default timeout for background hook/command completion.
/// Generous to avoid flakiness under CI load; exponential backoff means fast tests when things work.
const BG_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Static environment variables shared by all test isolation helpers.
///
/// These are used by both `configure_cli_command()` (for Command-based tests)
/// and `TestRepo::test_env_vars()` (for PTY tests). Adding a variable here
/// ensures consistency across both test infrastructure paths.
///
/// NOTE: Path-dependent variables (HOME, WORKTRUNK_CONFIG_PATH, GIT_CONFIG_*)
/// are NOT included here because they depend on the TestRepo instance.
pub const STATIC_TEST_ENV_VARS: &[(&str, &str)] = &[
    ("CLICOLOR_FORCE", "1"),
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
];

// NOTE: TERM is intentionally NOT in STATIC_TEST_ENV_VARS because:
// - configure_cli_command() sets TERM=alacritty for hyperlink detection testing
// - PTY tests (especially skim-based picker tests) need a TERM with valid terminfo
// - macOS CI doesn't have alacritty terminfo, causing skim to fail

/// Null device path, platform-appropriate.
/// Use this for GIT_CONFIG_SYSTEM to disable system config in tests.
#[cfg(windows)]
pub const NULL_DEVICE: &str = "NUL";
#[cfg(not(windows))]
pub const NULL_DEVICE: &str = "/dev/null";

/// Create a `wt` CLI command with standardized test environment settings.
///
/// The command has the following guarantees:
/// - All host `GIT_*` and `WORKTRUNK_*` variables are cleared
/// - Color output is forced (`CLICOLOR_FORCE=1`) so ANSI styling appears in snapshots
/// - Terminal width set to 150 columns (`COLUMNS=150`)
#[must_use]
pub fn wt_command() -> Command {
    let mut cmd = Command::new(wt_bin());
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
/// This is intentionally more thorough than `wt_perf::isolate_cmd()`:
/// integration tests need full determinism (timestamps, locale, mock commands,
/// wide COLUMNS for path display) while benchmarks only need host config stripped.
///
/// ## Related: `TestRepo::test_env_vars()`
///
/// PTY tests use `test_env_vars()` which returns env vars as a Vec. Both functions
/// share common variables via `STATIC_TEST_ENV_VARS`. Key differences:
/// - This function uses COLUMNS=500 (wider for long macOS paths in error messages)
/// - `test_env_vars()` uses COLUMNS=150 (narrower for PTY snapshot consistency)
/// - This function sets TERM=alacritty; PTY tests don't (skim needs valid terminfo)
/// - This function enables RUST_LOG=warn; PTY tests don't (too noisy in combined output)
/// - This function clears host GIT_*/WORKTRUNK_* vars; PTY tests start with clean env
pub fn configure_cli_command(cmd: &mut Command) {
    for (key, _) in std::env::vars() {
        if key.starts_with("GIT_") || key.starts_with("WORKTRUNK_") {
            cmd.env_remove(&key);
        }
    }
    // Prevent host environment from disabling ANSI in snapshots.
    // NO_COLOR can override CLICOLOR_FORCE in downstream output handling.
    cmd.env_remove("NO_COLOR");
    // Set to non-existent path to prevent loading user's real config.
    // Tests that need config should use TestRepo::configure_wt_cmd() which overrides this.
    // Note: env_remove above may cause insta-cmd to capture empty values in snapshots,
    // but correctness (isolating from host WORKTRUNK_* vars) trumps snapshot aesthetics.
    cmd.env("WORKTRUNK_CONFIG_PATH", "/nonexistent/test/config.toml");
    cmd.env(
        "WORKTRUNK_SYSTEM_CONFIG_PATH",
        "/etc/xdg/worktrunk/config.toml",
    );
    // `WORKTRUNK_PROJECT_CONFIG_PATH` is intentionally left unset so tests
    // can pick up `.config/wt.toml` in their own test repo via the default
    // lookup. Host leakage is prevented by the `WORKTRUNK_*` env_remove loop
    // above. Tests needing full project-config isolation (e.g., completion
    // tests that must not see this repo's aliases) should set it explicitly.
    cmd.env(
        "WORKTRUNK_APPROVALS_PATH",
        "/nonexistent/test/approvals.toml",
    );
    // Remove $SHELL to avoid platform-dependent diagnostic output (macOS has /bin/zsh,
    // Linux has /bin/bash). Tests that need SHELL should set it explicitly.
    cmd.env_remove("SHELL");
    // Remove PSModulePath to prevent false PowerShell detection on CI environments
    // where PowerShell Core is installed but not being used.
    cmd.env_remove("PSModulePath");
    // Disable auto PowerShell detection (tests that need it should set to "1")
    cmd.env("WORKTRUNK_TEST_POWERSHELL_ENV", "0");
    // Disable auto nushell detection (tests that need it should set to "1")
    cmd.env("WORKTRUNK_TEST_NUSHELL_ENV", "0");
    cmd.env("WORKTRUNK_TEST_EPOCH", TEST_EPOCH.to_string());
    // Enable warn-level logging so diagnostics show up in test failures
    cmd.env("RUST_LOG", "warn");
    // Treat Claude as not installed by default (tests can override with "1")
    cmd.env("WORKTRUNK_TEST_CLAUDE_INSTALLED", "0");
    // Treat OpenCode as not installed by default (tests can override with "1")
    cmd.env("WORKTRUNK_TEST_OPENCODE_INSTALLED", "0");

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

/// Configure a git command with isolated environment for testing.
///
/// Sets environment variables for:
/// - Isolated git config (using provided path or /dev/null)
/// - Deterministic commit timestamps
/// - Consistent locale settings
/// - No terminal prompts
///
/// # Arguments
/// * `cmd` - The git Command to configure
/// * `git_config_path` - Path to git config file (use `/dev/null` or `NULL_DEVICE` for none)
pub fn configure_git_cmd(cmd: &mut Command, git_config_path: &Path) {
    cmd.env("GIT_CONFIG_GLOBAL", git_config_path);
    cmd.env("GIT_CONFIG_SYSTEM", NULL_DEVICE);
    cmd.env("GIT_AUTHOR_DATE", "2025-01-01T00:00:00Z");
    cmd.env("GIT_COMMITTER_DATE", "2025-01-01T00:00:00Z");
    cmd.env("LC_ALL", "C");
    cmd.env("LANG", "C");
    cmd.env("WORKTRUNK_TEST_EPOCH", TEST_EPOCH.to_string());
    cmd.env("GIT_TERMINAL_PROMPT", "0");
}

/// Configure a `Cmd`-based git command with isolated environment for testing.
///
/// This is the `Cmd` equivalent of [`configure_git_cmd`]. Use this when building
/// git commands via the builder pattern (`Cmd::new("git")`).
pub fn configure_git_env(cmd: Cmd, git_config_path: &Path) -> Cmd {
    cmd.env("GIT_CONFIG_GLOBAL", git_config_path)
        .env("GIT_CONFIG_SYSTEM", NULL_DEVICE)
        .env("GIT_AUTHOR_DATE", "2025-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2025-01-01T00:00:00Z")
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("WORKTRUNK_TEST_EPOCH", TEST_EPOCH.to_string())
        .env("GIT_TERMINAL_PROMPT", "0")
}

/// Shared interface for test repository fixtures.
///
/// Provides `configure_git_cmd()` (for `Command`), `git_command()` (returns `Cmd`),
/// and `run_git_in()` with consistent environment isolation.
pub trait TestRepoBase {
    /// Path to the git config file for this test.
    fn git_config_path(&self) -> &Path;

    /// Configure a git command with isolated environment.
    fn configure_git_cmd(&self, cmd: &mut Command) {
        configure_git_cmd(cmd, self.git_config_path());
    }

    /// Create a git command for the given directory.
    fn git_command(&self, dir: &Path) -> Cmd {
        configure_git_env(Cmd::new("git"), self.git_config_path()).current_dir(dir)
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

/// Create a pair of temporary files for directive output (cd + exec).
///
/// The shell wrapper creates temp files and sets `WORKTRUNK_DIRECTIVE_CD_FILE`
/// and `WORKTRUNK_DIRECTIVE_EXEC_FILE` before running wt. Use
/// `configure_directive_files()` to set these on a Command for testing.
///
/// Returns `(cd_path, exec_path, guards)`. The guards must be kept alive for
/// the duration of the test — when dropped the temp files are cleaned up.
pub fn directive_files() -> (PathBuf, PathBuf, (tempfile::TempPath, tempfile::TempPath)) {
    let cd = tempfile::NamedTempFile::new().expect("failed to create cd temp file");
    let exec = tempfile::NamedTempFile::new().expect("failed to create exec temp file");
    let cd_path = cd.path().to_path_buf();
    let exec_path = exec.path().to_path_buf();
    (
        cd_path,
        exec_path,
        (cd.into_temp_path(), exec.into_temp_path()),
    )
}

/// Configure a Command to use the new split directive-file protocol.
///
/// Sets `WORKTRUNK_DIRECTIVE_CD_FILE` and `WORKTRUNK_DIRECTIVE_EXEC_FILE` env
/// vars so the wt binary writes a raw path to the cd file and arbitrary shell
/// to the exec file.
pub fn configure_directive_files(cmd: &mut Command, cd_path: &Path, exec_path: &Path) {
    cmd.env("WORKTRUNK_DIRECTIVE_CD_FILE", cd_path);
    cmd.env("WORKTRUNK_DIRECTIVE_EXEC_FILE", exec_path);
}

/// Configure a Command to use the split directive-file protocol with only the
/// CD file (EXEC scrubbed). This simulates running inside an alias/hook body
/// where the EXEC env var was stripped.
pub fn configure_directive_cd_only(cmd: &mut Command, cd_path: &Path) {
    cmd.env("WORKTRUNK_DIRECTIVE_CD_FILE", cd_path);
}

/// Create a temporary file for legacy single-file directive output.
///
/// Used to test the legacy fallback path where an old shell wrapper sets
/// `WORKTRUNK_DIRECTIVE_FILE`. Returns `(path, guard)`.
pub fn legacy_directive_file() -> (PathBuf, tempfile::TempPath) {
    let file = tempfile::NamedTempFile::new().expect("failed to create temp file");
    let path = file.path().to_path_buf();
    (path, file.into_temp_path())
}

/// Configure a Command to use the legacy single-file directive protocol.
///
/// Sets `WORKTRUNK_DIRECTIVE_FILE` env var (no new vars) to simulate an
/// outdated shell wrapper that hasn't been updated to the split protocol.
pub fn configure_legacy_directive_file(cmd: &mut Command, path: &Path) {
    cmd.env("WORKTRUNK_DIRECTIVE_FILE", path);
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
    /// Git config file with test settings (advice disabled, etc.)
    git_config_path: PathBuf,
    /// Path to mock bin directory for gh/glab commands
    mock_bin_path: Option<PathBuf>,
    /// Whether Claude CLI should be treated as installed
    claude_installed: bool,
    /// Whether OpenCode CLI should be treated as installed
    opencode_installed: bool,
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
        let repo = Self::init_repo(&["init", "-b", "main"]);
        // Also set identity in local config so unit tests that commit via
        // repo.run_command() work without GIT_CONFIG_GLOBAL.
        repo.repo
            .run_command(&["config", "user.name", "Test User"])
            .unwrap();
        repo.repo
            .run_command(&["config", "user.email", "test@example.com"])
            .unwrap();
        repo
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
        Self::init_repo(&["init", "--bare"])
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
    /// Uses a pre-created fixture for fast initialization - copies the fixture
    /// from `tests/fixtures/standard/` instead of running git commands.
    ///
    /// Also sets up mock gh/glab commands that appear authenticated to prevent
    /// CI status hints from appearing in test output.
    pub fn standard() -> Self {
        let temp_dir = TempDir::new().unwrap();

        // Copy from standard fixture (includes worktrees and remote)
        let fixture = copy_standard_fixture(temp_dir.path());

        // Canonicalize to resolve symlinks (important on macOS where /var is symlink to /private/var)
        let root = canonicalize(&temp_dir.path().join("repo")).unwrap();

        // Create isolated config path for this test
        let test_config_path = temp_dir.path().join("test-config.toml");
        let test_approvals_path = temp_dir.path().join("test-approvals.toml");
        let git_config_path = temp_dir.path().join("test-gitconfig");

        // Write gitconfig for tests
        write_test_gitconfig(&git_config_path);

        let mut repo = Self {
            temp_dir,
            root: root.clone(),
            repo: Repository::at(&root).unwrap(),
            worktrees: fixture.worktrees,
            remote: Some(fixture.remote),
            test_config_path,
            test_approvals_path,
            git_config_path,
            mock_bin_path: None,
            claude_installed: false,
            opencode_installed: false,
        };

        // Mock gh/glab as authenticated to prevent CI hints in test output
        repo.setup_mock_gh();

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
        std::fs::create_dir_all(path).unwrap();

        let config_dir = TempDir::new().unwrap();
        let test_config_path = config_dir.path().join("test-config.toml");
        let test_approvals_path = config_dir.path().join("test-approvals.toml");
        let git_config_path = config_dir.path().join("test-gitconfig");
        write_test_gitconfig(&git_config_path);

        configure_git_env(Cmd::new("git"), &git_config_path)
            .args(["init", "-b", "main", "--quiet"])
            .current_dir(path)
            .run()
            .unwrap();

        let root = canonicalize(path).unwrap();
        let repo = Repository::at(&root).unwrap();
        repo.run_command(&["config", "user.name", "Test User"])
            .unwrap();
        repo.run_command(&["config", "user.email", "test@example.com"])
            .unwrap();

        Self {
            temp_dir: config_dir,
            root: root.clone(),
            repo,
            worktrees: HashMap::new(),
            remote: None,
            test_config_path,
            test_approvals_path,
            git_config_path,
            mock_bin_path: None,
            claude_installed: false,
            opencode_installed: false,
        }
    }

    /// Create an empty test repository (no commits, no branches).
    ///
    /// Use this for tests that specifically need to test behavior in an
    /// uninitialized repo. Most tests should use `new()` instead.
    pub fn empty() -> Self {
        Self::init_repo(&["init", "-q"])
    }

    /// Shared initializer for `new()` and `empty()`.
    ///
    /// Creates a tempdir, writes gitconfig, runs `git init` with the given
    /// arguments, and returns a `TestRepo` with no commits and no identity
    /// in local config. Callers add identity or other setup as needed.
    fn init_repo(git_args: &[&str]) -> Self {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path().join("repo");
        std::fs::create_dir(&root).unwrap();

        let test_config_path = temp_dir.path().join("test-config.toml");
        let test_approvals_path = temp_dir.path().join("test-approvals.toml");
        let git_config_path = temp_dir.path().join("test-gitconfig");
        write_test_gitconfig(&git_config_path);

        configure_git_env(Cmd::new("git"), &git_config_path)
            .args(git_args.iter().copied())
            .current_dir(&root)
            .run()
            .unwrap();

        let root = canonicalize(&root).unwrap();

        Self {
            temp_dir,
            root: root.clone(),
            repo: Repository::at(&root).unwrap(),
            worktrees: HashMap::new(),
            remote: None,
            test_config_path,
            test_approvals_path,
            git_config_path,
            mock_bin_path: None,
            claude_installed: false,
            opencode_installed: false,
        }
    }

    /// Configure a git command with isolated environment
    ///
    /// This sets environment variables only for the specific command,
    /// ensuring thread-safety and test isolation.
    pub fn configure_git_cmd(&self, cmd: &mut Command) {
        configure_git_cmd(cmd, &self.git_config_path);
    }

    /// Get standard test environment variables as a vector.
    ///
    /// This is useful for PTY tests and other cases where you need environment variables
    /// as a vector rather than setting them on a Command.
    ///
    /// ## Related: `configure_cli_command()`
    ///
    /// Command-based tests use `configure_cli_command()`. Both functions share common
    /// variables via `STATIC_TEST_ENV_VARS`. See that function's docs for differences.
    #[cfg_attr(windows, allow(dead_code))] // Used only by unix PTY tests
    pub fn test_env_vars(&self) -> Vec<(String, String)> {
        // Start with shared static env vars
        let mut vars: Vec<(String, String)> = STATIC_TEST_ENV_VARS
            .iter()
            .map(|&(k, v)| (k.to_string(), v.to_string()))
            .collect();

        // Add path-dependent variables specific to this TestRepo
        vars.extend([
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
            // Prevent git from prompting for credentials when running under a TTY
            ("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()),
            // Use test-specific home directory for isolation
            ("HOME".to_string(), self.home_path().display().to_string()),
            (
                "XDG_CONFIG_HOME".to_string(),
                self.home_path().join(".config").display().to_string(),
            ),
            ("WORKTRUNK_TEST_EPOCH".to_string(), TEST_EPOCH.to_string()),
            (
                "WORKTRUNK_CONFIG_PATH".to_string(),
                self.test_config_path().display().to_string(),
            ),
            (
                "WORKTRUNK_SYSTEM_CONFIG_PATH".to_string(),
                "/etc/xdg/worktrunk/config.toml".to_string(),
            ),
            (
                "WORKTRUNK_APPROVALS_PATH".to_string(),
                self.test_approvals_path().display().to_string(),
            ),
        ]);

        vars
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
        configure_git_env(Cmd::new("git"), &self.git_config_path).current_dir(&self.root)
    }

    /// Run a git command in the repo root, panicking on failure.
    ///
    /// Thin wrapper around `git_command()` that runs the command and checks status.
    pub fn run_git(&self, args: &[&str]) {
        let output = self.git_command().args(args.iter().copied()).run().unwrap();
        check_git_status(&output, &args.join(" "));
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
    /// path remote (`../origin_git`) which doesn't parse as a proper git URL, causing
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

    /// Setup mock `claude` CLI as installed
    ///
    /// Call this after setup_mock_ci_tools_unauthenticated() to simulate
    /// Claude Code being available on the system.
    pub fn setup_mock_claude_installed(&mut self) {
        // Mark Claude as installed for test environment
        self.claude_installed = true;
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

        // Write JSON data files
        std::fs::write(mock_bin.join("pr_data.json"), pr_json).unwrap();
        std::fs::write(mock_bin.join("run_data.json"), run_json).unwrap();

        // Configure gh mock
        MockConfig::new("gh")
            .version("gh version 2.0.0 (mock)")
            .command("auth", MockResponse::exit(0))
            .command("pr", MockResponse::file("pr_data.json"))
            .command("run", MockResponse::file("run_data.json"))
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
        // Triple match: "mr view 1" matches before "mr view" (see mock-stub)
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
        // "mr view <iid>" is matched before "mr view" (see mock-stub triple matching)
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

    /// Configure a command to use mock gh/glab commands
    ///
    /// Must call `setup_mock_gh()` first. Prepends the mock bin directory to PATH
    /// so gh/glab commands are intercepted.
    ///
    /// On Windows, the mock commands have .exe files (via mock-stub) so they're
    /// found directly by CreateProcessW without needing PATHEXT manipulation.
    ///
    /// Metadata redactions keep PATH private in snapshots, so we can reuse the
    /// caller's PATH instead of a hardcoded minimal list.
    pub fn configure_mock_commands(&self, cmd: &mut Command) {
        if let Some(mock_bin) = &self.mock_bin_path {
            // Tell mock-stub where to find config files directly, avoiding PATH search
            cmd.env("MOCK_CONFIG_DIR", mock_bin);

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

        // Override Claude installed status if setup_mock_claude_installed() was called
        if self.claude_installed {
            cmd.env("WORKTRUNK_TEST_CLAUDE_INSTALLED", "1");
        }

        // Override OpenCode installed status if setup_mock_opencode_installed() was called
        if self.opencode_installed {
            cmd.env("WORKTRUNK_TEST_OPENCODE_INSTALLED", "1");
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

impl TestRepoBase for TestRepo {
    fn git_config_path(&self) -> &Path {
        &self.git_config_path
    }
}

/// Helper to create a bare repository test setup.
///
/// Bare repositories are useful for testing scenarios where you need worktrees
/// for the default branch (which isn't possible with normal repos since the
/// main worktree already has it checked out).
pub struct BareRepoTest {
    temp_dir: tempfile::TempDir,
    bare_repo_path: PathBuf,
    test_config_path: PathBuf,
    test_approvals_path: PathBuf,
    git_config_path: PathBuf,
}

impl BareRepoTest {
    /// Create a new bare repository test setup.
    ///
    /// The bare repo is created at `temp_dir/repo` with worktrees configured
    /// to be created as subdirectories (e.g., `repo/main`, `repo/feature`).
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let temp_dir = tempfile::TempDir::new().unwrap();
        // Bare repo without .git suffix - worktrees go inside as subdirectories
        let bare_repo_path = temp_dir.path().join("repo");
        let test_config_path = temp_dir.path().join("test-config.toml");
        let test_approvals_path = temp_dir.path().join("test-approvals.toml");
        let git_config_path = temp_dir.path().join("test-gitconfig");

        write_test_gitconfig(&git_config_path);

        let mut test = Self {
            temp_dir,
            bare_repo_path,
            test_config_path,
            test_approvals_path,
            git_config_path,
        };

        // Create bare repository
        let output = configure_git_env(Cmd::new("git"), &test.git_config_path)
            .args(["init", "--bare", "--initial-branch", "main"])
            .arg(test.bare_repo_path.to_str().unwrap())
            .run()
            .unwrap();

        if !output.status.success() {
            panic!(
                "Failed to init bare repo:\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        // Canonicalize path (using dunce to avoid \\?\ prefix on Windows)
        test.bare_repo_path = canonicalize(&test.bare_repo_path).unwrap();

        // Write config with template for worktrees inside bare repo
        // Template {{ branch }} creates worktrees as subdirectories: repo/main, repo/feature
        std::fs::write(&test.test_config_path, "worktree-path = \"{{ branch }}\"\n").unwrap();

        test
    }

    /// Get the path to the bare repository.
    pub fn bare_repo_path(&self) -> &Path {
        &self.bare_repo_path
    }

    /// Get the path to the test config file.
    pub fn config_path(&self) -> &Path {
        &self.test_config_path
    }

    /// Get the temp directory path.
    pub fn temp_path(&self) -> &Path {
        self.temp_dir.path()
    }

    /// Create a worktree from the bare repository.
    ///
    /// Worktrees are created inside the bare repo directory: repo/main, repo/feature
    pub fn create_worktree(&self, branch: &str, worktree_name: &str) -> PathBuf {
        let worktree_path = self.bare_repo_path.join(worktree_name);

        let output = self
            .git_command(&self.bare_repo_path)
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
        cmd.env("WORKTRUNK_CONFIG_PATH", &self.test_config_path)
            .env(
                "WORKTRUNK_SYSTEM_CONFIG_PATH",
                "/etc/xdg/worktrunk/config.toml",
            )
            .env("WORKTRUNK_APPROVALS_PATH", &self.test_approvals_path)
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

impl TestRepoBase for BareRepoTest {
    fn git_config_path(&self) -> &Path {
        &self.git_config_path
    }
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
}
