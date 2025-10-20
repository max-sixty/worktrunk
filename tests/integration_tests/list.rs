use crate::common::TestRepo;
use insta::Settings;
use insta_cmd::{assert_cmd_snapshot, get_cargo_bin};
use std::process::Command;

/// Helper to create snapshot with normalized paths and SHAs
fn snapshot_list(test_name: &str, repo: &TestRepo) {
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    // Normalize paths - replace absolute paths with semantic names
    settings.add_filter(repo.root_path().to_str().unwrap(), "[REPO]");
    for (name, path) in &repo.worktrees {
        settings.add_filter(
            path.to_str().unwrap(),
            format!("[WORKTREE_{}]", name.to_uppercase().replace('-', "_")),
        );
    }

    // Normalize git SHAs (7-40 hex chars) to [SHA] padded to 8 chars
    settings.add_filter(r"\b[0-9a-f]{7,40}\b", "[SHA]   ");

    // Normalize Windows paths to Unix style
    settings.add_filter(r"\\", "/");

    settings.bind(|| {
        let mut cmd = Command::new(get_cargo_bin("wt"));
        // Clean environment to avoid interference from global git config
        repo.clean_cli_env(&mut cmd);
        cmd.arg("list").current_dir(repo.root_path());

        assert_cmd_snapshot!(test_name, cmd);
    });
}

#[test]
fn test_list_single_worktree() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    snapshot_list("single_worktree", &repo);
}

#[test]
fn test_list_multiple_worktrees() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    repo.add_worktree("feature-a", "feature-a");
    repo.add_worktree("feature-b", "feature-b");

    snapshot_list("multiple_worktrees", &repo);
}

#[test]
fn test_list_detached_head() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    repo.detach_head();

    snapshot_list("detached_head", &repo);
}

#[test]
fn test_list_locked_worktree() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    repo.add_worktree("locked-feature", "locked-feature");
    repo.lock_worktree("locked-feature", Some("Testing lock functionality"));

    snapshot_list("locked_worktree", &repo);
}

#[test]
fn test_list_locked_no_reason() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    repo.add_worktree("locked-no-reason", "locked-no-reason");
    repo.lock_worktree("locked-no-reason", None);

    snapshot_list("locked_no_reason", &repo);
}

#[test]
fn test_list_long_branch_name() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create worktree with very long branch name
    repo.add_worktree(
        "feature-this-is-a-very-long-branch-name-that-should-test-column-alignment",
        "feature-this-is-a-very-long-branch-name-that-should-test-column-alignment",
    );

    snapshot_list("long_branch_name", &repo);
}

#[test]
fn test_list_long_commit_message() {
    let mut repo = TestRepo::new();

    // Create commit with very long message
    repo.commit("This is a very long commit message that should test how the message column handles truncation and word boundary detection in the list output");

    repo.add_worktree("feature-a", "feature-a");
    repo.commit("Short message");

    snapshot_list("long_commit_message", &repo);
}

#[test]
fn test_list_unicode_branch_name() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create worktree with Unicode in branch name
    repo.add_worktree("feature-日本語-test", "feature-日本語-test");
    repo.add_worktree("fix-émoji-🎉", "fix-émoji-🎉");

    snapshot_list("unicode_branch_name", &repo);
}

#[test]
fn test_list_unicode_commit_message() {
    let mut repo = TestRepo::new();

    // Create commit with Unicode message
    repo.commit("Add support for 日本語 and émoji 🎉");

    repo.add_worktree("feature-test", "feature-test");
    repo.commit("Fix bug with café ☕ handling");

    snapshot_list("unicode_commit_message", &repo);
}

#[test]
fn test_list_many_worktrees_with_varied_stats() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create multiple worktrees with different characteristics
    repo.add_worktree("short", "short");

    repo.add_worktree("medium-name", "medium-name");

    repo.add_worktree("very-long-branch-name-here", "very-long-branch-name-here");

    // Add some with files to create diff stats
    repo.add_worktree("with-changes", "with-changes");

    snapshot_list("many_worktrees_varied", &repo);
}
