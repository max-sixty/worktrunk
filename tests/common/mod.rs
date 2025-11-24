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

// Test utilities are Unix-only since integration tests are Unix-only
#![cfg(unix)]

pub mod list_snapshots;
pub mod progressive_output;
pub mod shell;

use insta_cmd::get_cargo_bin;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

/// Create a `wt` CLI command with standardized test environment settings.
///
/// The command has the following guarantees:
/// - All host `GIT_*` and `WORKTRUNK_*` variables are cleared
/// - Color output is forced (`CLICOLOR_FORCE=1`) so ANSI styling appears in snapshots
/// - Terminal width defaults to 150 columns when `COLUMNS` is not already set
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
    let index = words.len().saturating_sub(1);
    cmd.arg("--");
    cmd.args(words);
    cmd.env("COMPLETE", "bash");
    cmd.env("_CLAP_COMPLETE_INDEX", index.to_string());
    cmd.env("_CLAP_COMPLETE_COMP_TYPE", "9"); // normal completion
    cmd.env("_CLAP_COMPLETE_SPACE", "true");
    cmd.env("_CLAP_IFS", "\n");
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
    // Set to non-existent path to prevent loading user's real config
    // Tests that need config should use TestRepo::clean_cli_env() which overrides this
    cmd.env("WORKTRUNK_CONFIG_PATH", "/nonexistent/test/config.toml");
    cmd.env("CLICOLOR_FORCE", "1");
    if std::env::var("COLUMNS").is_err() {
        cmd.env("COLUMNS", "150");
    }
}

/// Set `HOME` and `XDG_CONFIG_HOME` for commands that rely on isolated temp homes.
pub fn set_temp_home_env(cmd: &mut Command, home: &Path) {
    cmd.env("HOME", home);
    cmd.env("XDG_CONFIG_HOME", home.join(".config"));
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
    /// Create a new test repository with isolated git environment
    pub fn new() -> Self {
        let temp_dir = TempDir::new().unwrap();
        // Create main repo as a subdirectory so worktrees can be siblings
        let root = temp_dir.path().join("test-repo");
        std::fs::create_dir(&root).unwrap();
        // Canonicalize to resolve symlinks (important on macOS where /var is symlink to /private/var)
        let root = root.canonicalize().unwrap();

        // Create isolated config path for this test
        let test_config_path = temp_dir.path().join("test-config.toml");

        // Create git config file with test settings
        let git_config_path = temp_dir.path().join("test-gitconfig");
        std::fs::write(
            &git_config_path,
            "[advice]\n\tmergeConflict = false\n\tresolveConflict = false\n",
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

        // Initialize git repo with isolated environment
        let mut cmd = Command::new("git");
        repo.configure_git_cmd(&mut cmd);
        cmd.args(["init", "-b", "main"])
            .current_dir(&repo.root)
            .output()
            .unwrap();

        // Configure git user
        let mut cmd = Command::new("git");
        repo.configure_git_cmd(&mut cmd);
        cmd.args(["config", "user.name", "Test User"])
            .current_dir(&repo.root)
            .output()
            .unwrap();

        let mut cmd = Command::new("git");
        repo.configure_git_cmd(&mut cmd);
        cmd.args(["config", "user.email", "test@example.com"])
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
        cmd.env("GIT_CONFIG_SYSTEM", "/dev/null");
        cmd.env("GIT_AUTHOR_DATE", "2025-01-01T00:00:00Z");
        cmd.env("GIT_COMMITTER_DATE", "2025-01-01T00:00:00Z");
        cmd.env("LC_ALL", "C");
        cmd.env("LANG", "C");
        // Oct 28, 2025 - exactly 300 days (10 months) after commit date for deterministic relative times
        cmd.env("SOURCE_DATE_EPOCH", "1761609600");
    }

    /// Get standard test environment variables as a vector
    ///
    /// This is useful for PTY tests and other cases where you need environment variables
    /// as a vector rather than setting them on a Command.
    pub fn test_env_vars(&self) -> Vec<(String, String)> {
        vec![
            ("CLICOLOR_FORCE".to_string(), "1".to_string()),
            ("COLUMNS".to_string(), "150".to_string()),
            // Use test git config file with advice settings disabled
            (
                "GIT_CONFIG_GLOBAL".to_string(),
                self.git_config_path.display().to_string(),
            ),
            ("GIT_CONFIG_SYSTEM".to_string(), "/dev/null".to_string()),
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
            ("SOURCE_DATE_EPOCH".to_string(), "1761609600".to_string()),
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

    /// Clean environment for worktrunk CLI commands
    ///
    /// Removes potentially interfering environment variables and sets
    /// deterministic git environment for CLI tests.
    ///
    /// This also sets `WORKTRUNK_CONFIG_PATH` to an isolated test config
    /// to prevent tests from polluting the user's real config file.
    pub fn clean_cli_env(&self, cmd: &mut Command) {
        configure_cli_command(cmd);
        self.configure_git_cmd(cmd);
        // Set isolated config path to prevent polluting user's config
        cmd.env("WORKTRUNK_CONFIG_PATH", &self.test_config_path);
        // Set consistent terminal width for stable snapshot output
        // (can be overridden by individual tests that want to test specific widths)
        // NOTE: We don't set PATH here. Tests inherit PATH from the test runner,
        // which allows them to find git, shells, etc. Since we don't explicitly set it,
        // insta-cmd won't capture it in snapshots, avoiding privacy leaks.
    }

    /// Prepare a `wt` command configured for shell completions within this repo.
    pub fn completion_cmd(&self, words: &[&str]) -> Command {
        let mut cmd = wt_completion_command(words);
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
        // Create a unique file to ensure there's something to commit
        use std::time::{SystemTime, UNIX_EPOCH};
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let file_path = self.root.join(format!("file-{}.txt", timestamp));
        std::fs::write(&file_path, "content").unwrap();

        self.git_command(&["add", "."]).output().unwrap();

        self.git_command(&["commit", "-m", message])
            .output()
            .unwrap();
    }

    /// Add a worktree with the given name and branch
    pub fn add_worktree(&mut self, name: &str, branch: &str) -> PathBuf {
        // Create worktree inside temp directory to ensure cleanup
        let worktree_path = self.temp_dir.path().join(name);

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
        let canonical_path = worktree_path.canonicalize().unwrap();
        self.worktrees
            .insert(name.to_string(), canonical_path.clone());
        canonical_path
    }

    /// Creates a worktree for the main branch (required for merge operations)
    ///
    /// This is a convenience method that creates a worktree for the main branch
    /// in the standard location expected by merge tests. Returns the path to the
    /// created worktree.
    pub fn add_main_worktree(&self) -> PathBuf {
        let main_wt = self.root_path().parent().unwrap().join("test-repo.main-wt");
        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        cmd.args(["worktree", "add", main_wt.to_str().unwrap(), "main"])
            .current_dir(self.root_path())
            .output()
            .unwrap();
        main_wt
    }

    /// Detach HEAD in the repository
    pub fn detach_head(&self) {
        // Get current commit SHA
        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        let output = cmd
            .args(["rev-parse", "HEAD"])
            .current_dir(&self.root)
            .output()
            .unwrap();

        let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();

        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        cmd.args(["checkout", "--detach", &sha])
            .current_dir(&self.root)
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
        let remote_path = remote_path.canonicalize().unwrap();

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
    /// - `gh pr view`: exits with error (no PR found) - fails fast
    /// - `gh run list`: exits with error (no runs found) - fails fast
    ///
    /// This prevents CI detection from blocking tests with network calls.
    pub fn setup_mock_gh(&mut self) {
        let mock_bin = self.temp_dir.path().join("mock-bin");
        std::fs::create_dir(&mock_bin).unwrap();

        // Create mock gh script
        let gh_script = mock_bin.join("gh");
        std::fs::write(
            &gh_script,
            r#"#!/bin/sh
# Mock gh command that fails fast without network calls

case "$1" in
    auth)
        # gh auth status - succeed immediately
        exit 0
        ;;
    pr|run)
        # gh pr view / gh run list - fail fast (no PR/runs found)
        exit 1
        ;;
    *)
        # Unknown command - fail
        exit 1
        ;;
esac
"#,
        )
        .unwrap();

        // Make executable
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&gh_script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        // Create mock glab script (fails immediately)
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

        self.mock_bin_path = Some(mock_bin);
    }

    /// Configure a command to use mock gh/glab commands
    ///
    /// Must call `setup_mock_gh()` first. Prepends the mock bin directory to PATH
    /// so gh/glab commands are intercepted.
    ///
    /// Metadata redactions keep PATH private in snapshots, so we can reuse the
    /// caller's PATH instead of a hardcoded minimal list.
    pub fn configure_mock_commands(&self, cmd: &mut Command) {
        if let Some(mock_bin) = &self.mock_bin_path {
            let mut paths: Vec<PathBuf> = std::env::var_os("PATH")
                .as_deref()
                .map(|p| std::env::split_paths(p).collect())
                .unwrap_or_default();
            paths.insert(0, mock_bin.clone());
            let new_path = std::env::join_paths(paths).unwrap();
            cmd.env("PATH", new_path);
        }
    }
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
        .and_then(|p| std::path::PathBuf::from(p).canonicalize().ok());
    if let Some(root) = project_root {
        settings.add_filter(&regex::escape(root.to_str().unwrap()), "[PROJECT_ROOT]");
    }

    // Normalize paths (escape for regex to handle Windows backslashes)
    settings.add_filter(&regex::escape(repo.root_path().to_str().unwrap()), "[REPO]");
    for (name, path) in &repo.worktrees {
        settings.add_filter(
            &regex::escape(path.to_str().unwrap()),
            format!("[WORKTREE_{}]", name.to_uppercase().replace('-', "_")),
        );
    }

    // Normalize git SHAs and backslashes
    // First filter SHAs wrapped in ANSI color codes (more specific pattern)
    // Match: ESC[COLORmSHAESC[RESETm where RESET can be empty, 0, or other codes
    // Examples: \x1b[33m0b07a58\x1b[m or \x1b[2m0b07a58\x1b[0m
    settings.add_filter(r"\x1b\[[0-9;]*m[0-9a-f]{7,40}\x1b\[[0-9;]*m", "[SHA]");
    // Then filter plain SHAs (more general pattern)
    settings.add_filter(r"\b[0-9a-f]{7,40}\b", "[SHA]");
    settings.add_filter(r"\\", "/");

    // Normalize temp directory paths in project identifiers (approval prompts)
    // Example: /private/var/folders/wf/.../T/.tmpABC123/origin -> [PROJECT_ID]
    settings.add_filter(
        r"/private/var/folders/[^/]+/[^/]+/T/\.[^/]+/[^)]+",
        "[PROJECT_ID]",
    );

    // Normalize WORKTRUNK_CONFIG_PATH temp paths in stdout/stderr output
    // (metadata is handled via redactions below)
    settings.add_filter(r".*/\.tmp[^/]+/test-config\.toml", "[TEST_CONFIG]");

    // Normalize GIT_CONFIG_GLOBAL temp paths
    settings.add_filter(r".*/\.tmp[^/]+/test-gitconfig", "[TEST_GIT_CONFIG]");

    // Normalize HOME temp directory in snapshots (stdout/stderr content)
    // Matches any temp directory path (without trailing filename)
    // Examples:
    //   macOS: HOME: /var/folders/.../T/.tmpXXX
    //   Linux: HOME: /tmp/.tmpXXX
    //   Windows: HOME: C:\Users\...\Temp\.tmpXXX (after backslash normalization)
    settings.add_filter(r"HOME: .*/\.tmp[^/\s]+", "HOME: [TEST_HOME]");

    // Redact volatile metadata captured by insta-cmd (applies to the `info` block)
    settings.add_redaction(".env.GIT_CONFIG_GLOBAL", "[TEST_GIT_CONFIG]");
    settings.add_redaction(".env.WORKTRUNK_CONFIG_PATH", "[TEST_CONFIG]");
    settings.add_redaction(".env.HOME", "[TEST_HOME]");
    settings.add_redaction(".env.XDG_CONFIG_HOME", "[TEST_CONFIG_HOME]");
    settings.add_redaction(".env.PATH", "[PATH]");

    // Normalize timestamps in log filenames (format: YYYYMMDD-HHMMSS)
    // The SHA filter runs first, so we match: post-start-NAME-[SHA]-HHMMSS.log
    settings.add_filter(
        r"post-start-[^-]+-\[SHA\]-\d{6}\.log",
        "post-start-[NAME]-[TIMESTAMP].log",
    );

    // Filter out Git hint messages that vary across Git versions
    // These hints appear during rebase conflicts and can differ between versions
    // Pattern matches lines with gutter formatting + "hint:" + message + newline
    // The gutter is: ESC[40m ESC[0m followed by spaces
    settings.add_filter(r"(?m)^\x1b\[40m \x1b\[0m {1,2}hint:.*\n", "");

    // Normalize Git error message format differences across versions
    // Older Git (< 2.43): "Could not apply [SHA]... # commit message"
    // Newer Git (>= 2.43): "Could not apply [SHA]... commit message"
    // Add the "# " prefix to newer Git output for consistency with snapshots
    // Match if followed by a letter/character (not "#")
    settings.add_filter(r"(Could not apply \[SHA\]\.\.\.) ([A-Za-z])", "$1 # $2");

    settings
}

/// Create configured insta Settings for snapshot tests with a temporary home directory
///
/// This extends `setup_snapshot_settings` by adding a filter for the temporary home directory.
/// Use this for tests that need both a TestRepo and a temporary home (for user config testing).
pub fn setup_snapshot_settings_with_home(repo: &TestRepo, temp_home: &TempDir) -> insta::Settings {
    let mut settings = setup_snapshot_settings(repo);
    settings.add_filter(&temp_home.path().to_string_lossy(), "[TEMP_HOME]");
    settings
}

/// Create configured insta Settings for snapshot tests with only a temporary home directory
///
/// Use this for tests that don't need a TestRepo but do need a temporary home directory
/// (e.g., shell configuration tests, config init tests).
pub fn setup_home_snapshot_settings(temp_home: &TempDir) -> insta::Settings {
    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    settings.add_filter(&temp_home.path().to_string_lossy(), "[TEMP_HOME]");
    settings.add_filter(r"\\", "/");
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

    // Redact volatile metadata captured by insta-cmd
    settings.add_redaction(".env.GIT_CONFIG_GLOBAL", "[TEST_GIT_CONFIG]");
    settings.add_redaction(".env.WORKTRUNK_CONFIG_PATH", "[TEST_CONFIG]");

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

#[cfg(test)]
mod tests {
    use super::*;

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
