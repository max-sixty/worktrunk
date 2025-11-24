//! Tests for command approval UI

use crate::common::{TestRepo, make_snapshot_cmd, setup_snapshot_settings};
use insta_cmd::assert_cmd_snapshot;
use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

/// Helper to create snapshot with test environment
fn snapshot_approval(test_name: &str, repo: &TestRepo, args: &[&str], approve: bool) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(repo, "switch", args, None);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().unwrap();

        // Write approval response
        {
            let stdin = child.stdin.as_mut().unwrap();
            let response = if approve { b"y\n" } else { b"n\n" };
            stdin.write_all(response).unwrap();
        }

        let output = child.wait_with_output().unwrap();

        // Use insta snapshot for combined output
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!(
            "exit_code: {}\n----- stdout -----\n{}\n----- stderr -----\n{}",
            output.status.code().unwrap_or(-1),
            stdout,
            stderr
        );

        insta::assert_snapshot!(test_name, combined);
    });
}

#[test]
fn test_approval_single_command() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    repo.write_project_config(r#"post-create-command = "echo 'Worktree path: {{ worktree }}'""#);

    repo.commit("Add config");

    snapshot_approval(
        "approval_single_command",
        &repo,
        &["--create", "feature/test-approval"],
        false,
    );
}

#[test]
fn test_approval_multiple_commands() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    repo.write_project_config(
        r#"post-create-command = [
    "echo 'Branch: {{ branch }}'",
    "echo 'Worktree: {{ worktree }}'",
    "echo 'Repo: {{ main_worktree }}'",
    "cd {{ worktree }} && pwd"
]"#,
    );

    repo.commit("Add config");

    snapshot_approval(
        "approval_multiple_commands",
        &repo,
        &["--create", "test/nested-branch"],
        false,
    );
}

#[test]
fn test_approval_mixed_approved_unapproved() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    repo.write_project_config(
        r#"post-create-command = [
    "echo 'First command'",
    "echo 'Second command'",
    "echo 'Third command'"
]"#,
    );

    repo.commit("Add config");

    // Pre-approve the second command
    let project_id = repo.root_path().file_name().unwrap().to_str().unwrap();
    repo.write_test_config(&format!(
        r#"[projects."{}"]
approved-commands = ["echo 'Second command'"]
"#,
        project_id
    ));

    snapshot_approval(
        "approval_mixed_approved_unapproved",
        &repo,
        &["--create", "test-mixed"],
        false,
    );
}

#[test]
fn test_force_flag_does_not_save_approvals() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    repo.write_project_config(r#"post-create-command = "echo 'test command' > output.txt""#);

    repo.commit("Add config");

    // Run with --force
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(
            &repo,
            "switch",
            &["--create", "test-force", "--force"],
            None,
        );
        assert_cmd_snapshot!("force_does_not_save_approvals_first_run", cmd);
    });

    // Clean up the worktree
    let mut cmd = Command::new(insta_cmd::get_cargo_bin("wt"));
    repo.clean_cli_env(&mut cmd);
    cmd.arg("remove")
        .arg("test-force")
        .arg("--force")
        .current_dir(repo.root_path());
    cmd.output().unwrap();

    // Run again WITHOUT --force - should prompt
    snapshot_approval(
        "force_does_not_save_approvals_second_run",
        &repo,
        &["--create", "test-force-2"],
        false,
    );
}

#[test]
fn test_already_approved_commands_skip_prompt() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    repo.write_project_config(r#"post-create-command = "echo 'approved' > output.txt""#);

    repo.commit("Add config");

    // Pre-approve the command
    let project_id = repo.root_path().file_name().unwrap().to_str().unwrap();
    repo.write_test_config(&format!(
        r#"[projects."{}"]
approved-commands = ["echo 'approved' > output.txt"]
"#,
        project_id
    ));

    // Should execute without prompting
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["--create", "test-approved"], None);
        assert_cmd_snapshot!("already_approved_skip_prompt", cmd);
    });
}

#[test]
fn test_decline_approval_skips_only_unapproved() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    repo.write_project_config(
        r#"post-create-command = [
    "echo 'First command'",
    "echo 'Second command'",
    "echo 'Third command'"
]"#,
    );

    repo.commit("Add config");

    // Pre-approve the second command
    let project_id = repo.root_path().file_name().unwrap().to_str().unwrap();
    fs::write(
        repo.test_config_path(),
        format!(
            r#"[projects."{}"]
approved-commands = ["echo 'Second command'"]
"#,
            project_id
        ),
    )
    .unwrap();

    snapshot_approval(
        "decline_approval_skips_only_unapproved",
        &repo,
        &["--create", "test-decline"],
        false,
    );
}

#[test]
fn test_approval_named_commands() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    repo.write_project_config(
        r#"[post-create-command]
install = "echo 'Installing dependencies...'"
build = "echo 'Building project...'"
test = "echo 'Running tests...'"
"#,
    );

    repo.commit("Add config");

    snapshot_approval(
        "approval_named_commands",
        &repo,
        &["--create", "test-named"],
        false,
    );
}

/// Helper for step hook snapshot tests with approval prompt
fn snapshot_run_hook(test_name: &str, repo: &TestRepo, hook_type: &str, approve: bool) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(repo, "step", &[hook_type], None);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().unwrap();

        // Write approval response
        {
            let stdin = child.stdin.as_mut().unwrap();
            let response = if approve { b"y\n" } else { b"n\n" };
            stdin.write_all(response).unwrap();
        }

        let output = child.wait_with_output().unwrap();

        // Use insta snapshot for combined output
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!(
            "exit_code: {}\n----- stdout -----\n{}\n----- stderr -----\n{}",
            output.status.code().unwrap_or(-1),
            stdout,
            stderr
        );

        insta::assert_snapshot!(test_name, combined);
    });
}

/// Test that `wt step pre-merge` requires approval (security boundary test)
///
/// This verifies the fix for the security issue where step hooks were bypassing approval.
/// Before the fix, pre-merge hooks ran with auto_trust=true, skipping approval prompts.
#[test]
fn test_run_hook_pre_merge_requires_approval() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    repo.write_project_config(
        r#"pre-merge-command = "echo 'Running pre-merge checks on {{ branch }}'""#,
    );

    repo.commit("Add pre-merge hook");

    // Decline approval to verify the prompt appears
    snapshot_run_hook(
        "run_hook_pre_merge_requires_approval",
        &repo,
        "pre-merge",
        false,
    );
}

/// Test that `wt step post-merge` requires approval (security boundary test)
///
/// This verifies the fix for the security issue where step hooks were bypassing approval.
/// Before the fix, post-merge hooks ran with auto_trust=true, skipping approval prompts.
#[test]
fn test_run_hook_post_merge_requires_approval() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    repo.write_project_config(
        r#"post-merge-command = "echo 'Post-merge cleanup for {{ branch }}'""#,
    );

    repo.commit("Add post-merge hook");

    // Decline approval to verify the prompt appears
    snapshot_run_hook(
        "run_hook_post_merge_requires_approval",
        &repo,
        "post-merge",
        false,
    );
}

/// Test that approval fails in non-TTY environment with clear error message
///
/// When stdin is not a TTY (e.g., CI/CD, piped input), approval prompts cannot be shown.
/// The command should fail with a clear error telling users to use --force.
#[test]
fn test_approval_fails_in_non_tty() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    repo.write_project_config(r#"post-create-command = "echo 'test command'""#);
    repo.commit("Add config");

    // Run WITHOUT piping stdin - this simulates non-TTY environment
    // When running under cargo test, stdin is not a TTY
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["--create", "test-non-tty"], None);
        assert_cmd_snapshot!("approval_fails_in_non_tty", cmd);
    });
}

/// Test that --force flag bypasses TTY requirement
///
/// Even in non-TTY environments, --force should allow commands to execute.
#[test]
fn test_force_bypasses_tty_check() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    repo.write_project_config(r#"post-create-command = "echo 'test command'""#);
    repo.commit("Add config");

    // Run with --force to bypass approval entirely
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(
            &repo,
            "switch",
            &["--create", "test-force-tty", "--force"],
            None,
        );
        assert_cmd_snapshot!("force_bypasses_tty_check", cmd);
    });
}
