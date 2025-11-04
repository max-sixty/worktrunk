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

        let mut child = cmd.spawn().expect("Failed to spawn command");

        // Write approval response
        {
            let stdin = child.stdin.as_mut().expect("Failed to get stdin");
            let response = if approve { b"y\n" } else { b"n\n" };
            stdin.write_all(response).expect("Failed to write to stdin");
        }

        let output = child
            .wait_with_output()
            .expect("Failed to wait for command");

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

    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create .config dir");
    fs::write(
        config_dir.join("wt.toml"),
        r#"post-create-command = "echo 'Worktree path: {worktree}'""#,
    )
    .expect("Failed to write config");

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

    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create .config dir");
    fs::write(
        config_dir.join("wt.toml"),
        r#"post-create-command = [
    "echo 'Branch: {branch}'",
    "echo 'Worktree: {worktree}'",
    "echo 'Repo: {main-worktree}'",
    "cd {worktree} && pwd"
]"#,
    )
    .expect("Failed to write config");

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

    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create .config dir");
    fs::write(
        config_dir.join("wt.toml"),
        r#"post-create-command = [
    "echo 'First command'",
    "echo 'Second command'",
    "echo 'Third command'"
]"#,
    )
    .expect("Failed to write config");

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
    .expect("Failed to write test config");

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

    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create .config dir");
    fs::write(
        config_dir.join("wt.toml"),
        r#"post-create-command = "echo 'test command' > output.txt""#,
    )
    .expect("Failed to write config");

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
    cmd.output().expect("Failed to remove worktree");

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

    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create .config dir");
    fs::write(
        config_dir.join("wt.toml"),
        r#"post-create-command = "echo 'approved' > output.txt""#,
    )
    .expect("Failed to write config");

    repo.commit("Add config");

    // Pre-approve the command
    let project_id = repo.root_path().file_name().unwrap().to_str().unwrap();
    fs::write(
        repo.test_config_path(),
        format!(
            r#"[projects."{}"]
approved-commands = ["echo 'approved' > output.txt"]
"#,
            project_id
        ),
    )
    .expect("Failed to write test config");

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

    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create .config dir");
    fs::write(
        config_dir.join("wt.toml"),
        r#"post-create-command = [
    "echo 'First command'",
    "echo 'Second command'",
    "echo 'Third command'"
]"#,
    )
    .expect("Failed to write config");

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
    .expect("Failed to write test config");

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

    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create .config dir");
    fs::write(
        config_dir.join("wt.toml"),
        r#"[post-create-command]
install = "echo 'Installing dependencies...'"
build = "echo 'Building project...'"
test = "echo 'Running tests...'"
"#,
    )
    .expect("Failed to write config");

    repo.commit("Add config");

    snapshot_approval(
        "approval_named_commands",
        &repo,
        &["--create", "test-named"],
        false,
    );
}
