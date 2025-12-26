use crate::common::{
    DAY, HOUR, MINUTE, TestRepo, list_snapshots, repo, repo_with_remote, setup_snapshot_settings,
    wt_command,
};
use insta::Settings;
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::path::Path;
use std::process::Command;

fn snapshot_list(test_name: &str, repo: &TestRepo) {
    run_snapshot(
        setup_snapshot_settings(repo),
        test_name,
        list_snapshots::command(repo, repo.root_path()),
    );
}

fn snapshot_list_from_dir(test_name: &str, repo: &TestRepo, cwd: &Path) {
    run_snapshot(
        setup_snapshot_settings(repo),
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
        setup_snapshot_settings(repo),
        test_name,
        list_snapshots::command_branches(repo),
    );
}

fn snapshot_list_with_remotes(test_name: &str, repo: &TestRepo) {
    run_snapshot(
        setup_snapshot_settings(repo),
        test_name,
        list_snapshots::command_remotes(repo),
    );
}

fn snapshot_list_with_branches_and_remotes(test_name: &str, repo: &TestRepo) {
    run_snapshot(
        setup_snapshot_settings(repo),
        test_name,
        list_snapshots::command_branches_and_remotes(repo),
    );
}

fn snapshot_list_full(test_name: &str, repo: &TestRepo) {
    let mut cmd = list_snapshots::command(repo, repo.root_path());
    cmd.arg("--full");
    run_snapshot(setup_snapshot_settings(repo), test_name, cmd);
}

fn snapshot_list_progressive(test_name: &str, repo: &TestRepo) {
    run_snapshot(
        setup_snapshot_settings(repo),
        test_name,
        list_snapshots::command_progressive(repo),
    );
}

fn snapshot_list_no_progressive(test_name: &str, repo: &TestRepo) {
    run_snapshot(
        setup_snapshot_settings(repo),
        test_name,
        list_snapshots::command_no_progressive(repo),
    );
}

fn snapshot_list_progressive_branches(test_name: &str, repo: &TestRepo) {
    run_snapshot(
        setup_snapshot_settings(repo),
        test_name,
        list_snapshots::command_progressive_branches(repo),
    );
}

fn snapshot_list_progressive_full(test_name: &str, repo: &TestRepo) {
    run_snapshot(
        setup_snapshot_settings(repo),
        test_name,
        list_snapshots::command_progressive_full(repo),
    );
}

// README example snapshots - use narrower width for doc site code blocks
fn snapshot_readme_list_from_dir(test_name: &str, repo: &TestRepo, cwd: &Path) {
    run_snapshot(
        setup_snapshot_settings(repo),
        test_name,
        list_snapshots::command_readme(repo, cwd),
    );
}

fn snapshot_readme_list_full_from_dir(test_name: &str, repo: &TestRepo, cwd: &Path) {
    run_snapshot(
        setup_snapshot_settings(repo),
        test_name,
        list_snapshots::command_readme_full_from_dir(repo, cwd),
    );
}

fn snapshot_readme_list_branches_full_from_dir(test_name: &str, repo: &TestRepo, cwd: &Path) {
    run_snapshot(
        setup_snapshot_settings(repo),
        test_name,
        list_snapshots::command_readme_branches_full_from_dir(repo, cwd),
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
/// Expected order: main (^), feature-current (@), then by timestamp descending:
/// feature-newest (03:00), feature-middle (02:00), feature-oldest (00:30)
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
        .unwrap();

        let mut cmd = Command::new("git");
        repo.configure_git_cmd(&mut cmd);
        cmd.env("GIT_AUTHOR_DATE", time);
        cmd.env("GIT_COMMITTER_DATE", time);
        cmd.args(["add", "."]).current_dir(path).output().unwrap();

        let mut cmd = Command::new("git");
        repo.configure_git_cmd(&mut cmd);
        cmd.env("GIT_AUTHOR_DATE", time);
        cmd.env("GIT_COMMITTER_DATE", time);
        cmd.args(["commit", "-m", &format!("Commit at {}", time_short)])
            .current_dir(path)
            .output()
            .unwrap();
    }

    // 1. Create feature-current (01:00) - we'll run test from here
    let current_path = repo.add_worktree("feature-current");
    commit_at_time(
        repo,
        &current_path,
        "current.txt",
        "2025-01-01T01:00:00Z",
        "01:00",
    );

    // 2. Create feature-newest (03:00) - most recent, should be 3rd
    let newest_path = repo.add_worktree("feature-newest");
    commit_at_time(
        repo,
        &newest_path,
        "newest.txt",
        "2025-01-01T03:00:00Z",
        "03:00",
    );

    // 3. Create feature-middle (02:00) - should be 4th
    let middle_path = repo.add_worktree("feature-middle");
    commit_at_time(
        repo,
        &middle_path,
        "middle.txt",
        "2025-01-01T02:00:00Z",
        "02:00",
    );

    // 4. Create feature-oldest (00:30) - should be 5th
    let oldest_path = repo.add_worktree("feature-oldest");
    commit_at_time(
        repo,
        &oldest_path,
        "oldest.txt",
        "2025-01-01T00:30:00Z",
        "00:30",
    );

    current_path
}

#[rstest]
fn test_list_single_worktree(repo: TestRepo) {
    snapshot_list("single_worktree", &repo);
}

#[rstest]
fn test_list_multiple_worktrees(mut repo: TestRepo) {
    repo.add_worktree("feature-a");
    repo.add_worktree("feature-b");

    snapshot_list("multiple_worktrees", &repo);
}

/// Test that the `-` gutter symbol appears for the previous worktree (target of `wt switch -`).
///
/// Simulates realistic usage by running switch commands from the correct worktree directories.
#[rstest]
fn test_list_previous_worktree_gutter(mut repo: TestRepo) {
    repo.add_worktree("feature");

    let feature_path = repo.root_path().parent().unwrap().join(format!(
        "{}.feature",
        repo.root_path().file_name().unwrap().to_str().unwrap()
    ));

    // Step 1: From main, switch to feature (history: current=feature, previous=main)
    let mut cmd = wt_command();
    repo.clean_cli_env(&mut cmd);
    cmd.args(["switch", "feature"])
        .current_dir(repo.root_path());
    cmd.output().unwrap();

    // Step 2: From feature, switch back to main (history: current=main, previous=feature)
    let mut cmd = wt_command();
    repo.clean_cli_env(&mut cmd);
    cmd.args(["switch", "main"]).current_dir(&feature_path);
    cmd.output().unwrap();

    // Now list should show `-` for feature (the previous worktree, target of `wt switch -`)
    snapshot_list("previous_worktree_gutter", &repo);
}

#[rstest]
fn test_list_detached_head(repo: TestRepo) {
    repo.detach_head();

    snapshot_list("detached_head", &repo);
}

#[rstest]
fn test_list_detached_head_in_worktree(mut repo: TestRepo) {
    // Non-main worktree in detached HEAD SHOULD show path mismatch flag
    // (detached HEAD = "not at home", not on any branch)

    repo.add_worktree("feature");
    repo.detach_head_in_worktree("feature");

    snapshot_list("detached_head_in_worktree", &repo);
}

#[rstest]
fn test_list_locked_worktree(mut repo: TestRepo) {
    repo.add_worktree("locked-feature");
    repo.lock_worktree("locked-feature", Some("Testing lock functionality"));

    snapshot_list("locked_worktree", &repo);
}

#[rstest]
fn test_list_locked_no_reason(mut repo: TestRepo) {
    repo.add_worktree("locked-no-reason");
    repo.lock_worktree("locked-no-reason", None);

    snapshot_list("locked_no_reason", &repo);
}

// Removed: test_list_long_branch_name - covered by spacing_edge_cases.rs

#[rstest]
fn test_list_long_commit_message(mut repo: TestRepo) {
    // Create commit with very long message
    repo.commit("This is a very long commit message that should test how the message column handles truncation and word boundary detection in the list output");

    repo.add_worktree("feature-a");
    repo.commit("Short message");

    snapshot_list("long_commit_message", &repo);
}

// Removed: test_list_unicode_branch_name - covered by spacing_edge_cases.rs

#[rstest]
fn test_list_unicode_commit_message(mut repo: TestRepo) {
    // Create commit with Unicode message
    repo.commit("Add support for 日本語 and émoji 🎉");

    repo.add_worktree("feature-test");
    repo.commit("Fix bug with café ☕ handling");

    snapshot_list("unicode_commit_message", &repo);
}

#[rstest]
fn test_list_many_worktrees_with_varied_stats(mut repo: TestRepo) {
    // Create multiple worktrees with different characteristics
    repo.add_worktree("short");

    repo.add_worktree("medium-name");

    repo.add_worktree("very-long-branch-name-here");

    // Add some with files to create diff stats
    repo.add_worktree("with-changes");

    snapshot_list("many_worktrees_varied", &repo);
}

// Removed: test_list_json_single_worktree and test_list_json_multiple_worktrees
// Basic JSON serialization is covered by test_list_json_with_metadata

#[rstest]
fn test_list_json_with_metadata(mut repo: TestRepo) {
    // Create worktree with detached head
    repo.add_worktree("feature-detached");

    // Create locked worktree
    repo.add_worktree("locked-feature");
    repo.lock_worktree("locked-feature", Some("Testing"));

    snapshot_list_json("json_with_metadata", &repo);
}

/// Test that committed_trees_match is true when a branch has commits ahead but identical tree content.
/// This tests the merge commit scenario where content matches main even with different commit history.
#[rstest]
fn test_list_json_tree_matches_main_after_merge(mut repo: TestRepo) {
    // Create feature branch with a worktree
    let feature_path = repo.add_worktree_with_commit(
        "feature-merged",
        "feature.txt",
        "feature content",
        "Feature commit",
    );

    // Make the same commit on main (so trees will match after merge)
    std::fs::write(repo.root_path().join("feature.txt"), "feature content").unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(repo.root_path())
        .output()
        .unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Same content on main"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    // Merge main into feature (creates merge commit, but tree matches main)
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["merge", "main", "-m", "Merge main into feature"])
        .current_dir(&feature_path)
        .output()
        .unwrap();

    // Now feature-merged is ahead of main (has merge commit) but tree content matches main
    // JSON output should show branch_op_state: "TreesMatch" with ahead > 0
    snapshot_list_json("json_tree_matches_main_after_merge", &repo);
}

#[rstest]
fn test_list_with_branches_flag(mut repo: TestRepo) {
    // Create some branches without worktrees
    repo.create_branch("feature-without-worktree");
    repo.create_branch("another-branch");
    repo.create_branch("fix-bug");

    // Create one branch with a worktree
    repo.add_worktree("feature-with-worktree");

    snapshot_list_with_branches("with_branches_flag", &repo);
}

#[rstest]
fn test_list_with_branches_flag_no_available(mut repo: TestRepo) {
    // All branches have worktrees (only main exists and has worktree)
    repo.add_worktree("feature-a");
    repo.add_worktree("feature-b");

    snapshot_list_with_branches("with_branches_flag_none_available", &repo);
}

#[rstest]
fn test_list_with_branches_flag_only_branches(repo: TestRepo) {
    // Create several branches without worktrees
    repo.create_branch("branch-alpha");
    repo.create_branch("branch-beta");
    repo.create_branch("branch-gamma");

    snapshot_list_with_branches("with_branches_flag_only_branches", &repo);
}

#[rstest]
fn test_list_with_remotes_flag(#[from(repo_with_remote)] repo: TestRepo) {
    // Create feature branches in the main repo and push them
    repo.create_branch("remote-feature-1");
    repo.create_branch("remote-feature-2");
    repo.push_branch("remote-feature-1");
    repo.push_branch("remote-feature-2");

    // Delete the local branches - now they only exist as origin/remote-feature-*
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["branch", "-D", "remote-feature-1", "remote-feature-2"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    // Should show:
    // - main worktree (primary)
    // - origin/remote-feature-1 (remote branch without local worktree)
    // - origin/remote-feature-2 (remote branch without local worktree)
    // Should NOT show origin/main (main has a worktree)
    snapshot_list_with_remotes("with_remotes_flag", &repo);
}

#[rstest]
fn test_list_with_remotes_and_branches(#[from(repo_with_remote)] repo: TestRepo) {
    // Create local-only branches (not worktrees, not pushed)
    repo.create_branch("local-only-1");
    repo.create_branch("local-only-2");

    // Create branches, push them, then delete locally to make them remote-only
    repo.create_branch("remote-only-1");
    repo.create_branch("remote-only-2");
    repo.push_branch("remote-only-1");
    repo.push_branch("remote-only-2");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["branch", "-D", "remote-only-1", "remote-only-2"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    // Should show:
    // - main worktree
    // - local-only-1 branch (local, no worktree)
    // - local-only-2 branch (local, no worktree)
    // - origin/remote-only-1 (remote, no local)
    // - origin/remote-only-2 (remote, no local)
    snapshot_list_with_branches_and_remotes("with_remotes_and_branches", &repo);
}

#[rstest]
fn test_list_with_remotes_filters_existing_worktrees(#[from(repo_with_remote)] mut repo: TestRepo) {
    // Create a worktree and push the branch
    repo.add_worktree("feature-with-worktree");
    repo.push_branch("feature-with-worktree");

    // Create a branch, push it, delete it locally (remote-only)
    repo.create_branch("remote-only");
    repo.push_branch("remote-only");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["branch", "-D", "remote-only"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    // Should show:
    // - main worktree
    // - feature-with-worktree worktree
    // - origin/remote-only (remote branch without local worktree)
    // Should NOT show origin/main or origin/feature-with-worktree (both have worktrees)
    snapshot_list_with_remotes("with_remotes_filters_worktrees", &repo);
}

#[rstest]
fn test_list_json_with_display_fields(mut repo: TestRepo) {
    repo.commit("Initial commit on main");

    // Create feature branch with commits (ahead of main)
    repo.add_worktree("feature-ahead");

    // Make commits in the feature worktree
    let feature_path = repo.worktree_path("feature-ahead");
    std::fs::write(feature_path.join("feature.txt"), "feature content").unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(feature_path)
        .output()
        .unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Feature commit 1"])
        .current_dir(feature_path)
        .output()
        .unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "--allow-empty", "-m", "Feature commit 2"])
        .current_dir(feature_path)
        .output()
        .unwrap();

    // Add uncommitted changes to show working_diff_display
    std::fs::write(feature_path.join("uncommitted.txt"), "uncommitted").unwrap();
    std::fs::write(feature_path.join("feature.txt"), "modified content").unwrap();

    // Create another feature that will be behind after main advances
    repo.add_worktree("feature-behind");

    // Make more commits on main (so feature-behind is behind)
    repo.commit("Main commit 1");
    repo.commit("Main commit 2");

    snapshot_list_json("json_with_display_fields", &repo);
}

#[rstest]
fn test_list_ordering_rules(mut repo: TestRepo) {
    let current_path = setup_timestamped_worktrees(&mut repo);

    // Run from feature-current worktree to test "current worktree" logic
    snapshot_list_from_dir("list_ordering_rules", &repo, &current_path);
}

#[rstest]
fn test_list_with_upstream_tracking(mut repo: TestRepo) {
    repo.commit("Initial commit on main");

    // Set up remote - this already pushes main
    repo.setup_remote("main");

    // Scenario 1: Branch in sync with remote (should show ↑0 ↓0)
    let in_sync_wt = repo.add_worktree("in-sync");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["push", "-u", "origin", "in-sync"])
        .current_dir(&in_sync_wt)
        .output()
        .unwrap();

    // Scenario 2: Branch ahead of remote (should show ↑2)
    let ahead_wt = repo.add_worktree("ahead");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["push", "-u", "origin", "ahead"])
        .current_dir(&ahead_wt)
        .output()
        .unwrap();

    // Make 2 commits ahead
    std::fs::write(ahead_wt.join("ahead1.txt"), "ahead 1").unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(&ahead_wt)
        .output()
        .unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Ahead commit 1"])
        .current_dir(&ahead_wt)
        .output()
        .unwrap();

    std::fs::write(ahead_wt.join("ahead2.txt"), "ahead 2").unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(&ahead_wt)
        .output()
        .unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Ahead commit 2"])
        .current_dir(&ahead_wt)
        .output()
        .unwrap();

    // Scenario 3: Branch behind remote (should show ↓1)
    let behind_wt = repo.add_worktree("behind");
    std::fs::write(behind_wt.join("behind.txt"), "behind").unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(&behind_wt)
        .output()
        .unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Behind commit"])
        .current_dir(&behind_wt)
        .output()
        .unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["push", "-u", "origin", "behind"])
        .current_dir(&behind_wt)
        .output()
        .unwrap();
    // Reset local to one commit behind
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["reset", "--hard", "HEAD~1"])
        .current_dir(&behind_wt)
        .output()
        .unwrap();

    // Scenario 4: Branch both ahead and behind (should show ↑1 ↓1)
    let diverged_wt = repo.add_worktree("diverged");
    std::fs::write(diverged_wt.join("diverged.txt"), "diverged").unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(&diverged_wt)
        .output()
        .unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Diverged remote commit"])
        .current_dir(&diverged_wt)
        .output()
        .unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["push", "-u", "origin", "diverged"])
        .current_dir(&diverged_wt)
        .output()
        .unwrap();
    // Reset and make different commit
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["reset", "--hard", "HEAD~1"])
        .current_dir(&diverged_wt)
        .output()
        .unwrap();
    std::fs::write(diverged_wt.join("different.txt"), "different").unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(&diverged_wt)
        .output()
        .unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Diverged local commit"])
        .current_dir(&diverged_wt)
        .output()
        .unwrap();

    // Scenario 5: Branch without upstream (should show blank)
    let no_upstream_wt = repo.add_worktree("no-upstream");

    // Run git status to ensure the worktree is fully initialized on Windows.
    // Without this, Windows CI may show 55y (Unix epoch) because the worktree
    // isn't ready when wt list tries to read commit timestamps.
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["status", "--porcelain"])
        .current_dir(&no_upstream_wt)
        .output()
        .unwrap();

    // Run list --branches --full to show all columns including Remote
    let settings = setup_snapshot_settings(&repo);

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

#[rstest]
fn test_list_primary_on_different_branch(mut repo: TestRepo) {
    repo.switch_primary_to("develop");
    assert_eq!(repo.current_branch(), "develop");

    repo.add_worktree("feature-a");
    repo.add_worktree("feature-b");

    snapshot_list("list_primary_on_different_branch", &repo);
}

#[rstest]
fn test_list_with_user_marker(mut repo: TestRepo) {
    repo.commit_with_age("Initial commit", DAY);

    // Branch ahead of main with commits and user marker 🤖
    let _feature_wt = repo.add_worktree_with_commit(
        "feature-api",
        "api.rs",
        "// API implementation",
        "Add REST API endpoints",
    );
    // Set user marker
    repo.set_marker("feature-api", "🤖");

    // Branch with uncommitted changes and user marker 💬
    let review_wt = repo.add_worktree_with_commit(
        "review-ui",
        "component.tsx",
        "// UI component",
        "Add dashboard component",
    );
    // Add uncommitted changes
    std::fs::write(review_wt.join("styles.css"), "/* pending styles */").unwrap();
    // Set user marker
    repo.set_marker("review-ui", "💬");

    // Branch with uncommitted changes only (no user marker)
    let wip_wt = repo.add_worktree("wip-docs");
    std::fs::write(wip_wt.join("README.md"), "# Documentation").unwrap();

    snapshot_list("with_user_marker", &repo);
}

#[rstest]
fn test_list_json_with_user_marker(mut repo: TestRepo) {
    repo.commit_with_age("Initial commit", DAY);

    // Worktree with user marker (emoji only)
    repo.add_worktree("with-status");

    // Set user marker (branch-keyed)
    repo.set_marker("with-status", "🔧");

    // Worktree without user marker
    repo.add_worktree("without-status");

    snapshot_list_json("json_with_user_marker", &repo);
}

#[rstest]
fn test_list_json_with_git_operation(mut repo: TestRepo) {
    // Test JSON output includes git_operation field when worktree is in rebase state

    // Create initial commit with a file that will conflict
    std::fs::write(
        repo.root_path().join("conflict.txt"),
        "original line 1\noriginal line 2\n",
    )
    .unwrap();
    repo.commit("Initial commit");

    // Create feature worktree
    let feature = repo.add_worktree_with_commit(
        "feature",
        "conflict.txt",
        "feature line 1\nfeature line 2\n",
        "Feature changes",
    );

    // Main makes conflicting changes
    std::fs::write(
        repo.root_path().join("conflict.txt"),
        "main line 1\nmain line 2\n",
    )
    .unwrap();
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

    // JSON output should show git_operation: "rebase" for the feature worktree
    snapshot_list_json("json_with_git_operation", &repo);
}

#[rstest]
fn test_list_branch_only_with_status(repo: TestRepo) {
    // Test that branch-only entries (no worktree) can display branch-keyed status

    // Create a branch-only entry (no worktree)
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["branch", "branch-only"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    // Set branch-keyed status for the branch-only entry
    repo.set_marker("branch-only", "🌿");

    // Use --branches flag to show branch-only entries
    snapshot_list_with_branches("branch_only_with_status", &repo);
}

#[rstest]
fn test_list_user_marker_with_special_characters(mut repo: TestRepo) {
    // Test with single emoji
    repo.add_worktree("emoji");
    repo.set_marker("emoji", "🔄");

    // Test with compound emoji (multi-codepoint)
    repo.add_worktree("multi");
    repo.set_marker("multi", "👨‍💻");

    snapshot_list("user_marker_special_chars", &repo);
}

/// Set up a repo for README examples showing realistic worktree states.
///
/// **Project context**: API modernization — migrating from legacy handlers to REST,
/// hardening auth before v2 launch.
///
/// ## Branch Narratives
///
/// **`@ feature-api`** — REST migration in progress
///   Midway through migrating from function-based handlers to a REST module.
///   Staged the new controller base class; still removing the legacy dispatcher.
///   Three commits ready to push once local tests pass.
///   - `+234 -24` main…± — Major refactoring: new Router, handlers, middleware (~250 LOC)
///   - `+` staged, `↑⇡` ahead of main and remote, `⇡3` unpushed commits
///
/// **`^ main`**
///   Teammate merged the auth hotfix while you were refactoring.
///   Need to pull and rebase feature-api before continuing.
///   - `⇣1` behind remote
///
/// **`+ fix-auth`** — Token validation hardening
///   Replaced manual token parsing with constant-time comparison and added rate limiting.
///   Pushed and CI green — waiting on security review before merge.
///   - `+25 -11` main…± — Deleted insecure validation, added proper checks
///   - `|` in sync with remote, ready for merge
///
/// **`exp`** — GraphQL spike
///   Spike branch exploring GraphQL for the subscription API. Added schema definitions
///   and proof-of-concept resolvers with Query, Mutation, and Subscription roots.
///   - `+137` main…± — Schema types, resolvers, pagination (~140 LOC)
///   - `⎇` branch without worktree
///
/// **`wip`** — REST docs (stale)
///   Started API docs last week but got pulled away. Main has since moved on
///   (fix-auth was merged). Needs rebase before continuing.
///   - `↓1` behind main — main advanced while branch was idle
///   - `+33` main…± — Doc skeleton with structure
///   - `⎇` branch without worktree
///
/// Returns feature_api_path for running commands from feature-api.
fn setup_readme_example_repo(repo: &mut TestRepo) -> std::path::PathBuf {
    // === Set up main branch with initial codebase ===
    // Main has a working API with security issues that fix-auth will harden
    std::fs::write(
        repo.root_path().join("api.rs"),
        r#"//! API module - initial implementation
pub mod auth {
    // INSECURE: Manual string comparison vulnerable to timing attacks
    pub fn check_token(token: &str) -> bool {
        if token.is_empty() { return false; }
        // Just check format, no real validation
        token.len() > 0 && token.starts_with("tk_")
    }

    // INSECURE: No rate limiting, no audit logging
    pub fn validate_request(token: &str) -> bool {
        check_token(token)
    }

    // INSECURE: Tokens stored in plain text
    pub fn store_token(user_id: u32, token: &str) {
        std::fs::write(format!("/tmp/tokens/{}", user_id), token).ok();
    }
}

pub mod handlers {
    pub fn health() -> &'static str { "ok" }
    // Legacy endpoint - needs refactoring
    pub fn get_user(id: u32) -> String { format!("user:{}", id) }
    pub fn get_post(id: u32) -> String { format!("post:{}", id) }
}
"#,
    )
    .unwrap();
    run_git(repo, &["add", "api.rs"], repo.root_path());
    repo.commit_staged_with_age("Initial API implementation", DAY, repo.root_path());
    repo.setup_remote("main");

    // Make main behind its remote: push a teammate's commit, then reset local
    // Story: A teammate pushed a hotfix while we were working on features
    repo.commit_with_age("Fix production timeout issue", 2 * HOUR);
    run_git(repo, &["push", "origin", "main"], repo.root_path());
    run_git(repo, &["reset", "--hard", "HEAD~1"], repo.root_path());

    // === Create fix-auth worktree ===
    // Story: Security audit found the token validation was too weak.
    // This branch fixes it by replacing the permissive check with proper validation.
    let fix_auth = repo.add_worktree("fix-auth");

    // First commit: Replace weak validation with constant-time comparison
    std::fs::write(
        fix_auth.join("api.rs"),
        r#"//! API module - auth hardened
pub mod auth {
    use constant_time_eq::constant_time_eq;

    /// Validates token with constant-time comparison (timing attack resistant)
    pub fn check_token(token: &str) -> bool {
        if token.len() < 32 { return false; }
        if !token.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') { return false; }
        let prefix = token.as_bytes().get(..3).unwrap_or(&[]);
        constant_time_eq(prefix, b"tk_")
    }

    /// Rate-limited request validation with audit logging
    pub fn validate_request(token: &str, client_ip: &str) -> Result<(), AuthError> {
        if is_rate_limited(client_ip) {
            log_auth_attempt(client_ip, "rate_limited");
            return Err(AuthError::RateLimited);
        }
        if !check_token(token) {
            log_auth_attempt(client_ip, "invalid_token");
            return Err(AuthError::InvalidToken);
        }
        Ok(())
    }
}

pub mod handlers {
    pub fn health() -> &'static str { "ok" }
    // Legacy endpoint - needs refactoring
    pub fn get_user(id: u32) -> String { format!("user:{}", id) }
    pub fn get_post(id: u32) -> String { format!("post:{}", id) }
}
"#,
    )
    .unwrap();
    run_git(repo, &["add", "api.rs"], &fix_auth);
    repo.commit_staged_with_age("Harden token validation", 6 * HOUR, &fix_auth);

    // Second commit: Add secure token storage
    std::fs::write(
        fix_auth.join("api.rs"),
        r#"//! API module - auth hardened
pub mod auth {
    use constant_time_eq::constant_time_eq;

    /// Validates token with constant-time comparison (timing attack resistant)
    pub fn check_token(token: &str) -> bool {
        if token.len() < 32 { return false; }
        if !token.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') { return false; }
        let prefix = token.as_bytes().get(..3).unwrap_or(&[]);
        constant_time_eq(prefix, b"tk_")
    }

    /// Rate-limited request validation with audit logging
    pub fn validate_request(token: &str, client_ip: &str) -> Result<(), AuthError> {
        if is_rate_limited(client_ip) {
            log_auth_attempt(client_ip, "rate_limited");
            return Err(AuthError::RateLimited);
        }
        if !check_token(token) {
            log_auth_attempt(client_ip, "invalid_token");
            return Err(AuthError::InvalidToken);
        }
        Ok(())
    }

    /// Stores token hash with per-user salt (never stores plaintext)
    pub fn store_token(user_id: u32, token: &str) -> Result<(), AuthError> {
        let salt = generate_salt(user_id);
        let hash = argon2_hash(token, &salt);
        db::tokens().insert(user_id, hash)?;
        Ok(())
    }
}

pub mod handlers {
    pub fn health() -> &'static str { "ok" }
    // Legacy endpoint - needs refactoring
    pub fn get_user(id: u32) -> String { format!("user:{}", id) }
    pub fn get_post(id: u32) -> String { format!("post:{}", id) }
}
"#,
    )
    .unwrap();
    run_git(repo, &["add", "api.rs"], &fix_auth);
    repo.commit_staged_with_age("Add secure token storage", 5 * HOUR, &fix_auth);

    // Push fix-auth and sync with remote
    run_git(repo, &["push", "-u", "origin", "fix-auth"], &fix_auth);

    // === Create feature-api worktree ===
    // Story: Major API refactoring - replacing the legacy handlers with a proper
    // REST structure. This involves deleting the old inline handlers and building
    // a modular system with middleware, validation, and caching.
    let feature_api = repo.add_worktree("feature-api");

    // First commit: Refactor api.rs - remove legacy handlers, add module structure
    // This replaces main's monolithic api.rs with a cleaner module layout
    std::fs::write(
        feature_api.join("api.rs"),
        r#"//! API module - refactored for REST architecture
//!
//! This module provides the public interface for the REST API.
//! All handlers have been moved to dedicated route modules.

pub mod routes;
pub mod middleware;
pub mod errors;

// Re-export commonly used types
pub use routes::{Router, Route, Handler};
pub use middleware::{RequestContext, ResponseBuilder};
pub use errors::{ApiError, ApiResult};
"#,
    )
    .unwrap();
    std::fs::write(
        feature_api.join("routes.rs"),
        r#"//! REST route definitions and handler implementations
use crate::middleware::{RequestContext, ResponseBuilder};
use crate::errors::{ApiError, ApiResult};

pub struct Router {
    routes: Vec<Route>,
}

pub struct Route {
    method: Method,
    path: String,
    handler: Box<dyn Handler>,
}

pub trait Handler: Send + Sync {
    fn handle(&self, ctx: &RequestContext) -> ApiResult<ResponseBuilder>;
}

impl Router {
    pub fn new() -> Self {
        Self { routes: Vec::new() }
    }

    pub fn get<H: Handler + 'static>(&mut self, path: &str, handler: H) -> &mut Self {
        self.routes.push(Route {
            method: Method::Get,
            path: path.to_string(),
            handler: Box::new(handler),
        });
        self
    }

    pub fn post<H: Handler + 'static>(&mut self, path: &str, handler: H) -> &mut Self {
        self.routes.push(Route {
            method: Method::Post,
            path: path.to_string(),
            handler: Box::new(handler),
        });
        self
    }

    pub fn route(&self, method: Method, path: &str) -> Option<&dyn Handler> {
        self.routes.iter()
            .find(|r| r.method == method && r.path == path)
            .map(|r| r.handler.as_ref())
    }
}

// Health check endpoint
pub struct HealthHandler;
impl Handler for HealthHandler {
    fn handle(&self, _ctx: &RequestContext) -> ApiResult<ResponseBuilder> {
        Ok(ResponseBuilder::new().status(200).body("ok"))
    }
}

// User endpoints
pub struct GetUserHandler;
impl Handler for GetUserHandler {
    fn handle(&self, ctx: &RequestContext) -> ApiResult<ResponseBuilder> {
        let user_id = ctx.param("id").ok_or(ApiError::BadRequest)?;
        // Fetch user from database
        Ok(ResponseBuilder::new().status(200).json(&user_id))
    }
}

pub struct ListUsersHandler;
impl Handler for ListUsersHandler {
    fn handle(&self, ctx: &RequestContext) -> ApiResult<ResponseBuilder> {
        let limit = ctx.query("limit").unwrap_or(20);
        let offset = ctx.query("offset").unwrap_or(0);
        // Paginated user list
        Ok(ResponseBuilder::new().status(200).json(&(limit, offset)))
    }
}

// Post endpoints
pub struct GetPostHandler;
impl Handler for GetPostHandler {
    fn handle(&self, ctx: &RequestContext) -> ApiResult<ResponseBuilder> {
        let post_id = ctx.param("id").ok_or(ApiError::BadRequest)?;
        Ok(ResponseBuilder::new().status(200).json(&post_id))
    }
}

pub struct CreatePostHandler;
impl Handler for CreatePostHandler {
    fn handle(&self, ctx: &RequestContext) -> ApiResult<ResponseBuilder> {
        let body = ctx.body().ok_or(ApiError::BadRequest)?;
        // Validate and create post
        Ok(ResponseBuilder::new().status(201).json(&body))
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Method { Get, Post, Put, Delete }
"#,
    )
    .unwrap();
    run_git(repo, &["add", "api.rs", "routes.rs"], &feature_api);
    repo.commit_staged_with_age("Refactor API to REST modules", 4 * HOUR, &feature_api);
    run_git(repo, &["push", "-u", "origin", "feature-api"], &feature_api);

    // More commits (ahead of remote - unpushed local work)
    std::fs::write(
        feature_api.join("middleware.rs"),
        r#"//! Middleware stack for request processing
use std::time::Instant;
use std::collections::HashMap;

/// Context passed through the middleware chain
pub struct RequestContext {
    pub user_id: Option<u32>,
    pub started_at: Instant,
    pub headers: HashMap<String, String>,
    pub params: HashMap<String, String>,
    pub query: HashMap<String, String>,
    body: Option<Vec<u8>>,
}

impl RequestContext {
    pub fn new() -> Self {
        Self {
            user_id: None,
            started_at: Instant::now(),
            headers: HashMap::new(),
            params: HashMap::new(),
            query: HashMap::new(),
            body: None,
        }
    }

    pub fn param(&self, key: &str) -> Option<&str> {
        self.params.get(key).map(|s| s.as_str())
    }

    pub fn query<T: std::str::FromStr>(&self, key: &str) -> Option<T> {
        self.query.get(key).and_then(|s| s.parse().ok())
    }

    pub fn body(&self) -> Option<&[u8]> {
        self.body.as_deref()
    }

    pub fn header(&self, key: &str) -> Option<&str> {
        self.headers.get(key).map(|s| s.as_str())
    }
}

/// Builder for HTTP responses
pub struct ResponseBuilder {
    status: u16,
    headers: HashMap<String, String>,
    body: Option<Vec<u8>>,
}

impl ResponseBuilder {
    pub fn new() -> Self {
        Self {
            status: 200,
            headers: HashMap::new(),
            body: None,
        }
    }

    pub fn status(mut self, code: u16) -> Self {
        self.status = code;
        self
    }

    pub fn header(mut self, key: &str, value: &str) -> Self {
        self.headers.insert(key.to_string(), value.to_string());
        self
    }

    pub fn body(mut self, content: &str) -> Self {
        self.body = Some(content.as_bytes().to_vec());
        self
    }

    pub fn json<T: serde::Serialize>(mut self, value: &T) -> Self {
        self.headers.insert("Content-Type".into(), "application/json".into());
        self.body = serde_json::to_vec(value).ok();
        self
    }
}

/// Timing middleware for performance monitoring
pub fn timing<F, R>(name: &str, f: F) -> R where F: FnOnce() -> R {
    let start = Instant::now();
    let result = f();
    log::debug!("{} completed in {:?}", name, start.elapsed());
    result
}

/// Authentication middleware
pub fn authenticate(ctx: &mut RequestContext) -> Result<(), AuthError> {
    let token = ctx.header("Authorization")
        .and_then(|h| h.strip_prefix("Bearer "))
        .ok_or(AuthError::MissingToken)?;

    let user_id = validate_token(token)?;
    ctx.user_id = Some(user_id);
    Ok(())
}

fn validate_token(token: &str) -> Result<u32, AuthError> {
    // Token validation logic
    if token.len() < 32 { return Err(AuthError::InvalidToken); }
    Ok(1) // Placeholder
}

pub enum AuthError { MissingToken, InvalidToken }
"#,
    )
    .unwrap();
    run_git(repo, &["add", "middleware.rs"], &feature_api);
    repo.commit_staged_with_age("Add request middleware", 3 * HOUR, &feature_api);

    std::fs::write(
        feature_api.join("validation.rs"),
        r#"//! Request validation
pub fn validate(body: &[u8], headers: &Headers) -> Result<(), Error> {
    if body.is_empty() { return Err(Error::EmptyBody); }
    if body.len() > MAX_SIZE { return Err(Error::TooLarge); }
    if !headers.contains_key("Authorization") { return Err(Error::Unauthorized); }
    Ok(())
}
"#,
    )
    .unwrap();
    run_git(repo, &["add", "validation.rs"], &feature_api);
    repo.commit_staged_with_age("Add request validation", 2 * HOUR, &feature_api);

    std::fs::write(
        feature_api.join("tests.rs"),
        r#"//! API tests
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health() { assert_eq!(routes::health(), "ok"); }

    #[test]
    fn test_validation_empty() {
        assert!(validation::validate(&[], &headers()).is_err());
    }
}
"#,
    )
    .unwrap();
    run_git(repo, &["add", "tests.rs"], &feature_api);
    repo.commit_staged_with_age("Add API tests", 30 * MINUTE, &feature_api);

    // Staged changes: new files + refactor existing (creates mixed +/- for HEAD±)
    // Adding caching and rate limiting, plus refactoring validation
    std::fs::write(
        feature_api.join("cache.rs"),
        r#"//! Caching layer
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub struct Cache<T> {
    store: HashMap<String, (T, Instant)>,
    ttl: Duration,
}

impl<T: Clone> Cache<T> {
    pub fn new(ttl_secs: u64) -> Self {
        Self { store: HashMap::new(), ttl: Duration::from_secs(ttl_secs) }
    }
    pub fn get(&self, key: &str) -> Option<T> {
        self.store.get(key).and_then(|(v, t)| {
            if t.elapsed() < self.ttl { Some(v.clone()) } else { None }
        })
    }
}
"#,
    )
    .unwrap();
    std::fs::write(
        feature_api.join("rate_limit.rs"),
        r#"//! Rate limiting
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub struct RateLimiter {
    requests: HashMap<String, Vec<Instant>>,
    window: Duration,
    limit: u32,
}

impl RateLimiter {
    pub fn check(&mut self, key: &str) -> bool {
        let now = Instant::now();
        let reqs = self.requests.entry(key.to_string()).or_default();
        reqs.retain(|t| now.duration_since(*t) < self.window);
        reqs.len() < self.limit as usize
    }
}
"#,
    )
    .unwrap();
    // Refactor validation.rs to use the new error types
    std::fs::write(
        feature_api.join("validation.rs"),
        r#"//! Request validation (refactored)
use crate::error::ValidationError;

pub fn validate(body: &[u8], headers: &Headers) -> Result<(), ValidationError> {
    validate_body(body)?;
    validate_headers(headers)?;
    Ok(())
}

fn validate_body(body: &[u8]) -> Result<(), ValidationError> {
    if body.is_empty() { return Err(ValidationError::Empty); }
    if body.len() > MAX_SIZE { return Err(ValidationError::TooLarge); }
    Ok(())
}

fn validate_headers(h: &Headers) -> Result<(), ValidationError> {
    h.get("Authorization").ok_or(ValidationError::Unauthorized)?;
    Ok(())
}
"#,
    )
    .unwrap();
    run_git(
        repo,
        &["add", "cache.rs", "rate_limit.rs", "validation.rs"],
        &feature_api,
    );

    // === Create branches without worktrees ===
    // These demonstrate the --branches flag showing branch-only entries

    // Create 'exp' branch with commits (experimental GraphQL work)
    // Narrative: Someone explored GraphQL as an alternative to REST, got pretty far with
    // schema design and resolvers, but the team decided to stick with REST for now.
    let exp_wt = repo.root_path().parent().unwrap().join("temp-exp");
    run_git(
        repo,
        &["worktree", "add", "-b", "exp", exp_wt.to_str().unwrap()],
        repo.root_path(),
    );

    std::fs::write(
        exp_wt.join("graphql.rs"),
        r#"//! GraphQL schema exploration - evaluating GraphQL for real-time subscriptions
//!
//! This spike branch explores whether GraphQL could replace REST for the subscription
//! API. Key evaluation criteria:
//! - Real-time updates via subscriptions
//! - Efficient data fetching (avoid over-fetching)
//! - Type safety with code generation

use async_graphql::*;

/// Core user type with all fields exposed via GraphQL
#[derive(SimpleObject, Clone)]
pub struct User {
    pub id: ID,
    pub name: String,
    pub email: String,
    pub avatar_url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Blog post with author relationship
#[derive(SimpleObject, Clone)]
pub struct Post {
    pub id: ID,
    pub title: String,
    pub body: String,
    pub author: User,
    pub published_at: Option<DateTime<Utc>>,
    pub tags: Vec<String>,
}

/// Comment on a post
#[derive(SimpleObject, Clone)]
pub struct Comment {
    pub id: ID,
    pub body: String,
    pub author: User,
    pub post_id: ID,
    pub created_at: DateTime<Utc>,
}

/// Subscription events for real-time updates
#[derive(Clone)]
pub enum SubscriptionEvent {
    PostCreated(Post),
    PostUpdated(Post),
    CommentAdded { post_id: ID, comment: Comment },
}

/// Pagination support
#[derive(InputObject)]
pub struct PaginationInput {
    pub limit: Option<i32>,
    pub offset: Option<i32>,
    pub cursor: Option<String>,
}

#[derive(SimpleObject)]
pub struct PageInfo {
    pub has_next_page: bool,
    pub has_previous_page: bool,
    pub start_cursor: Option<String>,
    pub end_cursor: Option<String>,
}
"#,
    )
    .unwrap();
    run_git(repo, &["add", "graphql.rs"], &exp_wt);
    repo.commit_staged_with_age("Explore GraphQL schema design", 2 * DAY, &exp_wt);

    std::fs::write(
        exp_wt.join("resolvers.rs"),
        r#"//! GraphQL resolvers - Query, Mutation, and Subscription roots
use crate::graphql::*;
use async_graphql::*;

pub struct QueryRoot;

#[Object]
impl QueryRoot {
    /// Fetch a single user by ID
    async fn user(&self, ctx: &Context<'_>, id: ID) -> Result<Option<User>> {
        let db = ctx.data::<Database>()?;
        Ok(db.get_user(&id).await?)
    }

    /// List users with pagination
    async fn users(&self, ctx: &Context<'_>, pagination: Option<PaginationInput>) -> Result<Vec<User>> {
        let db = ctx.data::<Database>()?;
        let page = pagination.unwrap_or_default();
        Ok(db.list_users(page.limit.unwrap_or(20), page.offset.unwrap_or(0)).await?)
    }

    /// Fetch a single post by ID
    async fn post(&self, ctx: &Context<'_>, id: ID) -> Result<Option<Post>> {
        let db = ctx.data::<Database>()?;
        Ok(db.get_post(&id).await?)
    }

    /// List posts with optional author filter
    async fn posts(&self, ctx: &Context<'_>, author_id: Option<ID>) -> Result<Vec<Post>> {
        let db = ctx.data::<Database>()?;
        match author_id {
            Some(id) => Ok(db.posts_by_author(&id).await?),
            None => Ok(db.list_posts().await?),
        }
    }
}

pub struct MutationRoot;

#[Object]
impl MutationRoot {
    /// Create a new post
    async fn create_post(&self, ctx: &Context<'_>, title: String, body: String) -> Result<Post> {
        let db = ctx.data::<Database>()?;
        let user = ctx.data::<AuthenticatedUser>()?;
        let post = db.create_post(user.id.clone(), title, body).await?;
        Ok(post)
    }

    /// Add a comment to a post
    async fn add_comment(&self, ctx: &Context<'_>, post_id: ID, body: String) -> Result<Comment> {
        let db = ctx.data::<Database>()?;
        let user = ctx.data::<AuthenticatedUser>()?;
        let comment = db.add_comment(user.id.clone(), post_id, body).await?;
        Ok(comment)
    }
}

pub struct SubscriptionRoot;

#[Subscription]
impl SubscriptionRoot {
    /// Subscribe to new comments on a post
    async fn comment_added(&self, post_id: ID) -> impl Stream<Item = Comment> {
        todo!("Implement subscription stream")
    }

    /// Subscribe to all post updates
    async fn post_updates(&self) -> impl Stream<Item = Post> {
        todo!("Implement subscription stream")
    }
}
"#,
    )
    .unwrap();
    run_git(repo, &["add", "resolvers.rs"], &exp_wt);
    repo.commit_staged_with_age("Add GraphQL resolvers scaffold", 2 * DAY, &exp_wt);

    // Remove the worktree but keep the branch
    run_git(
        repo,
        &["worktree", "remove", exp_wt.to_str().unwrap()],
        repo.root_path(),
    );

    // Create 'wip' branch with commits (work-in-progress docs)
    // Narrative: Someone started API docs last week. Main has since advanced
    // (fix-auth was merged), so wip is now behind and needs a rebase.

    // Save current main commit, then add a commit to main (simulating fix-auth merge)
    let wip_base = {
        let output = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo.root_path())
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };

    // Add commit to main (simulating fix-auth being merged while wip was idle)
    repo.commit_with_age("Merge fix-auth: hardened token validation", 4 * DAY);

    // Create wip from the earlier commit (before main advanced)
    let wip_wt = repo.root_path().parent().unwrap().join("temp-wip");
    run_git(
        repo,
        &[
            "worktree",
            "add",
            "-b",
            "wip",
            wip_wt.to_str().unwrap(),
            &wip_base,
        ],
        repo.root_path(),
    );

    std::fs::write(
        wip_wt.join("API.md"),
        r#"# API Documentation

## Overview

This document describes the REST API endpoints for the application.

## Authentication

All endpoints require a valid Bearer token in the `Authorization` header.

```
Authorization: Bearer <token>
```

## Endpoints

### Users

- `GET /users` - List all users (paginated)
- `GET /users/:id` - Get user by ID
- `POST /users` - Create new user

### Posts

- `GET /posts` - List all posts
- `GET /posts/:id` - Get post by ID
- `POST /posts` - Create new post

## Error Responses

All errors return JSON with `error` and `message` fields.

TODO: Add request/response examples for each endpoint
"#,
    )
    .unwrap();
    run_git(repo, &["add", "API.md"], &wip_wt);
    repo.commit_staged_with_age("Start API documentation", 3 * DAY, &wip_wt);

    // Remove the worktree but keep the branch
    run_git(
        repo,
        &["worktree", "remove", wip_wt.to_str().unwrap()],
        repo.root_path(),
    );

    // === Mock CI status ===
    // CI requires --full flag, but we mock it so examples show realistic output
    // Note: main's CI is mocked AFTER the merge commit so the hash matches
    mock_ci_status(repo, "main", "passed", "pull-request", false);
    mock_ci_status(repo, "fix-auth", "passed", "pull-request", false);
    // feature-api has unpushed commits, so CI is stale (shows dimmed)
    mock_ci_status(repo, "feature-api", "running", "pull-request", true);

    feature_api
}

/// Helper to run git commands
fn run_git(repo: &TestRepo, args: &[&str], cwd: &std::path::Path) {
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(args).current_dir(cwd).output().unwrap();
}

/// Mock CI status by writing to file-based cache
fn mock_ci_status(repo: &TestRepo, branch: &str, status: &str, source: &str, is_stale: bool) {
    // Get HEAD commit for the branch
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    let output = cmd
        .args(["rev-parse", branch])
        .current_dir(repo.root_path())
        .output()
        .unwrap();
    let head = String::from_utf8_lossy(&output.stdout).trim().to_string();

    // Build the cache JSON (matches CachedCiStatus struct)
    let cache_json = format!(
        r#"{{"status":{{"ci_status":"{}","source":"{}","is_stale":{}}},"checked_at":{},"head":"{}"}}"#,
        status,
        source,
        is_stale,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        head
    );

    // Get git common dir for cache location
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    let output = cmd
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();
    let git_dir = String::from_utf8_lossy(&output.stdout).trim().to_string();

    // Resolve relative path if needed
    let git_path = if std::path::Path::new(&git_dir).is_absolute() {
        std::path::PathBuf::from(&git_dir)
    } else {
        repo.root_path().join(&git_dir)
    };

    // Create cache directory and write file
    let cache_dir = git_path.join("wt-cache").join("ci-status");
    std::fs::create_dir_all(&cache_dir).unwrap();

    // Sanitize branch name for filename (replace / and \ with -)
    let safe_branch: String = branch
        .chars()
        .map(|c| if c == '/' || c == '\\' { '-' } else { c })
        .collect();
    let cache_file = cache_dir.join(format!("{safe_branch}.json"));
    std::fs::write(&cache_file, &cache_json).unwrap();
}

/// Generate README example: Basic `wt list` output
///
/// Shows worktree states with status symbols, divergence, and remote tracking.
/// Uses narrower width (100 cols) to fit in doc site code blocks.
/// Output: tests/snapshots/integration__integration_tests__list__readme_example_list.snap
#[rstest]
fn test_readme_example_list(mut repo: TestRepo) {
    let feature_api = setup_readme_example_repo(&mut repo);
    snapshot_readme_list_from_dir("readme_example_list", &repo, &feature_api);
}

/// Generate README example: `wt list --full` output
///
/// Shows additional columns: main…± (line diffs in commits) and CI status.
/// Uses narrower width (100 cols) to fit in doc site code blocks.
/// Output: tests/snapshots/integration__integration_tests__list__readme_example_list_full.snap
#[rstest]
fn test_readme_example_list_full(mut repo: TestRepo) {
    let feature_api = setup_readme_example_repo(&mut repo);
    snapshot_readme_list_full_from_dir("readme_example_list_full", &repo, &feature_api);
}

/// Generate README example: `wt list --branches --full` output
///
/// Shows branches without worktrees (⎇ symbol) alongside worktrees, plus CI status.
/// Uses narrower width (100 cols) to fit in doc site code blocks.
/// Output: tests/snapshots/integration__integration_tests__list__readme_example_list_branches.snap
#[rstest]
fn test_readme_example_list_branches(mut repo: TestRepo) {
    let feature_api = setup_readme_example_repo(&mut repo);
    snapshot_readme_list_branches_full_from_dir(
        "readme_example_list_branches",
        &repo,
        &feature_api,
    );
}

/// Generate tips-patterns.md example: dev server per worktree workflow
///
/// Uses the realistic README example repo and adds URL config.
/// URLs appear dimmed (no servers running) - realistic for documentation.
#[rstest]
fn test_tips_dev_server_workflow(mut repo: TestRepo) {
    // Set up the realistic README example repo
    let _feature_api = setup_readme_example_repo(&mut repo);

    // Add project config with URL template for dev servers
    repo.write_project_config(
        r#"[post-start]
server = "npm run dev -- --port {{ branch | hash_port }} &"

[list]
url = "http://localhost:{{ branch | hash_port }}"
"#,
    );

    // Run from main worktree (URLs dim since no servers running)
    run_snapshot(
        setup_snapshot_settings(&repo),
        "tips_dev_server_workflow",
        list_snapshots::command_readme(&repo, repo.root_path()),
    );
}

#[rstest]
fn test_list_progressive_flag(mut repo: TestRepo) {
    repo.add_worktree("feature-a");
    repo.add_worktree("feature-b");

    // Force progressive mode even in non-TTY test environment
    // Output should be identical to buffered mode (only process differs)
    snapshot_list_progressive("progressive_flag", &repo);
}

#[rstest]
fn test_list_no_progressive_flag(mut repo: TestRepo) {
    repo.add_worktree("feature");

    // Explicitly force buffered mode
    snapshot_list_no_progressive("no_progressive_flag", &repo);
}

#[rstest]
fn test_list_progressive_with_branches(mut repo: TestRepo) {
    // Create worktrees
    repo.add_worktree("feature-a");

    // Create branches without worktrees
    repo.create_branch("orphan-1");
    repo.create_branch("orphan-2");

    // Critical: test that --branches works with --progressive
    // This ensures progressive mode supports the --branches flag
    snapshot_list_progressive_branches("progressive_with_branches", &repo);
}

// ============================================================================
// Task DAG Mode Tests
// ============================================================================

#[rstest]
fn test_list_task_dag_single_worktree(repo: TestRepo) {
    snapshot_list_progressive("task_dag_single_worktree", &repo);
}

#[rstest]
fn test_list_task_dag_multiple_worktrees(mut repo: TestRepo) {
    repo.add_worktree("feature-a");
    repo.add_worktree("feature-b");
    repo.add_worktree("feature-c");

    snapshot_list_progressive("task_dag_multiple_worktrees", &repo);
}

#[rstest]
fn test_list_task_dag_full_with_diffs(mut repo: TestRepo) {
    // Create worktree with changes
    let feature_a = repo.add_worktree("feature-a");
    std::fs::write(feature_a.join("new.txt"), "content").unwrap();

    // Create another worktree with commits
    let _feature_b = repo.add_worktree_with_commit("feature-b", "file.txt", "test", "Test commit");

    snapshot_list_progressive_full("task_dag_full_with_diffs", &repo);
}

#[rstest]
fn test_list_task_dag_with_upstream(mut repo: TestRepo) {
    repo.commit("Initial commit on main");
    repo.setup_remote("main");

    // Branch in sync
    let in_sync = repo.add_worktree("in-sync");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["push", "-u", "origin", "in-sync"])
        .current_dir(&in_sync)
        .output()
        .unwrap();

    // Branch ahead
    let ahead = repo.add_worktree("ahead");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["push", "-u", "origin", "ahead"])
        .current_dir(&ahead)
        .output()
        .unwrap();
    std::fs::write(ahead.join("ahead.txt"), "ahead").unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."]).current_dir(&ahead).output().unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Ahead commit"])
        .current_dir(&ahead)
        .output()
        .unwrap();

    snapshot_list_progressive_full("task_dag_with_upstream", &repo);
}

#[rstest]
fn test_list_task_dag_many_worktrees(mut repo: TestRepo) {
    // Create 10 worktrees to test parallel processing
    for i in 1..=10 {
        repo.add_worktree(&format!("feature-{}", i));
    }

    snapshot_list_progressive("task_dag_many_worktrees", &repo);
}

#[rstest]
fn test_list_task_dag_with_locked_worktree(mut repo: TestRepo) {
    repo.add_worktree("normal");
    repo.add_worktree("locked");
    repo.lock_worktree("locked", Some("Testing task DAG with locked worktree"));

    snapshot_list_progressive("task_dag_with_locked", &repo);
}

#[rstest]
fn test_list_task_dag_detached_head(repo: TestRepo) {
    repo.detach_head();

    snapshot_list_progressive("task_dag_detached_head", &repo);
}

#[rstest]
fn test_list_task_dag_ordering_stability(mut repo: TestRepo) {
    // Test that task_dag mode produces same ordering as buffered mode
    // Regression test for progressive rendering order instability
    let current_path = setup_timestamped_worktrees(&mut repo);

    // Run from feature-current worktree
    // Expected order: main, feature-current, then by timestamp: feature-newest, feature-middle, feature-oldest
    run_snapshot(
        setup_snapshot_settings(&repo),
        "task_dag_ordering_stability",
        list_snapshots::command_progressive_from_dir(&repo, &current_path),
    );
}

#[rstest]
fn test_list_progressive_vs_buffered_identical_data(mut repo: TestRepo) {
    // Critical test: Verify that progressive and buffered modes collect identical data
    // despite using different rendering strategies (real-time UI vs collect-then-print).
    // This ensures consolidation on task DAG data collection works correctly.
    //
    // Note: We compare JSON output, not table output, because:
    // - Progressive mode renders headers before knowing final column widths (uses estimates)
    // - Buffered mode renders headers after data collection (uses actual widths)
    // - The DATA must be identical, but table formatting may differ slightly

    // Create varied worktrees to test multiple data points
    repo.add_worktree("feature-a");
    repo.add_worktree("feature-b");

    // Modify a worktree to have uncommitted changes
    let feature_a_path = repo.worktree_path("feature-a");
    std::fs::write(feature_a_path.join("changes.txt"), "test").unwrap();

    // Run both modes with JSON output to compare data (not formatting)
    let progressive_output = list_snapshots::command_progressive_json(&repo)
        .output()
        .unwrap();

    let buffered_output = list_snapshots::command_no_progressive_json(&repo)
        .output()
        .unwrap();

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
    let progressive_json: serde_json::Value =
        serde_json::from_slice(&progressive_output.stdout).unwrap();
    let buffered_json: serde_json::Value = serde_json::from_slice(&buffered_output.stdout).unwrap();

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

#[rstest]
fn test_list_with_c_flag(mut repo: TestRepo) {
    // Create some worktrees
    repo.add_worktree("feature-a");
    repo.add_worktree("feature-b");

    // Run wt -C <repo_path> list from a completely different directory
    let mut settings = setup_snapshot_settings(&repo);
    // Redact the -C path argument in metadata (try different selector formats)
    settings.add_redaction(".args[1]", "[REPO_PATH]");
    settings.bind(|| {
        let mut cmd = wt_command();
        cmd.args(["-C", repo.root_path().to_str().unwrap(), "list"]);
        // Run from system temp dir to ensure -C is actually being used
        cmd.current_dir(std::env::temp_dir());
        assert_cmd_snapshot!("list_with_c_flag", cmd);
    });
}

#[rstest]
fn test_list_large_diffs_alignment(mut repo: TestRepo) {
    // Worktree with large uncommitted changes and ahead commits
    // Use a longer branch name similar to user's "wli-sequence" to trigger column width
    let large_wt = repo.add_worktree("feature-changes");

    // Create a file with many lines for large diff
    let large_content = (1..=100)
        .map(|i| format!("line {}", i))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(large_wt.join("large.txt"), &large_content).unwrap();

    // Commit it
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(&large_wt)
        .output()
        .unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add 100 lines"])
        .current_dir(&large_wt)
        .output()
        .unwrap();

    // Add large uncommitted changes (both added and deleted lines)
    // Add a new file with many lines
    std::fs::write(large_wt.join("uncommitted.txt"), &large_content).unwrap();

    // Modify the existing file to create deletions
    let modified_content = (1..=50)
        .map(|i| format!("modified line {}", i))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(large_wt.join("large.txt"), &modified_content).unwrap();

    // Add another new file with many lines
    let another_large = (1..=80)
        .map(|i| format!("another line {}", i))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(large_wt.join("another.txt"), &another_large).unwrap();

    // Set user marker
    repo.set_marker("feature-changes", "🤖");

    // Worktree with short name to show gap before Status column
    let short_wt = repo.add_worktree("fix");
    std::fs::write(short_wt.join("quick.txt"), "quick fix").unwrap();

    // Set user marker for short branch
    repo.set_marker("fix", "💬");

    // Worktree with diverged status and working tree changes
    let diverged_wt = repo.add_worktree("diverged");

    // Commit some changes
    let diverged_content = (1..=60)
        .map(|i| format!("diverged line {}", i))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(diverged_wt.join("test.txt"), &diverged_content).unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(&diverged_wt)
        .output()
        .unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Diverged commit"])
        .current_dir(&diverged_wt)
        .output()
        .unwrap();

    // Add uncommitted changes
    let modified_diverged = (1..=40)
        .map(|i| format!("modified diverged line {}", i))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(diverged_wt.join("test.txt"), &modified_diverged).unwrap();

    // Set user marker
    repo.set_marker("diverged", "💬");

    snapshot_list("large_diffs_alignment", &repo);
}

#[rstest]
fn test_list_status_column_padding_with_emoji(mut repo: TestRepo) {
    // Create worktree matching user's exact scenario: "wli-sequence"
    let wli_seq = repo.add_worktree("wli-sequence");

    // Create large working tree changes: +164, -111
    // Need ~164 added lines and ~111 deleted lines
    let initial_content = (1..=200)
        .map(|i| format!("original line {}", i))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(wli_seq.join("main.txt"), &initial_content).unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(&wli_seq)
        .output()
        .unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Initial content"])
        .current_dir(&wli_seq)
        .output()
        .unwrap();

    // Modify to create desired diff: remove ~111 lines, add different content
    let modified_content = (1..=89)
        .map(|i| format!("original line {}", i))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(wli_seq.join("main.txt"), &modified_content).unwrap();

    // Add new file with ~164 lines to get +164
    let new_content = (1..=164)
        .map(|i| format!("new line {}", i))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(wli_seq.join("new.txt"), &new_content).unwrap();

    // Set user marker emoji 🤖
    repo.set_marker("wli-sequence", "🤖");

    // Create "pr-link" worktree with different status (fewer symbols, same emoji type)
    let pr_link = repo.add_worktree("pr-link");

    // Commit to make it ahead
    std::fs::write(pr_link.join("pr.txt"), "pr content").unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(&pr_link)
        .output()
        .unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "PR commit"])
        .current_dir(&pr_link)
        .output()
        .unwrap();

    // Set same emoji type
    repo.set_marker("pr-link", "🤖");

    // Create "main-symbol" with different emoji
    let main_sym = repo.add_worktree("main-symbol");
    std::fs::write(main_sym.join("sym.txt"), "symbol").unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(&main_sym)
        .output()
        .unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Symbol commit"])
        .current_dir(&main_sym)
        .output()
        .unwrap();

    repo.set_marker("main-symbol", "💬");

    snapshot_list("status_column_padding_emoji", &repo);
}

#[rstest]
fn test_list_maximum_working_tree_symbols(mut repo: TestRepo) {
    // Test that all 5 working tree symbols can appear simultaneously:
    // ? (untracked), ! (modified), + (staged), » (renamed), ✘ (deleted)
    // This verifies the maximum width of the working_tree position (5 chars)

    let feature = repo.add_worktree("feature");

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

#[rstest]
fn test_list_maximum_status_with_git_operation(mut repo: TestRepo) {
    // Test maximum status symbols including git operation (rebase/merge):
    // ?!+ (working_tree) + = (conflicts) + ↻ (rebase) + ↕ (diverged) + ⊠ (locked) + 🤖 (user marker)
    // This pushes the Status column to ~10-11 chars of actual content

    // Create initial commit with a file that will conflict
    std::fs::write(
        repo.root_path().join("conflict.txt"),
        "original line 1\noriginal line 2\n",
    )
    .unwrap();
    std::fs::write(repo.root_path().join("shared.txt"), "shared content").unwrap();
    repo.commit("Initial commit");

    // Create feature worktree
    let feature = repo.add_worktree("feature");

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

    // Set user marker emoji (🤖)
    repo.set_marker("feature", "🤖");

    // Result should show: ?!+ (working_tree) + = (conflicts) + ↻ (rebase) + ↕ (diverged) + ⊠ (locked) + 🤖 (user marker)
    // Use --full to enable conflict detection
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = list_snapshots::command(&repo, repo.root_path());
        cmd.arg("--full");
        assert_cmd_snapshot!("maximum_status_with_git_operation", cmd);
    });
}

#[rstest]
fn test_list_maximum_status_symbols(mut repo: TestRepo) {
    // Test the maximum status symbols possible:
    // ?!+»✘ (5) + ⚠ (1) + ⊠ (1) + ↕ (1) + ⇅ (1) + 🤖 (2) = 11 chars
    // Missing: ✖ (actual conflicts), ↻ (git operation - can't have with divergence), ◇ (bare), ⚠ (prunable)

    // Create initial commit on main with shared files
    std::fs::write(repo.root_path().join("shared.txt"), "original").unwrap();
    std::fs::write(repo.root_path().join("file-a.txt"), "a").unwrap();
    std::fs::write(repo.root_path().join("file-b.txt"), "b").unwrap();
    std::fs::write(repo.root_path().join("file-c.txt"), "c").unwrap();
    std::fs::write(repo.root_path().join("file-d.txt"), "d").unwrap();
    repo.commit("Initial commit");

    // Create feature worktree
    let feature = repo.add_worktree("feature");

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

    // Set user marker emoji (🤖)
    repo.set_marker("feature", "🤖");

    // Result should show 11 chars: ?!+»✘=⊠↕⇅🤖
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = list_snapshots::command(&repo, repo.root_path());
        cmd.arg("--full");
        assert_cmd_snapshot!("maximum_status_symbols", cmd);
    });
}

/// Test that --full detects working tree conflicts (uncommitted changes that would conflict).
///
/// This specifically tests the WorkingTreeConflicts task which:
/// 1. Uses `git stash create` to get a tree object from uncommitted changes
/// 2. Runs merge-tree against the default branch to detect conflicts
///
/// The key distinction from commit-level conflicts:
/// - Commit-level: HEAD conflicts with main (always checked)
/// - Working tree: Uncommitted changes conflict with main (only with --full)
#[rstest]
fn test_list_full_working_tree_conflicts(mut repo: TestRepo) {
    // Create initial commit with a shared file
    std::fs::write(repo.root_path().join("shared.txt"), "original content").unwrap();
    repo.commit("Initial commit");

    // Create feature worktree - at this point feature and main are identical
    let feature = repo.add_worktree("feature");

    // Advance main with a change to the shared file
    std::fs::write(repo.root_path().join("shared.txt"), "main's version").unwrap();
    repo.commit("Main changes shared.txt");

    // Feature's HEAD is still at the original commit - no commit-level conflict
    // because feature hasn't committed anything that conflicts

    // Now add uncommitted changes to feature that would conflict with main
    std::fs::write(feature.join("shared.txt"), "feature's uncommitted version").unwrap();

    // Without --full: no conflict symbol (only checks commit-level)
    snapshot_list("working_tree_conflicts_without_full", &repo);

    // With --full: should show conflict symbol because uncommitted changes conflict
    snapshot_list_full("working_tree_conflicts_with_full", &repo);
}

/// Test that clean working trees don't affect conflict detection with --full.
///
/// Even with --full, if the working tree is clean, we skip the stash-based check
/// and just use the commit-level conflict detection.
#[rstest]
fn test_list_full_clean_working_tree_uses_commit_conflicts(mut repo: TestRepo) {
    // Create initial commit with a shared file
    std::fs::write(repo.root_path().join("shared.txt"), "original content").unwrap();
    repo.commit("Initial commit");

    // Create feature worktree
    let feature = repo.add_worktree("feature");

    // Make a conflicting commit on feature (different change to shared.txt)
    std::fs::write(feature.join("shared.txt"), "feature's committed version").unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(&feature)
        .output()
        .unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Feature changes shared.txt"])
        .current_dir(&feature)
        .output()
        .unwrap();

    // Advance main with a different change to the shared file
    std::fs::write(repo.root_path().join("shared.txt"), "main's version").unwrap();
    repo.commit("Main changes shared.txt");

    // Feature has a committed conflict, working tree is clean
    // Both with and without --full should show the conflict symbol
    snapshot_list("commit_conflicts_without_full", &repo);
    snapshot_list_full("commit_conflicts_with_full", &repo);
}

#[rstest]
fn test_list_warns_when_default_branch_missing_worktree(repo: TestRepo) {
    // Move primary worktree off the default branch so no worktree holds it
    repo.switch_primary_to("develop");

    snapshot_list("default_branch_missing_worktree", &repo);
}

/// Test that git errors during task execution are collected and displayed as warnings.
///
/// Corrupts a branch ref to point to a non-existent commit, which causes
/// ahead_behind and other git operations to fail. Verifies the warning
/// message appears after the table.
#[rstest]
fn test_list_shows_warning_on_git_error(mut repo: TestRepo) {
    repo.add_worktree("feature");

    // Corrupt the feature branch ref to point to a non-existent commit.
    // Branch refs are stored in the main repo's .git/refs/heads, not the worktree.
    let git_dir = repo.root_path().join(".git");
    let ref_path = git_dir.join("refs/heads/feature");

    // Write an invalid SHA that doesn't exist in the repo
    std::fs::write(&ref_path, "0000000000000000000000000000000000000000\n").unwrap();

    snapshot_list("git_error_warning", &repo);
}
