//! Integration tests for `wt step for-each`

use crate::common::{TestRepo, make_snapshot_cmd, repo};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;

#[rstest]
fn test_for_each_single_worktree(repo: TestRepo) {
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["for-each", "--", "git", "status", "--short"],
        None,
    ));
}

#[rstest]
fn test_for_each_multiple_worktrees(mut repo: TestRepo) {
    repo.add_worktree("feature-a");
    repo.add_worktree("feature-b");

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["for-each", "--", "git", "branch", "--show-current"],
        None,
    ));
}

#[rstest]
fn test_for_each_command_fails_in_one(mut repo: TestRepo) {
    repo.add_worktree("feature");

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["for-each", "--", "git", "show", "nonexistent-ref"],
        None,
    ));
}

#[rstest]
fn test_for_each_no_args_error(repo: TestRepo) {
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "step", &["for-each"], None));
}

#[rstest]
fn test_for_each_with_detached_head(mut repo: TestRepo) {
    repo.add_worktree("detached-test");
    repo.detach_head_in_worktree("detached-test");

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["for-each", "--", "git", "status", "--short"],
        None,
    ));
}

#[rstest]
fn test_for_each_with_template(repo: TestRepo) {
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["for-each", "--", "echo", "Branch: {{ branch }}"],
        None,
    ));
}

#[rstest]
fn test_for_each_detached_branch_variable(mut repo: TestRepo) {
    repo.add_worktree("detached-test");
    repo.detach_head_in_worktree("detached-test");

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["for-each", "--", "echo", "Branch: {{ branch }}"],
        None,
    ));
}

#[rstest]
fn test_for_each_spawn_fails(mut repo: TestRepo) {
    repo.add_worktree("feature");

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["for-each", "--", "nonexistent-command-12345", "--some-arg"],
        None,
    ));
}

#[rstest]
fn test_for_each_skips_prunable_worktrees(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature");
    // Delete the worktree directory to make it prunable
    std::fs::remove_dir_all(&worktree_path).unwrap();

    // Verify git sees it as prunable
    let output = repo
        .git_command()
        .args(["worktree", "list", "--porcelain"])
        .run()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("prunable"),
        "Expected worktree to be prunable after deleting directory"
    );

    // wt step for-each should skip the prunable worktree and complete without errors
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["for-each", "--", "echo", "Running in {{ branch }}"],
        None,
    ));
}

// ============================================================================
// --format=json
// ============================================================================

#[rstest]
fn test_for_each_json(mut repo: TestRepo) {
    repo.commit("initial");
    repo.add_worktree("feature");

    let output = repo
        .wt_command()
        .args(["step", "for-each", "--format=json", "--", "true"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();
    let items = json.as_array().unwrap();
    assert!(items.len() >= 2, "expected at least 2 worktrees");
    for item in items {
        assert_eq!(item["success"], true);
        assert_eq!(item["exit_code"], 0);
        assert!(item["path"].as_str().is_some());
    }
    // feature worktree should be in results
    assert!(
        items.iter().any(|i| i["branch"] == "feature"),
        "feature branch should be in results"
    );
}

#[rstest]
fn test_for_each_json_with_failure(repo: TestRepo) {
    repo.commit("initial");

    let output = repo
        .wt_command()
        .args(["step", "for-each", "--format=json", "--", "false"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();
    let items = json.as_array().unwrap();
    assert!(!items.is_empty());
    for item in items {
        assert_eq!(item["success"], false);
        assert_eq!(item["exit_code"], 1);
        // error field is always present on failure (both ExitCode and SpawnFailed)
        assert_eq!(item["error"], "exit code 1");
    }
}
