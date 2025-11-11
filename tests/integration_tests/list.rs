use crate::common::{TestRepo, list_snapshots, wt_command};
use insta::Settings;
use insta_cmd::assert_cmd_snapshot;
use std::path::Path;
use std::process::Command;

fn snapshot_list(test_name: &str, repo: &TestRepo) {
    run_snapshot(
        list_snapshots::standard_settings(repo),
        test_name,
        list_snapshots::command(repo, repo.root_path()),
    );
}

fn snapshot_list_from_dir(test_name: &str, repo: &TestRepo, cwd: &Path) {
    run_snapshot(
        list_snapshots::standard_settings(repo),
        test_name,
        list_snapshots::command(repo, cwd),
    );
}

fn snapshot_list_json(test_name: &str, repo: &TestRepo) {
    run_snapshot(
        list_snapshots::json_settings(repo),
        test_name,
        list_snapshots::command_json(repo),
    );
}

fn snapshot_list_with_branches(test_name: &str, repo: &TestRepo) {
    run_snapshot(
        list_snapshots::standard_settings(repo),
        test_name,
        list_snapshots::command_branches(repo),
    );
}

fn run_snapshot(settings: Settings, test_name: &str, mut cmd: Command) {
    settings.bind(|| {
        assert_cmd_snapshot!(test_name, cmd);
    });
}

/// Helper to create a branch without a worktree
fn create_branch(repo: &TestRepo, branch_name: &str) {
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["branch", branch_name])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to create branch");
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

// Removed: test_list_long_branch_name - covered by spacing_edge_cases.rs

#[test]
fn test_list_long_commit_message() {
    let mut repo = TestRepo::new();

    // Create commit with very long message
    repo.commit("This is a very long commit message that should test how the message column handles truncation and word boundary detection in the list output");

    repo.add_worktree("feature-a", "feature-a");
    repo.commit("Short message");

    snapshot_list("long_commit_message", &repo);
}

// Removed: test_list_unicode_branch_name - covered by spacing_edge_cases.rs

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

// Removed: test_list_json_single_worktree and test_list_json_multiple_worktrees
// Basic JSON serialization is covered by test_list_json_with_metadata

#[test]
fn test_list_json_with_metadata() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create worktree with detached head
    repo.add_worktree("feature-detached", "feature-detached");

    // Create locked worktree
    repo.add_worktree("locked-feature", "locked-feature");
    repo.lock_worktree("locked-feature", Some("Testing"));

    snapshot_list_json("json_with_metadata", &repo);
}

#[test]
fn test_list_with_branches_flag() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create some branches without worktrees
    create_branch(&repo, "feature-without-worktree");
    create_branch(&repo, "another-branch");
    create_branch(&repo, "fix-bug");

    // Create one branch with a worktree
    repo.add_worktree("feature-with-worktree", "feature-with-worktree");

    snapshot_list_with_branches("with_branches_flag", &repo);
}

#[test]
fn test_list_with_branches_flag_no_available() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // All branches have worktrees (only main exists and has worktree)
    repo.add_worktree("feature-a", "feature-a");
    repo.add_worktree("feature-b", "feature-b");

    snapshot_list_with_branches("with_branches_flag_none_available", &repo);
}

#[test]
fn test_list_with_branches_flag_only_branches() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create several branches without worktrees
    create_branch(&repo, "branch-alpha");
    create_branch(&repo, "branch-beta");
    create_branch(&repo, "branch-gamma");

    snapshot_list_with_branches("with_branches_flag_only_branches", &repo);
}

#[test]
fn test_list_json_with_display_fields() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit on main");

    // Create feature branch with commits (ahead of main)
    repo.add_worktree("feature-ahead", "feature-ahead");

    // Make commits in the feature worktree
    let feature_path = repo.worktree_path("feature-ahead");
    std::fs::write(feature_path.join("feature.txt"), "feature content")
        .expect("Failed to write file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(feature_path)
        .output()
        .expect("Failed to git add");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Feature commit 1"])
        .current_dir(feature_path)
        .output()
        .expect("Failed to commit");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "--allow-empty", "-m", "Feature commit 2"])
        .current_dir(feature_path)
        .output()
        .expect("Failed to commit");

    // Add uncommitted changes to show working_diff_display
    std::fs::write(feature_path.join("uncommitted.txt"), "uncommitted")
        .expect("Failed to write file");
    std::fs::write(feature_path.join("feature.txt"), "modified content")
        .expect("Failed to write file");

    // Create another feature that will be behind after main advances
    repo.add_worktree("feature-behind", "feature-behind");

    // Make more commits on main (so feature-behind is behind)
    repo.commit("Main commit 1");
    repo.commit("Main commit 2");

    snapshot_list_json("json_with_display_fields", &repo);
}

#[test]
fn test_list_ordering_rules() {
    let mut repo = TestRepo::new();

    // Create main with earliest timestamp (00:00)
    repo.commit("Initial commit on main");

    // Create worktrees in non-chronological order to prove we sort by timestamp

    // 1. Create feature-current (01:00) - we'll run test from here
    let current_path = repo.add_worktree("feature-current", "feature-current");
    {
        // Create commit with timestamp 01:00
        let file_path = current_path.join("current.txt");
        std::fs::write(&file_path, "current content").expect("Failed to write file");

        let mut cmd = Command::new("git");
        repo.configure_git_cmd(&mut cmd);
        cmd.env("GIT_AUTHOR_DATE", "2025-01-01T01:00:00Z");
        cmd.env("GIT_COMMITTER_DATE", "2025-01-01T01:00:00Z");
        cmd.args(["add", "."])
            .current_dir(&current_path)
            .output()
            .expect("Failed to git add");

        let mut cmd = Command::new("git");
        repo.configure_git_cmd(&mut cmd);
        cmd.env("GIT_AUTHOR_DATE", "2025-01-01T01:00:00Z");
        cmd.env("GIT_COMMITTER_DATE", "2025-01-01T01:00:00Z");
        cmd.args(["commit", "-m", "Commit at 01:00"])
            .current_dir(&current_path)
            .output()
            .expect("Failed to git commit");
    }

    // 2. Create feature-newest (03:00) - most recent, should be 3rd
    let newest_path = repo.add_worktree("feature-newest", "feature-newest");
    {
        let file_path = newest_path.join("newest.txt");
        std::fs::write(&file_path, "newest content").expect("Failed to write file");

        let mut cmd = Command::new("git");
        repo.configure_git_cmd(&mut cmd);
        cmd.env("GIT_AUTHOR_DATE", "2025-01-01T03:00:00Z");
        cmd.env("GIT_COMMITTER_DATE", "2025-01-01T03:00:00Z");
        cmd.args(["add", "."])
            .current_dir(&newest_path)
            .output()
            .expect("Failed to git add");

        let mut cmd = Command::new("git");
        repo.configure_git_cmd(&mut cmd);
        cmd.env("GIT_AUTHOR_DATE", "2025-01-01T03:00:00Z");
        cmd.env("GIT_COMMITTER_DATE", "2025-01-01T03:00:00Z");
        cmd.args(["commit", "-m", "Commit at 03:00"])
            .current_dir(&newest_path)
            .output()
            .expect("Failed to git commit");
    }

    // 3. Create feature-middle (02:00) - should be 4th
    let middle_path = repo.add_worktree("feature-middle", "feature-middle");
    {
        let file_path = middle_path.join("middle.txt");
        std::fs::write(&file_path, "middle content").expect("Failed to write file");

        let mut cmd = Command::new("git");
        repo.configure_git_cmd(&mut cmd);
        cmd.env("GIT_AUTHOR_DATE", "2025-01-01T02:00:00Z");
        cmd.env("GIT_COMMITTER_DATE", "2025-01-01T02:00:00Z");
        cmd.args(["add", "."])
            .current_dir(&middle_path)
            .output()
            .expect("Failed to git add");

        let mut cmd = Command::new("git");
        repo.configure_git_cmd(&mut cmd);
        cmd.env("GIT_AUTHOR_DATE", "2025-01-01T02:00:00Z");
        cmd.env("GIT_COMMITTER_DATE", "2025-01-01T02:00:00Z");
        cmd.args(["commit", "-m", "Commit at 02:00"])
            .current_dir(&middle_path)
            .output()
            .expect("Failed to git commit");
    }

    // 4. Create feature-oldest (00:30) - should be 5th
    let oldest_path = repo.add_worktree("feature-oldest", "feature-oldest");
    {
        let file_path = oldest_path.join("oldest.txt");
        std::fs::write(&file_path, "oldest content").expect("Failed to write file");

        let mut cmd = Command::new("git");
        repo.configure_git_cmd(&mut cmd);
        cmd.env("GIT_AUTHOR_DATE", "2025-01-01T00:30:00Z");
        cmd.env("GIT_COMMITTER_DATE", "2025-01-01T00:30:00Z");
        cmd.args(["add", "."])
            .current_dir(&oldest_path)
            .output()
            .expect("Failed to git add");

        let mut cmd = Command::new("git");
        repo.configure_git_cmd(&mut cmd);
        cmd.env("GIT_AUTHOR_DATE", "2025-01-01T00:30:00Z");
        cmd.env("GIT_COMMITTER_DATE", "2025-01-01T00:30:00Z");
        cmd.args(["commit", "-m", "Commit at 00:30"])
            .current_dir(&oldest_path)
            .output()
            .expect("Failed to git commit");
    }

    // Run from feature-current worktree to test "current worktree" logic
    snapshot_list_from_dir("list_ordering_rules", &repo, &current_path);
}

#[test]
fn test_list_with_upstream_tracking() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit on main");

    // Set up remote - this already pushes main
    repo.setup_remote("main");

    // Scenario 1: Branch in sync with remote (should show ↑0 ↓0)
    let in_sync_wt = repo.add_worktree("in-sync", "in-sync");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["push", "-u", "origin", "in-sync"])
        .current_dir(&in_sync_wt)
        .output()
        .expect("Failed to push in-sync");

    // Scenario 2: Branch ahead of remote (should show ↑2)
    let ahead_wt = repo.add_worktree("ahead", "ahead");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["push", "-u", "origin", "ahead"])
        .current_dir(&ahead_wt)
        .output()
        .expect("Failed to push ahead");

    // Make 2 commits ahead
    std::fs::write(ahead_wt.join("ahead1.txt"), "ahead 1").expect("Failed to write");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(&ahead_wt)
        .output()
        .expect("Failed to add");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Ahead commit 1"])
        .current_dir(&ahead_wt)
        .output()
        .expect("Failed to commit");

    std::fs::write(ahead_wt.join("ahead2.txt"), "ahead 2").expect("Failed to write");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(&ahead_wt)
        .output()
        .expect("Failed to add");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Ahead commit 2"])
        .current_dir(&ahead_wt)
        .output()
        .expect("Failed to commit");

    // Scenario 3: Branch behind remote (should show ↓1)
    let behind_wt = repo.add_worktree("behind", "behind");
    std::fs::write(behind_wt.join("behind.txt"), "behind").expect("Failed to write");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(&behind_wt)
        .output()
        .expect("Failed to add");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Behind commit"])
        .current_dir(&behind_wt)
        .output()
        .expect("Failed to commit");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["push", "-u", "origin", "behind"])
        .current_dir(&behind_wt)
        .output()
        .expect("Failed to push");
    // Reset local to one commit behind
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["reset", "--hard", "HEAD~1"])
        .current_dir(&behind_wt)
        .output()
        .expect("Failed to reset");

    // Scenario 4: Branch both ahead and behind (should show ↑1 ↓1)
    let diverged_wt = repo.add_worktree("diverged", "diverged");
    std::fs::write(diverged_wt.join("diverged.txt"), "diverged").expect("Failed to write");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(&diverged_wt)
        .output()
        .expect("Failed to add");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Diverged remote commit"])
        .current_dir(&diverged_wt)
        .output()
        .expect("Failed to commit");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["push", "-u", "origin", "diverged"])
        .current_dir(&diverged_wt)
        .output()
        .expect("Failed to push");
    // Reset and make different commit
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["reset", "--hard", "HEAD~1"])
        .current_dir(&diverged_wt)
        .output()
        .expect("Failed to reset");
    std::fs::write(diverged_wt.join("different.txt"), "different").expect("Failed to write");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(&diverged_wt)
        .output()
        .expect("Failed to add");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Diverged local commit"])
        .current_dir(&diverged_wt)
        .output()
        .expect("Failed to commit");

    // Scenario 5: Branch without upstream (should show blank)
    repo.add_worktree("no-upstream", "no-upstream");

    // Run list --branches --full to show all columns including Remote
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    // Normalize paths
    settings.add_filter(repo.root_path().to_str().unwrap(), "[REPO]");
    for (name, path) in &repo.worktrees {
        settings.add_filter(
            path.to_str().unwrap(),
            format!("[WORKTREE_{}]", name.to_uppercase().replace('-', "_")),
        );
    }
    settings.add_filter(r"\b[0-9a-f]{7,40}\b", "[SHA]   ");
    settings.add_filter(r"\\", "/");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("list")
            .arg("--branches")
            .arg("--full")
            .current_dir(repo.root_path());
        assert_cmd_snapshot!("with_upstream_tracking", cmd);
    });
}

#[test]
fn test_list_primary_on_different_branch() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    repo.switch_primary_to("develop");
    assert_eq!(repo.current_branch(), "develop");

    repo.add_worktree("feature-a", "feature-a");
    repo.add_worktree("feature-b", "feature-b");

    snapshot_list("list_primary_on_different_branch", &repo);
}

#[test]
fn test_list_with_user_status() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Worktree with user status only (no git changes)
    repo.add_worktree("clean-with-status", "clean-with-status");

    // Set user status (emoji only, branch-keyed)
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["config", "worktrunk.status.clean-with-status", "💬"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to set user status");

    // Worktree with both git status and user status
    let dirty_wt = repo.add_worktree("dirty-with-status", "dirty-with-status");

    // Set user status (emoji only, branch-keyed)
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["config", "worktrunk.status.dirty-with-status", "🤖"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to set user status");

    // Add uncommitted changes
    std::fs::write(dirty_wt.join("new.txt"), "content").expect("Failed to write file");

    // Worktree with git status only (no user status)
    let dirty_no_status_wt = repo.add_worktree("dirty-no-status", "dirty-no-status");
    std::fs::write(dirty_no_status_wt.join("file.txt"), "content").expect("Failed to write file");

    // Worktree with neither (control)
    repo.add_worktree("clean-no-status", "clean-no-status");

    snapshot_list("with_user_status", &repo);
}

#[test]
fn test_list_json_with_user_status() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Worktree with user status (emoji only)
    repo.add_worktree("with-status", "with-status");

    // Set user status (branch-keyed)
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["config", "worktrunk.status.with-status", "🔧"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to set user status");

    // Worktree without user status
    repo.add_worktree("without-status", "without-status");

    snapshot_list_json("json_with_user_status", &repo);
}

#[test]
fn test_list_branch_only_ignores_main_worktree_status() {
    // Regression test for bug where branch-only entries incorrectly inherited
    // main worktree's worktree-specific status
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Set worktree-specific status on main worktree (requires extensions.worktreeConfig)
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["config", "extensions.worktreeConfig", "true"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to enable worktreeConfig");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["config", "--worktree", "worktrunk.status", "🏠"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to set main worktree status");

    // Create a branch-only entry (no worktree)
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["branch", "branch-only"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to create branch");

    // Set branch-keyed status for the branch-only entry
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["config", "worktrunk.status.branch-only", "🌿"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to set branch status");

    // Use --branches flag to show branch-only entries
    snapshot_list_with_branches("branch_only_ignores_main_worktree_status", &repo);
}

#[test]
fn test_list_user_status_with_special_characters() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Test with single emoji
    repo.add_worktree("emoji", "emoji");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["config", "worktrunk.status.emoji", "🔄"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to set user status");

    // Test with compound emoji (multi-codepoint)
    repo.add_worktree("multi", "multi");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["config", "worktrunk.status.multi", "👨‍💻"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to set user status");

    snapshot_list("user_status_special_chars", &repo);
}

/// Generate README example: Simple list output showing multiple worktrees
/// This demonstrates the basic list output format used in the Quick Start section.
/// Output: tests/snapshots/integration__integration_tests__list__readme_example_simple_list.snap
#[test]
fn test_readme_example_simple_list() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create worktrees with various states
    let feature_x = repo.add_worktree("feature-x", "feature-x");
    let bugfix_y = repo.add_worktree("bugfix-y", "bugfix-y");

    // feature-x: ahead with uncommitted changes
    // Make 3 commits
    for i in 1..=3 {
        std::fs::write(
            feature_x.join(format!("file{}.txt", i)),
            format!("content {}", i),
        )
        .expect("Failed to write file");
        let mut cmd = Command::new("git");
        repo.configure_git_cmd(&mut cmd);
        cmd.args(["add", &format!("file{}.txt", i)])
            .current_dir(&feature_x)
            .output()
            .expect("Failed to add");
        let mut cmd = Command::new("git");
        repo.configure_git_cmd(&mut cmd);
        cmd.args(["commit", "-m", &format!("Add file {}", i)])
            .current_dir(&feature_x)
            .output()
            .expect("Failed to commit");
    }
    // Add unstaged changes (+5 -2 lines)
    std::fs::write(
        feature_x.join("modified.txt"),
        "line1\nline2\nline3\nline4\nline5\n",
    )
    .expect("Failed to write");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "modified.txt"])
        .current_dir(&feature_x)
        .output()
        .expect("Failed to add");

    // bugfix-y: 1 commit ahead, clean tree
    std::fs::write(bugfix_y.join("bugfix.txt"), "fix content").expect("Failed to write file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "bugfix.txt"])
        .current_dir(&bugfix_y)
        .output()
        .expect("Failed to add");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Fix bug"])
        .current_dir(&bugfix_y)
        .output()
        .expect("Failed to commit");

    snapshot_list("readme_example_simple_list", &repo);
}
