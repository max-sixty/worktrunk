use crate::common::{TestRepo, make_snapshot_cmd_with_global_flags, setup_snapshot_settings};
use insta_cmd::assert_cmd_snapshot;
use std::process::Command;

/// Helper to create snapshot with normalized paths
fn snapshot_remove(test_name: &str, repo: &TestRepo, args: &[&str], cwd: Option<&std::path::Path>) {
    snapshot_remove_with_global_flags(test_name, repo, args, cwd, &[]);
}

/// Helper to create snapshot with global flags (e.g., --internal)
fn snapshot_remove_with_global_flags(
    test_name: &str,
    repo: &TestRepo,
    args: &[&str],
    cwd: Option<&std::path::Path>,
    global_flags: &[&str],
) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd_with_global_flags(repo, "remove", args, cwd, global_flags);
        assert_cmd_snapshot!(test_name, cmd);
    });
}

/// Common setup for remove tests - creates repo with initial commit and remote
fn setup_remove_repo() -> TestRepo {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");
    repo
}

#[test]
fn test_remove_already_on_default() {
    let repo = setup_remove_repo();

    // Already on main branch
    snapshot_remove("remove_already_on_default", &repo, &[], None);
}

#[test]
fn test_remove_switch_to_default() {
    let repo = setup_remove_repo();

    // Create and switch to a feature branch in the main repo
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["switch", "-c", "feature"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    snapshot_remove("remove_switch_to_default", &repo, &[], None);
}

#[test]
fn test_remove_from_worktree() {
    let mut repo = setup_remove_repo();

    let worktree_path = repo.add_worktree("feature-wt");

    // Run remove from within the worktree
    snapshot_remove("remove_from_worktree", &repo, &[], Some(&worktree_path));
}

#[test]
fn test_remove_internal_mode() {
    let mut repo = setup_remove_repo();

    let worktree_path = repo.add_worktree("feature-internal");

    snapshot_remove_with_global_flags(
        "remove_internal_mode",
        &repo,
        &[],
        Some(&worktree_path),
        &["--internal"],
    );
}

#[test]
fn test_remove_dirty_working_tree() {
    let repo = setup_remove_repo();

    // Create a dirty file
    std::fs::write(repo.root_path().join("dirty.txt"), "uncommitted changes").unwrap();

    snapshot_remove("remove_dirty_working_tree", &repo, &[], None);
}

#[test]
fn test_remove_by_name_from_main() {
    let mut repo = setup_remove_repo();

    // Create a worktree
    let _worktree_path = repo.add_worktree("feature-a");

    // Remove it by name from main repo
    snapshot_remove("remove_by_name_from_main", &repo, &["feature-a"], None);
}

#[test]
fn test_remove_by_name_from_other_worktree() {
    let mut repo = setup_remove_repo();

    // Create two worktrees
    let worktree_a = repo.add_worktree("feature-a");
    let _worktree_b = repo.add_worktree("feature-b");

    // From worktree A, remove worktree B by name
    snapshot_remove(
        "remove_by_name_from_other_worktree",
        &repo,
        &["feature-b"],
        Some(&worktree_a),
    );
}

#[test]
fn test_remove_current_by_name() {
    let mut repo = setup_remove_repo();

    let worktree_path = repo.add_worktree("feature-current");

    // Remove current worktree by specifying its name
    snapshot_remove(
        "remove_current_by_name",
        &repo,
        &["feature-current"],
        Some(&worktree_path),
    );
}

#[test]
fn test_remove_nonexistent_worktree() {
    let repo = setup_remove_repo();

    // Try to remove a worktree that doesn't exist
    snapshot_remove("remove_nonexistent_worktree", &repo, &["nonexistent"], None);
}

#[test]
fn test_remove_by_name_dirty_target() {
    let mut repo = setup_remove_repo();

    let worktree_path = repo.add_worktree("feature-dirty");

    // Create a dirty file in the target worktree
    std::fs::write(worktree_path.join("dirty.txt"), "uncommitted changes").unwrap();

    // Try to remove it by name from main repo
    snapshot_remove(
        "remove_by_name_dirty_target",
        &repo,
        &["feature-dirty"],
        None,
    );
}

#[test]
fn test_remove_multiple_worktrees() {
    let mut repo = setup_remove_repo();

    // Create three worktrees
    let _worktree_a = repo.add_worktree("feature-a");
    let _worktree_b = repo.add_worktree("feature-b");
    let _worktree_c = repo.add_worktree("feature-c");

    // Remove all three at once from main repo
    snapshot_remove(
        "remove_multiple_worktrees",
        &repo,
        &["feature-a", "feature-b", "feature-c"],
        None,
    );
}

#[test]
fn test_remove_multiple_including_current() {
    let mut repo = setup_remove_repo();

    // Create three worktrees
    let worktree_a = repo.add_worktree("feature-a");
    let _worktree_b = repo.add_worktree("feature-b");
    let _worktree_c = repo.add_worktree("feature-c");

    // From worktree A, remove all three (including current)
    snapshot_remove(
        "remove_multiple_including_current",
        &repo,
        &["feature-a", "feature-b", "feature-c"],
        Some(&worktree_a),
    );
}

#[test]
fn test_remove_branch_not_fully_merged() {
    let mut repo = setup_remove_repo();

    // Create a worktree with an unmerged commit
    let worktree_path = repo.add_worktree("feature-unmerged");

    // Add a commit to the feature branch that's not in main
    std::fs::write(worktree_path.join("feature.txt"), "new feature").unwrap();
    repo.git_command(&["add", "feature.txt"])
        .current_dir(&worktree_path)
        .output()
        .unwrap();
    repo.git_command(&["commit", "-m", "Add feature"])
        .current_dir(&worktree_path)
        .output()
        .unwrap();

    // Try to remove it from the main repo
    // Branch deletion should fail but worktree removal should succeed
    snapshot_remove(
        "remove_branch_not_fully_merged",
        &repo,
        &["feature-unmerged"],
        None,
    );
}

#[test]
fn test_remove_foreground() {
    let mut repo = setup_remove_repo();

    // Create a worktree
    let _worktree_path = repo.add_worktree("feature-fg");

    // Remove it with --no-background flag from main repo
    snapshot_remove(
        "remove_foreground",
        &repo,
        &["--no-background", "feature-fg"],
        None,
    );
}

#[test]
fn test_remove_no_delete_branch() {
    let mut repo = setup_remove_repo();

    // Create a worktree
    let _worktree_path = repo.add_worktree("feature-keep");

    // Remove worktree but keep the branch using --no-delete-branch flag
    snapshot_remove(
        "remove_no_delete_branch",
        &repo,
        &["--no-delete-branch", "feature-keep"],
        None,
    );
}

#[test]
fn test_remove_branch_only_merged() {
    let repo = setup_remove_repo();

    // Create a branch from main without a worktree (already merged)
    repo.git_command(&["branch", "feature-merged"])
        .output()
        .unwrap();

    // Remove the branch (no worktree exists)
    snapshot_remove(
        "remove_branch_only_merged",
        &repo,
        &["feature-merged"],
        None,
    );
}

#[test]
fn test_remove_branch_only_unmerged() {
    let repo = setup_remove_repo();

    // Create a branch with a unique commit (not in main)
    repo.git_command(&["branch", "feature-unmerged"])
        .output()
        .unwrap();

    // Add a commit to the branch that's not in main
    repo.git_command(&["checkout", "feature-unmerged"])
        .output()
        .unwrap();
    std::fs::write(repo.root_path().join("feature.txt"), "new feature").unwrap();
    repo.git_command(&["add", "feature.txt"]).output().unwrap();
    repo.git_command(&["commit", "-m", "Add feature"])
        .output()
        .unwrap();
    repo.git_command(&["checkout", "main"]).output().unwrap();

    // Try to remove the branch (no worktree exists, branch not merged)
    // Branch deletion should fail but not error
    snapshot_remove(
        "remove_branch_only_unmerged",
        &repo,
        &["feature-unmerged"],
        None,
    );
}

#[test]
fn test_remove_branch_only_force_delete() {
    let repo = setup_remove_repo();

    // Create a branch with a unique commit (not in main)
    repo.git_command(&["branch", "feature-force"])
        .output()
        .unwrap();

    // Add a commit to the branch that's not in main
    repo.git_command(&["checkout", "feature-force"])
        .output()
        .unwrap();
    std::fs::write(repo.root_path().join("feature.txt"), "new feature").unwrap();
    repo.git_command(&["add", "feature.txt"]).output().unwrap();
    repo.git_command(&["commit", "-m", "Add feature"])
        .output()
        .unwrap();
    repo.git_command(&["checkout", "main"]).output().unwrap();

    // Force delete the branch (no worktree exists)
    snapshot_remove(
        "remove_branch_only_force_delete",
        &repo,
        &["--force-delete", "feature-force"],
        None,
    );
}

/// Test that a branch with matching tree content (but not an ancestor) is deleted.
///
/// This simulates a squash merge workflow where:
/// - Feature branch has commits ahead of main
/// - Main is updated (e.g., via squash merge on GitHub) with the same content
/// - Branch is NOT an ancestor of main, but tree SHAs match
/// - Branch should be deleted because content is integrated
#[test]
fn test_remove_branch_matching_tree_content() {
    let repo = setup_remove_repo();

    // Create a feature branch from main
    repo.git_command(&["branch", "feature-squashed"])
        .output()
        .unwrap();

    // On feature branch: add a file
    repo.git_command(&["checkout", "feature-squashed"])
        .output()
        .unwrap();
    std::fs::write(repo.root_path().join("feature.txt"), "squash content").unwrap();
    repo.git_command(&["add", "feature.txt"]).output().unwrap();
    repo.git_command(&["commit", "-m", "Add feature (on feature branch)"])
        .output()
        .unwrap();

    // On main: add the same file with same content (simulates squash merge result)
    repo.git_command(&["checkout", "main"]).output().unwrap();
    std::fs::write(repo.root_path().join("feature.txt"), "squash content").unwrap();
    repo.git_command(&["add", "feature.txt"]).output().unwrap();
    repo.git_command(&["commit", "-m", "Add feature (squash merged)"])
        .output()
        .unwrap();

    // Verify the setup: feature-squashed is NOT an ancestor of main (different commits)
    let is_ancestor = repo
        .git_command(&["merge-base", "--is-ancestor", "feature-squashed", "main"])
        .output()
        .unwrap();
    assert!(
        !is_ancestor.status.success(),
        "feature-squashed should NOT be an ancestor of main"
    );

    // Verify: tree SHAs should match
    let feature_tree = String::from_utf8(
        repo.git_command(&["rev-parse", "feature-squashed^{tree}"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let main_tree = String::from_utf8(
        repo.git_command(&["rev-parse", "main^{tree}"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(
        feature_tree.trim(),
        main_tree.trim(),
        "Tree SHAs should match (same content)"
    );

    // Remove the branch - should succeed because tree content matches main
    snapshot_remove(
        "remove_branch_matching_tree_content",
        &repo,
        &["feature-squashed"],
        None,
    );
}
