#![cfg(all(unix, feature = "shell-integration-tests"))]
//! PTY-based tests for interactive approval prompts
//!
//! These tests verify the approval workflow in a real PTY environment where stdin is a TTY.
//! This allows testing the actual interactive prompt behavior that users experience.
//!
//! Note: These tests are separate from `approval_ui.rs` because they require PTY setup
//! to simulate interactive terminals. The non-PTY tests in `approval_ui.rs` verify the
//! error case (non-TTY environments).
//!
//! TODO: PTY snapshots show environment-specific linebreak variations due to timing-dependent
//! buffering of input/output interleaving. Consider normalizing extra blank lines to make
//! snapshots more stable across different environments (local vs CI vs Claude Code web).

use crate::common::{TestRepo, repo};
use insta::assert_snapshot;
use insta_cmd::get_cargo_bin;
use portable_pty::CommandBuilder;
use rstest::rstest;
use std::io::{Read, Write};
use std::path::Path;

/// Execute a command in a PTY with interactive input
///
/// Returns (combined_output, exit_code)
fn exec_in_pty_with_input(
    command: &str,
    args: &[&str],
    working_dir: &Path,
    env_vars: &[(String, String)],
    input: &str,
) -> (String, i32) {
    let pair = crate::common::open_pty();

    let mut cmd = CommandBuilder::new(command);
    for arg in args {
        cmd.arg(arg);
    }
    cmd.cwd(working_dir);

    // Set up isolated environment with coverage passthrough
    crate::common::configure_pty_command(&mut cmd);

    // Add test-specific environment variables
    for (key, value) in env_vars {
        cmd.env(key, value);
    }

    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave); // Close slave in parent

    // Get reader and writer for the PTY master
    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();

    // Write input to the PTY (simulating user typing)
    writer.write_all(input.as_bytes()).unwrap();
    writer.flush().unwrap();
    drop(writer); // Close writer so command sees EOF

    // Read all output
    let mut buf = String::new();
    reader.read_to_string(&mut buf).unwrap();

    // Wait for child to exit
    let exit_status = child.wait().unwrap();
    let exit_code = exit_status.exit_code() as i32;

    (buf, exit_code)
}

/// Normalize output for snapshot testing
fn normalize_output(output: &str) -> String {
    // Remove platform-specific PTY control sequences
    // macOS PTYs emit ^D (literal caret-D) followed by backspaces (0x08),
    // while Linux PTYs don't. Strip these to ensure consistent snapshots.
    // Use multiline mode to match after user input (e.g., "n\n\n^D\b\b")
    let output = regex::Regex::new(r"\^D\x08+")
        .unwrap()
        .replace_all(output, "");

    // Remove repository paths (macOS uses .tmp dirs like /var/folders/.../test.tmpXXX)
    // Match repo.branch-name but not trailing ANSI codes (which start with escape char \x1b)
    let output = regex::Regex::new(r"/[^\s]+\.tmp[^\s/]+/repo\.[a-zA-Z0-9_-]+")
        .unwrap()
        .replace_all(&output, "_REPO_/repo.BRANCH");

    // Also normalize remaining .tmp paths (for non-worktree paths)
    let output = regex::Regex::new(r"/[^\s]+\.tmp[^\s/]+")
        .unwrap()
        .replace_all(&output, "_REPO_");

    // Normalize worktree paths that show as ~/repo.branch-name
    // When HOME is set to the test temp dir, format_path_for_display abbreviates to ~
    // Match branch names but not trailing ANSI codes
    let output = regex::Regex::new(r"~/repo\.[a-zA-Z0-9_-]+")
        .unwrap()
        .replace_all(&output, "_REPO_/repo.BRANCH");

    // Remove config paths
    let output = regex::Regex::new(r"/var/folders/[^\s]+/test-config\.toml")
        .unwrap()
        .replace_all(&output, "[CONFIG]");

    // Normalize binary path in shell integration hint (e.g., /path/to/target/debug/wt -> [BIN])
    // Also handles llvm-cov-target used by cargo-llvm-cov in coverage builds
    // The path may contain ANSI codes (bold), so match any non-whitespace before /target
    let output = regex::Regex::new(r"[^\s]+/target/(llvm-cov-target/)?debug/wt")
        .unwrap()
        .replace_all(&output, "[BIN]");

    // Normalize blank lines due to PTY timing variations
    // Different environments (local, CI, Claude Code web) may have blank lines
    // in different positions due to timing-dependent buffering.

    // PTYs use \r\n line endings, normalize to \n first
    let mut output_str = output.replace("\r\n", "\n");

    // Ensure blank line after user input (y or n)
    if output_str.starts_with("y\n") || output_str.starts_with("n\n") {
        // Check if there's already a blank line after y/n
        if !output_str.starts_with("y\n\n") && !output_str.starts_with("n\n\n") {
            // Add blank line after y/n
            output_str = output_str.replacen("y\n🟡", "y\n\n🟡", 1);
            output_str = output_str.replacen("n\n🟡", "n\n\n🟡", 1);
        }
    }

    // Remove blank line between prompt and subsequent message
    // The prompt ends with "] " followed by ANSI codes (like [0m or [22m), space, newline,
    // then we may have a blank line. Use regex to handle varying ANSI sequences.
    let blank_after_prompt = regex::Regex::new(r"\[y/N\](\x1b\[\d+m)* \n\n(🔄|⚪)").unwrap();
    output_str = blank_after_prompt
        .replace_all(&output_str, |caps: &regex::Captures| {
            format!(
                "[y/N]{} \n{}",
                caps.get(1).map_or("", |m| m.as_str()),
                &caps[2]
            )
        })
        .to_string();

    output_str
}

/// Get test env vars with shell integration configured.
///
/// This adds SHELL=/bin/zsh to the env vars, which is needed because:
/// - Tests write to .zshrc to simulate configured shell integration
/// - scan_shell_configs() uses $SHELL to determine which config file to check
/// - Without this, CI (which has SHELL=/bin/bash) wouldn't find the .zshrc config
fn test_env_vars_with_shell(repo: &TestRepo) -> Vec<(String, String)> {
    let mut env_vars = repo.test_env_vars();
    env_vars.push(("SHELL".to_string(), "/bin/zsh".to_string()));
    env_vars
}

#[rstest]
fn test_approval_prompt_accept(repo: TestRepo) {
    // Remove origin so worktrunk uses directory name as project identifier
    repo.run_git(&["remote", "remove", "origin"]);

    repo.write_project_config(r#"post-create = "echo 'test command'""#);
    repo.commit("Add config");

    // Configure shell integration so we get the "Restart shell" hint instead of the prompt
    repo.configure_shell_integration();
    let env_vars = test_env_vars_with_shell(&repo);
    let (output, exit_code) = exec_in_pty_with_input(
        get_cargo_bin("wt").to_str().unwrap(),
        &["switch", "--create", "test-approve"],
        repo.root_path(),
        &env_vars,
        "y\n",
    );

    let normalized = normalize_output(&output);
    assert_eq!(exit_code, 0);
    assert_snapshot!("approval_prompt_accept", normalized);
}

#[rstest]
fn test_approval_prompt_decline(repo: TestRepo) {
    // Remove origin so worktrunk uses directory name as project identifier
    repo.run_git(&["remote", "remove", "origin"]);

    repo.write_project_config(r#"post-create = "echo 'test command'""#);
    repo.commit("Add config");

    // Configure shell integration so we get the "Restart shell" hint instead of the prompt
    repo.configure_shell_integration();
    let env_vars = test_env_vars_with_shell(&repo);
    let (output, exit_code) = exec_in_pty_with_input(
        get_cargo_bin("wt").to_str().unwrap(),
        &["switch", "--create", "test-decline"],
        repo.root_path(),
        &env_vars,
        "n\n",
    );

    let normalized = normalize_output(&output);
    assert_eq!(exit_code, 0);
    assert_snapshot!("approval_prompt_decline", normalized);
}

#[rstest]
fn test_approval_prompt_multiple_commands(repo: TestRepo) {
    // Remove origin so worktrunk uses directory name as project identifier
    repo.run_git(&["remote", "remove", "origin"]);

    repo.write_project_config(
        r#"[post-create]
first = "echo 'First command'"
second = "echo 'Second command'"
third = "echo 'Third command'"
"#,
    );
    repo.commit("Add config");

    // Configure shell integration so we get the "Restart shell" hint instead of the prompt
    repo.configure_shell_integration();
    let env_vars = test_env_vars_with_shell(&repo);
    let (output, exit_code) = exec_in_pty_with_input(
        get_cargo_bin("wt").to_str().unwrap(),
        &["switch", "--create", "test-multi"],
        repo.root_path(),
        &env_vars,
        "y\n",
    );

    let normalized = normalize_output(&output);
    assert_eq!(exit_code, 0);
    assert_snapshot!("approval_prompt_multiple_commands", normalized);
}

/// TODO: Find a way to test permission errors without skipping when running as root.
/// See test_permission_error_prevents_save in approval_save.rs for details.
#[rstest]
fn test_approval_prompt_permission_error(repo: TestRepo) {
    // Remove origin so worktrunk uses directory name as project identifier
    repo.run_git(&["remote", "remove", "origin"]);

    repo.write_project_config(r#"post-create = "echo 'test command'""#);
    repo.commit("Add config");

    // Create config file and make it read-only to trigger permission error when saving approval
    let config_path = repo.test_config_path();
    #[cfg(unix)]
    {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        // Create the config file first
        fs::write(config_path, "# read-only config\n").unwrap();

        // Make it read-only
        let mut perms = fs::metadata(config_path).unwrap().permissions();
        perms.set_mode(0o444); // Read-only
        fs::set_permissions(config_path, perms).unwrap();

        // Test if permissions actually restrict us (skip if running as root)
        if fs::write(config_path, "test write").is_ok() {
            // Running as root - restore permissions and skip test
            let mut perms = fs::metadata(config_path).unwrap().permissions();
            perms.set_mode(0o644);
            fs::set_permissions(config_path, perms).unwrap();
            eprintln!("Skipping permission test - running with elevated privileges");
            return;
        }
    }

    // Configure shell integration so we get the "Restart shell" hint instead of the prompt
    repo.configure_shell_integration();
    let env_vars = test_env_vars_with_shell(&repo);
    let (output, exit_code) = exec_in_pty_with_input(
        get_cargo_bin("wt").to_str().unwrap(),
        &["switch", "--create", "test-permission"],
        repo.root_path(),
        &env_vars,
        "y\n",
    );

    let normalized = normalize_output(&output);
    assert_eq!(
        exit_code, 0,
        "Command should succeed even when saving approval fails"
    );
    assert!(
        normalized.contains("Failed to save command approval"),
        "Should show permission error warning"
    );
    assert!(
        normalized.contains("Approval will be requested again next time"),
        "Should show hint about approval being requested again"
    );
    assert!(
        normalized.contains("test command"),
        "Command should still execute despite save failure"
    );
    assert_snapshot!("approval_prompt_permission_error", normalized);
}

#[rstest]
fn test_approval_prompt_named_commands(repo: TestRepo) {
    // Remove origin so worktrunk uses directory name as project identifier
    repo.run_git(&["remote", "remove", "origin"]);

    repo.write_project_config(
        r#"[post-create]
install = "echo 'Installing dependencies...'"
build = "echo 'Building project...'"
test = "echo 'Running tests...'"
"#,
    );
    repo.commit("Add config");

    // Configure shell integration so we get the "Restart shell" hint instead of the prompt
    repo.configure_shell_integration();
    let env_vars = test_env_vars_with_shell(&repo);
    let (output, exit_code) = exec_in_pty_with_input(
        get_cargo_bin("wt").to_str().unwrap(),
        &["switch", "--create", "test-named"],
        repo.root_path(),
        &env_vars,
        "y\n",
    );

    let normalized = normalize_output(&output);
    assert_eq!(exit_code, 0);
    assert!(
        normalized.contains("install") && normalized.contains("Installing dependencies"),
        "Should show command name 'install' and execute it"
    );
    assert!(
        normalized.contains("build") && normalized.contains("Building project"),
        "Should show command name 'build' and execute it"
    );
    assert!(
        normalized.contains("test") && normalized.contains("Running tests"),
        "Should show command name 'test' and execute it"
    );
    assert_snapshot!("approval_prompt_named_commands", normalized);
}

#[rstest]
fn test_approval_prompt_mixed_approved_unapproved_accept(repo: TestRepo) {
    // Remove origin so worktrunk uses directory name as project identifier
    repo.run_git(&["remote", "remove", "origin"]);

    repo.write_project_config(
        r#"[post-create]
first = "echo 'First command'"
second = "echo 'Second command'"
third = "echo 'Third command'"
"#,
    );
    repo.commit("Add config");

    // Pre-approve the second command
    repo.write_test_config(&format!(
        r#"[projects."{}"]
approved-commands = ["echo 'Second command'"]
"#,
        repo.project_id()
    ));

    // Configure shell integration so we get the "Restart shell" hint instead of the prompt
    repo.configure_shell_integration();
    let env_vars = test_env_vars_with_shell(&repo);
    let (output, exit_code) = exec_in_pty_with_input(
        get_cargo_bin("wt").to_str().unwrap(),
        &["switch", "--create", "test-mixed-accept"],
        repo.root_path(),
        &env_vars,
        "y\n",
    );

    let normalized = normalize_output(&output);
    assert_eq!(exit_code, 0);

    // Check that only 2 commands are shown in the prompt (ANSI codes may be in between)
    assert!(
        normalized.contains("execute")
            && normalized.contains("2")
            && normalized.contains("command"),
        "Should show 2 unapproved commands in prompt"
    );
    assert!(
        normalized.contains("First command"),
        "Should execute first command"
    );
    assert!(
        normalized.contains("Second command"),
        "Should execute pre-approved second command"
    );
    assert!(
        normalized.contains("Third command"),
        "Should execute third command"
    );
    assert_snapshot!(
        "approval_prompt_mixed_approved_unapproved_accept",
        normalized
    );
}

#[rstest]
fn test_approval_prompt_mixed_approved_unapproved_decline(repo: TestRepo) {
    // Remove origin so worktrunk uses directory name as project identifier
    repo.run_git(&["remote", "remove", "origin"]);

    repo.write_project_config(
        r#"[post-create]
first = "echo 'First command'"
second = "echo 'Second command'"
third = "echo 'Third command'"
"#,
    );
    repo.commit("Add config");

    // Pre-approve the second command
    repo.write_test_config(&format!(
        r#"[projects."{}"]
approved-commands = ["echo 'Second command'"]
"#,
        repo.project_id()
    ));

    // Configure shell integration so we get the "Restart shell" hint instead of the prompt
    repo.configure_shell_integration();
    let env_vars = test_env_vars_with_shell(&repo);
    let (output, exit_code) = exec_in_pty_with_input(
        get_cargo_bin("wt").to_str().unwrap(),
        &["switch", "--create", "test-mixed-decline"],
        repo.root_path(),
        &env_vars,
        "n\n",
    );

    let normalized = normalize_output(&output);

    assert_eq!(
        exit_code, 0,
        "Command should succeed even when declined (worktree still created)"
    );
    // Check that only 2 commands are shown in the prompt (ANSI codes may be in between)
    assert!(
        normalized.contains("execute")
            && normalized.contains("2")
            && normalized.contains("command"),
        "Should show only 2 unapproved commands in prompt (not 3)"
    );
    // When declined, ALL commands are skipped (including pre-approved ones)
    assert!(
        normalized.contains("Commands declined"),
        "Should show 'Commands declined' message"
    );
    // Commands appear in the prompt, but should not be executed
    // Check for "Running post-create" which indicates execution
    assert!(
        !normalized.contains("Running post-create"),
        "Should NOT execute any commands when declined"
    );
    assert!(
        normalized.contains("Created branch") && normalized.contains("and worktree"),
        "Should still create worktree even when commands declined"
    );
    assert_snapshot!(
        "approval_prompt_mixed_approved_unapproved_decline",
        normalized
    );
}

#[rstest]
fn test_approval_prompt_remove_decline(repo: TestRepo) {
    // Remove origin so worktrunk uses directory name as project identifier
    repo.run_git(&["remote", "remove", "origin"]);

    // Create a worktree to remove
    let output = repo
        .wt_command()
        .args(["switch", "--create", "to-remove", "--yes"])
        .output()
        .unwrap();
    assert!(output.status.success(), "Initial switch should succeed");

    // Add pre-remove hook
    repo.write_project_config(r#"pre-remove = "echo 'pre-remove hook'""#);
    repo.commit("Add pre-remove config");

    // Configure shell integration
    repo.configure_shell_integration();
    let env_vars = test_env_vars_with_shell(&repo);

    // Decline the approval prompt
    let (output, exit_code) = exec_in_pty_with_input(
        get_cargo_bin("wt").to_str().unwrap(),
        &["remove", "to-remove"],
        repo.root_path(),
        &env_vars,
        "n\n",
    );

    let normalized = normalize_output(&output);
    assert_eq!(
        exit_code, 0,
        "Remove should succeed even when hooks declined"
    );
    assert!(
        normalized.contains("Commands declined"),
        "Should show 'Commands declined' message"
    );
    assert_snapshot!("approval_prompt_remove_decline", normalized);
}
