//! Shell wrapper integration tests
//!
//! Tests that verify the complete shell integration path - commands executed through
//! the actual shell wrapper (wt_exec in bash/zsh/fish).
//!
//! These tests ensure that:
//! - Directives are never leaked to users
//! - Output is properly formatted for humans
//! - Shell integration works end-to-end as users experience it
//!
//! ## Why Manual PTY Execution + File Snapshots (Not insta_cmd)?
//!
//! These tests use a pattern that might seem redundant at first glance:
//! - Manual command execution through PTY (`exec_in_pty`)
//! - Manual output normalization (`normalized()`)
//! - File snapshots via `assert_snapshot!(output.normalized())`
//!
//! This is the correct approach because:
//!
//! 1. **PTY execution is required** - Testing shell wrappers requires real TTY behavior
//!    (streaming output, ANSI codes, signal handling). `insta_cmd` uses `std::process::Command`
//!    which doesn't provide a TTY to child processes.
//!
//! 2. **File snapshots are appropriate** - The output contains ANSI escape codes and complex
//!    formatting. File snapshots keep these out of source files (unlike inline snapshots).
//!
//! 3. **Full output is valuable** - While specific assertions verify critical properties
//!    (no directive leaks, correct exit codes), file snapshots make it easy for humans to
//!    see the complete user experience at a glance.
//!
//! In summary: This isn't a case of "should use insta_cmd instead" - the manual execution
//! is necessary, and file snapshots are the right storage format for escape-code-heavy output.

use crate::common::TestRepo;
use insta::assert_snapshot;
use insta_cmd::get_cargo_bin;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::LazyLock;

/// Regex for normalizing temporary directory paths in test snapshots
static TMPDIR_REGEX: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(/private/var/folders/[^/]+/[^/]+/T/\.tmp[^\s/'"]+|/tmp/\.(?:tmp|psub)[^\s/'"]+)"#,
    )
    .expect("Invalid tmpdir regex pattern")
});

/// Regex that collapses repeated TMPDIR placeholders (caused by nested mktemp paths)
/// so `[TMPDIR][TMPDIR]/foo` becomes `[TMPDIR]/foo` and `[TMPDIR]/[TMPDIR]` becomes `[TMPDIR]`
static TMPDIR_PLACEHOLDER_COLLAPSE_REGEX: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\[TMPDIR](?:/?\[TMPDIR])+").expect("Invalid TMPDIR placeholder regex")
});

/// Regex for normalizing workspace paths (dynamically built from CARGO_MANIFEST_DIR)
/// Matches: <project_root>/tests/fixtures/
/// Replaces with: [WORKSPACE]/tests/fixtures/
static WORKSPACE_REGEX: LazyLock<regex::Regex> = LazyLock::new(|| {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let pattern = format!(r"{}/tests/fixtures/", regex::escape(manifest_dir));
    regex::Regex::new(&pattern).expect("Invalid workspace regex pattern")
});

/// Regex for normalizing git commit hashes (7-character hex)
/// Matches 7-character lowercase hex sequences (git short hashes)
/// Note: No word boundaries because ANSI codes (ending with 'm') directly precede hashes
static COMMIT_HASH_REGEX: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[0-9a-f]{7}").expect("Invalid commit hash regex pattern"));

/// Output from executing a command through a shell wrapper
#[derive(Debug)]
struct ShellOutput {
    /// Combined stdout and stderr as user would see
    combined: String,
    /// Exit code from the command
    exit_code: i32,
}

impl ShellOutput {
    /// Check if output contains no directive leaks
    fn assert_no_directive_leaks(&self) {
        assert!(
            !self.combined.contains("__WORKTRUNK_CD__"),
            "Output contains leaked __WORKTRUNK_CD__ directive:\n{}",
            self.combined
        );
        assert!(
            !self.combined.contains("__WORKTRUNK_EXEC__"),
            "Output contains leaked __WORKTRUNK_EXEC__ directive:\n{}",
            self.combined
        );
    }

    /// Normalize paths and ANSI codes in output for snapshot testing
    fn normalized(&self) -> String {
        // First normalize temporary directory paths
        let tmpdir_normalized = TMPDIR_REGEX.replace_all(&self.combined, "[TMPDIR]");

        // Then normalize workspace paths (varying directory names)
        let workspace_normalized =
            WORKSPACE_REGEX.replace_all(&tmpdir_normalized, "[WORKSPACE]/tests/fixtures/");

        // Normalize commit hashes (7-character hex strings)
        let hash_normalized = COMMIT_HASH_REGEX.replace_all(&workspace_normalized, "[HASH]");

        // Collapse duplicate TMPDIR placeholders that can appear with nested mktemp paths.
        let tmpdir_collapsed =
            TMPDIR_PLACEHOLDER_COLLAPSE_REGEX.replace_all(&hash_normalized, "[TMPDIR]");

        // Then normalize ANSI codes: remove redundant leading reset codes
        // This handles differences between macOS and Linux PTY ANSI generation
        let has_trailing_newline = tmpdir_collapsed.ends_with('\n');
        let mut result = tmpdir_collapsed
            .lines()
            .map(|line| {
                // Strip leading \x1b[0m reset codes (may appear as ESC[0m in the output)
                line.strip_prefix("\x1b[0m").unwrap_or(line)
            })
            .collect::<Vec<_>>()
            .join("\n");

        // Preserve trailing newline if it existed
        if has_trailing_newline {
            result.push('\n');
        }
        result
    }
}

/// Generate a shell wrapper script using the actual `wt config shell init` command
fn generate_wrapper(repo: &TestRepo, shell: &str) -> String {
    let wt_bin = get_cargo_bin("wt");

    let mut cmd = Command::new(&wt_bin);
    cmd.arg("config").arg("shell").arg("init").arg(shell);

    // Configure environment
    repo.clean_cli_env(&mut cmd);

    let output = cmd
        .output()
        .unwrap_or_else(|_| panic!("Failed to run wt config shell init {}", shell));

    if !output.status.success() {
        panic!(
            "wt config shell init {} failed with exit code: {:?}\nOutput:\n{}",
            shell,
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    String::from_utf8(output.stdout)
        .unwrap_or_else(|_| panic!("wt config shell init {} produced invalid UTF-8", shell))
}

/// Quote a shell argument if it contains special characters
fn quote_arg(arg: &str) -> String {
    if arg.contains(' ') || arg.contains(';') || arg.contains('\'') {
        format!("'{}'", arg.replace('\'', "'\\''"))
    } else {
        arg.to_string()
    }
}

/// Build a shell script that sources the wrapper and runs a command
fn build_shell_script(shell: &str, repo: &TestRepo, subcommand: &str, args: &[&str]) -> String {
    let wt_bin = get_cargo_bin("wt");
    let wrapper_script = generate_wrapper(repo, shell);
    let mut script = String::new();

    // Set environment variables - syntax varies by shell
    // Don't use 'set -e' in bash/zsh - we want to capture failures and their exit codes.
    // This is tested by test_wrapper_handles_command_failure which verifies
    // that command failures return proper exit codes rather than aborting the script.
    match shell {
        "fish" => {
            script.push_str(&format!("set -x WORKTRUNK_BIN '{}'\n", wt_bin.display()));
            script.push_str(&format!(
                "set -x WORKTRUNK_CONFIG_PATH '{}'\n",
                repo.test_config_path().display()
            ));
            script.push_str("set -x CLICOLOR_FORCE 1\n");
        }
        "zsh" => {
            // For zsh, initialize the completion system first
            // This allows static completions (which call compdef) to work in isolated mode
            // We run with --no-rcs to prevent user rc files from touching /dev/tty,
            // but compinit is safe since it only sets up completion functions
            script.push_str("autoload -Uz compinit && compinit -i 2>/dev/null\n");

            script.push_str(&format!("export WORKTRUNK_BIN='{}'\n", wt_bin.display()));
            script.push_str(&format!(
                "export WORKTRUNK_CONFIG_PATH='{}'\n",
                repo.test_config_path().display()
            ));
            script.push_str("export CLICOLOR_FORCE=1\n");
        }
        _ => {
            // bash
            script.push_str(&format!("export WORKTRUNK_BIN='{}'\n", wt_bin.display()));
            script.push_str(&format!(
                "export WORKTRUNK_CONFIG_PATH='{}'\n",
                repo.test_config_path().display()
            ));
            script.push_str("export CLICOLOR_FORCE=1\n");
        }
    }

    // Source the wrapper
    script.push_str(&wrapper_script);
    script.push('\n');

    // Build the command
    script.push_str("wt ");
    script.push_str(subcommand);
    for arg in args {
        script.push(' ');
        script.push_str(&quote_arg(arg));
    }
    script.push('\n');

    // Merge stderr to stdout to simulate real terminal behavior
    // In a real terminal, both streams interleave naturally by the OS.
    // The .output() method captures them separately, so we merge them here
    // to preserve temporal locality (output appears when operations complete, not batched at the end)
    match shell {
        "fish" => {
            // Fish uses begin...end for grouping
            // Note: This exposes a Fish wrapper buffering bug where child output appears out of order
            // (see templates/fish.fish - psub causes buffering). Tests document current behavior.
            format!("begin\n{}\nend 2>&1", script)
        }
        _ => {
            // bash/zsh use parentheses for subshell grouping
            format!("( {} ) 2>&1", script)
        }
    }
}

/// Normalize line endings (CRLF -> LF)
fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n")
}

/// Execute a command in a PTY with interactive input support
///
/// This is similar to `exec_in_pty` but allows sending input during execution.
/// The PTY will automatically echo the input (like a real terminal), so you'll
/// see both the prompts and the input in the captured output.
///
/// # Arguments
/// * `shell` - The shell to use (e.g., "bash", "zsh")
/// * `script` - The script to execute
/// * `working_dir` - Working directory for the command
/// * `env_vars` - Environment variables to set
/// * `inputs` - A slice of strings to send as input (e.g., `&["y\n", "feature\n"]`)
///
/// # Example
/// ```no_run
/// let (output, exit_code) = exec_in_pty_interactive(
///     "bash",
///     "wt switch --create",
///     repo.root_path(),
///     &[("CLICOLOR_FORCE", "1")],
///     &["y\n"],  // Send 'y' and newline when prompted
/// );
/// // The output will show: "Allow? [y/N] y"
/// ```
#[cfg(test)]
fn exec_in_pty_interactive(
    shell: &str,
    script: &str,
    working_dir: &std::path::Path,
    env_vars: &[(&str, &str)],
    inputs: &[&str],
) -> (String, i32) {
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};
    use std::io::{Read, Write};

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 48,
            cols: 200,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("Failed to open PTY");

    // Spawn the shell inside the PTY
    let mut cmd = CommandBuilder::new(shell);

    // Clear inherited environment for test isolation
    cmd.env_clear();

    // Set minimal required environment for shells to function
    cmd.env(
        "HOME",
        std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()),
    );
    cmd.env(
        "PATH",
        std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string()),
    );
    cmd.env("USER", "testuser");
    cmd.env("SHELL", shell);

    // For zsh, isolate from user rc files
    // Note: We no longer pass NO_MONITOR here - job control notifications are now
    // suppressed by the shell integration code itself (setopt LOCAL_OPTIONS NO_MONITOR
    // in wt_exec). This ensures tests match real user experience.
    if shell == "zsh" {
        cmd.env("ZDOTDIR", "/dev/null");
        cmd.arg("--no-rcs");
        cmd.arg("-o");
        cmd.arg("NO_GLOBAL_RCS");
        cmd.arg("-o");
        cmd.arg("NO_RCS");
    }

    cmd.arg("-c");
    cmd.arg(script);
    cmd.cwd(working_dir);

    // Add test-specific environment variables (convert &str tuples to String tuples)
    for (key, value) in env_vars {
        cmd.env(key, value);
    }

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .expect("Failed to spawn command in PTY");
    drop(pair.slave); // Close slave in parent

    // Clone the reader for capturing output
    let mut reader = pair
        .master
        .try_clone_reader()
        .expect("Failed to clone PTY reader");

    // Write input synchronously if we have any (matches approval_pty.rs approach)
    if !inputs.is_empty() {
        let mut writer = pair.master.take_writer().expect("Failed to get PTY writer");
        for input in inputs {
            writer
                .write_all(input.as_bytes())
                .expect("Failed to write input to PTY");
            writer.flush().expect("Failed to flush PTY writer");
        }
        drop(writer); // Explicitly drop writer so PTY sees EOF
    }

    // Read everything the "terminal" would display (including echoed input)
    let mut buf = String::new();
    reader
        .read_to_string(&mut buf)
        .expect("Failed to read from PTY"); // Blocks until child exits & PTY closes

    let status = child.wait().expect("Failed to wait for child process");

    (normalize_newlines(&buf), status.exit_code() as i32)
}

/// Execute a command through a shell wrapper
///
/// This simulates what actually happens when users run `wt switch`, etc. in their shell:
/// 1. The `wt` function is defined (from shell integration)
/// 2. It calls `wt_exec --internal switch ...`
/// 3. The wrapper parses NUL-delimited output and handles directives
/// 4. Users see only the final human-friendly output
///
/// Now uses PTY interactive mode for consistent behavior and potential input echoing.
///
/// Returns ShellOutput with combined output and exit code
fn exec_through_wrapper(
    shell: &str,
    repo: &TestRepo,
    subcommand: &str,
    args: &[&str],
) -> ShellOutput {
    exec_through_wrapper_from(shell, repo, subcommand, args, repo.root_path())
}

fn exec_through_wrapper_from(
    shell: &str,
    repo: &TestRepo,
    subcommand: &str,
    args: &[&str],
    working_dir: &std::path::Path,
) -> ShellOutput {
    // Delegate to interactive version with no input
    // This provides consistent PTY behavior across all tests
    exec_through_wrapper_interactive(shell, repo, subcommand, args, working_dir, &[])
}

/// Execute a command through a shell wrapper with interactive input support
///
/// This is similar to `exec_through_wrapper_from` but allows sending input during execution
/// (e.g., approval responses). The PTY will automatically echo the input, so you'll see
/// both the prompts and the responses in the captured output.
///
/// # Arguments
/// * `shell` - The shell to use (e.g., "bash", "zsh", "fish")
/// * `repo` - The test repository
/// * `subcommand` - The wt subcommand (e.g., "merge", "switch")
/// * `args` - Arguments to the subcommand (without --force)
/// * `working_dir` - Working directory for the command
/// * `inputs` - Input strings to send (e.g., `&["y\n"]` for approval prompts)
///
/// # Example
/// ```no_run
/// // Test merge with approval prompt visible in output
/// let output = exec_through_wrapper_interactive(
///     "bash",
///     &repo,
///     "merge",
///     &["main"],
///     repo.root_path(),
///     &["y\n"],  // Approve the merge
/// );
/// // Output will show: "💡 Allow and remember? [y/N] y"
/// ```
#[cfg(test)]
fn exec_through_wrapper_interactive(
    shell: &str,
    repo: &TestRepo,
    subcommand: &str,
    args: &[&str],
    working_dir: &std::path::Path,
    inputs: &[&str],
) -> ShellOutput {
    let script = build_shell_script(shell, repo, subcommand, args);

    // Keep config path as owned String because:
    // 1. repo.test_config_path() returns a PathBuf
    // 2. .to_string_lossy() returns a Cow<str> that borrows from the PathBuf
    // 3. We need to borrow a &str for the env_vars vector
    // 4. The owned String lives long enough to satisfy the borrow in env_vars
    let config_path = repo.test_config_path().to_string_lossy().to_string();

    // Same environment setup as exec_through_wrapper_from
    let env_vars: Vec<(&str, &str)> = vec![
        ("CLICOLOR_FORCE", "1"),
        ("WORKTRUNK_CONFIG_PATH", &config_path),
        ("TERM", "xterm"),
        ("GIT_AUTHOR_NAME", "Test User"),
        ("GIT_AUTHOR_EMAIL", "test@example.com"),
        ("GIT_COMMITTER_NAME", "Test User"),
        ("GIT_COMMITTER_EMAIL", "test@example.com"),
        ("GIT_AUTHOR_DATE", "2025-10-28T12:00:00Z"),
        ("GIT_COMMITTER_DATE", "2025-10-28T12:00:00Z"),
        ("LANG", "C"),
        ("LC_ALL", "C"),
        ("SOURCE_DATE_EPOCH", "1761609600"),
    ];

    let (combined, exit_code) =
        exec_in_pty_interactive(shell, &script, working_dir, &env_vars, inputs);

    ShellOutput {
        combined,
        exit_code,
    }
}

mod tests {
    use super::*;
    use rstest::rstest;

    // ========================================================================
    // Cross-Shell Error Handling Tests
    // ========================================================================
    //
    // These tests use parametrized testing to verify consistent behavior
    // across all supported shells (bash, zsh, fish).
    //
    // Note: Zsh tests run in isolated mode (--no-rcs, ZDOTDIR=/dev/null) to prevent
    // user startup files from touching /dev/tty, which would cause SIGTTIN/TTOU/TSTP
    // signals. This isolation ensures tests are deterministic across all environments.
    //
    // SNAPSHOT CONSOLIDATION:
    // Tests use `insta::allow_duplicates!` to share a single snapshot across all shells
    // when output is deterministic and identical. This reduces snapshot count from 3×N to N.
    //
    // Trade-off: If future changes introduce shell-specific output differences, all three
    // shells will fail with "doesn't match snapshot" rather than showing which specific
    // shell differs. For tests with non-deterministic output (PTY buffering causes varying
    // order), we keep shell-specific snapshots.
    //
    // TODO: Consider adding a test assertion that compares bash/zsh/fish outputs are
    // byte-identical before the snapshot check, so we can identify which shell diverged.

    #[rstest]
    #[case("bash")]
    #[case("zsh")]
    #[case("fish")]
    fn test_wrapper_handles_command_failure(#[case] shell: &str) {
        let mut repo = TestRepo::new();
        repo.commit("Initial commit");

        // Create a worktree that already exists
        repo.add_worktree("existing", "existing");

        // Try to create it again - should fail
        let output = exec_through_wrapper(shell, &repo, "switch", &["--create", "existing"]);

        // Shell-agnostic assertions: these must be true for ALL shells
        assert_eq!(
            output.exit_code, 1,
            "{}: Command should fail with exit code 1",
            shell
        );
        output.assert_no_directive_leaks();
        assert!(
            output.combined.contains("already exists"),
            "{}: Error message should mention 'already exists'.\nOutput:\n{}",
            shell,
            output.combined
        );

        // Consolidated snapshot - output should be identical across all shells
        insta::allow_duplicates! {
            assert_snapshot!("command_failure", output.normalized());
        }
    }

    #[rstest]
    #[case("bash")]
    #[case("zsh")]
    #[case("fish")]
    fn test_wrapper_switch_create(#[case] shell: &str) {
        let repo = TestRepo::new();
        repo.commit("Initial commit");

        let output = exec_through_wrapper(shell, &repo, "switch", &["--create", "feature"]);

        // Shell-agnostic assertions
        assert_eq!(output.exit_code, 0, "{}: Command should succeed", shell);
        output.assert_no_directive_leaks();

        assert!(
            output.combined.contains("Created new worktree"),
            "{}: Should show success message",
            shell
        );

        // Consolidated snapshot - output should be identical across all shells
        insta::allow_duplicates! {
            assert_snapshot!("switch_create", output.normalized());
        }
    }

    #[rstest]
    #[case("bash")]
    #[case("zsh")]
    #[case("fish")]
    fn test_wrapper_remove(#[case] shell: &str) {
        let mut repo = TestRepo::new();
        repo.commit("Initial commit");

        // Create a worktree to remove
        repo.add_worktree("to-remove", "to-remove");

        let output = exec_through_wrapper(shell, &repo, "remove", &["to-remove"]);

        // Shell-agnostic assertions
        assert_eq!(output.exit_code, 0, "{}: Command should succeed", shell);
        output.assert_no_directive_leaks();

        // Consolidated snapshot - output should be identical across all shells
        insta::allow_duplicates! {
            assert_snapshot!("remove", output.normalized());
        }
    }

    #[rstest]
    #[case("bash")]
    #[case("zsh")]
    #[case("fish")]
    fn test_wrapper_merge(#[case] shell: &str) {
        let mut repo = TestRepo::new();
        repo.commit("Initial commit");

        // Create a feature branch
        repo.add_worktree("feature", "feature");

        let output = exec_through_wrapper(shell, &repo, "merge", &["main"]);

        // Shell-agnostic assertions
        assert_eq!(output.exit_code, 0, "{}: Command should succeed", shell);
        output.assert_no_directive_leaks();

        // Consolidated snapshot - output should be identical across all shells
        insta::allow_duplicates! {
            assert_snapshot!("merge", output.normalized());
        }
    }

    #[rstest]
    #[case("bash")]
    #[case("zsh")]
    #[case("fish")]
    fn test_wrapper_switch_with_execute(#[case] shell: &str) {
        let repo = TestRepo::new();
        repo.commit("Initial commit");

        // Use --force to skip approval prompt in tests
        let output = exec_through_wrapper(
            shell,
            &repo,
            "switch",
            &[
                "--create",
                "test-exec",
                "--execute",
                "echo executed",
                "--force",
            ],
        );

        // Shell-agnostic assertions
        assert_eq!(output.exit_code, 0, "{}: Command should succeed", shell);
        output.assert_no_directive_leaks();

        assert!(
            output.combined.contains("executed"),
            "{}: Execute command output missing",
            shell
        );

        // Consolidated snapshot - output should be identical across all shells
        insta::allow_duplicates! {
            assert_snapshot!("switch_with_execute", output.normalized());
        }
    }

    /// Test that --execute command exit codes are propagated
    /// Verifies that when wt succeeds but the --execute command fails,
    /// the wrapper returns the command's exit code, not wt's.
    #[rstest]
    #[case("bash")]
    #[case("zsh")]
    #[case("fish")]
    fn test_wrapper_execute_exit_code_propagation(#[case] shell: &str) {
        let repo = TestRepo::new();
        repo.commit("Initial commit");

        // Use --force to skip approval prompt in tests
        // wt should succeed (creates worktree), but the execute command should fail with exit 42
        let output = exec_through_wrapper(
            shell,
            &repo,
            "switch",
            &[
                "--create",
                "test-exit-code",
                "--execute",
                "exit 42",
                "--force",
            ],
        );

        // Shell-agnostic assertions
        assert_eq!(
            output.exit_code, 42,
            "{}: Should propagate execute command's exit code (42), not wt's (0)",
            shell
        );
        output.assert_no_directive_leaks();

        // Should still show wt's success message (worktree was created)
        assert!(
            output.combined.contains("Created new worktree"),
            "{}: Should show wt's success message even though execute command failed",
            shell
        );
    }

    /// Test switch --create with post-create-command (blocking) and post-start-command (background)
    /// Note: bash and fish disabled due to flaky PTY buffering race conditions
    ///
    /// TODO: Fix timing/race condition in bash where "Building project..." output appears
    /// before the command display, causing snapshot mismatch (appears on line 7 instead of line 9).
    /// This is a non-deterministic PTY output ordering issue.
    #[rstest]
    // #[case("bash")] // TODO: Flaky PTY output ordering - command output appears before command display
    #[case("zsh")]
    // #[case("fish")] // TODO: Fish shell has non-deterministic PTY output ordering
    fn test_wrapper_switch_with_hooks(#[case] shell: &str) {
        let repo = TestRepo::new();
        repo.commit("Initial commit");

        // Create project config with both post-create and post-start hooks
        let config_dir = repo.root_path().join(".config");
        fs::create_dir_all(&config_dir).expect("Failed to create .config dir");
        fs::write(
            config_dir.join("wt.toml"),
            r#"# Blocking command that runs before worktree is ready
post-create-command = [
    "echo 'Installing dependencies...'",
    "echo 'Building project...'"
]

# Background command that runs in parallel
[post-start-command]
server = "echo 'Starting dev server on port 3000'"
watch = "echo 'Watching for file changes'"
"#,
        )
        .expect("Failed to write project config");

        repo.commit("Add hooks");

        // Pre-approve the commands in user config
        fs::write(
            repo.test_config_path(),
            r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."test-repo"]
approved-commands = [
    "echo 'Installing dependencies...'",
    "echo 'Building project...'",
    "echo 'Starting dev server on port 3000'",
    "echo 'Watching for file changes'",
]
"#,
        )
        .expect("Failed to write user config");

        let output = exec_through_wrapper(shell, &repo, "switch", &["--create", "feature-hooks"]);

        assert_eq!(output.exit_code, 0, "{}: Command should succeed", shell);
        output.assert_no_directive_leaks();

        // Shell-specific snapshot - output ordering varies due to PTY buffering
        assert_snapshot!(format!("switch_with_hooks_{}", shell), output.normalized());
    }

    /// Test merge with successful pre-merge-command validation
    /// Note: fish disabled due to flaky PTY buffering race conditions
    /// TODO: bash variant occasionally fails on Ubuntu CI with snapshot mismatches due to PTY timing
    #[rstest]
    #[case("bash")]
    #[case("zsh")]
    // #[case("fish")] // TODO: Fish shell has non-deterministic PTY output ordering
    fn test_wrapper_merge_with_pre_merge_success(#[case] shell: &str) {
        let mut repo = TestRepo::new();
        repo.commit("Initial commit");
        repo.setup_remote("main");

        // Create project config with pre-merge validation
        let config_dir = repo.root_path().join(".config");
        fs::create_dir_all(&config_dir).expect("Failed to create .config dir");
        fs::write(
            config_dir.join("wt.toml"),
            r#"[pre-merge-command]
format = "echo '✓ Code formatting check passed'"
lint = "echo '✓ Linting passed - no warnings'"
test = "echo '✓ All 47 tests passed in 2.3s'"
"#,
        )
        .expect("Failed to write config");

        repo.commit("Add pre-merge validation");

        // Create a main worktree
        let main_wt = repo.root_path().parent().unwrap().join("test-repo.main-wt");
        let mut cmd = Command::new("git");
        repo.configure_git_cmd(&mut cmd);
        cmd.args(["worktree", "add", main_wt.to_str().unwrap(), "main"])
            .current_dir(repo.root_path())
            .output()
            .expect("Failed to add main worktree");

        // Create feature worktree with a commit
        let feature_wt = repo.add_worktree("feature", "feature");
        fs::write(feature_wt.join("feature.txt"), "feature content").expect("Failed to write file");

        let mut cmd = Command::new("git");
        repo.configure_git_cmd(&mut cmd);
        cmd.args(["add", "feature.txt"])
            .current_dir(&feature_wt)
            .output()
            .expect("Failed to add file");

        let mut cmd = Command::new("git");
        repo.configure_git_cmd(&mut cmd);
        cmd.args(["commit", "-m", "Add feature"])
            .current_dir(&feature_wt)
            .output()
            .expect("Failed to commit");

        // Pre-approve commands
        fs::write(
            repo.test_config_path(),
            r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."test-repo"]
approved-commands = [
    "echo '✓ Code formatting check passed'",
    "echo '✓ Linting passed - no warnings'",
    "echo '✓ All 47 tests passed in 2.3s'",
]
"#,
        )
        .expect("Failed to write user config");

        // Run merge from the feature worktree
        let output =
            exec_through_wrapper_from(shell, &repo, "merge", &["main", "--force"], &feature_wt);

        assert_eq!(output.exit_code, 0, "{}: Merge should succeed", shell);
        output.assert_no_directive_leaks();

        // Shell-specific snapshot - output ordering varies due to PTY buffering
        assert_snapshot!(
            format!("merge_with_pre_merge_success_{}", shell),
            output.normalized()
        );
    }

    /// Test merge with failing pre-merge-command that aborts the merge
    /// Note: fish disabled due to flaky PTY buffering race conditions
    #[rstest]
    #[case("bash")]
    #[case("zsh")]
    // #[case("fish")] // TODO: Fish shell has non-deterministic PTY output ordering
    fn test_wrapper_merge_with_pre_merge_failure(#[case] shell: &str) {
        let mut repo = TestRepo::new();
        repo.commit("Initial commit");
        repo.setup_remote("main");

        // Create project config with failing pre-merge validation
        let config_dir = repo.root_path().join(".config");
        fs::create_dir_all(&config_dir).expect("Failed to create .config dir");
        fs::write(
            config_dir.join("wt.toml"),
            r#"[pre-merge-command]
format = "echo '✓ Code formatting check passed'"
test = "echo '✗ Test suite failed: 3 tests failing' && exit 1"
"#,
        )
        .expect("Failed to write config");

        repo.commit("Add failing pre-merge validation");

        // Create a main worktree
        let main_wt = repo.root_path().parent().unwrap().join("test-repo.main-wt");
        let mut cmd = Command::new("git");
        repo.configure_git_cmd(&mut cmd);
        cmd.args(["worktree", "add", main_wt.to_str().unwrap(), "main"])
            .current_dir(repo.root_path())
            .output()
            .expect("Failed to add main worktree");

        // Create feature worktree with a commit
        let feature_wt = repo.add_worktree("feature-fail", "feature-fail");
        fs::write(feature_wt.join("feature.txt"), "feature content").expect("Failed to write file");

        let mut cmd = Command::new("git");
        repo.configure_git_cmd(&mut cmd);
        cmd.args(["add", "feature.txt"])
            .current_dir(&feature_wt)
            .output()
            .expect("Failed to add file");

        let mut cmd = Command::new("git");
        repo.configure_git_cmd(&mut cmd);
        cmd.args(["commit", "-m", "Add feature"])
            .current_dir(&feature_wt)
            .output()
            .expect("Failed to commit");

        // Pre-approve the commands
        fs::write(
            repo.test_config_path(),
            r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."test-repo"]
approved-commands = [
    "echo '✓ Code formatting check passed'",
    "echo '✗ Test suite failed: 3 tests failing' && exit 1",
]
"#,
        )
        .expect("Failed to write user config");

        // Run merge from the feature worktree
        let output =
            exec_through_wrapper_from(shell, &repo, "merge", &["main", "--force"], &feature_wt);

        output.assert_no_directive_leaks();

        // Shell-specific snapshot - output ordering varies due to PTY buffering
        assert_snapshot!(
            format!("merge_with_pre_merge_failure_{}", shell),
            output.normalized()
        );
    }

    /// Test merge with pre-merge commands that output to both stdout and stderr
    /// Verifies that interleaved stdout/stderr appears in correct temporal order
    /// Note: fish disabled due to flaky PTY buffering race conditions
    #[rstest]
    #[case("bash")]
    #[case("zsh")]
    // #[case("fish")] // TODO: Fish shell has non-deterministic PTY output ordering
    fn test_wrapper_merge_with_mixed_stdout_stderr(#[case] shell: &str) {
        let mut repo = TestRepo::new();
        repo.commit("Initial commit");
        repo.setup_remote("main");

        // Get path to the test fixture script
        let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let script_path = fixtures_dir.join("mixed-output.sh");

        // Create project config with pre-merge commands that output to both stdout and stderr
        let config_dir = repo.root_path().join(".config");
        fs::create_dir_all(&config_dir).expect("Failed to create .config dir");
        fs::write(
            config_dir.join("wt.toml"),
            format!(
                r#"[pre-merge-command]
check1 = "{} check1 3"
check2 = "{} check2 3"
"#,
                script_path.display(),
                script_path.display()
            ),
        )
        .expect("Failed to write config");

        repo.commit("Add pre-merge validation with mixed output");

        // Create a main worktree
        let main_wt = repo.root_path().parent().unwrap().join("test-repo.main-wt");
        let mut cmd = Command::new("git");
        repo.configure_git_cmd(&mut cmd);
        cmd.args(["worktree", "add", main_wt.to_str().unwrap(), "main"])
            .current_dir(repo.root_path())
            .output()
            .expect("Failed to add main worktree");

        // Create feature worktree with a commit
        let feature_wt = repo.add_worktree("feature", "feature");
        fs::write(feature_wt.join("feature.txt"), "feature content").expect("Failed to write file");

        let mut cmd = Command::new("git");
        repo.configure_git_cmd(&mut cmd);
        cmd.args(["add", "feature.txt"])
            .current_dir(&feature_wt)
            .output()
            .expect("Failed to add file");

        let mut cmd = Command::new("git");
        repo.configure_git_cmd(&mut cmd);
        cmd.args(["commit", "-m", "Add feature"])
            .current_dir(&feature_wt)
            .output()
            .expect("Failed to commit");

        // Pre-approve commands
        fs::write(
            repo.test_config_path(),
            format!(
                r#"worktree-path = "../{{{{ main_worktree }}}}.{{{{ branch }}}}"

[projects."test-repo"]
approved-commands = [
    "{} check1 3",
    "{} check2 3",
]
"#,
                script_path.display(),
                script_path.display()
            ),
        )
        .expect("Failed to write user config");

        // Run merge from the feature worktree
        let output =
            exec_through_wrapper_from(shell, &repo, "merge", &["main", "--force"], &feature_wt);

        assert_eq!(output.exit_code, 0, "{}: Merge should succeed", shell);
        output.assert_no_directive_leaks();

        // Verify output shows proper temporal ordering:
        // header1 → all check1 output (interleaved stdout/stderr) → header2 → all check2 output
        // This ensures that stdout/stderr from child processes properly stream through
        // to the terminal in real-time, maintaining correct ordering
        assert_snapshot!(
            format!("merge_with_mixed_stdout_stderr_{}", shell),
            output.normalized()
        );
    }

    // ========================================================================
    // Bash Shell Wrapper Tests
    // ========================================================================

    #[test]
    fn test_switch_with_post_start_command_no_directive_leak() {
        let repo = TestRepo::new();
        repo.commit("Initial commit");

        // Configure a post-start command in the project config (this is where the bug manifests)
        // The println! in handle_post_start_commands causes directive leaks
        let config_dir = repo.root_path().join(".config");
        fs::create_dir_all(&config_dir).expect("Failed to create .config dir");
        fs::write(
            config_dir.join("wt.toml"),
            r#"post-start-command = "echo 'test command executed'""#,
        )
        .expect("Failed to write project config");

        repo.commit("Add post-start command");

        // Pre-approve the command in user config
        fs::write(
            repo.test_config_path(),
            r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."test-repo"]
approved-commands = ["echo 'test command executed'"]
"#,
        )
        .expect("Failed to write user config");

        let output =
            exec_through_wrapper("bash", &repo, "switch", &["--create", "feature-with-hooks"]);

        // The critical assertion: directives must never appear in user-facing output
        // This is where the bug occurs - "🔄 Starting (background):" is printed with println!
        // which causes it to concatenate with the directive
        output.assert_no_directive_leaks();

        assert_eq!(output.exit_code, 0, "Command should succeed");

        // Normalize paths in output for snapshot testing
        // Snapshot the output
        assert_snapshot!(output.normalized());
    }

    #[test]
    fn test_switch_with_execute_through_wrapper() {
        let repo = TestRepo::new();
        repo.commit("Initial commit");

        // Use --force to skip approval prompt in tests
        let output = exec_through_wrapper(
            "bash",
            &repo,
            "switch",
            &[
                "--create",
                "test-exec",
                "--execute",
                "echo executed",
                "--force",
            ],
        );

        // No directives should leak
        output.assert_no_directive_leaks();

        assert_eq!(output.exit_code, 0, "Command should succeed");

        // The executed command output should appear
        assert!(
            output.combined.contains("executed"),
            "Execute command output missing"
        );

        // Normalize paths in output for snapshot testing
        // Snapshot the output
        assert_snapshot!(output.normalized());
    }

    #[test]
    fn test_bash_shell_integration_hint_suppressed() {
        let repo = TestRepo::new();
        repo.commit("Initial commit");

        // When running through the shell wrapper, the "To enable automatic cd" hint
        // should NOT appear because the user already has shell integration
        let output = exec_through_wrapper("bash", &repo, "switch", &["--create", "bash-test"]);

        // Critical: shell integration hint must be suppressed in directive mode
        assert!(
            !output.combined.contains("To enable automatic cd"),
            "Shell integration hint should not appear when running through wrapper. Output:\n{}",
            output.combined
        );

        // Should still have the success message
        assert!(
            output.combined.contains("Created new worktree"),
            "Success message missing"
        );

        assert_snapshot!(output.normalized());
    }

    #[test]
    fn test_readme_example_simple_switch() {
        let mut repo = TestRepo::new();
        repo.commit("Initial commit");
        repo.setup_remote("main");

        // Create worktree through shell wrapper (suppresses hint)
        let output = exec_through_wrapper("bash", &repo, "switch", &["--create", "fix-auth"]);

        assert!(
            !output.combined.contains("To enable automatic cd"),
            "Shell integration hint should be suppressed"
        );

        assert_snapshot!(output.normalized());
    }

    #[test]
    fn test_wrapper_preserves_progress_messages() {
        let repo = TestRepo::new();
        repo.commit("Initial commit");

        // Configure a post-start background command that will trigger progress output
        let config_dir = repo.root_path().join(".config");
        fs::create_dir_all(&config_dir).expect("Failed to create .config dir");
        fs::write(
            config_dir.join("wt.toml"),
            r#"post-start-command = "echo 'background task'""#,
        )
        .expect("Failed to write project config");

        repo.commit("Add post-start command");

        // Pre-approve the command in user config
        fs::write(
            repo.test_config_path(),
            r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."test-repo"]
approved-commands = ["echo 'background task'"]
"#,
        )
        .expect("Failed to write user config");

        let output = exec_through_wrapper("bash", &repo, "switch", &["--create", "feature-bg"]);

        // No directives should leak
        output.assert_no_directive_leaks();

        assert_eq!(output.exit_code, 0, "Command should succeed");

        // Critical assertion: progress messages should appear to users
        // This is the test that catches the bug where progress() is suppressed in directive mode
        assert!(
            output.combined.contains("Running post-start"),
            "Progress message 'Running post-start' missing from output. \
         Output:\n{}",
            output.combined
        );

        // The background command itself should be shown via gutter formatting
        assert!(
            output.combined.contains("background task"),
            "Background command content missing from output"
        );

        // Normalize paths in output for snapshot testing
        // Snapshot the full output
        assert_snapshot!(output.normalized());
    }

    // ============================================================================
    // Fish Shell Wrapper Tests
    // ============================================================================
    //
    // These tests verify that the Fish shell wrapper correctly:
    // 1. Parses NUL-delimited directives from `wt --internal`
    // 2. Never leaks directives to users
    // 3. Preserves all user-visible output (progress, success, hints)
    // 4. Handles Fish-specific psub process substitution correctly
    //
    // Fish uses `read -z` to parse NUL-delimited chunks and `psub` for
    // process substitution. These have known limitations (fish-shell #1040)
    // but work correctly for our use case.

    #[cfg(unix)]
    #[test]
    fn test_fish_wrapper_preserves_progress_messages() {
        let repo = TestRepo::new();
        repo.commit("Initial commit");

        // Configure a post-start background command that will trigger progress output
        let config_dir = repo.root_path().join(".config");
        fs::create_dir_all(&config_dir).expect("Failed to create .config dir");
        fs::write(
            config_dir.join("wt.toml"),
            r#"post-start-command = "echo 'fish background task'""#,
        )
        .expect("Failed to write project config");

        repo.commit("Add post-start command");

        // Pre-approve the command in user config
        fs::write(
            repo.test_config_path(),
            r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."test-repo"]
approved-commands = ["echo 'fish background task'"]
"#,
        )
        .expect("Failed to write user config");

        let output = exec_through_wrapper("fish", &repo, "switch", &["--create", "fish-bg"]);

        // No directives should leak
        output.assert_no_directive_leaks();

        assert_eq!(output.exit_code, 0, "Command should succeed");

        // Critical: progress messages should appear to users through Fish wrapper
        assert!(
            output.combined.contains("Running post-start"),
            "Progress message 'Running post-start' missing from Fish wrapper output. \
         Output:\n{}",
            output.combined
        );

        // The background command itself should be shown via gutter formatting
        assert!(
            output.combined.contains("fish background task"),
            "Background command content missing from output"
        );

        assert_snapshot!(output.normalized());
    }

    #[cfg(unix)]
    #[test]
    fn test_fish_multiline_command_execution() {
        let repo = TestRepo::new();
        repo.commit("Initial commit");

        // Test that Fish wrapper handles multi-line commands correctly
        // This tests Fish's NUL-byte parsing with embedded newlines
        // Use actual newlines in the command string
        let multiline_cmd = "echo 'line 1'; echo 'line 2'; echo 'line 3'";

        // Use --force to skip approval prompt in tests
        let output = exec_through_wrapper(
            "fish",
            &repo,
            "switch",
            &[
                "--create",
                "fish-multiline",
                "--execute",
                multiline_cmd,
                "--force",
            ],
        );

        // No directives should leak
        output.assert_no_directive_leaks();

        assert_eq!(output.exit_code, 0, "Command should succeed");

        // All three lines should be executed and visible
        assert!(output.combined.contains("line 1"), "First line missing");
        assert!(output.combined.contains("line 2"), "Second line missing");
        assert!(output.combined.contains("line 3"), "Third line missing");

        // Normalize paths in output for snapshot testing
        assert_snapshot!(output.normalized());
    }

    #[cfg(unix)]
    #[test]
    fn test_fish_wrapper_handles_empty_chunks() {
        let repo = TestRepo::new();
        repo.commit("Initial commit");

        // Test edge case: command that produces minimal output
        // This verifies Fish's `test -n "$chunk"` check works correctly
        let output = exec_through_wrapper("fish", &repo, "switch", &["--create", "fish-minimal"]);

        // No directives should leak even with minimal output
        output.assert_no_directive_leaks();

        assert_eq!(output.exit_code, 0, "Command should succeed");

        // Should still show success message
        assert!(
            output.combined.contains("Created new worktree"),
            "Success message missing from minimal output"
        );

        // Normalize paths in output for snapshot testing
        assert_snapshot!(output.normalized());
    }

    // ========================================================================
    // --source Flag Error Passthrough Tests
    // ========================================================================
    //
    // These tests verify that actual error messages pass through correctly
    // when using the --source flag (instead of being hidden with generic
    // wrapper error messages like "Error: cargo build failed").

    #[rstest]
    #[case("bash")]
    #[case("zsh")]
    #[case("fish")]
    fn test_source_flag_forwards_errors(#[case] shell: &str) {
        use std::env;

        let repo = TestRepo::new();
        repo.commit("Initial commit");

        // Get the worktrunk source directory (where this test is running from)
        // This is the directory that contains Cargo.toml with the workspace
        let worktrunk_source = env::current_dir()
            .expect("Failed to get current directory")
            .canonicalize()
            .expect("Failed to canonicalize path");

        // Build a shell script that runs from the worktrunk source directory
        let wt_bin = get_cargo_bin("wt");
        let wrapper_script = generate_wrapper(&repo, shell);
        let mut script = String::new();

        // Set environment variables
        match shell {
            "fish" => {
                script.push_str(&format!("set -x WORKTRUNK_BIN '{}'\n", wt_bin.display()));
                script.push_str(&format!(
                    "set -x WORKTRUNK_CONFIG_PATH '{}'\n",
                    repo.test_config_path().display()
                ));
                script.push_str("set -x CLICOLOR_FORCE 1\n");
            }
            "zsh" => {
                script.push_str("autoload -Uz compinit && compinit -i 2>/dev/null\n");
                script.push_str(&format!("export WORKTRUNK_BIN='{}'\n", wt_bin.display()));
                script.push_str(&format!(
                    "export WORKTRUNK_CONFIG_PATH='{}'\n",
                    repo.test_config_path().display()
                ));
                script.push_str("export CLICOLOR_FORCE=1\n");
            }
            _ => {
                // bash
                script.push_str(&format!("export WORKTRUNK_BIN='{}'\n", wt_bin.display()));
                script.push_str(&format!(
                    "export WORKTRUNK_CONFIG_PATH='{}'\n",
                    repo.test_config_path().display()
                ));
                script.push_str("export CLICOLOR_FORCE=1\n");
            }
        }

        // Source the wrapper
        script.push_str(&wrapper_script);
        script.push('\n');

        // Try to run wt --source with an invalid subcommand
        // The --source flag triggers cargo build (which succeeds)
        // Then it tries to run 'wt foo' which should fail with "unrecognized subcommand"
        script.push_str("wt --source foo\n");

        // Wrap in subshell to merge stderr
        let final_script = match shell {
            "fish" => format!("begin\n{}\nend 2>&1", script),
            _ => format!("( {} ) 2>&1", script),
        };

        let config_path = repo.test_config_path().to_string_lossy().to_string();
        let env_vars: Vec<(&str, &str)> = vec![
            ("CLICOLOR_FORCE", "1"),
            ("WORKTRUNK_CONFIG_PATH", &config_path),
            ("TERM", "xterm"),
            ("GIT_AUTHOR_NAME", "Test User"),
            ("GIT_AUTHOR_EMAIL", "test@example.com"),
            ("GIT_COMMITTER_NAME", "Test User"),
            ("GIT_COMMITTER_EMAIL", "test@example.com"),
            ("GIT_AUTHOR_DATE", "2025-10-28T12:00:00Z"),
            ("GIT_COMMITTER_DATE", "2025-10-28T12:00:00Z"),
            ("LANG", "C"),
            ("LC_ALL", "C"),
            ("SOURCE_DATE_EPOCH", "1761609600"),
        ];

        let (combined, exit_code) =
            exec_in_pty_interactive(shell, &final_script, &worktrunk_source, &env_vars, &[]);
        let output = ShellOutput {
            combined,
            exit_code,
        };

        // Shell-agnostic assertions
        assert_ne!(output.exit_code, 0, "{}: Command should fail", shell);

        // CRITICAL: Should see wt's actual error message about unrecognized subcommand
        assert!(
            output.combined.contains("unrecognized subcommand"),
            "{}: Should show actual wt error message 'unrecognized subcommand'.\nOutput:\n{}",
            shell,
            output.combined
        );

        // CRITICAL: Should NOT see the old generic wrapper error message
        assert!(
            !output.combined.contains("Error: cargo build failed"),
            "{}: Should not contain old generic error message",
            shell
        );

        // Consolidated snapshot - output should be identical across shells
        // (wt error messages are deterministic)
        insta::allow_duplicates! {
            assert_snapshot!("source_flag_error_passthrough", output.normalized());
        }
    }

    // ========================================================================
    // Job Control Notification Tests
    // ========================================================================
    //
    // These tests verify that job control notifications ([1] 12345, [1] + done)
    // don't leak into user output. Zsh suppresses these with NO_MONITOR,
    // bash shows them at the next prompt (less intrusive).

    /// Test that zsh doesn't show job control notifications inline
    /// The NO_MONITOR option should suppress [1] 12345 and [1] + done messages
    #[test]
    fn test_zsh_no_job_control_notifications() {
        let repo = TestRepo::new();
        repo.commit("Initial commit");

        // Configure a post-start command that will trigger background job
        let config_dir = repo.root_path().join(".config");
        fs::create_dir_all(&config_dir).expect("Failed to create .config dir");
        fs::write(
            config_dir.join("wt.toml"),
            r#"post-start-command = "echo 'background job'""#,
        )
        .expect("Failed to write project config");

        repo.commit("Add post-start command");

        // Pre-approve the command
        fs::write(
            repo.test_config_path(),
            r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."test-repo"]
approved-commands = ["echo 'background job'"]
"#,
        )
        .expect("Failed to write user config");

        let output = exec_through_wrapper("zsh", &repo, "switch", &["--create", "zsh-job-test"]);

        assert_eq!(output.exit_code, 0, "Command should succeed");
        output.assert_no_directive_leaks();

        // Critical: zsh should NOT show job control notifications
        // These patterns indicate job control messages leaked through
        assert!(
            !output.combined.contains("[1]"),
            "Zsh should suppress job control notifications with NO_MONITOR.\nOutput:\n{}",
            output.combined
        );
        assert!(
            !output.combined.contains("+ done"),
            "Zsh should suppress job completion notifications.\nOutput:\n{}",
            output.combined
        );
    }

    /// Test bash background job behavior
    /// Note: Bash shows [1] + done at the next prompt, which we can't fully suppress
    /// in functions. This test documents the behavior.
    #[test]
    fn test_bash_job_control_behavior() {
        let repo = TestRepo::new();
        repo.commit("Initial commit");

        // Configure a post-start command that will trigger background job
        let config_dir = repo.root_path().join(".config");
        fs::create_dir_all(&config_dir).expect("Failed to create .config dir");
        fs::write(
            config_dir.join("wt.toml"),
            r#"post-start-command = "echo 'bash background'""#,
        )
        .expect("Failed to write project config");

        repo.commit("Add post-start command");

        // Pre-approve the command
        fs::write(
            repo.test_config_path(),
            r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."test-repo"]
approved-commands = ["echo 'bash background'"]
"#,
        )
        .expect("Failed to write user config");

        let output = exec_through_wrapper("bash", &repo, "switch", &["--create", "bash-job-test"]);

        assert_eq!(output.exit_code, 0, "Command should succeed");
        output.assert_no_directive_leaks();

        // Bash may show job notifications - this test documents current behavior
        // The key is that directives don't leak (checked above)
        // Job notifications at next prompt are acceptable (less intrusive than inline)

        // Verify the command completed successfully
        assert!(
            output.combined.contains("Created new worktree"),
            "Should show success message"
        );
    }

    // ========================================================================
    // Completion Functionality Tests
    // ========================================================================

    /// Test that bash completions are properly registered
    #[test]
    fn test_bash_completions_registered() {
        let repo = TestRepo::new();
        repo.commit("Initial commit");

        let wt_bin = get_cargo_bin("wt");
        let wrapper_script = generate_wrapper(&repo, "bash");

        // Script that sources wrapper and checks if completion is registered
        let script = format!(
            r#"
            export WORKTRUNK_BIN='{}'
            export WORKTRUNK_CONFIG_PATH='{}'
            {}
            # Check if wt completion is registered
            complete -p wt 2>/dev/null && echo "__COMPLETION_REGISTERED__" || echo "__NO_COMPLETION__"
            "#,
            wt_bin.display(),
            repo.test_config_path().display(),
            wrapper_script
        );

        let final_script = format!("( {} ) 2>&1", script);
        let config_path = repo.test_config_path().to_string_lossy().to_string();
        let env_vars: Vec<(&str, &str)> =
            vec![("WORKTRUNK_CONFIG_PATH", &config_path), ("TERM", "xterm")];

        let (combined, exit_code) =
            exec_in_pty_interactive("bash", &final_script, repo.root_path(), &env_vars, &[]);

        assert_eq!(exit_code, 0, "Script should succeed");
        assert!(
            combined.contains("__COMPLETION_REGISTERED__"),
            "Bash completions should be registered after sourcing wrapper.\nOutput:\n{}",
            combined
        );
    }

    /// Test that fish completions are properly registered
    #[test]
    fn test_fish_completions_registered() {
        let repo = TestRepo::new();
        repo.commit("Initial commit");

        let wt_bin = get_cargo_bin("wt");
        let wrapper_script = generate_wrapper(&repo, "fish");

        // Script that sources wrapper and checks if completion is registered
        let script = format!(
            r#"
            set -x WORKTRUNK_BIN '{}'
            set -x WORKTRUNK_CONFIG_PATH '{}'
            {}
            # Check if wt completions are registered
            if complete -c wt 2>/dev/null | grep -q .
                echo "__COMPLETION_REGISTERED__"
            else
                echo "__NO_COMPLETION__"
            end
            "#,
            wt_bin.display(),
            repo.test_config_path().display(),
            wrapper_script
        );

        let final_script = format!("begin\n{}\nend 2>&1", script);
        let config_path = repo.test_config_path().to_string_lossy().to_string();
        let env_vars: Vec<(&str, &str)> =
            vec![("WORKTRUNK_CONFIG_PATH", &config_path), ("TERM", "xterm")];

        let (combined, exit_code) =
            exec_in_pty_interactive("fish", &final_script, repo.root_path(), &env_vars, &[]);

        assert_eq!(exit_code, 0, "Script should succeed");
        assert!(
            combined.contains("__COMPLETION_REGISTERED__"),
            "Fish completions should be registered after sourcing wrapper.\nOutput:\n{}",
            combined
        );
    }

    /// Test that zsh completions are properly registered
    #[test]
    fn test_zsh_completions_registered() {
        let repo = TestRepo::new();
        repo.commit("Initial commit");

        let wt_bin = get_cargo_bin("wt");
        let wrapper_script = generate_wrapper(&repo, "zsh");

        // Script that sources wrapper and checks if completion function exists
        let script = format!(
            r#"
            autoload -Uz compinit && compinit -i 2>/dev/null
            export WORKTRUNK_BIN='{}'
            export WORKTRUNK_CONFIG_PATH='{}'
            {}
            # Check if _wt completion function exists
            if (( $+functions[_wt] )); then
                echo "__COMPLETION_REGISTERED__"
            else
                echo "__NO_COMPLETION__"
            fi
            "#,
            wt_bin.display(),
            repo.test_config_path().display(),
            wrapper_script
        );

        let final_script = format!("( {} ) 2>&1", script);
        let config_path = repo.test_config_path().to_string_lossy().to_string();
        let env_vars: Vec<(&str, &str)> = vec![
            ("WORKTRUNK_CONFIG_PATH", &config_path),
            ("TERM", "xterm"),
            ("ZDOTDIR", "/dev/null"),
        ];

        let (combined, exit_code) =
            exec_in_pty_interactive("zsh", &final_script, repo.root_path(), &env_vars, &[]);

        assert_eq!(exit_code, 0, "Script should succeed");
        assert!(
            combined.contains("__COMPLETION_REGISTERED__"),
            "Zsh completions should be registered after sourcing wrapper.\nOutput:\n{}",
            combined
        );
    }

    // ========================================================================
    // Special Characters in Branch Names Tests
    // ========================================================================

    /// Test that branch names with special characters work correctly
    #[rstest]
    #[case("bash")]
    #[case("zsh")]
    #[case("fish")]
    fn test_branch_name_with_slashes(#[case] shell: &str) {
        let repo = TestRepo::new();
        repo.commit("Initial commit");

        // Branch name with slashes (common git convention)
        let output =
            exec_through_wrapper(shell, &repo, "switch", &["--create", "feature/test-branch"]);

        assert_eq!(output.exit_code, 0, "{}: Command should succeed", shell);
        output.assert_no_directive_leaks();

        assert!(
            output.combined.contains("Created new worktree"),
            "{}: Should create worktree for branch with slashes",
            shell
        );
    }

    /// Test that branch names with dashes and underscores work
    #[rstest]
    #[case("bash")]
    #[case("zsh")]
    #[case("fish")]
    fn test_branch_name_with_dashes_underscores(#[case] shell: &str) {
        let repo = TestRepo::new();
        repo.commit("Initial commit");

        let output = exec_through_wrapper(shell, &repo, "switch", &["--create", "fix-bug_123"]);

        assert_eq!(output.exit_code, 0, "{}: Command should succeed", shell);
        output.assert_no_directive_leaks();

        assert!(
            output.combined.contains("Created new worktree"),
            "{}: Should create worktree for branch with dashes/underscores",
            shell
        );
    }

    // ========================================================================
    // WORKTRUNK_BIN Fallback Tests
    // ========================================================================

    /// Test that shell integration works when wt is not in PATH but WORKTRUNK_BIN is set
    #[rstest]
    #[case("bash")]
    #[case("zsh")]
    #[case("fish")]
    fn test_worktrunk_bin_fallback(#[case] shell: &str) {
        let repo = TestRepo::new();
        repo.commit("Initial commit");

        let wt_bin = get_cargo_bin("wt");
        let wrapper_script = generate_wrapper(&repo, shell);

        // Script that explicitly removes wt from PATH but sets WORKTRUNK_BIN
        let script = match shell {
            "zsh" => format!(
                r#"
                autoload -Uz compinit && compinit -i 2>/dev/null
                # Clear PATH to ensure wt is not found via PATH
                export PATH="/usr/bin:/bin"
                export WORKTRUNK_BIN='{}'
                export WORKTRUNK_CONFIG_PATH='{}'
                export CLICOLOR_FORCE=1
                {}
                wt switch --create fallback-test
                echo "__PWD__ $PWD"
                "#,
                wt_bin.display(),
                repo.test_config_path().display(),
                wrapper_script
            ),
            "fish" => format!(
                r#"
                # Clear PATH to ensure wt is not found via PATH
                set -x PATH /usr/bin /bin
                set -x WORKTRUNK_BIN '{}'
                set -x WORKTRUNK_CONFIG_PATH '{}'
                set -x CLICOLOR_FORCE 1
                {}
                wt switch --create fallback-test
                echo "__PWD__ $PWD"
                "#,
                wt_bin.display(),
                repo.test_config_path().display(),
                wrapper_script
            ),
            _ => format!(
                r#"
                # Clear PATH to ensure wt is not found via PATH
                export PATH="/usr/bin:/bin"
                export WORKTRUNK_BIN='{}'
                export WORKTRUNK_CONFIG_PATH='{}'
                export CLICOLOR_FORCE=1
                {}
                wt switch --create fallback-test
                echo "__PWD__ $PWD"
                "#,
                wt_bin.display(),
                repo.test_config_path().display(),
                wrapper_script
            ),
        };

        let final_script = match shell {
            "fish" => format!("begin\n{}\nend 2>&1", script),
            _ => format!("( {} ) 2>&1", script),
        };

        let config_path = repo.test_config_path().to_string_lossy().to_string();
        let env_vars: Vec<(&str, &str)> = vec![
            ("WORKTRUNK_CONFIG_PATH", &config_path),
            ("TERM", "xterm"),
            ("GIT_AUTHOR_NAME", "Test User"),
            ("GIT_AUTHOR_EMAIL", "test@example.com"),
            ("GIT_COMMITTER_NAME", "Test User"),
            ("GIT_COMMITTER_EMAIL", "test@example.com"),
        ];

        let (combined, exit_code) =
            exec_in_pty_interactive(shell, &final_script, repo.root_path(), &env_vars, &[]);

        let output = ShellOutput {
            combined,
            exit_code,
        };

        assert_eq!(
            output.exit_code, 0,
            "{}: Command should succeed with WORKTRUNK_BIN fallback",
            shell
        );
        output.assert_no_directive_leaks();

        assert!(
            output.combined.contains("Created new worktree"),
            "{}: Should create worktree using WORKTRUNK_BIN fallback.\nOutput:\n{}",
            shell,
            output.combined
        );

        // Verify we actually cd'd to the new worktree
        assert!(
            output.combined.contains("fallback-test"),
            "{}: Should be in the new worktree directory.\nOutput:\n{}",
            shell,
            output.combined
        );
    }

    // ========================================================================
    // Interrupt/Cleanup Tests
    // ========================================================================

    /// Test that shell integration completes without leaving zombie processes
    /// Note: Temp directory cleanup is verified implicitly by successful test completion.
    /// We can't check for specific temp files because tests run in parallel.
    #[rstest]
    #[case("bash")]
    #[case("zsh")]
    #[case("fish")]
    fn test_shell_completes_cleanly(#[case] shell: &str) {
        let repo = TestRepo::new();
        repo.commit("Initial commit");

        // Configure a post-start command to exercise the background job code path
        let config_dir = repo.root_path().join(".config");
        fs::create_dir_all(&config_dir).expect("Failed to create .config dir");
        fs::write(
            config_dir.join("wt.toml"),
            r#"post-start-command = "echo 'cleanup test'""#,
        )
        .expect("Failed to write project config");

        repo.commit("Add post-start command");

        // Pre-approve the command
        fs::write(
            repo.test_config_path(),
            r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."test-repo"]
approved-commands = ["echo 'cleanup test'"]
"#,
        )
        .expect("Failed to write user config");

        // Run a command that exercises the full FIFO/background job code path
        let output = exec_through_wrapper(shell, &repo, "switch", &["--create", "cleanup-test"]);

        // Verify command completed successfully
        // If cleanup failed (e.g., FIFO not removed, zombie process),
        // the command would hang or fail
        assert_eq!(
            output.exit_code, 0,
            "{}: Command should complete cleanly",
            shell
        );
        output.assert_no_directive_leaks();

        assert!(
            output.combined.contains("Created new worktree"),
            "{}: Should complete successfully",
            shell
        );
    }
}
