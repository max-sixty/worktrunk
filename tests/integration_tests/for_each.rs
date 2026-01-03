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
