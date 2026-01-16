//! Tests for command approval UI

use crate::common::{TestRepo, make_snapshot_cmd, repo};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::fs;
use std::io::Write;
use std::process::Stdio;

/// Helper to create snapshot with test environment
fn snapshot_approval(test_name: &str, repo: &TestRepo, args: &[&str], approve: bool) {
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
}

#[rstest]
fn test_approval_single_command(repo: TestRepo) {
    repo.write_project_config(r#"post-create = "echo 'Worktree path: {{ worktree_path }}'""#);

    repo.commit("Add config");

    snapshot_approval(
        "approval_single_command",
        &repo,
        &["--create", "feature/test-approval"],
        false,
    );
}

#[rstest]
fn test_approval_multiple_commands(repo: TestRepo) {
    repo.write_project_config(
        r#"[post-create]
branch = "echo 'Branch: {{ branch }}'"
worktree = "echo 'Worktree: {{ worktree_path }}'"
repo = "echo 'Repo: {{ repo }}'"
pwd = "cd {{ worktree_path }} && pwd"
"#,
    );

    repo.commit("Add config");

    snapshot_approval(
        "approval_multiple_commands",
        &repo,
        &["--create", "test/nested-branch"],
        false,
    );
}

#[rstest]
fn test_approval_mixed_approved_unapproved(repo: TestRepo) {
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

    snapshot_approval(
        "approval_mixed_approved_unapproved",
        &repo,
        &["--create", "test-mixed"],
        false,
    );
}

#[rstest]
fn test_yes_flag_does_not_save_approvals(repo: TestRepo) {
    repo.write_project_config(r#"post-create = "echo 'test command' > output.txt""#);

    repo.commit("Add config");

    // Run with --yes
    assert_cmd_snapshot!(
        "yes_does_not_save_approvals_first_run",
        make_snapshot_cmd(&repo, "switch", &["--create", "test-yes", "--yes"], None)
    );

    // Clean up the worktree
    repo.wt_command()
        .args(["remove", "test-yes", "--yes"])
        .output()
        .unwrap();

    // Run again WITHOUT --yes - should prompt
    snapshot_approval(
        "yes_does_not_save_approvals_second_run",
        &repo,
        &["--create", "test-yes-2"],
        false,
    );
}

#[rstest]
fn test_already_approved_commands_skip_prompt(repo: TestRepo) {
    // Remove origin so worktrunk uses directory name as project identifier
    repo.run_git(&["remote", "remove", "origin"]);

    repo.write_project_config(r#"post-create = "echo 'approved' > output.txt""#);

    repo.commit("Add config");

    // Pre-approve the command
    repo.write_test_config(&format!(
        r#"[projects."{}"]
approved-commands = ["echo 'approved' > output.txt"]
"#,
        repo.project_id()
    ));

    // Should execute without prompting
    assert_cmd_snapshot!(
        "already_approved_skip_prompt",
        make_snapshot_cmd(&repo, "switch", &["--create", "test-approved"], None)
    );
}

#[rstest]
fn test_decline_approval_skips_only_unapproved(repo: TestRepo) {
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
    fs::write(
        repo.test_config_path(),
        format!(
            r#"[projects."{}"]
approved-commands = ["echo 'Second command'"]
"#,
            repo.project_id()
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

#[rstest]
fn test_approval_named_commands(repo: TestRepo) {
    repo.write_project_config(
        r#"[post-create]
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
    let mut cmd = make_snapshot_cmd(repo, "hook", &[hook_type], None);
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
}

///
/// This verifies the fix for the security issue where hooks were bypassing approval.
/// Before the fix, pre-merge hooks ran with auto_trust=true, skipping approval prompts.
#[rstest]
fn test_run_hook_pre_merge_requires_approval(repo: TestRepo) {
    repo.write_project_config(r#"pre-merge = "echo 'Running pre-merge checks on {{ branch }}'""#);

    repo.commit("Add pre-merge hook");

    // Decline approval to verify the prompt appears
    snapshot_run_hook(
        "run_hook_pre_merge_requires_approval",
        &repo,
        "pre-merge",
        false,
    );
}

///
/// This verifies the fix for the security issue where hooks were bypassing approval.
/// Before the fix, post-merge hooks ran with auto_trust=true, skipping approval prompts.
#[rstest]
fn test_run_hook_post_merge_requires_approval(repo: TestRepo) {
    repo.write_project_config(r#"post-merge = "echo 'Post-merge cleanup for {{ branch }}'""#);

    repo.commit("Add post-merge hook");

    // Decline approval to verify the prompt appears
    snapshot_run_hook(
        "run_hook_post_merge_requires_approval",
        &repo,
        "post-merge",
        false,
    );
}

///
/// When stdin is not a TTY (e.g., CI/CD, piped input), approval prompts cannot be shown.
/// The command should fail with a clear error telling users to use --yes.
#[rstest]
fn test_approval_fails_in_non_tty(repo: TestRepo) {
    repo.write_project_config(r#"post-create = "echo 'test command'""#);
    repo.commit("Add config");

    // Run WITHOUT piping stdin - this simulates non-TTY environment
    // When running under cargo test, stdin is not a TTY
    assert_cmd_snapshot!(
        "approval_fails_in_non_tty",
        make_snapshot_cmd(&repo, "switch", &["--create", "test-non-tty"], None)
    );
}

///
/// Even in non-TTY environments, --yes should allow commands to execute.
#[rstest]
fn test_yes_bypasses_tty_check(repo: TestRepo) {
    repo.write_project_config(r#"post-create = "echo 'test command'""#);
    repo.commit("Add config");

    // Run with --yes to bypass approval entirely
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "switch",
        &["--create", "test-yes-tty", "--yes"],
        None
    ));
}

///
/// When `wt hook post-merge` runs standalone (not via `wt merge`), the `{{ target }}`
/// variable should be the current branch, not always the default branch.
/// This allows hooks to behave correctly when testing from feature worktrees.
#[rstest]
fn test_hook_post_merge_target_is_current_branch(repo: TestRepo) {
    // Hook that writes {{ target }} to a file so we can verify its value
    repo.write_project_config(r#"post-merge = "echo '{{ target }}' > target-branch.txt""#);
    repo.commit("Add post-merge hook");

    // Create and switch to a feature branch
    repo.run_git(&["checkout", "-b", "my-feature-branch"]);

    // Run the hook with --yes to skip approval
    let output = repo
        .wt_command()
        .args(["hook", "post-merge", "--yes"])
        .env("NO_COLOR", "1")
        .output()
        .expect("Failed to run wt hook post-merge");

    assert!(
        output.status.success(),
        "wt hook post-merge failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify {{ target }} was set to the current branch, not "main"
    let target_file = repo.root_path().join("target-branch.txt");
    let target_content = fs::read_to_string(&target_file).expect("target-branch.txt should exist");

    assert_eq!(
        target_content.trim(),
        "my-feature-branch",
        "{{ target }} should be current branch, not default branch"
    );
}

#[rstest]
fn test_hook_pre_merge_target_is_current_branch(repo: TestRepo) {
    // Hook that writes {{ target }} to a file so we can verify its value
    repo.write_project_config(r#"pre-merge = "echo '{{ target }}' > target-branch.txt""#);
    repo.commit("Add pre-merge hook");

    // Create and switch to a feature branch
    repo.run_git(&["checkout", "-b", "my-feature-branch"]);

    // Run the hook with --yes to skip approval
    let output = repo
        .wt_command()
        .args(["hook", "pre-merge", "--yes"])
        .env("NO_COLOR", "1")
        .output()
        .expect("Failed to run wt hook pre-merge");

    assert!(
        output.status.success(),
        "wt hook pre-merge failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify {{ target }} was set to the current branch, not "main"
    let target_file = repo.root_path().join("target-branch.txt");
    let target_content = fs::read_to_string(&target_file).expect("target-branch.txt should exist");

    assert_eq!(
        target_content.trim(),
        "my-feature-branch",
        "{{ target }} should be current branch, not default branch"
    );
}

#[rstest]
fn test_step_hook_run_named_command(repo: TestRepo) {
    // Config with multiple named commands
    repo.write_project_config(
        r#"[pre-merge]
test = "echo 'running test' > test.txt"
lint = "echo 'running lint' > lint.txt"
build = "echo 'running build' > build.txt"
"#,
    );
    repo.commit("Add pre-merge hooks");

    // Run only the "lint" command with --yes to skip approval
    let output = repo
        .wt_command()
        .args(["hook", "pre-merge", "lint", "--yes"])
        .env("NO_COLOR", "1")
        .output()
        .expect("Failed to run wt hook pre-merge lint");

    assert!(
        output.status.success(),
        "wt hook pre-merge lint failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Only lint.txt should exist
    assert!(
        repo.root_path().join("lint.txt").exists(),
        "lint.txt should exist (lint command ran)"
    );
    assert!(
        !repo.root_path().join("test.txt").exists(),
        "test.txt should NOT exist (test command should not have run)"
    );
    assert!(
        !repo.root_path().join("build.txt").exists(),
        "build.txt should NOT exist (build command should not have run)"
    );
}

#[rstest]
fn test_step_hook_unknown_name_error(repo: TestRepo) {
    // Config with multiple named commands
    repo.write_project_config(
        r#"[pre-merge]
test = "echo 'test'"
lint = "echo 'lint'"
"#,
    );
    repo.commit("Add pre-merge hooks");

    // Run with a name that doesn't exist
    assert_cmd_snapshot!(
        "step_hook_unknown_name_error",
        make_snapshot_cmd(&repo, "hook", &["pre-merge", "nonexistent", "--yes"], None)
    );
}

#[rstest]
fn test_step_hook_name_filter_on_unnamed_command(repo: TestRepo) {
    // Config with a single unnamed command (no table)
    repo.write_project_config(r#"pre-merge = "echo 'test'""#);
    repo.commit("Add pre-merge hook");

    // Run with a name filter on a hook that has no named commands
    assert_cmd_snapshot!(
        "step_hook_name_filter_on_unnamed",
        make_snapshot_cmd(&repo, "hook", &["pre-merge", "test", "--yes"], None)
    );
}

/// Helper for step hook snapshot tests with extra args and approval prompt
fn snapshot_run_hook_with_args(test_name: &str, repo: &TestRepo, args: &[&str], approve: bool) {
    let mut cmd = make_snapshot_cmd(repo, "hook", args, None);
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
}

///
/// This verifies the fix for the approval bypass vulnerability where `project:name`
/// filter syntax was not correctly parsed by the approval gate, allowing project
/// hooks to run without approval.
#[rstest]
fn test_project_prefix_requires_approval(repo: TestRepo) {
    repo.write_project_config(
        r#"[pre-merge]
test = "echo 'Running project test'"
"#,
    );
    repo.commit("Add pre-merge hook");

    // Running with project: prefix should still require approval
    // Decline to verify the prompt appears
    snapshot_run_hook_with_args(
        "project_prefix_requires_approval",
        &repo,
        &["pre-merge", "project:test"],
        false,
    );
}

#[rstest]
fn test_project_prefix_all_requires_approval(repo: TestRepo) {
    repo.write_project_config(
        r#"[pre-merge]
test = "echo 'Running project test'"
lint = "echo 'Running project lint'"
"#,
    );
    repo.commit("Add pre-merge hooks");

    // Running with project: (no name) should require approval for all project hooks
    snapshot_run_hook_with_args(
        "project_prefix_all_requires_approval",
        &repo,
        &["pre-merge", "project:"],
        false,
    );
}

#[rstest]
fn test_user_prefix_skips_approval(repo: TestRepo) {
    // Set up user config with a hook
    repo.write_test_config(
        r#"[pre-merge]
test = "echo 'user test'"
"#,
    );

    // Running with user: prefix should not prompt for approval
    assert_cmd_snapshot!(
        "user_prefix_skips_approval",
        make_snapshot_cmd(&repo, "hook", &["pre-merge", "user:test"], None)
    );
}

#[rstest]
fn test_step_hook_run_all_commands(repo: TestRepo) {
    // Config with multiple named commands
    repo.write_project_config(
        r#"[pre-merge]
first = "echo 'first' >> output.txt"
second = "echo 'second' >> output.txt"
third = "echo 'third' >> output.txt"
"#,
    );
    repo.commit("Add pre-merge hooks");

    // Run without name filter (all commands should run)
    let output = repo
        .wt_command()
        .args(["hook", "pre-merge", "--yes"])
        .env("NO_COLOR", "1")
        .output()
        .expect("Failed to run wt hook pre-merge");

    assert!(
        output.status.success(),
        "wt hook pre-merge failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // All three commands should have written to output.txt
    let output_file = repo.root_path().join("output.txt");
    let content = fs::read_to_string(&output_file).expect("output.txt should exist");
    let lines: Vec<&str> = content.lines().collect();

    assert_eq!(
        lines,
        vec!["first", "second", "third"],
        "All commands should have run in order"
    );
}

///
/// The select command requires an interactive terminal for its TUI.
/// When stdin is not a TTY, it should fail with a clear error.
#[cfg(unix)] // select command is unix-only
#[rstest]
fn test_select_fails_in_non_tty(repo: TestRepo) {
    assert_cmd_snapshot!(
        "select_fails_in_non_tty",
        make_snapshot_cmd(&repo, "select", &[], None)
    );
}
