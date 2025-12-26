use crate::common::{
    TestRepo, make_snapshot_cmd, repo, repo_with_feature_worktree, setup_snapshot_settings,
};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
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

#[rstest]
fn test_push_fast_forward(mut repo: TestRepo) {
    // Create a worktree for main
    repo.add_main_worktree();

    // Make a commit in a feature worktree
    let feature_wt =
        repo.add_worktree_with_commit("feature", "test.txt", "test content", "Add test file");

    // Push from feature to main
    snapshot_push("push_fast_forward", &repo, &["main"], Some(&feature_wt));
}

#[rstest]
fn test_push_not_fast_forward(mut repo: TestRepo) {
    // Create commits in both worktrees
    // Note: We use commit_in_worktree on root to match the original file layout
    // (file named main.txt instead of file.txt that repo.commit() creates)
    repo.commit_in_worktree(
        repo.root_path(),
        "main.txt",
        "main content",
        "Add main file",
    );

    // Create a feature worktree branching from before the main commit
    let feature_wt = repo.add_feature();

    // Try to push from feature to main (should fail - not fast-forward)
    snapshot_push("push_not_fast_forward", &repo, &["main"], Some(&feature_wt));
}

#[rstest]
fn test_push_to_default_branch(#[from(repo_with_feature_worktree)] repo: TestRepo) {
    let feature_wt = repo.worktree_path("feature");

    // Push without specifying target (should use default branch)
    snapshot_push("push_to_default", &repo, &[], Some(feature_wt));
}

#[rstest]
fn test_push_with_dirty_target(mut repo: TestRepo) {
    // Make main worktree (repo root) dirty with a conflicting file
    std::fs::write(repo.root_path().join("conflict.txt"), "old content").unwrap();

    let feature_wt = repo.add_worktree_with_commit(
        "feature",
        "conflict.txt",
        "new content",
        "Add conflict file",
    );

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

#[rstest]
fn test_push_dirty_target_autostash(mut repo: TestRepo) {
    // Make main worktree (repo root) dirty with a non-conflicting file
    std::fs::write(repo.root_path().join("notes.txt"), "temporary notes").unwrap();

    let feature_wt = repo.add_feature();

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

#[rstest]
fn test_push_dirty_target_overlap_renamed_file(mut repo: TestRepo) {
    // Regression test: overlap detection must detect conflicts when a file is renamed
    // in the source branch but has uncommitted changes under the old name in the target.
    //
    // Setup:
    // 1. main has file.txt (committed)
    // 2. main (target) has uncommitted modifications to file.txt
    // 3. feature renames file.txt -> renamed.txt (committed)
    // 4. Push from feature to main should FAIL (conflict on the same file)

    // Create initial file in main
    repo.commit_in_worktree(
        repo.root_path(),
        "file.txt",
        "original content",
        "Initial file",
    );

    // Create feature branch from main
    let feature_wt = repo.add_worktree("feature");

    // Make uncommitted changes to file.txt in main (target worktree)
    std::fs::write(repo.root_path().join("file.txt"), "modified in target").unwrap();

    // In feature worktree, rename file.txt to renamed.txt and commit
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["mv", "file.txt", "renamed.txt"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Rename file.txt to renamed.txt"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    // Try to push from feature to main (should fail due to conflicting changes)
    // The renamed file.txt (now renamed.txt) conflicts with uncommitted file.txt changes
    snapshot_push(
        "push_dirty_target_overlap_renamed_file",
        &repo,
        &["main"],
        Some(&feature_wt),
    );

    // Ensure target worktree still has the modified file.txt and no stash was created
    let main_contents = std::fs::read_to_string(repo.root_path().join("file.txt")).unwrap();
    assert_eq!(main_contents, "modified in target");

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

#[rstest]
fn test_push_error_not_fast_forward(mut repo: TestRepo) {
    // Create feature branch from initial commit
    let feature_wt = repo.add_worktree("feature");

    // Make a commit in the main worktree (repo root) and push it
    // Note: Must match original file layout for snapshot consistency
    repo.commit_in_worktree(
        repo.root_path(),
        "main-file.txt",
        "main content",
        "Main commit",
    );
    repo.push_branch("main");

    // Make a commit in feature (which doesn't have main's commit)
    repo.commit_in_worktree(
        &feature_wt,
        "feature.txt",
        "feature content",
        "Feature commit",
    );

    // Try to push feature to main (should fail - main has commits not in feature)
    snapshot_push(
        "push_error_not_fast_forward",
        &repo,
        &["main"],
        Some(&feature_wt),
    );
}

#[rstest]
fn test_push_with_merge_commits(mut repo: TestRepo) {
    // Create feature branch with initial commit
    let feature_wt = repo.add_worktree_with_commit("feature", "file1.txt", "content1", "Commit 1");

    // Create another branch for merging
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["checkout", "-b", "temp"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    repo.commit_in_worktree(&feature_wt, "file2.txt", "content2", "Commit 2");

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

    // Push to main (should succeed - merge commits are allowed)
    snapshot_push(
        "push_with_merge_commits",
        &repo,
        &["main"],
        Some(&feature_wt),
    );
}

#[rstest]
fn test_push_no_remote(#[from(repo_with_feature_worktree)] repo: TestRepo) {
    // Note: repo_with_feature_worktree doesn't call setup_remote(), so this tests the "no remote" error case
    let feature_wt = repo.worktree_path("feature");

    // Try to push without specifying target (should fail - no remote to get default branch)
    snapshot_push("push_no_remote", &repo, &[], Some(feature_wt));
}
