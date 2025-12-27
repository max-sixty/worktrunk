//! Integration tests for add-approvals and clear-approvals commands

use crate::common::{TestRepo, make_snapshot_cmd, repo, setup_snapshot_settings};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use worktrunk::config::WorktrunkConfig;

/// Helper to snapshot add-approvals command
fn snapshot_add_approvals(test_name: &str, repo: &TestRepo, args: &[&str]) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(repo, "hook", &[], None);
        cmd.arg("approvals").arg("add").args(args);
        assert_cmd_snapshot!(test_name, cmd);
    });
}

/// Helper to snapshot clear-approvals command
fn snapshot_clear_approvals(test_name: &str, repo: &TestRepo, args: &[&str]) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(repo, "hook", &[], None);
        cmd.arg("approvals").arg("clear").args(args);
        assert_cmd_snapshot!(test_name, cmd);
    });
}

// ============================================================================
// add-approvals tests
// ============================================================================

#[rstest]
fn test_add_approvals_no_config(repo: TestRepo) {
    snapshot_add_approvals("add_approvals_no_config", &repo, &[]);
}

#[rstest]
fn test_add_approvals_all_with_none_approved(repo: TestRepo) {
    repo.write_project_config(r#"post-create = "echo 'test'""#);
    repo.commit("Add config");

    snapshot_add_approvals("add_approvals_all_none_approved", &repo, &["--all"]);
}

#[rstest]
fn test_add_approvals_empty_config(repo: TestRepo) {
    repo.write_project_config("");
    repo.commit("Add empty config");

    snapshot_add_approvals("add_approvals_empty_config", &repo, &[]);
}

// ============================================================================
// clear-approvals tests
// ============================================================================

#[rstest]
fn test_clear_approvals_no_approvals(repo: TestRepo) {
    snapshot_clear_approvals("clear_approvals_no_approvals", &repo, &[]);
}

#[rstest]
fn test_clear_approvals_with_approvals(repo: TestRepo) {
    let project_id = format!("{}/origin", repo.root_path().display());
    repo.commit("Initial commit");
    repo.write_project_config(r#"post-create = "echo 'test'""#);
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

#[rstest]
fn test_clear_approvals_global_no_approvals(repo: TestRepo) {
    snapshot_clear_approvals("clear_approvals_global_no_approvals", &repo, &["--global"]);
}

#[rstest]
fn test_clear_approvals_global_with_approvals(repo: TestRepo) {
    let project_id = format!("{}/origin", repo.root_path().display());
    repo.commit("Initial commit");
    repo.write_project_config(r#"post-create = "echo 'test'""#);
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

#[rstest]
fn test_clear_approvals_after_clear(repo: TestRepo) {
    let project_id = format!("{}/origin", repo.root_path().display());
    repo.commit("Initial commit");
    repo.write_project_config(r#"post-create = "echo 'test'""#);
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
    let mut cmd = make_snapshot_cmd(&repo, "hook", &[], None);
    cmd.arg("approvals").arg("clear");
    cmd.output().unwrap();

    // Try to clear again (should show "no approvals")
    snapshot_clear_approvals("clear_approvals_after_clear", &repo, &[]);
}

#[rstest]
fn test_clear_approvals_multiple_approvals(repo: TestRepo) {
    repo.write_project_config(
        r#"
post-create = "echo 'first'"
post-start = "echo 'second'"
[pre-commit]
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

// ============================================================================
// add-approvals additional coverage tests
// ============================================================================

#[rstest]
fn test_add_approvals_all_already_approved(repo: TestRepo) {
    let project_id = format!("{}/origin", repo.root_path().display());
    repo.commit("Initial commit");
    repo.write_project_config(r#"post-create = "echo 'test'""#);
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

    // Try to add approvals - should show "all already approved"
    snapshot_add_approvals("add_approvals_all_already_approved", &repo, &[]);
}

#[rstest]
fn test_add_approvals_project_config_no_commands(repo: TestRepo) {
    // Create project config with only non-hook settings
    repo.write_project_config(
        r#"# Project config without any hook sections
worktree-path = "../project.{{ branch }}"
"#,
    );
    repo.commit("Add config without hooks");

    // Try to add approvals - should show "no commands configured"
    snapshot_add_approvals("add_approvals_no_commands", &repo, &[]);
}
