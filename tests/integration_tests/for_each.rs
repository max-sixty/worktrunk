//! Integration tests for `wt step for-each`

use crate::common::{TestRepo, make_snapshot_cmd, setup_snapshot_settings};
use insta_cmd::assert_cmd_snapshot;

/// Helper to create snapshot for for-each command
fn snapshot_for_each(test_name: &str, repo: &TestRepo, args: &[&str]) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(repo, "step", args, None);
        assert_cmd_snapshot!(test_name, cmd);
    });
}

/// Common setup for for-each tests - creates repo with initial commit
fn setup_repo() -> TestRepo {
    let repo = TestRepo::new();
    repo.commit("Initial commit");
    repo
}

#[test]
fn test_for_each_single_worktree() {
    let repo = setup_repo();

    // Only main worktree exists
    snapshot_for_each(
        "for_each_single_worktree",
        &repo,
        &["for-each", "--", "git", "status", "--short"],
    );
}

#[test]
fn test_for_each_multiple_worktrees() {
    let mut repo = setup_repo();

    // Create additional worktrees
    repo.add_worktree("feature-a");
    repo.add_worktree("feature-b");

    snapshot_for_each(
        "for_each_multiple_worktrees",
        &repo,
        &["for-each", "--", "git", "branch", "--show-current"],
    );
}

#[test]
fn test_for_each_command_fails_in_one() {
    let mut repo = setup_repo();

    repo.add_worktree("feature");

    // Use a command that will fail: try to show a non-existent ref
    snapshot_for_each(
        "for_each_command_fails",
        &repo,
        &["for-each", "--", "git", "show", "nonexistent-ref"],
    );
}

#[test]
fn test_for_each_no_args_error() {
    let repo = setup_repo();

    // Missing arguments should show error
    snapshot_for_each("for_each_no_args", &repo, &["for-each"]);
}

#[test]
fn test_for_each_with_detached_head() {
    let mut repo = setup_repo();

    // Create a worktree and detach its HEAD
    repo.add_worktree("detached-test");
    repo.detach_head_in_worktree("detached-test");

    snapshot_for_each(
        "for_each_with_detached",
        &repo,
        &["for-each", "--", "git", "status", "--short"],
    );
}

#[test]
fn test_for_each_with_template() {
    let repo = setup_repo();

    // Test template expansion with {{ branch }}
    snapshot_for_each(
        "for_each_with_template",
        &repo,
        &["for-each", "--", "echo", "Branch: {{ branch }}"],
    );
}
