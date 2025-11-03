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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

pub struct TestRepo {
    temp_dir: TempDir, // Must keep to ensure cleanup on drop
    root: PathBuf,
    pub worktrees: HashMap<String, PathBuf>,
    remote: Option<PathBuf>, // Path to bare remote repo if created
    /// Isolated config file for this test (prevents pollution of user's config)
    test_config_path: PathBuf,
}

impl TestRepo {
    /// Create a new test repository with isolated git environment
    pub fn new() -> Self {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        // Create main repo as a subdirectory so worktrees can be siblings
        let root = temp_dir.path().join("test-repo");
        std::fs::create_dir(&root).expect("Failed to create main repo directory");
        // Canonicalize to resolve symlinks (important on macOS where /var is symlink to /private/var)
        let root = root
            .canonicalize()
            .expect("Failed to canonicalize temp path");

        // Create isolated config path for this test
        let test_config_path = temp_dir.path().join("test-config.toml");

        let repo = Self {
            temp_dir,
            root,
            worktrees: HashMap::new(),
            remote: None,
            test_config_path,
        };

        // Initialize git repo with isolated environment
        let mut cmd = Command::new("git");
        repo.configure_git_cmd(&mut cmd);
        cmd.args(["init", "-b", "main"])
            .current_dir(&repo.root)
            .output()
            .expect("Failed to init git repo");

        // Configure git user
        let mut cmd = Command::new("git");
        repo.configure_git_cmd(&mut cmd);
        cmd.args(["config", "user.name", "Test User"])
            .current_dir(&repo.root)
            .output()
            .expect("Failed to set user.name");

        let mut cmd = Command::new("git");
        repo.configure_git_cmd(&mut cmd);
        cmd.args(["config", "user.email", "test@example.com"])
            .current_dir(&repo.root)
            .output()
            .expect("Failed to set user.email");

        repo
    }

    /// Configure a git command with isolated environment
    ///
    /// This sets environment variables only for the specific command,
    /// ensuring thread-safety and test isolation.
    pub fn configure_git_cmd(&self, cmd: &mut Command) {
        cmd.env("GIT_CONFIG_GLOBAL", "/dev/null");
        cmd.env("GIT_CONFIG_SYSTEM", "/dev/null");
        cmd.env("GIT_AUTHOR_DATE", "2025-01-01T00:00:00Z");
        cmd.env("GIT_COMMITTER_DATE", "2025-01-01T00:00:00Z");
        cmd.env("LC_ALL", "C");
        cmd.env("LANG", "C");
        // Oct 28, 2025 - exactly 300 days (10 months) after commit date for deterministic relative times
        cmd.env("SOURCE_DATE_EPOCH", "1761609600");
    }

    /// Clean environment for worktrunk CLI commands
    ///
    /// Removes potentially interfering environment variables and sets
    /// deterministic git environment for CLI tests.
    ///
    /// This also sets `WORKTRUNK_CONFIG_PATH` to an isolated test config
    /// to prevent tests from polluting the user's real config file.
    pub fn clean_cli_env(&self, cmd: &mut Command) {
        // Remove git-related env vars that might interfere
        for (key, _) in std::env::vars() {
            if key.starts_with("GIT_") || key.starts_with("WORKTRUNK_") {
                cmd.env_remove(&key);
            }
        }
        // Set deterministic environment for git
        self.configure_git_cmd(cmd);
        // Force color output for snapshot testing (captures ANSI codes)
        cmd.env("CLICOLOR_FORCE", "1");
        // Set isolated config path to prevent polluting user's config
        cmd.env("WORKTRUNK_CONFIG_PATH", &self.test_config_path);
        // Set consistent terminal width for stable snapshot output
        // (can be overridden by individual tests that want to test specific widths)
        if std::env::var("COLUMNS").is_err() {
            cmd.env("COLUMNS", "150");
        }
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

    /// Get the path to a named worktree
    pub fn worktree_path(&self, name: &str) -> &Path {
        self.worktrees
            .get(name)
            .unwrap_or_else(|| panic!("Worktree '{}' not found", name))
    }

    /// Read a file from the repo root
    #[allow(dead_code)]
    pub fn read_file(&self, path: &str) -> String {
        std::fs::read_to_string(self.root.join(path))
            .unwrap_or_else(|_| panic!("Failed to read {}", path))
    }

    /// List all files in the repository (excluding .git)
    #[allow(dead_code)]
    pub fn file_tree(&self) -> Vec<String> {
        let mut files = Vec::new();
        Self::collect_files(&self.root, "", &mut files);
        files.sort();
        files
    }

    #[allow(dead_code)]
    fn collect_files(dir: &Path, prefix: &str, files: &mut Vec<String>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name();

                // Skip .git directory
                if name == ".git" {
                    continue;
                }

                let display_name = if prefix.is_empty() {
                    name.to_string_lossy().to_string()
                } else {
                    format!("{}/{}", prefix, name.to_string_lossy())
                };

                if path.is_dir() {
                    Self::collect_files(&path, &display_name, files);
                } else {
                    files.push(display_name);
                }
            }
        }
    }

    /// Create a commit with the given message
    pub fn commit(&self, message: &str) {
        // Create a file to ensure there's something to commit
        let file_path = self.root.join("file.txt");
        std::fs::write(&file_path, message).expect("Failed to write file");

        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        cmd.args(["add", "."])
            .current_dir(&self.root)
            .output()
            .expect("Failed to git add");

        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        cmd.args(["commit", "-m", message])
            .current_dir(&self.root)
            .output()
            .expect("Failed to git commit");
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
        std::fs::write(&file_path, "content").expect("Failed to write file");

        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        cmd.args(["add", "."])
            .current_dir(&self.root)
            .output()
            .expect("Failed to git add");

        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        cmd.args(["commit", "-m", message])
            .current_dir(&self.root)
            .output()
            .expect("Failed to git commit");
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
            .expect("Failed to execute git worktree add");

        if !output.status.success() {
            panic!(
                "Failed to add worktree:\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        // Canonicalize worktree path to match what git returns
        let canonical_path = worktree_path
            .canonicalize()
            .expect("Failed to canonicalize worktree path");
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
            .expect("Failed to add worktree");
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
            .expect("Failed to get HEAD SHA");

        let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();

        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        cmd.args(["checkout", "--detach", &sha])
            .current_dir(&self.root)
            .output()
            .expect("Failed to detach HEAD");
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
        cmd.args(&args)
            .current_dir(&self.root)
            .output()
            .expect("Failed to lock worktree");
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
        std::fs::create_dir(&remote_path).expect("Failed to create remote directory");

        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        cmd.args(["init", "--bare", "--initial-branch", default_branch])
            .current_dir(&remote_path)
            .output()
            .expect("Failed to init bare remote");

        // Canonicalize remote path
        let remote_path = remote_path
            .canonicalize()
            .expect("Failed to canonicalize remote path");

        // Add as remote
        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        cmd.args(["remote", "add", remote_name, remote_path.to_str().unwrap()])
            .current_dir(&self.root)
            .output()
            .expect("Failed to add remote");

        // Push current branch to remote
        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        cmd.args(["push", "-u", remote_name, default_branch])
            .current_dir(&self.root)
            .output()
            .expect("Failed to push to remote");

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
            .expect("Failed to clear origin/HEAD");
    }

    /// Get the path to the remote repository if one was set up
    #[allow(dead_code)]
    pub fn remote_path(&self) -> Option<&Path> {
        self.remote.as_deref()
    }

    /// Check if origin/HEAD is set
    pub fn has_origin_head(&self) -> bool {
        let mut cmd = Command::new("git");
        self.configure_git_cmd(&mut cmd);
        let output = cmd
            .args(["rev-parse", "--abbrev-ref", "origin/HEAD"])
            .current_dir(&self.root)
            .output()
            .expect("Failed to check origin/HEAD");
        output.status.success()
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
        settings.add_filter(root.to_str().unwrap(), "[PROJECT_ROOT]");
    }

    // Normalize paths
    settings.add_filter(repo.root_path().to_str().unwrap(), "[REPO]");
    for (name, path) in &repo.worktrees {
        settings.add_filter(
            path.to_str().unwrap(),
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
    // NOTE: This filter only applies to output content, not the info/env section.
    // The env section paths cannot be normalized due to insta-cmd architecture
    // (it calls set_info() which bypasses filters). This means env paths will vary
    // between test runs, but this doesn't affect test functionality.
    settings.add_filter(r".*/\.tmp[^/]+/test-config\.toml", "[TEST_CONFIG]");

    // Normalize HOME temp directory in snapshots
    // Matches any temp directory path (without trailing filename)
    // Examples:
    //   macOS: HOME: /var/folders/.../T/.tmpXXX
    //   Linux: HOME: /tmp/.tmpXXX
    //   Windows: HOME: C:\Users\...\Temp\.tmpXXX (after backslash normalization)
    settings.add_filter(r"HOME: .*/\.tmp[^/\s]+", "HOME: [TEST_HOME]");

    // Normalize timestamps in log filenames (format: YYYYMMDD-HHMMSS)
    // The SHA filter runs first, so we match: post-start-NAME-[SHA]-HHMMSS.log
    settings.add_filter(
        r"post-start-[^-]+-\[SHA\]-\d{6}\.log",
        "post-start-[NAME]-[TIMESTAMP].log",
    );

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

/// Run a command and capture combined stdout+stderr output for snapshot testing
///
/// This mimics real terminal usage where stdout and stderr are interleaved (like `2>&1`).
/// Returns the combined output as a String with exit code information.
pub fn run_with_combined_output(
    repo: &TestRepo,
    subcommand: &str,
    args: &[&str],
    cwd: Option<&Path>,
) -> String {
    use std::process::Stdio;

    // Build the command with all arguments
    let wt_bin = insta_cmd::get_cargo_bin("wt");
    let mut cmd_parts = vec![wt_bin.to_str().unwrap(), subcommand];
    cmd_parts.extend(args.iter());

    // Run through shell with 2>&1 to get true interleaved output
    let cmd_str = cmd_parts.join(" ");
    let shell_cmd = format!("{} 2>&1", cmd_str);

    let mut cmd = Command::new("sh");
    repo.clean_cli_env(&mut cmd);
    cmd.arg("-c")
        .arg(&shell_cmd)
        .current_dir(cwd.unwrap_or(repo.root_path()))
        .stdout(Stdio::piped());

    let output = cmd.output().expect("Failed to execute command");
    let combined = String::from_utf8_lossy(&output.stdout).to_string();

    format!(
        "Exit code: {}\n{}",
        output.status.code().unwrap_or(-1),
        combined
    )
}

/// Resolve the actual git directory path from a worktree path
///
/// In worktrees, `.git` is a file containing `gitdir: /path/to/git/dir`,
/// not a directory. This helper reads that file and returns the actual
/// git directory path.
///
/// # Arguments
/// * `worktree_path` - Path to the worktree root
///
/// # Returns
/// The resolved git directory path
pub fn resolve_git_dir(worktree_path: &Path) -> PathBuf {
    let git_path = worktree_path.join(".git");

    if git_path.is_file() {
        // Read the gitdir path from the file
        let content = std::fs::read_to_string(&git_path).expect("Failed to read .git file");

        // Format is "gitdir: /path/to/git/dir"
        let gitdir_path = content
            .trim()
            .strip_prefix("gitdir: ")
            .expect("Invalid .git file format");

        PathBuf::from(gitdir_path)
    } else {
        // Not a worktree, .git is already a directory
        git_path
    }
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
