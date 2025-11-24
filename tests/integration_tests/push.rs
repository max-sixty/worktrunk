use crate::common::{TestRepo, make_snapshot_cmd, setup_snapshot_settings};
use insta_cmd::assert_cmd_snapshot;
use std::process::Command;

/// Helper to create snapshot with normalized paths
fn snapshot_push(test_name: &str, repo: &TestRepo, args: &[&str], cwd: Option<&std::path::Path>) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        // Prepend "push" to args for `wt step push` command
        let mut step_args = vec!["push"];
        step_args.extend_from_slice(args);
        let mut cmd = make_snapshot_cmd(repo, "step", &step_args, cwd);
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
        .unwrap();

    // Make a commit in a feature worktree
    let feature_wt = repo.add_worktree("feature", "feature");
    std::fs::write(feature_wt.join("test.txt"), "test content").unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "test.txt"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add test file"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

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
    std::fs::write(repo.root_path().join("main.txt"), "main content").unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "main.txt"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add main file"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    let feature_wt = repo.add_worktree("feature", "feature");
    std::fs::write(feature_wt.join("feature.txt"), "feature content").unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "feature.txt"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add feature file"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

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
        .unwrap();

    let feature_wt = repo.add_worktree("feature", "feature");
    std::fs::write(feature_wt.join("feature.txt"), "feature content").unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "feature.txt"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add feature file"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    // Push without specifying target (should use default branch)
    snapshot_push("push_to_default", &repo, &[], Some(&feature_wt));
}

#[test]
fn test_push_with_dirty_target() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Make main worktree (repo root) dirty with a conflicting file
    std::fs::write(repo.root_path().join("conflict.txt"), "old content").unwrap();

    let feature_wt = repo.add_worktree("feature", "feature");
    std::fs::write(feature_wt.join("conflict.txt"), "new content").unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "conflict.txt"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add conflict file"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    // Try to push (should fail due to conflicting changes)
    snapshot_push(
        "push_dirty_target_overlap",
        &repo,
        &["main"],
        Some(&feature_wt),
    );

    // Ensure target worktree still has original file content and no stash was created
    let main_contents = std::fs::read_to_string(repo.root_path().join("conflict.txt")).unwrap();
    assert_eq!(main_contents, "old content");

    let mut git_cmd = Command::new("git");
    repo.configure_git_cmd(&mut git_cmd);
    let stash_list = git_cmd
        .args(["stash", "list"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&stash_list.stdout)
            .trim()
            .is_empty()
    );
}

#[test]
fn test_push_dirty_target_autostash() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Make main worktree (repo root) dirty with a non-conflicting file
    std::fs::write(repo.root_path().join("notes.txt"), "temporary notes").unwrap();

    let feature_wt = repo.add_worktree("feature", "feature");
    std::fs::write(feature_wt.join("feature.txt"), "feature content").unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "feature.txt"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add feature file"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    // Push should succeed by auto-stashing the non-conflicting target changes
    snapshot_push(
        "push_dirty_target_autostash",
        &repo,
        &["main"],
        Some(&feature_wt),
    );

    // Ensure the target worktree content is restored
    let notes = std::fs::read_to_string(repo.root_path().join("notes.txt")).unwrap();
    assert_eq!(notes, "temporary notes");

    // Autostash should clean up after itself
    let mut git_cmd = Command::new("git");
    repo.configure_git_cmd(&mut git_cmd);
    let stash_list = git_cmd
        .args(["stash", "list"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&stash_list.stdout)
            .trim()
            .is_empty()
    );
}

#[test]
fn test_push_error_not_fast_forward() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create feature branch from initial commit
    let feature_wt = repo.add_worktree("feature", "feature");

    // Make a commit in the main worktree (repo root) and push it
    std::fs::write(repo.root_path().join("main-file.txt"), "main content").unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "main-file.txt"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Main commit"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["push", "origin", "main"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    // Make a commit in feature (which doesn't have main's commit)
    std::fs::write(feature_wt.join("feature.txt"), "feature content").unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "feature.txt"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Feature commit"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

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
    std::fs::write(feature_wt.join("file1.txt"), "content1").unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "file1.txt"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Commit 1"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    // Create another branch for merging
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["checkout", "-b", "temp"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    std::fs::write(feature_wt.join("file2.txt"), "content2").unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "file2.txt"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Commit 2"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    // Switch back to feature and merge temp (creating merge commit)
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["checkout", "feature"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["merge", "temp", "--no-ff", "-m", "Merge temp"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

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
    std::fs::write(feature_wt.join("file1.txt"), "content1").unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "file1.txt"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Commit 1"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    // Create another branch for merging
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["checkout", "-b", "temp"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    std::fs::write(feature_wt.join("file2.txt"), "content2").unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "file2.txt"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Commit 2"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    // Switch back to feature and merge temp (creating merge commit)
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["checkout", "feature"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["merge", "temp", "--no-ff", "-m", "Merge temp"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

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
    std::fs::write(feature_wt.join("feature.txt"), "feature content").unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "feature.txt"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add feature file"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    // Try to push without specifying target (should fail - no remote to get default branch)
    snapshot_push("push_no_remote", &repo, &[], Some(&feature_wt));
}
