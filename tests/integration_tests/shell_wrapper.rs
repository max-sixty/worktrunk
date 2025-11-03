//! Shell wrapper integration tests
//!
//! Tests that verify the complete shell integration path - commands executed through
//! the actual shell wrapper (_wt_exec in bash/zsh/fish).
//!
//! These tests ensure that:
//! - Directives are never leaked to users
//! - Output is properly formatted for humans
//! - Shell integration works end-to-end as users experience it

use crate::common::TestRepo;
use insta::assert_snapshot;
use insta_cmd::get_cargo_bin;
use std::fs;
use std::process::Command;
use std::sync::LazyLock;

/// Regex for normalizing temporary directory paths in test snapshots
static TMPDIR_REGEX: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(/private/var/folders/[^/]+/[^/]+/T/\.tmp[^/]+|/tmp/\.tmp[^/]+)")
        .expect("Invalid tmpdir regex pattern")
});

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

    /// Normalize paths in output for snapshot testing
    fn normalized(&self) -> std::borrow::Cow<'_, str> {
        TMPDIR_REGEX.replace_all(&self.combined, "[TMPDIR]")
    }
}

/// Generate a shell wrapper script using the actual `wt init` command
fn generate_wrapper(repo: &TestRepo, shell: &str) -> String {
    let wt_bin = get_cargo_bin("wt");

    let mut cmd = Command::new(&wt_bin);
    cmd.arg("init").arg(shell);

    // Configure environment
    repo.clean_cli_env(&mut cmd);

    let output = cmd
        .output()
        .unwrap_or_else(|_| panic!("Failed to run wt init {}", shell));

    if !output.status.success() {
        panic!(
            "wt init {} failed with exit code: {:?}\nOutput:\n{}",
            shell,
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    String::from_utf8(output.stdout)
        .unwrap_or_else(|_| panic!("wt init {} produced invalid UTF-8", shell))
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

/// Execute a command in a PTY (pseudo-terminal)
///
/// This provides the most accurate test environment - child processes see a real TTY
/// and behave exactly as they would in a user's terminal (streaming, colors, etc.)
#[cfg(test)]
fn exec_in_pty(
    shell: &str,
    script: &str,
    working_dir: &std::path::Path,
    env_vars: &[(String, String)],
) -> (String, i32) {
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};
    use std::io::Read;

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

    // For zsh, isolate from user rc files to prevent TTY access issues
    // User startup files (~/.zshenv, ~/.zshrc) can touch /dev/tty (e.g., stty, zle,
    // compinit, GPG_TTY=$(tty)) which causes SIGTTIN/TTOU/TSTP signals when the
    // process tries to access the controlling terminal.
    // See: https://zsh.sourceforge.io/Doc/Release/Files.html
    if shell == "zsh" {
        // Set ZDOTDIR to /dev/null so ~/.zshenv is not found
        cmd.env("ZDOTDIR", "/dev/null");

        // Prevent loading any other rc files
        cmd.arg("--no-rcs");
        cmd.arg("-o");
        cmd.arg("NO_GLOBAL_RCS");
        cmd.arg("-o");
        cmd.arg("NO_RCS");
        cmd.arg("-o");
        cmd.arg("NO_MONITOR");
    }

    cmd.arg("-c");
    cmd.arg(script);
    cmd.cwd(working_dir);

    // Add environment variables
    for (key, value) in env_vars {
        cmd.env(key, value);
    }

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .expect("Failed to spawn command in PTY");
    drop(pair.slave); // Close slave in parent

    // Read everything the "terminal" would display
    let mut reader = pair
        .master
        .try_clone_reader()
        .expect("Failed to clone PTY reader");

    let mut buf = String::new();
    reader
        .read_to_string(&mut buf)
        .expect("Failed to read from PTY"); // Blocks until child exits & PTY closes

    let status = child.wait().expect("Failed to wait for child process");

    (normalize_newlines(&buf), status.exit_code() as i32)
}

/// Normalize line endings (CRLF -> LF)
fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n")
}

/// Execute a command through a shell wrapper
///
/// This simulates what actually happens when users run `wt switch`, etc. in their shell:
/// 1. The `wt` function is defined (from shell integration)
/// 2. It calls `_wt_exec --internal switch ...`
/// 3. The wrapper parses NUL-delimited output and handles directives
/// 4. Users see only the final human-friendly output
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
    let script = build_shell_script(shell, repo, subcommand, args);

    // PTY execution - provides exact terminal behavior for all shells
    // Note: Fish's psub (process substitution) uses file-backed buffers, which causes
    // different temporal ordering than bash/zsh. Child command output may appear before
    // the progress messages that spawned them. This is actual fish behavior, not a test bug.

    // Collect environment variables that clean_cli_env would set
    let env_vars = vec![
        ("CLICOLOR_FORCE".to_string(), "1".to_string()),
        (
            "WORKTRUNK_CONFIG_PATH".to_string(),
            repo.test_config_path().to_string_lossy().to_string(),
        ),
        // Git config from configure_git_cmd
        ("GIT_AUTHOR_NAME".to_string(), "Test User".to_string()),
        (
            "GIT_AUTHOR_EMAIL".to_string(),
            "test@example.com".to_string(),
        ),
        ("GIT_COMMITTER_NAME".to_string(), "Test User".to_string()),
        (
            "GIT_COMMITTER_EMAIL".to_string(),
            "test@example.com".to_string(),
        ),
        (
            "GIT_AUTHOR_DATE".to_string(),
            "2025-10-28T12:00:00Z".to_string(),
        ),
        (
            "GIT_COMMITTER_DATE".to_string(),
            "2025-10-28T12:00:00Z".to_string(),
        ),
        ("LANG".to_string(), "C".to_string()),
        ("LC_ALL".to_string(), "C".to_string()),
        ("SOURCE_DATE_EPOCH".to_string(), "1761609600".to_string()),
    ];

    let (combined, exit_code) = exec_in_pty(shell, &script, working_dir, &env_vars);

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
            assert_snapshot!("command_failure", output.normalized().as_ref());
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
            assert_snapshot!("switch_create", output.normalized().as_ref());
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
            assert_snapshot!("remove", output.normalized().as_ref());
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
            assert_snapshot!("merge", output.normalized().as_ref());
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
            assert_snapshot!("switch_with_execute", output.normalized().as_ref());
        }
    }

    /// Test switch --create with post-create-command (blocking) and post-start-command (background)
    // TODO: Fix non-deterministic test - PTY buffering causes inconsistent output ordering
    // All shells (bash/zsh/fish) have varying output order due to background post-start commands
    // Fish fails ~50% in CI, bash/zsh also use shell-specific snapshots due to ordering issues
    // #[rstest]
    // #[case("bash")]
    // #[case("zsh")]
    // #[case("fish")]
    #[allow(dead_code)]
    fn test_wrapper_switch_with_hooks(shell: &str) {
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
            r#"worktree-path = "../{main-worktree}.{branch}"

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
        assert_snapshot!(
            format!("switch_with_hooks_{}", shell),
            output.normalized().as_ref()
        );
    }

    /// Test merge with successful pre-merge-command validation
    // TODO: Fix non-deterministic test - PTY buffering causes inconsistent output ordering
    // Shell-specific snapshots exist but still produce inconsistent results across runs
    // #[rstest]
    // #[case("bash")]
    // #[case("zsh")]
    // #[case("fish")]
    #[allow(dead_code)]
    fn test_wrapper_merge_with_pre_merge_success(shell: &str) {
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
            r#"worktree-path = "../{main-worktree}.{branch}"

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
            output.normalized().as_ref()
        );
    }

    /// Test merge with failing pre-merge-command that aborts the merge
    // TODO: Fix non-deterministic test - PTY buffering causes inconsistent output ordering
    // Shell-specific snapshots exist but still produce inconsistent results across runs
    // #[rstest]
    // #[case("bash")]
    // #[case("zsh")]
    // #[case("fish")]
    #[allow(dead_code)]
    fn test_wrapper_merge_with_pre_merge_failure(shell: &str) {
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
            r#"worktree-path = "../{main-worktree}.{branch}"

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
            output.normalized().as_ref()
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
            r#"worktree-path = "../{main-worktree}.{branch}"

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
        assert_snapshot!(output.normalized().as_ref());
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
        assert_snapshot!(output.normalized().as_ref());
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

        assert_snapshot!(output.normalized().as_ref());
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
            r#"worktree-path = "../{main-worktree}.{branch}"

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
        assert_snapshot!(output.normalized().as_ref());
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
            r#"worktree-path = "../{main-worktree}.{branch}"

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

        assert_snapshot!(output.normalized().as_ref());
    }

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
        assert_snapshot!(output.normalized().as_ref());
    }

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
        assert_snapshot!(output.normalized().as_ref());
    }
}
