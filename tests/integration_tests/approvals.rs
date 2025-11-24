//! Integration tests for ask-approvals and clear-approvals commands

use crate::common::{TestRepo, make_snapshot_cmd, setup_snapshot_settings};
use insta_cmd::assert_cmd_snapshot;
use worktrunk::config::WorktrunkConfig;

/// Helper to snapshot ask-approvals command
fn snapshot_ask_approvals(test_name: &str, repo: &TestRepo, args: &[&str]) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(repo, "config", &[], None);
        cmd.arg("approvals").arg("ask").args(args);
        assert_cmd_snapshot!(test_name, cmd);
    });
}

/// Helper to snapshot clear-approvals command
fn snapshot_clear_approvals(test_name: &str, repo: &TestRepo, args: &[&str]) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(repo, "config", &[], None);
        cmd.arg("approvals").arg("clear").args(args);
        assert_cmd_snapshot!(test_name, cmd);
    });
}

// ============================================================================
// ask-approvals tests
// ============================================================================

#[test]
fn test_ask_approvals_no_config() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    snapshot_ask_approvals("ask_approvals_no_config", &repo, &[]);
}

#[test]
fn test_ask_approvals_force() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.write_project_config(r#"post-create-command = "echo 'test'""#);
    repo.commit("Add config");

    snapshot_ask_approvals("ask_approvals_force", &repo, &["--force"]);
}

#[test]
fn test_ask_approvals_all_with_none_approved() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.write_project_config(r#"post-create-command = "echo 'test'""#);
    repo.commit("Add config");

    snapshot_ask_approvals("ask_approvals_all_none_approved", &repo, &["--all"]);
}

#[test]
fn test_ask_approvals_empty_config() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.write_project_config("");
    repo.commit("Add empty config");

    snapshot_ask_approvals("ask_approvals_empty_config", &repo, &[]);
}

// ============================================================================
// clear-approvals tests
// ============================================================================

#[test]
fn test_clear_approvals_no_approvals() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    snapshot_clear_approvals("clear_approvals_no_approvals", &repo, &[]);
}

#[test]
fn test_clear_approvals_with_approvals() {
    let repo = TestRepo::new();
    let project_id = format!("{}/origin", repo.root_path().display());
    repo.commit("Initial commit");
    repo.write_project_config(r#"post-create-command = "echo 'test'""#);
    repo.commit("Add config");

    // Manually approve the command by writing to test config
    let mut config = WorktrunkConfig::default();
    config
        .approve_command_to(
            project_id,
            "echo 'test'".to_string(),
            Some(repo.test_config_path()),
        )
        .unwrap();

    // Now clear approvals
    snapshot_clear_approvals("clear_approvals_with_approvals", &repo, &[]);
}

#[test]
fn test_clear_approvals_global_no_approvals() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    snapshot_clear_approvals("clear_approvals_global_no_approvals", &repo, &["--global"]);
}

#[test]
fn test_clear_approvals_global_with_approvals() {
    let repo = TestRepo::new();
    let project_id = format!("{}/origin", repo.root_path().display());
    repo.commit("Initial commit");
    repo.write_project_config(r#"post-create-command = "echo 'test'""#);
    repo.commit("Add config");

    // Manually approve the command
    let mut config = WorktrunkConfig::default();
    config
        .approve_command_to(
            project_id,
            "echo 'test'".to_string(),
            Some(repo.test_config_path()),
        )
        .unwrap();

    // Now clear all global approvals
    snapshot_clear_approvals(
        "clear_approvals_global_with_approvals",
        &repo,
        &["--global"],
    );
}

#[test]
fn test_clear_approvals_after_clear() {
    let repo = TestRepo::new();
    let project_id = format!("{}/origin", repo.root_path().display());
    repo.commit("Initial commit");
    repo.write_project_config(r#"post-create-command = "echo 'test'""#);
    repo.commit("Add config");

    // Manually approve the command
    let mut config = WorktrunkConfig::default();
    config
        .approve_command_to(
            project_id.clone(),
            "echo 'test'".to_string(),
            Some(repo.test_config_path()),
        )
        .unwrap();

    // Clear approvals
    let mut cmd = make_snapshot_cmd(&repo, "config", &[], None);
    cmd.arg("approvals").arg("clear");
    cmd.output().unwrap();

    // Try to clear again (should show "no approvals")
    snapshot_clear_approvals("clear_approvals_after_clear", &repo, &[]);
}

#[test]
fn test_clear_approvals_multiple_approvals() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.write_project_config(
        r#"
post-create-command = "echo 'first'"
post-start-command = "echo 'second'"
[pre-commit-command]
lint = "echo 'third'"
"#,
    );
    repo.commit("Add config with multiple commands");

    // Manually approve all commands
    let project_id = format!("{}/origin", repo.root_path().display());
    let mut config = WorktrunkConfig::default();
    config
        .approve_command_to(
            project_id.clone(),
            "echo 'first'".to_string(),
            Some(repo.test_config_path()),
        )
        .unwrap();
    config
        .approve_command_to(
            project_id.clone(),
            "echo 'second'".to_string(),
            Some(repo.test_config_path()),
        )
        .unwrap();
    config
        .approve_command_to(
            project_id,
            "echo 'third'".to_string(),
            Some(repo.test_config_path()),
        )
        .unwrap();

    // Now clear approvals (should show count of 3)
    snapshot_clear_approvals("clear_approvals_multiple_approvals", &repo, &[]);
}
