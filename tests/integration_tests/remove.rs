use crate::common::{
    TestRepo, make_snapshot_cmd_with_global_flags, setup_snapshot_settings,
    setup_temp_snapshot_settings, wt_command,
};
use insta_cmd::assert_cmd_snapshot;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

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

/// Test that remove works from a detached HEAD state in a worktree.
///
/// When in detached HEAD, we should still be able to remove the current worktree
/// using path-based removal (no branch deletion).
#[test]
fn test_remove_from_detached_head_in_worktree() {
    let mut repo = setup_remove_repo();

    let worktree_path = repo.add_worktree("feature-detached");

    // Detach HEAD in the worktree
    repo.detach_head_in_worktree("feature-detached");

    // Run remove from within the detached worktree (should still work)
    snapshot_remove(
        "remove_from_detached_head_in_worktree",
        &repo,
        &[],
        Some(&worktree_path),
    );
}

/// Test that `wt remove @` works from a detached HEAD state in a worktree.
///
/// This should behave identically to `wt remove` (no args) - path-based removal
/// without branch deletion. The `@` symbol refers to the current worktree.
#[test]
fn test_remove_at_from_detached_head_in_worktree() {
    let mut repo = setup_remove_repo();

    let worktree_path = repo.add_worktree("feature-detached-at");

    // Detach HEAD in the worktree
    repo.detach_head_in_worktree("feature-detached-at");

    // Run `wt remove @` from within the detached worktree (should behave same as no args)
    snapshot_remove(
        "remove_at_from_detached_head_in_worktree",
        &repo,
        &["@"],
        Some(&worktree_path),
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
/// Test the explicit difference between removing main worktree (error) vs linked worktree (success).
///
/// This test documents the expected behavior:
/// 1. Linked worktrees can be removed (whether from within them or from elsewhere)
/// 2. The main worktree cannot be removed under any circumstances
/// 3. This is true regardless of which branch is checked out in the main worktree
#[test]
fn test_remove_main_worktree_vs_linked_worktree() {
    let mut repo = setup_remove_repo();

    // Create a linked worktree
    let linked_wt_path = repo.add_worktree("feature");

    // Part 1: Verify linked worktree CAN be removed (from within it)
    snapshot_remove(
        "remove_main_vs_linked__from_linked_succeeds",
        &repo,
        &[],
        Some(&linked_wt_path),
    );

    // Part 2: Recreate the linked worktree for the next test
    let _linked_wt_path = repo.add_worktree("feature2");

    // Part 3: Verify linked worktree CAN be removed (from main, by name)
    snapshot_remove(
        "remove_main_vs_linked__from_main_by_name_succeeds",
        &repo,
        &["feature2"],
        None,
    );

    // Part 4: Verify main worktree CANNOT be removed (from main, on default branch)
    snapshot_remove(
        "remove_main_vs_linked__main_on_default_fails",
        &repo,
        &[],
        None,
    );

    // Part 5: Create a feature branch IN the main worktree, verify STILL cannot remove
    let mut cmd = std::process::Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["switch", "-c", "feature-in-main"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    snapshot_remove(
        "remove_main_vs_linked__main_on_feature_fails",
        &repo,
        &[],
        None,
    );
}

/// Test that removing a worktree for the default branch doesn't show tautological reason.
///
/// When removing a worktree for "main" branch, we should NOT show "(ancestor of main)"
/// because that would be tautological. The message should just be "Removed main worktree & branch".
///
/// This requires a bare repo setup since you can't have a linked worktree for the default
/// branch in a normal repo (the main worktree already has it checked out).
#[test]
fn test_remove_default_branch_no_tautology() {
    // Create bare repository
    let temp_dir = TempDir::new().unwrap();
    let bare_repo_path = temp_dir.path().join("repo.git");
    let test_config_path = temp_dir.path().join("test-config.toml");

    let output = Command::new("git")
        .args(["init", "--bare", "--initial-branch", "main"])
        .current_dir(temp_dir.path())
        .arg(&bare_repo_path)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap();
    assert!(output.status.success(), "Failed to init bare repo");

    let bare_repo_path: PathBuf = bare_repo_path.canonicalize().unwrap();

    // Create worktree for main branch
    let main_worktree = temp_dir.path().join("repo.main");
    let output = Command::new("git")
        .args([
            "-C",
            bare_repo_path.to_str().unwrap(),
            "worktree",
            "add",
            "-b",
            "main",
            main_worktree.to_str().unwrap(),
        ])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_DATE", "2025-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2025-01-01T00:00:00Z")
        .output()
        .unwrap();
    assert!(output.status.success(), "Failed to create main worktree");

    let main_worktree = main_worktree.canonicalize().unwrap();

    // Create initial commit in main worktree
    std::fs::write(main_worktree.join("file.txt"), "initial").unwrap();
    let output = Command::new("git")
        .args(["add", "file.txt"])
        .current_dir(&main_worktree)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap();
    assert!(output.status.success());
    let output = Command::new("git")
        .args(["commit", "-m", "Initial commit"])
        .current_dir(&main_worktree)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test User")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_AUTHOR_DATE", "2025-01-01T00:00:00Z")
        .env("GIT_COMMITTER_NAME", "Test User")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_DATE", "2025-01-01T00:00:00Z")
        .output()
        .unwrap();
    assert!(output.status.success());

    // Create a second worktree (feature) so we have somewhere to run remove from
    let feature_worktree = temp_dir.path().join("repo.feature");
    let output = Command::new("git")
        .args([
            "-C",
            bare_repo_path.to_str().unwrap(),
            "worktree",
            "add",
            "-b",
            "feature",
            feature_worktree.to_str().unwrap(),
        ])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_DATE", "2025-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2025-01-01T00:00:00Z")
        .output()
        .unwrap();
    assert!(output.status.success(), "Failed to create feature worktree");

    let feature_worktree = feature_worktree.canonicalize().unwrap();

    // Remove main worktree by name from feature worktree (foreground for snapshot)
    // Should NOT show "(ancestor of main)" - that would be tautological
    let settings = setup_temp_snapshot_settings(temp_dir.path());
    settings.bind(|| {
        let mut cmd = wt_command();
        cmd.args(["remove", "--no-background", "main"])
            .current_dir(&feature_worktree)
            .env("WORKTRUNK_CONFIG_PATH", test_config_path.to_str().unwrap())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_DATE", "2025-01-01T00:00:00Z")
            .env("GIT_COMMITTER_DATE", "2025-01-01T00:00:00Z")
            .env("GIT_EDITOR", "")
            .env("LANG", "C")
            .env("LC_ALL", "C");

        assert_cmd_snapshot!("remove_default_branch_no_tautology", cmd);
    });
}

/// Test that a squash-merged branch is detected as integrated even when main advances.
///
/// This tests the scenario:
/// 1. Create feature branch from main and make changes (file A)
/// 2. Squash-merge feature into main (main now has A via squash commit)
/// 3. Main advances with more commits (file B)
/// 4. Try to remove feature
///
/// The branch should be detected as integrated because its content (A) is
/// already in main, even though main has additional content (B).
///
/// This is detected via merge simulation: `git merge-tree --write-tree main feature`
/// produces the same tree as main, meaning merging feature would add nothing.
#[test]
fn test_remove_squash_merged_then_main_advanced() {
    let repo = setup_remove_repo();

    // Create feature branch
    repo.git_command(&["checkout", "-b", "feature-squash"])
        .output()
        .unwrap();

    // Make changes on feature branch (file A)
    std::fs::write(repo.root_path().join("feature-a.txt"), "feature content").unwrap();
    repo.git_command(&["add", "feature-a.txt"])
        .output()
        .unwrap();
    repo.git_command(&["commit", "-m", "Add feature A"])
        .output()
        .unwrap();

    // Go back to main
    repo.git_command(&["checkout", "main"]).output().unwrap();

    // Squash merge feature into main (simulating GitHub squash merge)
    // This creates a NEW commit on main with the same content changes
    std::fs::write(repo.root_path().join("feature-a.txt"), "feature content").unwrap();
    repo.git_command(&["add", "feature-a.txt"])
        .output()
        .unwrap();
    repo.git_command(&["commit", "-m", "Add feature A (squash merged)"])
        .output()
        .unwrap();

    // Main advances with another commit (file B)
    std::fs::write(repo.root_path().join("main-b.txt"), "main content").unwrap();
    repo.git_command(&["add", "main-b.txt"]).output().unwrap();
    repo.git_command(&["commit", "-m", "Main advances with B"])
        .output()
        .unwrap();

    // Verify setup: feature-squash is NOT an ancestor of main (squash creates different SHAs)
    let is_ancestor = repo
        .git_command(&["merge-base", "--is-ancestor", "feature-squash", "main"])
        .output()
        .unwrap();
    assert!(
        !is_ancestor.status.success(),
        "feature-squash should NOT be an ancestor of main (squash merge)"
    );

    // Verify setup: trees don't match (main has file B that feature doesn't)
    let feature_tree = String::from_utf8(
        repo.git_command(&["rev-parse", "feature-squash^{tree}"])
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
    assert_ne!(
        feature_tree.trim(),
        main_tree.trim(),
        "Tree SHAs should differ (main has file B that feature doesn't)"
    );

    // Remove the feature branch - should succeed because content is integrated
    // (detected via merge simulation using git merge-tree)
    snapshot_remove(
        "remove_squash_merged_then_main_advanced",
        &repo,
        &["feature-squash"],
        None,
    );
}
