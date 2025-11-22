use crate::common::{TestRepo, list_snapshots, setup_snapshot_settings, wt_command};
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

fn snapshot_list_with_remotes(test_name: &str, repo: &TestRepo) {
    run_snapshot(
        list_snapshots::standard_settings(repo),
        test_name,
        list_snapshots::command_remotes(repo),
    );
}

fn snapshot_list_with_branches_and_remotes(test_name: &str, repo: &TestRepo) {
    run_snapshot(
        list_snapshots::standard_settings(repo),
        test_name,
        list_snapshots::command_branches_and_remotes(repo),
    );
}

fn snapshot_list_progressive(test_name: &str, repo: &TestRepo) {
    run_snapshot(
        list_snapshots::standard_settings(repo),
        test_name,
        list_snapshots::command_progressive(repo),
    );
}

fn snapshot_list_no_progressive(test_name: &str, repo: &TestRepo) {
    run_snapshot(
        list_snapshots::standard_settings(repo),
        test_name,
        list_snapshots::command_no_progressive(repo),
    );
}

fn snapshot_list_progressive_branches(test_name: &str, repo: &TestRepo) {
    run_snapshot(
        list_snapshots::standard_settings(repo),
        test_name,
        list_snapshots::command_progressive_branches(repo),
    );
}

fn snapshot_list_task_dag(test_name: &str, repo: &TestRepo) {
    run_snapshot(
        list_snapshots::standard_settings(repo),
        test_name,
        list_snapshots::command_task_dag(repo),
    );
}

fn snapshot_list_task_dag_full(test_name: &str, repo: &TestRepo) {
    run_snapshot(
        list_snapshots::standard_settings(repo),
        test_name,
        list_snapshots::command_task_dag_full(repo),
    );
}

fn run_snapshot(settings: Settings, test_name: &str, mut cmd: Command) {
    settings.bind(|| {
        assert_cmd_snapshot!(test_name, cmd);
    });
}

/// Creates worktrees with specific timestamps for ordering tests.
/// Returns the path to feature-current (the worktree to run tests from).
///
/// Expected order: main (00:00), feature-current (01:00), feature-newest (03:00),
/// feature-middle (02:00), feature-oldest (00:30)
fn setup_timestamped_worktrees(repo: &mut TestRepo) -> std::path::PathBuf {
    // Create main with earliest timestamp (00:00)
    repo.commit("Initial commit on main");

    // Helper to create a commit with a specific timestamp
    fn commit_at_time(
        repo: &TestRepo,
        path: &std::path::Path,
        filename: &str,
        time: &str,
        time_short: &str,
    ) {
        let file_path = path.join(filename);
        std::fs::write(
            &file_path,
            format!("{} content", filename.trim_end_matches(".txt")),
        )
        .expect("Failed to write file");

        let mut cmd = Command::new("git");
        repo.configure_git_cmd(&mut cmd);
        cmd.env("GIT_AUTHOR_DATE", time);
        cmd.env("GIT_COMMITTER_DATE", time);
        cmd.args(["add", "."])
            .current_dir(path)
            .output()
            .expect("Failed to git add");

        let mut cmd = Command::new("git");
        repo.configure_git_cmd(&mut cmd);
        cmd.env("GIT_AUTHOR_DATE", time);
        cmd.env("GIT_COMMITTER_DATE", time);
        cmd.args(["commit", "-m", &format!("Commit at {}", time_short)])
            .current_dir(path)
            .output()
            .expect("Failed to git commit");
    }

    // 1. Create feature-current (01:00) - we'll run test from here
    let current_path = repo.add_worktree("feature-current", "feature-current");
    commit_at_time(
        repo,
        &current_path,
        "current.txt",
        "2025-01-01T01:00:00Z",
        "01:00",
    );

    // 2. Create feature-newest (03:00) - most recent, should be 3rd
    let newest_path = repo.add_worktree("feature-newest", "feature-newest");
    commit_at_time(
        repo,
        &newest_path,
        "newest.txt",
        "2025-01-01T03:00:00Z",
        "03:00",
    );

    // 3. Create feature-middle (02:00) - should be 4th
    let middle_path = repo.add_worktree("feature-middle", "feature-middle");
    commit_at_time(
        repo,
        &middle_path,
        "middle.txt",
        "2025-01-01T02:00:00Z",
        "02:00",
    );

    // 4. Create feature-oldest (00:30) - should be 5th
    let oldest_path = repo.add_worktree("feature-oldest", "feature-oldest");
    commit_at_time(
        repo,
        &oldest_path,
        "oldest.txt",
        "2025-01-01T00:30:00Z",
        "00:30",
    );

    current_path
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

/// Helper to push a branch to origin (creating a remote branch)
fn push_branch(repo: &TestRepo, branch_name: &str) {
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["push", "origin", branch_name])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to push branch to origin");
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
fn test_list_with_remotes_flag() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    // Setup remote creates origin and pushes main to it
    repo.setup_remote("main");

    // Create feature branches in the main repo and push them
    create_branch(&repo, "remote-feature-1");
    create_branch(&repo, "remote-feature-2");
    push_branch(&repo, "remote-feature-1");
    push_branch(&repo, "remote-feature-2");

    // Delete the local branches - now they only exist as origin/remote-feature-*
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["branch", "-D", "remote-feature-1", "remote-feature-2"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to delete local branches");

    // Should show:
    // - main worktree (primary)
    // - origin/remote-feature-1 (remote branch without local worktree)
    // - origin/remote-feature-2 (remote branch without local worktree)
    // Should NOT show origin/main (main has a worktree)
    snapshot_list_with_remotes("with_remotes_flag", &repo);
}

#[test]
fn test_list_with_remotes_and_branches() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create local-only branches (not worktrees, not pushed)
    create_branch(&repo, "local-only-1");
    create_branch(&repo, "local-only-2");

    // Create branches, push them, then delete locally to make them remote-only
    create_branch(&repo, "remote-only-1");
    create_branch(&repo, "remote-only-2");
    push_branch(&repo, "remote-only-1");
    push_branch(&repo, "remote-only-2");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["branch", "-D", "remote-only-1", "remote-only-2"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to delete branches");

    // Should show:
    // - main worktree
    // - local-only-1 branch (local, no worktree)
    // - local-only-2 branch (local, no worktree)
    // - origin/remote-only-1 (remote, no local)
    // - origin/remote-only-2 (remote, no local)
    snapshot_list_with_branches_and_remotes("with_remotes_and_branches", &repo);
}

#[test]
fn test_list_with_remotes_filters_existing_worktrees() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create a worktree and push the branch
    repo.add_worktree("feature-with-worktree", "feature-with-worktree");
    push_branch(&repo, "feature-with-worktree");

    // Create a branch, push it, delete it locally (remote-only)
    create_branch(&repo, "remote-only");
    push_branch(&repo, "remote-only");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["branch", "-D", "remote-only"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to delete branch");

    // Should show:
    // - main worktree
    // - feature-with-worktree worktree
    // - origin/remote-only (remote branch without local worktree)
    // Should NOT show origin/main or origin/feature-with-worktree (both have worktrees)
    snapshot_list_with_remotes("with_remotes_filters_worktrees", &repo);
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
    let current_path = setup_timestamped_worktrees(&mut repo);

    // Run from feature-current worktree to test "current worktree" logic
    snapshot_list_from_dir("list_ordering_rules", &repo, &current_path);
}

#[test]
fn test_list_with_upstream_tracking() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit on main");

    // Setup mock gh/glab to avoid network calls
    repo.setup_mock_gh();

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
    let mut settings = setup_snapshot_settings(&repo);

    // Additional custom filters
    settings.add_filter(r"\b[0-9a-f]{7,40}\b", "[SHA]   ");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
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
fn test_list_branch_only_with_status() {
    // Test that branch-only entries (no worktree) can display branch-keyed status
    let repo = TestRepo::new();
    repo.commit("Initial commit");

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
    snapshot_list_with_branches("branch_only_with_status", &repo);
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

#[test]
fn test_list_progressive_flag() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.add_worktree("feature-a", "feature-a");
    repo.add_worktree("feature-b", "feature-b");

    // Force progressive mode even in non-TTY test environment
    // Output should be identical to buffered mode (only process differs)
    snapshot_list_progressive("progressive_flag", &repo);
}

#[test]
fn test_list_no_progressive_flag() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.add_worktree("feature", "feature");

    // Explicitly force buffered mode
    snapshot_list_no_progressive("no_progressive_flag", &repo);
}

#[test]
fn test_list_progressive_with_branches() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create worktrees
    repo.add_worktree("feature-a", "feature-a");

    // Create branches without worktrees
    create_branch(&repo, "orphan-1");
    create_branch(&repo, "orphan-2");

    // Critical: test that --branches works with --progressive
    // This ensures progressive mode supports the --branches flag
    snapshot_list_progressive_branches("progressive_with_branches", &repo);
}

// ============================================================================
// Task DAG Mode Tests
// ============================================================================

#[test]
fn test_list_task_dag_single_worktree() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    snapshot_list_task_dag("task_dag_single_worktree", &repo);
}

#[test]
fn test_list_task_dag_multiple_worktrees() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    repo.add_worktree("feature-a", "feature-a");
    repo.add_worktree("feature-b", "feature-b");
    repo.add_worktree("feature-c", "feature-c");

    snapshot_list_task_dag("task_dag_multiple_worktrees", &repo);
}

#[test]
fn test_list_task_dag_full_with_diffs() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create worktree with changes
    let feature_a = repo.add_worktree("feature-a", "feature-a");
    std::fs::write(feature_a.join("new.txt"), "content").expect("Failed to write file");

    // Create another worktree with commits
    let feature_b = repo.add_worktree("feature-b", "feature-b");
    std::fs::write(feature_b.join("file.txt"), "test").expect("Failed to write file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(&feature_b)
        .output()
        .expect("Failed to add");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Test commit"])
        .current_dir(&feature_b)
        .output()
        .expect("Failed to commit");

    snapshot_list_task_dag_full("task_dag_full_with_diffs", &repo);
}

#[test]
fn test_list_task_dag_with_upstream() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit on main");
    repo.setup_remote("main");

    // Branch in sync
    let in_sync = repo.add_worktree("in-sync", "in-sync");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["push", "-u", "origin", "in-sync"])
        .current_dir(&in_sync)
        .output()
        .expect("Failed to push");

    // Branch ahead
    let ahead = repo.add_worktree("ahead", "ahead");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["push", "-u", "origin", "ahead"])
        .current_dir(&ahead)
        .output()
        .expect("Failed to push");
    std::fs::write(ahead.join("ahead.txt"), "ahead").expect("Failed to write");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(&ahead)
        .output()
        .expect("Failed to add");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Ahead commit"])
        .current_dir(&ahead)
        .output()
        .expect("Failed to commit");

    snapshot_list_task_dag_full("task_dag_with_upstream", &repo);
}

#[test]
fn test_list_task_dag_many_worktrees() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create 10 worktrees to test parallel processing
    for i in 1..=10 {
        repo.add_worktree(&format!("feature-{}", i), &format!("feature-{}", i));
    }

    snapshot_list_task_dag("task_dag_many_worktrees", &repo);
}

#[test]
fn test_list_task_dag_with_locked_worktree() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    repo.add_worktree("normal", "normal");
    repo.add_worktree("locked", "locked");
    repo.lock_worktree("locked", Some("Testing task DAG with locked worktree"));

    snapshot_list_task_dag("task_dag_with_locked", &repo);
}

#[test]
fn test_list_task_dag_detached_head() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.detach_head();

    snapshot_list_task_dag("task_dag_detached_head", &repo);
}

#[test]
fn test_list_task_dag_ordering_stability() {
    // Test that task_dag mode produces same ordering as buffered mode
    // Regression test for progressive rendering order instability
    let mut repo = TestRepo::new();
    let current_path = setup_timestamped_worktrees(&mut repo);

    // Run from feature-current worktree
    // Expected order: main, feature-current, feature-newest, feature-middle, feature-oldest
    run_snapshot(
        list_snapshots::standard_settings(&repo),
        "task_dag_ordering_stability",
        list_snapshots::command_task_dag_from_dir(&repo, &current_path),
    );
}

#[test]
fn test_list_progressive_vs_buffered_identical_data() {
    // Critical test: Verify that progressive and buffered modes collect identical data
    // despite using different rendering strategies (real-time UI vs collect-then-print).
    // This ensures consolidation on task DAG data collection works correctly.
    //
    // Note: We compare JSON output, not table output, because:
    // - Progressive mode renders headers before knowing final column widths (uses estimates)
    // - Buffered mode renders headers after data collection (uses actual widths)
    // - The DATA must be identical, but table formatting may differ slightly

    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create varied worktrees to test multiple data points
    repo.add_worktree("feature-a", "feature-a");
    repo.add_worktree("feature-b", "feature-b");

    // Modify a worktree to have uncommitted changes
    let feature_a_path = repo.worktree_path("feature-a");
    std::fs::write(feature_a_path.join("changes.txt"), "test").unwrap();

    // Run both modes with JSON output to compare data (not formatting)
    let progressive_output = list_snapshots::command_progressive_json(&repo)
        .output()
        .expect("Progressive JSON command failed");

    let buffered_output = list_snapshots::command_no_progressive_json(&repo)
        .output()
        .expect("Buffered JSON command failed");

    // Both should succeed
    assert!(
        progressive_output.status.success(),
        "Progressive mode failed: {}",
        String::from_utf8_lossy(&progressive_output.stderr)
    );
    assert!(
        buffered_output.status.success(),
        "Buffered mode failed: {}",
        String::from_utf8_lossy(&buffered_output.stderr)
    );

    // Parse JSON outputs
    let progressive_json: serde_json::Value = serde_json::from_slice(&progressive_output.stdout)
        .expect("Invalid JSON from progressive mode");
    let buffered_json: serde_json::Value =
        serde_json::from_slice(&buffered_output.stdout).expect("Invalid JSON from buffered mode");

    // The JSON data should be identical (ignoring display fields which may have formatting differences)
    // Compare the structured data to ensure both modes collect the same information
    assert_eq!(
        progressive_json,
        buffered_json,
        "Progressive and buffered modes produced different data!\n\nProgressive:\n{}\n\nBuffered:\n{}",
        serde_json::to_string_pretty(&progressive_json).unwrap(),
        serde_json::to_string_pretty(&buffered_json).unwrap()
    );
}

/// Test that errors from worktree collection include helpful context
/// This verifies that when a worktree fails to collect data, the error message
/// includes the worktree branch name and path for easier debugging.
///
/// TODO: This test is currently ignored because the parallel collection implementation
/// silently handles errors instead of propagating them. We need to add proper error
/// propagation through the CellUpdate channel. See collect_progressive_impl.rs TODOs.
#[test]
#[ignore = "Error handling needs improvement in parallel collection"]
fn test_list_error_shows_worktree_context() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create a worktree
    let feature_wt = repo.add_worktree("feature", "feature");

    // Delete the worktree directory manually to trigger an error
    // (but keep the git metadata, so git worktree list still shows it)
    std::fs::remove_dir_all(&feature_wt).expect("Failed to remove worktree directory");

    // Run list command and expect an error
    let mut cmd = wt_command();
    repo.clean_cli_env(&mut cmd);
    repo.configure_mock_commands(&mut cmd);
    cmd.arg("list").current_dir(repo.root_path());

    let output = cmd.output().expect("Failed to run command");

    // Should fail with non-zero exit code
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "Expected command to fail. stdout:\n{}\nstderr:\n{}",
        stdout,
        stderr
    );

    // Error message should include worktree context (could be in stdout or stderr)
    let combined = format!("{}{}", stdout, stderr);

    assert!(
        combined.contains("feature") && combined.contains("Failed to collect data for worktree"),
        "Error message should include worktree branch 'feature' and context, but got:\nstdout:\n{}\nstderr:\n{}",
        stdout,
        stderr
    );
}

#[test]
fn test_list_with_c_flag() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create some worktrees
    repo.add_worktree("feature-a", "feature-a");
    repo.add_worktree("feature-b", "feature-b");

    // Run wt -C <repo_path> list from a completely different directory
    let mut settings = list_snapshots::standard_settings(&repo);
    // Redact the -C path argument in metadata (try different selector formats)
    settings.add_redaction(".args[1]", "[REPO_PATH]");
    settings.bind(|| {
        let mut cmd = wt_command();
        cmd.args(["-C", repo.root_path().to_str().unwrap(), "list"]);
        // Run from /tmp to ensure -C is actually being used
        cmd.current_dir("/tmp");
        assert_cmd_snapshot!("list_with_c_flag", cmd);
    });
}

#[test]
fn test_list_large_diffs_alignment() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Worktree with large uncommitted changes and ahead commits
    // Use a longer branch name similar to user's "wli-sequence" to trigger column width
    let large_wt = repo.add_worktree("feature-changes", "feature-changes");

    // Create a file with many lines for large diff
    let large_content = (1..=100)
        .map(|i| format!("line {}", i))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(large_wt.join("large.txt"), &large_content).expect("Failed to write file");

    // Commit it
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(&large_wt)
        .output()
        .expect("Failed to add");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add 100 lines"])
        .current_dir(&large_wt)
        .output()
        .expect("Failed to commit");

    // Add large uncommitted changes (both added and deleted lines)
    // Add a new file with many lines
    std::fs::write(large_wt.join("uncommitted.txt"), &large_content).expect("Failed to write file");

    // Modify the existing file to create deletions
    let modified_content = (1..=50)
        .map(|i| format!("modified line {}", i))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(large_wt.join("large.txt"), &modified_content).expect("Failed to write file");

    // Add another new file with many lines
    let another_large = (1..=80)
        .map(|i| format!("another line {}", i))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(large_wt.join("another.txt"), &another_large).expect("Failed to write file");

    // Set user status
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["config", "worktrunk.status.feature-changes", "🤖"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to set user status");

    // Worktree with short name to show gap before Status column
    let short_wt = repo.add_worktree("fix", "fix");
    std::fs::write(short_wt.join("quick.txt"), "quick fix").expect("Failed to write file");

    // Set user status for short branch
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["config", "worktrunk.status.fix", "💬"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to set user status");

    // Worktree with diverged status and working tree changes
    let diverged_wt = repo.add_worktree("diverged", "diverged");

    // Commit some changes
    let diverged_content = (1..=60)
        .map(|i| format!("diverged line {}", i))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(diverged_wt.join("test.txt"), &diverged_content).expect("Failed to write file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(&diverged_wt)
        .output()
        .expect("Failed to add");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Diverged commit"])
        .current_dir(&diverged_wt)
        .output()
        .expect("Failed to commit");

    // Add uncommitted changes
    let modified_diverged = (1..=40)
        .map(|i| format!("modified diverged line {}", i))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(diverged_wt.join("test.txt"), &modified_diverged).expect("Failed to write file");

    // Set user status
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["config", "worktrunk.status.diverged", "💬"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to set user status");

    snapshot_list("large_diffs_alignment", &repo);
}

#[test]
fn test_list_status_column_padding_with_emoji() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create worktree matching user's exact scenario: "wli-sequence"
    let wli_seq = repo.add_worktree("wli-sequence", "wli-sequence");

    // Create large working tree changes: +164, -111
    // Need ~164 added lines and ~111 deleted lines
    let initial_content = (1..=200)
        .map(|i| format!("original line {}", i))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(wli_seq.join("main.txt"), &initial_content).expect("write failed");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(&wli_seq)
        .output()
        .expect("add failed");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Initial content"])
        .current_dir(&wli_seq)
        .output()
        .expect("commit failed");

    // Modify to create desired diff: remove ~111 lines, add different content
    let modified_content = (1..=89)
        .map(|i| format!("original line {}", i))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(wli_seq.join("main.txt"), &modified_content).expect("write failed");

    // Add new file with ~164 lines to get +164
    let new_content = (1..=164)
        .map(|i| format!("new line {}", i))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(wli_seq.join("new.txt"), &new_content).expect("write failed");

    // Set user status emoji 🤖
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["config", "worktrunk.status.wli-sequence", "🤖"])
        .current_dir(repo.root_path())
        .output()
        .expect("config failed");

    // Create "pr-link" worktree with different status (fewer symbols, same emoji type)
    let pr_link = repo.add_worktree("pr-link", "pr-link");

    // Commit to make it ahead
    std::fs::write(pr_link.join("pr.txt"), "pr content").expect("write failed");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(&pr_link)
        .output()
        .expect("add failed");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "PR commit"])
        .current_dir(&pr_link)
        .output()
        .expect("commit failed");

    // Set same emoji type
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["config", "worktrunk.status.pr-link", "🤖"])
        .current_dir(repo.root_path())
        .output()
        .expect("config failed");

    // Create "main-symbol" with different emoji
    let main_sym = repo.add_worktree("main-symbol", "main-symbol");
    std::fs::write(main_sym.join("sym.txt"), "symbol").expect("write failed");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(&main_sym)
        .output()
        .expect("add failed");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Symbol commit"])
        .current_dir(&main_sym)
        .output()
        .expect("commit failed");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["config", "worktrunk.status.main-symbol", "💬"])
        .current_dir(repo.root_path())
        .output()
        .expect("config failed");

    snapshot_list("status_column_padding_emoji", &repo);
}

#[test]
fn test_list_maximum_working_tree_symbols() {
    // Test that all 5 working tree symbols can appear simultaneously:
    // ? (untracked), ! (modified), + (staged), » (renamed), ✘ (deleted)
    // This verifies the maximum width of the working_tree position (5 chars)
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    let feature = repo.add_worktree("feature", "feature");

    // Create initial files to manipulate
    std::fs::write(feature.join("file-a.txt"), "original a").unwrap();
    std::fs::write(feature.join("file-b.txt"), "original b").unwrap();
    std::fs::write(feature.join("file-c.txt"), "original c").unwrap();
    std::fs::write(feature.join("file-d.txt"), "original d").unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(&feature)
        .output()
        .unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add files"])
        .current_dir(&feature)
        .output()
        .unwrap();

    // 1. Create untracked file (?)
    std::fs::write(feature.join("untracked.txt"), "new file").unwrap();

    // 2. Modify tracked file without staging (!)
    std::fs::write(feature.join("file-a.txt"), "modified content").unwrap();

    // 3. Stage some changes (+)
    std::fs::write(feature.join("file-b.txt"), "staged changes").unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "file-b.txt"])
        .current_dir(&feature)
        .output()
        .unwrap();

    // 4. Rename a file and stage it (»)
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["mv", "file-c.txt", "renamed-c.txt"])
        .current_dir(&feature)
        .output()
        .unwrap();

    // 5. Delete a file in index (✘)
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["rm", "file-d.txt"])
        .current_dir(&feature)
        .output()
        .unwrap();

    // Result should show: ?!+»✘
    snapshot_list("maximum_working_tree_symbols", &repo);
}

#[test]
fn test_list_maximum_status_with_git_operation() {
    // Test maximum status symbols including git operation (rebase/merge):
    // ?!+ (working_tree) + = (conflicts) + ↻ (rebase) + ↕ (diverged) + ⊠ (locked) + 🤖 (user status)
    // This pushes the Status column to ~10-11 chars of actual content
    let mut repo = TestRepo::new();

    // Create initial commit with a file that will conflict
    std::fs::write(
        repo.root_path().join("conflict.txt"),
        "original line 1\noriginal line 2\n",
    )
    .unwrap();
    std::fs::write(repo.root_path().join("shared.txt"), "shared content").unwrap();
    repo.commit("Initial commit");

    // Create feature worktree
    let feature = repo.add_worktree("feature", "feature");

    // Feature makes changes
    std::fs::write(
        feature.join("conflict.txt"),
        "feature line 1\nfeature line 2\n",
    )
    .unwrap();
    std::fs::write(feature.join("feature.txt"), "feature-specific content").unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(&feature)
        .output()
        .unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Feature changes"])
        .current_dir(&feature)
        .output()
        .unwrap();

    // Main makes conflicting changes
    std::fs::write(
        repo.root_path().join("conflict.txt"),
        "main line 1\nmain line 2\n",
    )
    .unwrap();
    std::fs::write(repo.root_path().join("main-only.txt"), "main content").unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(repo.root_path())
        .output()
        .unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Main conflicting changes"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    // Start rebase which will create conflicts and git operation state
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    let rebase_output = cmd
        .args(["rebase", "main"])
        .current_dir(&feature)
        .output()
        .unwrap();

    // Rebase should fail with conflicts - verify we're in rebase state
    assert!(
        !rebase_output.status.success(),
        "Rebase should fail with conflicts"
    );

    // Now add working tree symbols while in rebase state
    // 1. Untracked file (?)
    std::fs::write(feature.join("untracked.txt"), "untracked during rebase").unwrap();

    // 2. Modified file (!) - modify a non-conflicting file
    std::fs::write(feature.join("feature.txt"), "modified during rebase").unwrap();

    // 3. Staged file (+) - stage the conflict resolution
    std::fs::write(feature.join("new-staged.txt"), "staged during rebase").unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "new-staged.txt"])
        .current_dir(&feature)
        .output()
        .unwrap();

    // Lock the worktree (⊠)
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["worktree", "lock", feature.to_str().unwrap()])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    // Set user status emoji (🤖)
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["config", "worktrunk.status.feature", "🤖"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    // Result should show: ?!+ (working_tree) + = (conflicts) + ↻ (rebase) + ↕ (diverged) + ⊠ (locked) + 🤖 (user status)
    // Use --full to enable conflict detection
    let settings = list_snapshots::standard_settings(&repo);
    settings.bind(|| {
        let mut cmd = list_snapshots::command(&repo, repo.root_path());
        cmd.arg("--full");
        assert_cmd_snapshot!("maximum_status_with_git_operation", cmd);
    });
}

#[test]
fn test_list_maximum_status_symbols() {
    // Test the maximum status symbols possible:
    // ?!+»✘ (5) + ⚠ (1) + ⊠ (1) + ↕ (1) + ⇅ (1) + 🤖 (2) = 11 chars
    // Missing: ✖ (actual conflicts), ↻ (git operation - can't have with divergence), ◇ (bare), ⚠ (prunable)
    let mut repo = TestRepo::new();

    // Create initial commit on main with shared files
    std::fs::write(repo.root_path().join("shared.txt"), "original").unwrap();
    std::fs::write(repo.root_path().join("file-a.txt"), "a").unwrap();
    std::fs::write(repo.root_path().join("file-b.txt"), "b").unwrap();
    std::fs::write(repo.root_path().join("file-c.txt"), "c").unwrap();
    std::fs::write(repo.root_path().join("file-d.txt"), "d").unwrap();
    repo.commit("Initial commit");

    // Create feature worktree
    let feature = repo.add_worktree("feature", "feature");

    // Make feature diverge from main (ahead) with conflicting change
    std::fs::write(feature.join("shared.txt"), "feature version").unwrap();
    std::fs::write(feature.join("feature.txt"), "feature content").unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(&feature)
        .output()
        .unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Feature work"])
        .current_dir(&feature)
        .output()
        .unwrap();

    // Create a real bare remote so upstream exists, but keep all graph crafting local for determinism
    repo.setup_remote("main");

    // Remember the shared base (Feature work)
    let base_sha = {
        let output = repo
            .git_command(&["rev-parse", "HEAD"])
            .current_dir(&feature)
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };

    // Remote-only commit
    std::fs::write(feature.join("remote-file.txt"), "remote content").unwrap();
    repo.git_command(&["add", "remote-file.txt"])
        .current_dir(&feature)
        .output()
        .unwrap();
    repo.git_command(&["commit", "-m", "Remote commit"])
        .current_dir(&feature)
        .output()
        .unwrap();
    let remote_sha = {
        let output = repo
            .git_command(&["rev-parse", "HEAD"])
            .current_dir(&feature)
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };

    // Reset back to base so the remote commit is not in the local branch history
    repo.git_command(&["reset", "--hard", &base_sha])
        .current_dir(&feature)
        .output()
        .unwrap();

    // Local-only commit (divergence on the local side)
    std::fs::write(feature.join("local-file.txt"), "local content").unwrap();
    repo.git_command(&["add", "local-file.txt"])
        .current_dir(&feature)
        .output()
        .unwrap();
    repo.git_command(&["commit", "-m", "Local commit"])
        .current_dir(&feature)
        .output()
        .unwrap();

    // Wire up upstream tracking deterministically: point origin/feature at the remote-only commit
    repo.git_command(&["update-ref", "refs/remotes/origin/feature", &remote_sha])
        .current_dir(&feature)
        .output()
        .unwrap();
    repo.git_command(&["branch", "--set-upstream-to=origin/feature", "feature"])
        .current_dir(&feature)
        .output()
        .unwrap();

    // Make main advance with conflicting change (so feature is behind with conflicts)
    std::fs::write(repo.root_path().join("shared.txt"), "main version").unwrap();
    std::fs::write(repo.root_path().join("main2.txt"), "more main").unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(repo.root_path())
        .output()
        .unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Main advances"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    // Add all 5 working tree symbol types (without rebase, so we keep divergence)
    // 1. Untracked (?)
    std::fs::write(feature.join("untracked.txt"), "untracked").unwrap();

    // 2. Modified (!)
    std::fs::write(feature.join("feature.txt"), "modified").unwrap();

    // 3. Staged (+)
    std::fs::write(feature.join("new-staged.txt"), "staged content").unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "new-staged.txt"])
        .current_dir(&feature)
        .output()
        .unwrap();

    // 4. Renamed (»)
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["mv", "file-c.txt", "renamed-c.txt"])
        .current_dir(&feature)
        .output()
        .unwrap();

    // 5. Deleted (✘)
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["rm", "file-d.txt"])
        .current_dir(&feature)
        .output()
        .unwrap();

    // Lock the worktree (⊠)
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["worktree", "lock", feature.to_str().unwrap()])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    // Set user status emoji (🤖)
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["config", "worktrunk.status.feature", "🤖"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    // Result should show 11 chars: ?!+»✘=⊠↕⇅🤖
    let settings = list_snapshots::standard_settings(&repo);
    settings.bind(|| {
        let mut cmd = list_snapshots::command(&repo, repo.root_path());
        cmd.arg("--full");
        assert_cmd_snapshot!("maximum_status_symbols", cmd);
    });
}

#[test]
fn test_list_warns_when_default_branch_missing_worktree() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");
    // Move primary worktree off the default branch so no worktree holds it
    repo.switch_primary_to("develop");

    snapshot_list("default_branch_missing_worktree", &repo);
}
