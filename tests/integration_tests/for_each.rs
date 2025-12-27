//! Integration tests for `wt step for-each`

use crate::common::{TestRepo, make_snapshot_cmd, repo, setup_snapshot_settings};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;

/// Helper to create snapshot for for-each command
fn snapshot_for_each(test_name: &str, repo: &TestRepo, args: &[&str]) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(repo, "step", args, None);
        assert_cmd_snapshot!(test_name, cmd);
    });
}

#[rstest]
fn test_for_each_single_worktree(repo: TestRepo) {
    // Only main worktree exists
    snapshot_for_each(
        "for_each_single_worktree",
        &repo,
        &["for-each", "--", "git", "status", "--short"],
    );
}

#[rstest]
fn test_for_each_multiple_worktrees(mut repo: TestRepo) {
    // Create additional worktrees
    repo.add_worktree("feature-a");
    repo.add_worktree("feature-b");

    snapshot_for_each(
        "for_each_multiple_worktrees",
        &repo,
        &["for-each", "--", "git", "branch", "--show-current"],
    );
}

#[rstest]
fn test_for_each_command_fails_in_one(mut repo: TestRepo) {
    repo.add_worktree("feature");

    // Use a command that will fail: try to show a non-existent ref
    snapshot_for_each(
        "for_each_command_fails",
        &repo,
        &["for-each", "--", "git", "show", "nonexistent-ref"],
    );
}

#[rstest]
fn test_for_each_no_args_error(repo: TestRepo) {
    // Missing arguments should show error
    snapshot_for_each("for_each_no_args", &repo, &["for-each"]);
}

#[rstest]
fn test_for_each_with_detached_head(mut repo: TestRepo) {
    // Create a worktree and detach its HEAD
    repo.add_worktree("detached-test");
    repo.detach_head_in_worktree("detached-test");

    snapshot_for_each(
        "for_each_with_detached",
        &repo,
        &["for-each", "--", "git", "status", "--short"],
    );
}

#[rstest]
fn test_for_each_with_template(repo: TestRepo) {
    // Test template expansion with {{ branch }}
    snapshot_for_each(
        "for_each_with_template",
        &repo,
        &["for-each", "--", "echo", "Branch: {{ branch }}"],
    );
}

#[rstest]
fn test_for_each_detached_branch_variable(mut repo: TestRepo) {
    // Regression test: {{ branch }} should expand to "HEAD" in detached worktrees
    // Previously, it incorrectly expanded to "(detached)" because the display
    // string was passed to CommandContext instead of None
    repo.add_worktree("detached-test");
    repo.detach_head_in_worktree("detached-test");

    snapshot_for_each(
        "for_each_detached_branch_variable",
        &repo,
        &["for-each", "--", "echo", "Branch: {{ branch }}"],
    );
}

#[rstest]
fn test_for_each_spawn_fails(mut repo: TestRepo) {
    // Test when command cannot be spawned (binary doesn't exist)
    repo.add_worktree("feature");

    snapshot_for_each(
        "for_each_spawn_fails",
        &repo,
        &["for-each", "--", "nonexistent-command-12345", "--some-arg"],
    );
}
