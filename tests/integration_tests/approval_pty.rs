#![cfg(all(unix, feature = "shell-integration-tests"))]
//! PTY-based tests for interactive approval prompts
//!
//! These tests verify the approval workflow in a real PTY environment where stdin is a TTY.
//! This allows testing the actual interactive prompt behavior that users experience.
//!
//! Note: These tests are separate from `approval_ui.rs` because they require PTY setup
//! to simulate interactive terminals. The non-PTY tests in `approval_ui.rs` verify the
//! error case (non-TTY environments).

use crate::common::pty::exec_in_pty;
use crate::common::{TestRepo, add_pty_binary_path_filters, add_pty_filters, repo};
use insta::assert_snapshot;
use insta_cmd::get_cargo_bin;
use rstest::rstest;

/// Execute wt in a PTY with interactive input.
///
/// Thin wrapper around `exec_in_pty` that passes the wt binary path.
fn exec_wt_in_pty(
    repo: &TestRepo,
    args: &[&str],
    env_vars: &[(String, String)],
    input: &str,
) -> (String, i32) {
    exec_in_pty(
        get_cargo_bin("wt").to_str().unwrap(),
        args,
        repo.root_path(),
        env_vars,
        input,
    )
}

/// Create insta settings for approval PTY tests.
///
/// Uses shared PTY filters plus test-specific normalizations for config file paths.
fn approval_pty_settings(repo: &TestRepo) -> insta::Settings {
    let mut settings = crate::common::setup_snapshot_settings(repo);

    // Add PTY-specific filters (CRLF, ^D, ANSI resets)
    add_pty_filters(&mut settings);

    // Binary path normalization
    add_pty_binary_path_filters(&mut settings);

    // Config paths specific to these tests
    settings.add_filter(r"/var/folders/[^\s]+/test-config\.toml", "[CONFIG]");

    settings
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
    let (output, exit_code) = exec_wt_in_pty(
        &repo,
        &["switch", "--create", "test-approve"],
        &env_vars,
        "y\n",
    );

    assert_eq!(exit_code, 0);
    approval_pty_settings(&repo).bind(|| {
        assert_snapshot!("approval_prompt_accept", &output);
    });
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
    let (output, exit_code) = exec_wt_in_pty(
        &repo,
        &["switch", "--create", "test-decline"],
        &env_vars,
        "n\n",
    );

    assert_eq!(exit_code, 0);
    approval_pty_settings(&repo).bind(|| {
        assert_snapshot!("approval_prompt_decline", &output);
    });
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
    let (output, exit_code) = exec_wt_in_pty(
        &repo,
        &["switch", "--create", "test-multi"],
        &env_vars,
        "y\n",
    );

    assert_eq!(exit_code, 0);
    approval_pty_settings(&repo).bind(|| {
        assert_snapshot!("approval_prompt_multiple_commands", &output);
    });
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
    let (output, exit_code) = exec_wt_in_pty(
        &repo,
        &["switch", "--create", "test-permission"],
        &env_vars,
        "y\n",
    );

    assert_eq!(
        exit_code, 0,
        "Command should succeed even when saving approval fails"
    );
    assert!(
        output.contains("Failed to save command approval"),
        "Should show permission error warning"
    );
    assert!(
        output.contains("Approval will be requested again next time"),
        "Should show hint about approval being requested again"
    );
    assert!(
        output.contains("test command"),
        "Command should still execute despite save failure"
    );
    approval_pty_settings(&repo).bind(|| {
        assert_snapshot!("approval_prompt_permission_error", &output);
    });
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
    let (output, exit_code) = exec_wt_in_pty(
        &repo,
        &["switch", "--create", "test-named"],
        &env_vars,
        "y\n",
    );

    assert_eq!(exit_code, 0);
    assert!(
        output.contains("install") && output.contains("Installing dependencies"),
        "Should show command name 'install' and execute it"
    );
    assert!(
        output.contains("build") && output.contains("Building project"),
        "Should show command name 'build' and execute it"
    );
    assert!(
        output.contains("test") && output.contains("Running tests"),
        "Should show command name 'test' and execute it"
    );
    approval_pty_settings(&repo).bind(|| {
        assert_snapshot!("approval_prompt_named_commands", &output);
    });
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
    let (output, exit_code) = exec_wt_in_pty(
        &repo,
        &["switch", "--create", "test-mixed-accept"],
        &env_vars,
        "y\n",
    );

    assert_eq!(exit_code, 0);

    // Check that only 2 commands are shown in the prompt (ANSI codes may be in between)
    assert!(
        output.contains("execute") && output.contains("2") && output.contains("command"),
        "Should show 2 unapproved commands in prompt"
    );
    assert!(
        output.contains("First command"),
        "Should execute first command"
    );
    assert!(
        output.contains("Second command"),
        "Should execute pre-approved second command"
    );
    assert!(
        output.contains("Third command"),
        "Should execute third command"
    );
    approval_pty_settings(&repo).bind(|| {
        assert_snapshot!("approval_prompt_mixed_approved_unapproved_accept", &output);
    });
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
    let (output, exit_code) = exec_wt_in_pty(
        &repo,
        &["switch", "--create", "test-mixed-decline"],
        &env_vars,
        "n\n",
    );

    assert_eq!(
        exit_code, 0,
        "Command should succeed even when declined (worktree still created)"
    );
    // Check that only 2 commands are shown in the prompt (ANSI codes may be in between)
    assert!(
        output.contains("execute") && output.contains("2") && output.contains("command"),
        "Should show only 2 unapproved commands in prompt (not 3)"
    );
    // When declined, ALL commands are skipped (including pre-approved ones)
    assert!(
        output.contains("Commands declined"),
        "Should show 'Commands declined' message"
    );
    // Commands appear in the prompt, but should not be executed
    // Check for "Running post-create" which indicates execution
    assert!(
        !output.contains("Running post-create"),
        "Should NOT execute any commands when declined"
    );
    assert!(
        output.contains("Created branch") && output.contains("and worktree"),
        "Should still create worktree even when commands declined"
    );
    approval_pty_settings(&repo).bind(|| {
        assert_snapshot!("approval_prompt_mixed_approved_unapproved_decline", &output);
    });
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
    let (output, exit_code) = exec_wt_in_pty(&repo, &["remove", "to-remove"], &env_vars, "n\n");

    assert_eq!(
        exit_code, 0,
        "Remove should succeed even when hooks declined"
    );
    assert!(
        output.contains("Commands declined"),
        "Should show 'Commands declined' message"
    );
    approval_pty_settings(&repo).bind(|| {
        assert_snapshot!("approval_prompt_remove_decline", &output);
    });
}
