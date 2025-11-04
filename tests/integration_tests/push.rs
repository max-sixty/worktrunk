use crate::common::{TestRepo, make_snapshot_cmd, setup_snapshot_settings};
use insta_cmd::assert_cmd_snapshot;
use std::process::Command;

/// Helper to create snapshot with normalized paths
fn snapshot_push(test_name: &str, repo: &TestRepo, args: &[&str], cwd: Option<&std::path::Path>) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        // Prepend "push" to args for `wt dev push` command
        let mut dev_args = vec!["push"];
        dev_args.extend_from_slice(args);
        let mut cmd = make_snapshot_cmd(repo, "dev", &dev_args, cwd);
        assert_cmd_snapshot!(test_name, cmd);
    });
}

#[test]
fn test_push_fast_forward() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create a worktree for main (checking out existing branch)
    let main_wt = repo.root_path().parent().unwrap().join("test-repo.main-wt");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["worktree", "add", main_wt.to_str().unwrap(), "main"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to add worktree");

    // Make a commit in a feature worktree
    let feature_wt = repo.add_worktree("feature", "feature");
    std::fs::write(feature_wt.join("test.txt"), "test content").expect("Failed to write file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "test.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add test file"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Push from feature to main
    snapshot_push("push_fast_forward", &repo, &["main"], Some(&feature_wt));
}

#[test]
fn test_push_not_fast_forward() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create commits in both worktrees
    // The repo root is already the main worktree
    std::fs::write(repo.root_path().join("main.txt"), "main content")
        .expect("Failed to write file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "main.txt"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to add file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add main file"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to commit");

    let feature_wt = repo.add_worktree("feature", "feature");
    std::fs::write(feature_wt.join("feature.txt"), "feature content")
        .expect("Failed to write file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "feature.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add feature file"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Try to push from feature to main (should fail - not fast-forward)
    snapshot_push("push_not_fast_forward", &repo, &["main"], Some(&feature_wt));
}

#[test]
fn test_push_to_default_branch() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    let main_wt = repo.root_path().parent().unwrap().join("test-repo.main-wt");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["worktree", "add", main_wt.to_str().unwrap(), "main"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to add worktree");

    let feature_wt = repo.add_worktree("feature", "feature");
    std::fs::write(feature_wt.join("feature.txt"), "feature content")
        .expect("Failed to write file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "feature.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add feature file"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Push without specifying target (should use default branch)
    snapshot_push("push_to_default", &repo, &[], Some(&feature_wt));
}

#[test]
fn test_push_with_dirty_target() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Make main worktree (repo root) dirty with a conflicting file
    std::fs::write(repo.root_path().join("conflict.txt"), "old content")
        .expect("Failed to write file");

    let feature_wt = repo.add_worktree("feature", "feature");
    std::fs::write(feature_wt.join("conflict.txt"), "new content").expect("Failed to write file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "conflict.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add conflict file"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Try to push (should fail due to conflicting changes)
    snapshot_push("push_dirty_target", &repo, &["main"], Some(&feature_wt));
}

#[test]
fn test_push_error_not_fast_forward() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create feature branch from initial commit
    let feature_wt = repo.add_worktree("feature", "feature");

    // Make a commit in the main worktree (repo root) and push it
    std::fs::write(repo.root_path().join("main-file.txt"), "main content")
        .expect("Failed to write file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "main-file.txt"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to add file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Main commit"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to commit");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["push", "origin", "main"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to push");

    // Make a commit in feature (which doesn't have main's commit)
    std::fs::write(feature_wt.join("feature.txt"), "feature content")
        .expect("Failed to write file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "feature.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Feature commit"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Try to push feature to main (should fail - main has commits not in feature)
    snapshot_push(
        "push_error_not_fast_forward",
        &repo,
        &["main"],
        Some(&feature_wt),
    );
}

#[test]
fn test_push_error_with_merge_commits() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create feature branch
    let feature_wt = repo.add_worktree("feature", "feature");
    std::fs::write(feature_wt.join("file1.txt"), "content1").expect("Failed to write file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "file1.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Commit 1"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Create another branch for merging
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["checkout", "-b", "temp"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to create temp branch");

    std::fs::write(feature_wt.join("file2.txt"), "content2").expect("Failed to write file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "file2.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Commit 2"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Switch back to feature and merge temp (creating merge commit)
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["checkout", "feature"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to checkout feature");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["merge", "temp", "--no-ff", "-m", "Merge temp"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to merge");

    // Try to push to main (should fail - has merge commits)
    snapshot_push(
        "push_error_with_merge_commits",
        &repo,
        &["main"],
        Some(&feature_wt),
    );
}

#[test]
fn test_push_with_merge_commits_allowed() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create feature branch
    let feature_wt = repo.add_worktree("feature", "feature");
    std::fs::write(feature_wt.join("file1.txt"), "content1").expect("Failed to write file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "file1.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Commit 1"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Create another branch for merging
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["checkout", "-b", "temp"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to create temp branch");

    std::fs::write(feature_wt.join("file2.txt"), "content2").expect("Failed to write file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "file2.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Commit 2"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Switch back to feature and merge temp (creating merge commit)
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["checkout", "feature"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to checkout feature");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["merge", "temp", "--no-ff", "-m", "Merge temp"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to merge");

    // Push to main with --allow-merge-commits (should succeed with acknowledgment)
    snapshot_push(
        "push_with_merge_commits_allowed",
        &repo,
        &["main", "--allow-merge-commits"],
        Some(&feature_wt),
    );
}

#[test]
fn test_push_no_remote() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    // Deliberately NOT calling setup_remote to test the error case

    // Create a feature worktree and make a commit
    let feature_wt = repo.add_worktree("feature", "feature");
    std::fs::write(feature_wt.join("feature.txt"), "feature content")
        .expect("Failed to write file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "feature.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add feature file"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Try to push without specifying target (should fail - no remote to get default branch)
    snapshot_push("push_no_remote", &repo, &[], Some(&feature_wt));
}
