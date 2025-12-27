use crate::common::{
    TestRepo, canonicalize, configure_directive_file, directive_file, make_snapshot_cmd, repo,
    repo_with_remote, setup_snapshot_settings, setup_temp_snapshot_settings, wt_command,
};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use tempfile::TempDir;

/// Helper to create snapshot with normalized paths
fn snapshot_remove(test_name: &str, repo: &TestRepo, args: &[&str], cwd: Option<&std::path::Path>) {
    snapshot_remove_impl(test_name, repo, args, cwd, false);
}

/// Helper to create snapshot with directive file (simulates shell wrapper)
fn snapshot_remove_with_directive_file(
    test_name: &str,
    repo: &TestRepo,
    args: &[&str],
    cwd: Option<&std::path::Path>,
) {
    snapshot_remove_impl(test_name, repo, args, cwd, true);
}

fn snapshot_remove_impl(
    test_name: &str,
    repo: &TestRepo,
    args: &[&str],
    cwd: Option<&std::path::Path>,
    with_directive_file: bool,
) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        // Directive file guard - declared at closure scope to live through command execution
        let maybe_directive = if with_directive_file {
            Some(directive_file())
        } else {
            None
        };

        let mut cmd = make_snapshot_cmd(repo, "remove", args, cwd);
        if let Some((ref directive_path, ref _guard)) = maybe_directive {
            configure_directive_file(&mut cmd, directive_path);
        }
        assert_cmd_snapshot!(test_name, cmd);
    });
}

#[rstest]
fn test_remove_already_on_default(repo: TestRepo) {
    // Already on main branch
    snapshot_remove("remove_already_on_default", &repo, &[], None);
}

#[rstest]
fn test_remove_switch_to_default(repo: TestRepo) {
    // Create and switch to a feature branch in the main repo
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["switch", "-c", "feature"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    snapshot_remove("remove_switch_to_default", &repo, &[], None);
}

#[rstest]
fn test_remove_from_worktree(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature-wt");

    // Run remove from within the worktree
    snapshot_remove("remove_from_worktree", &repo, &[], Some(&worktree_path));
}

#[rstest]
fn test_remove_internal_mode(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature-internal");

    snapshot_remove_with_directive_file("remove_internal_mode", &repo, &[], Some(&worktree_path));
}

#[rstest]
fn test_remove_dirty_working_tree(repo: TestRepo) {
    // Create a dirty file
    std::fs::write(repo.root_path().join("dirty.txt"), "uncommitted changes").unwrap();

    snapshot_remove("remove_dirty_working_tree", &repo, &[], None);
}

#[rstest]
fn test_remove_by_name_from_main(mut repo: TestRepo) {
    // Create a worktree
    let _worktree_path = repo.add_worktree("feature-a");

    // Remove it by name from main repo
    snapshot_remove("remove_by_name_from_main", &repo, &["feature-a"], None);
}

#[rstest]
fn test_remove_by_name_from_other_worktree(mut repo: TestRepo) {
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

#[rstest]
fn test_remove_current_by_name(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature-current");

    // Remove current worktree by specifying its name
    snapshot_remove(
        "remove_current_by_name",
        &repo,
        &["feature-current"],
        Some(&worktree_path),
    );
}

#[rstest]
fn test_remove_nonexistent_worktree(repo: TestRepo) {
    // Try to remove a worktree that doesn't exist
    snapshot_remove("remove_nonexistent_worktree", &repo, &["nonexistent"], None);
}

/// Test removing a branch that exists but has no worktree, when expected path is occupied.
///
/// Regression test for bug where `wt remove npm` would show "Cannot create worktree for npm"
/// when the expected path was occupied. The fix uses `ResolutionContext::Remove` which skips
/// the path occupation check entirely, correctly treating this as a branch-only removal.
///
/// Setup:
/// - Branch `npm` exists but has no worktree
/// - The expected path for `npm` (repo.npm) is occupied by a different branch's worktree
///
/// Expected behavior:
/// - Warning: "No worktree found for branch npm"
/// - Success: Branch deleted (same commit as main)
#[rstest]
fn test_remove_branch_no_worktree_path_occupied(mut repo: TestRepo) {
    // Create branch `npm` without a worktree
    repo.git_command(&["branch", "npm"]).output().unwrap();

    // Create a worktree for a different branch at the path where `npm` worktree would be
    // (the path template puts worktrees at ../repo.branch, so ../repo.npm would be npm's path)
    let _other_worktree = repo.add_worktree("other");

    // Manually move the worktree to occupy npm's expected path
    // First, get the expected path for npm
    let npm_expected_path = repo.root_path().parent().unwrap().join(format!(
        "{}.npm",
        repo.root_path().file_name().unwrap().to_str().unwrap()
    ));
    let other_path = repo.root_path().parent().unwrap().join(format!(
        "{}.other",
        repo.root_path().file_name().unwrap().to_str().unwrap()
    ));

    // Remove the worktree metadata and move the directory
    repo.git_command(&[
        "worktree",
        "remove",
        "--force",
        other_path.to_str().unwrap(),
    ])
    .output()
    .unwrap();

    // Create worktree at npm's expected path but for the "other" branch
    repo.git_command(&[
        "worktree",
        "add",
        npm_expected_path.to_str().unwrap(),
        "other",
    ])
    .output()
    .unwrap();

    // Now: branch `npm` exists, no worktree for it, but npm's expected path has `other` branch
    // Running `wt remove npm` should show "No worktree found" NOT "Cannot create worktree"
    snapshot_remove(
        "remove_branch_no_worktree_path_occupied",
        &repo,
        &["npm"],
        None,
    );
}

#[rstest]
fn test_remove_multiple_nonexistent_force(repo: TestRepo) {
    // Try to force-remove multiple branches that don't exist
    snapshot_remove(
        "remove_multiple_nonexistent_force",
        &repo,
        &["-D", "foo", "bar", "baz"],
        None,
    );
}

#[rstest]
fn test_remove_remote_only_branch(#[from(repo_with_remote)] repo: TestRepo) {
    // Create a remote-only branch by pushing a branch then deleting it locally
    Command::new("git")
        .args(["branch", "remote-feature"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    Command::new("git")
        .args(["push", "origin", "remote-feature"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    Command::new("git")
        .args(["branch", "-D", "remote-feature"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    Command::new("git")
        .args(["fetch", "origin"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    // Try to remove a branch that only exists on remote - should get helpful error
    snapshot_remove(
        "remove_remote_only_branch",
        &repo,
        &["remote-feature"],
        None,
    );
}

#[rstest]
fn test_remove_nonexistent_branch(repo: TestRepo) {
    // Try to remove a branch that doesn't exist at all
    snapshot_remove("remove_nonexistent_branch", &repo, &["nonexistent"], None);
}

#[rstest]
fn test_remove_partial_success(mut repo: TestRepo) {
    // Create one valid worktree
    let _feature_path = repo.add_worktree("feature");

    // Try to remove both the valid worktree and a nonexistent one
    // The valid one should be removed; error for nonexistent; exit with failure
    snapshot_remove(
        "remove_partial_success",
        &repo,
        &["feature", "nonexistent"],
        None,
    );

    // Verify the valid worktree was actually removed
    let worktrees_dir = repo.root_path().parent().unwrap();
    assert!(
        !worktrees_dir.join("feature").exists(),
        "feature worktree should have been removed despite partial failure"
    );
}

#[rstest]
fn test_remove_by_name_dirty_target(mut repo: TestRepo) {
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

#[rstest]
fn test_remove_multiple_worktrees(mut repo: TestRepo) {
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

#[rstest]
fn test_remove_multiple_including_current(mut repo: TestRepo) {
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

#[rstest]
fn test_remove_branch_not_fully_merged(mut repo: TestRepo) {
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

#[rstest]
fn test_remove_foreground(mut repo: TestRepo) {
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

#[rstest]
fn test_remove_foreground_unmerged(mut repo: TestRepo) {
    // Create a worktree with an unmerged commit
    let worktree_path = repo.add_worktree("feature-unmerged-fg");

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

    // Remove it with --no-background flag from main repo
    // Branch deletion should fail but worktree removal should succeed
    snapshot_remove(
        "remove_foreground_unmerged",
        &repo,
        &["--no-background", "feature-unmerged-fg"],
        None,
    );
}

/// Tests foreground removal with --no-delete-branch on an integrated branch.
/// The hint should show "Branch integrated (reason); retained with --no-delete-branch"
#[rstest]
fn test_remove_foreground_no_delete_branch(mut repo: TestRepo) {
    // Create a worktree (integrated - same commit as main)
    let _worktree_path = repo.add_worktree("feature-fg-keep");

    // Remove with both --no-background and --no-delete-branch
    snapshot_remove(
        "remove_foreground_no_delete_branch",
        &repo,
        &["--no-background", "--no-delete-branch", "feature-fg-keep"],
        None,
    );
}

/// Tests foreground removal with --no-delete-branch on an unmerged branch.
/// No hint needed since the flag had no effect (branch wouldn't be deleted anyway).
#[rstest]
fn test_remove_foreground_no_delete_branch_unmerged(mut repo: TestRepo) {
    // Create a worktree with an unmerged commit
    let worktree_path = repo.add_worktree("feature-fg-unmerged-keep");

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

    // Go back to main
    repo.git_command(&["checkout", "main"]).output().unwrap();

    // Remove with both --no-background and --no-delete-branch
    // No hint because:
    // - Branch is unmerged (wouldn't be deleted anyway)
    // - --no-delete-branch had no effect
    snapshot_remove(
        "remove_foreground_no_delete_branch_unmerged",
        &repo,
        &[
            "--no-background",
            "--no-delete-branch",
            "feature-fg-unmerged-keep",
        ],
        None,
    );
}

#[rstest]
fn test_remove_no_delete_branch(mut repo: TestRepo) {
    // Create a worktree (integrated - same commit as main)
    let _worktree_path = repo.add_worktree("feature-keep");

    // Remove worktree but keep the branch using --no-delete-branch flag
    // Since branch is integrated, the flag has an effect - hint explains this
    snapshot_remove(
        "remove_no_delete_branch",
        &repo,
        &["--no-delete-branch", "feature-keep"],
        None,
    );
}

#[rstest]
fn test_remove_no_delete_branch_unmerged(mut repo: TestRepo) {
    // Create a worktree with an unmerged commit
    let worktree_path = repo.add_worktree("feature-unmerged-keep");

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

    // Go back to main before removing
    repo.git_command(&["checkout", "main"]).output().unwrap();

    // Remove worktree with --no-delete-branch flag
    // Since branch is unmerged, the flag has no effect - no hint shown
    snapshot_remove(
        "remove_no_delete_branch_unmerged",
        &repo,
        &["--no-delete-branch", "feature-unmerged-keep"],
        None,
    );
}

#[rstest]
fn test_remove_branch_only_merged(repo: TestRepo) {
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

#[rstest]
fn test_remove_branch_only_unmerged(repo: TestRepo) {
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

#[rstest]
fn test_remove_branch_only_force_delete(repo: TestRepo) {
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
#[rstest]
fn test_remove_from_detached_head_in_worktree(mut repo: TestRepo) {
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

/// Test foreground removal from a detached HEAD state.
///
/// Covers the foreground detached HEAD code path in handlers.rs.
/// The output should be "✓ Removed worktree (detached HEAD, no branch to delete)".
///
/// Ignored on Windows: subprocess tests stay in the worktree, causing file locking errors.
#[rstest]
#[cfg_attr(windows, ignore)]
fn test_remove_foreground_detached_head(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature-detached-fg");

    // Detach HEAD in the worktree
    repo.detach_head_in_worktree("feature-detached-fg");

    // Run foreground remove from within the detached worktree
    snapshot_remove(
        "remove_foreground_detached_head",
        &repo,
        &["--no-background"],
        Some(&worktree_path),
    );
}

/// Test that `wt remove @` works from a detached HEAD state in a worktree.
///
/// This should behave identically to `wt remove` (no args) - path-based removal
/// without branch deletion. The `@` symbol refers to the current worktree.
#[rstest]
fn test_remove_at_from_detached_head_in_worktree(mut repo: TestRepo) {
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
#[rstest]
fn test_remove_branch_matching_tree_content(repo: TestRepo) {
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
///
/// Skipped on Windows: Tests run as subprocesses which can't change directory via shell
/// integration. Real users are fine - shell integration cds to main before removing.
/// But subprocess tests stay in the worktree, causing Windows file locking errors.
#[rstest]
#[cfg_attr(windows, ignore)]
fn test_remove_main_worktree_vs_linked_worktree(mut repo: TestRepo) {
    // Create a linked worktree
    let linked_wt_path = repo.add_worktree("feature");

    // Part 1: Verify linked worktree CAN be removed (from within it)
    // Use --no-background to ensure removal completes before creating next worktree
    snapshot_remove(
        "remove_main_vs_linked__from_linked_succeeds",
        &repo,
        &["--no-background"],
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

    let bare_repo_path: PathBuf = canonicalize(&bare_repo_path).unwrap();

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

    let main_worktree = canonicalize(&main_worktree).unwrap();

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

    let feature_worktree = canonicalize(&feature_worktree).unwrap();

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
#[rstest]
fn test_remove_squash_merged_then_main_advanced(repo: TestRepo) {
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

// ============================================================================
// Pre-Remove Hook Tests
// ============================================================================

/// Test pre-remove hook executes before worktree removal.
#[rstest]
fn test_pre_remove_hook_executes(mut repo: TestRepo) {
    // Create project config with pre-remove hook
    repo.write_project_config(r#"pre-remove = "echo 'About to remove worktree'""#);
    repo.commit("Add config");

    // Pre-approve the command
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."repo"]
approved-commands = ["echo 'About to remove worktree'"]
"#,
    );

    // Create a worktree to remove
    let _worktree_path = repo.add_worktree("feature-hook");

    // Remove with --no-background to ensure synchronous execution
    snapshot_remove(
        "pre_remove_hook_executes",
        &repo,
        &["--no-background", "feature-hook"],
        None,
    );
}

/// Test pre-remove hook has access to template variables.
#[rstest]
fn test_pre_remove_hook_template_variables(mut repo: TestRepo) {
    // Create project config with template variables
    repo.write_project_config(
        r#"[pre-remove]
branch = "echo 'Branch: {{ branch }}'"
worktree = "echo 'Worktree: {{ worktree }}'"
worktree_name = "echo 'Name: {{ worktree_name }}'"
"#,
    );
    repo.commit("Add config with templates");

    // Pre-approve the commands (templates match what's shown in prompts)
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."repo"]
approved-commands = [
    "echo 'Branch: {{ branch }}'",
    "echo 'Worktree: {{ worktree }}'",
    "echo 'Name: {{ worktree_name }}'",
]
"#,
    );

    // Create a worktree to remove
    let _worktree_path = repo.add_worktree("feature-templates");

    // Remove with --no-background
    snapshot_remove(
        "pre_remove_hook_template_variables",
        &repo,
        &["--no-background", "feature-templates"],
        None,
    );
}

/// Test pre-remove hook runs even in background mode (before spawning background process).
#[rstest]
fn test_pre_remove_hook_runs_in_background_mode(mut repo: TestRepo) {
    use crate::common::wait_for_file;

    // Create a marker file that the hook will create
    let marker_file = repo.root_path().join("hook-ran.txt");

    // Create project config with hook that creates a file
    repo.write_project_config(&format!(
        r#"pre-remove = "echo 'hook ran' > {}""#,
        marker_file.to_string_lossy().replace('\\', "/")
    ));
    repo.commit("Add config");

    // Pre-approve the command
    repo.write_test_config(&format!(
        r#"worktree-path = "../{{{{ main_worktree }}}}.{{{{ branch }}}}"

[projects."repo"]
approved-commands = ["echo 'hook ran' > {}"]
"#,
        marker_file.to_string_lossy().replace('\\', "/")
    ));

    // Create a worktree to remove
    let _worktree_path = repo.add_worktree("feature-bg");

    // Remove in background mode (default)
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_wt"));
    repo.clean_cli_env(&mut cmd);
    cmd.current_dir(repo.root_path())
        .args(["remove", "feature-bg"])
        .output()
        .unwrap();

    // Wait for the hook to create the marker file
    wait_for_file(&marker_file, Duration::from_secs(5));

    // Marker file SHOULD exist - pre-remove hooks run before background removal starts
    assert!(
        marker_file.exists(),
        "Pre-remove hook should run even in background mode"
    );
}

/// Test pre-remove hook failure aborts removal (FailFast strategy).
#[rstest]
fn test_pre_remove_hook_failure_aborts(mut repo: TestRepo) {
    // Create project config with failing hook
    repo.write_project_config(r#"pre-remove = "exit 1""#);
    repo.commit("Add config");

    // Pre-approve the command
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."repo"]
approved-commands = ["exit 1"]
"#,
    );

    // Create a worktree to remove
    let worktree_path = repo.add_worktree("feature-fail");

    // Remove - should FAIL due to hook failure
    snapshot_remove(
        "pre_remove_hook_failure_aborts",
        &repo,
        &["--no-background", "feature-fail"],
        None,
    );

    // Verify worktree was NOT removed (hook failure aborted removal)
    assert!(
        worktree_path.exists(),
        "Worktree should NOT be removed when hook fails"
    );
}

/// Test pre-remove hook does NOT run for branch-only removal (no worktree).
#[rstest]
fn test_pre_remove_hook_not_for_branch_only(repo: TestRepo) {
    // Create a marker file that the hook would create
    let marker_file = repo.root_path().join("branch-only-hook.txt");

    // Create project config with hook
    repo.write_project_config(&format!(
        r#"pre-remove = "echo 'hook ran' > {}""#,
        marker_file.to_string_lossy().replace('\\', "/")
    ));
    repo.commit("Add config");

    // Pre-approve the command
    repo.write_test_config(&format!(
        r#"worktree-path = "../{{{{ main_worktree }}}}.{{{{ branch }}}}"

[projects."repo"]
approved-commands = ["echo 'hook ran' > {}"]
"#,
        marker_file.to_string_lossy().replace('\\', "/")
    ));

    // Create a branch without a worktree
    repo.git_command(&["branch", "branch-only"])
        .output()
        .unwrap();

    // Remove the branch (no worktree)
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_wt"));
    repo.clean_cli_env(&mut cmd);
    cmd.current_dir(repo.root_path())
        .args(["remove", "branch-only"])
        .output()
        .unwrap();

    // Marker file should NOT exist - pre-remove hooks only run for worktree removal
    assert!(
        !marker_file.exists(),
        "Pre-remove hook should NOT run for branch-only removal"
    );
}

/// Test --no-verify flag skips pre-remove hooks.
#[rstest]
fn test_pre_remove_hook_skipped_with_no_verify(mut repo: TestRepo) {
    use std::thread;

    // Create a marker file that the hook would create
    let marker_file = repo.root_path().join("should-not-exist.txt");

    // Create project config with hook that creates a file
    repo.write_project_config(&format!(
        r#"pre-remove = "echo 'hook ran' > {}""#,
        marker_file.to_string_lossy().replace('\\', "/")
    ));
    repo.commit("Add config");

    // Pre-approve the command (even though it shouldn't run)
    repo.write_test_config(&format!(
        r#"worktree-path = "../{{{{ main_worktree }}}}.{{{{ branch }}}}"

[projects."repo"]
approved-commands = ["echo 'hook ran' > {}"]
"#,
        marker_file.to_string_lossy().replace('\\', "/")
    ));

    // Create a worktree to remove
    let worktree_path = repo.add_worktree("feature-skip");

    // Remove with --no-verify to skip hooks
    snapshot_remove(
        "pre_remove_hook_skipped_with_no_verify",
        &repo,
        &["--no-background", "--no-verify", "feature-skip"],
        None,
    );

    // Wait for any potential hook execution (absence check - can't poll, use 500ms per guidelines)
    thread::sleep(Duration::from_millis(500));

    // Marker file should NOT exist - --no-verify skips the hook
    assert!(
        !marker_file.exists(),
        "Pre-remove hook should NOT run with --no-verify"
    );

    // Worktree should be removed (removal itself succeeds)
    assert!(
        !worktree_path.exists(),
        "Worktree should be removed even with --no-verify"
    );
}

/// Test that pre-remove hook runs for detached HEAD worktrees.
///
/// Even when a worktree is in detached HEAD state (no branch), the pre-remove
/// hook should still execute.
///
/// Skipped on Windows: Tests run as subprocesses which can't change directory via shell
/// integration. Real users are fine - shell integration cds to main before removing.
/// But subprocess tests stay in the worktree, causing Windows file locking errors.
#[rstest]
#[cfg_attr(windows, ignore)]
fn test_pre_remove_hook_runs_for_detached_head(mut repo: TestRepo) {
    // Create marker file path in the repo root
    // Use short filename to avoid terminal line-wrapping differences between platforms
    // (macOS temp paths are ~60 chars vs Linux ~20 chars, affecting wrap points)
    let marker_file = repo.root_path().join("m.txt");
    let marker_path = marker_file.to_string_lossy().replace('\\', "/");

    // Create project config with pre-remove hook that creates a marker file
    repo.write_project_config(&format!(r#"pre-remove = "touch {marker_path}""#,));
    repo.commit("Add config");

    // Pre-approve the command
    repo.write_test_config(&format!(
        r#"worktree-path = "../{{{{ main_worktree }}}}.{{{{ branch }}}}"

[projects."repo"]
approved-commands = ["touch {marker_path}"]
"#,
    ));

    // Create a worktree and detach HEAD
    let worktree_path = repo.add_worktree("feature-detached-hook");
    repo.detach_head_in_worktree("feature-detached-hook");

    // Remove with --no-background to ensure synchronous execution
    snapshot_remove(
        "pre_remove_hook_runs_for_detached_head",
        &repo,
        &["--no-background"],
        Some(&worktree_path),
    );

    // Marker file should exist - hook ran
    assert!(
        marker_file.exists(),
        "Pre-remove hook should run for detached HEAD worktrees"
    );
}

/// Test that pre-remove hook runs for detached HEAD worktrees in background mode.
///
/// This complements `test_pre_remove_hook_runs_for_detached_head` by verifying
/// the hook also runs when removal happens in background (the default).
#[rstest]
fn test_pre_remove_hook_runs_for_detached_head_background(mut repo: TestRepo) {
    // Create marker file path in the repo root
    let marker_file = repo.root_path().join("detached-bg-hook-marker.txt");

    // Create project config with pre-remove hook that creates a marker file
    let marker_path = marker_file.to_string_lossy().replace('\\', "/");
    repo.write_project_config(&format!(r#"pre-remove = "touch {marker_path}""#,));
    repo.commit("Add config");

    // Pre-approve the commands
    repo.write_test_config(&format!(
        r#"worktree-path = "../{{{{ main_worktree }}}}.{{{{ branch }}}}"

[projects."repo"]
approved-commands = ["touch {marker_path}"]
"#,
    ));

    // Create a worktree and detach HEAD
    let worktree_path = repo.add_worktree("feature-detached-bg");
    repo.detach_head_in_worktree("feature-detached-bg");

    // Remove in background mode (default)
    snapshot_remove(
        "pre_remove_hook_runs_for_detached_head_background",
        &repo,
        &[],
        Some(&worktree_path),
    );

    // Marker file should exist - hook ran before background spawn
    assert!(
        marker_file.exists(),
        "Pre-remove hook should run for detached HEAD worktrees in background mode"
    );
}

/// Test that {{ branch }} template variable expands to empty string for detached HEAD.
///
/// This is a non-snapshot test to avoid cross-platform line-wrapping differences
/// (macOS temp paths are ~60 chars vs Linux ~20 chars). The snapshot version
/// of this test (`test_pre_remove_hook_runs_for_detached_head`) verifies the hook runs;
/// this test verifies the specific template expansion behavior.
///
/// Skipped on Windows: Tests run as subprocesses which can't change directory via shell
/// integration. Real users are fine - shell integration cds to main before removing.
/// But subprocess tests stay in the worktree, causing Windows file locking errors.
#[rstest]
#[cfg_attr(windows, ignore)]
fn test_pre_remove_hook_branch_expansion_detached_head(mut repo: TestRepo) {
    // Create a file where the hook will write the branch template expansion
    let branch_file = repo.root_path().join("branch-expansion.txt");
    let branch_path = branch_file.to_string_lossy().replace('\\', "/");

    // Create project config with hook that writes {{ branch }} to file
    repo.write_project_config(&format!(
        r#"pre-remove = "echo 'branch={{{{ branch }}}}' > {branch_path}""#,
    ));
    repo.commit("Add config");

    // Pre-approve the command
    repo.write_test_config(&format!(
        r#"worktree-path = "../{{{{ main_worktree }}}}.{{{{ branch }}}}"

[projects."repo"]
approved-commands = ["echo 'branch={{{{ branch }}}}' > {branch_path}"]
"#,
    ));

    // Create a worktree and detach HEAD
    let worktree_path = repo.add_worktree("feature-branch-test");
    repo.detach_head_in_worktree("feature-branch-test");

    // Run wt remove (not a snapshot test - just verify behavior)
    let output = wt_command()
        .args(["remove", "--no-background"])
        .current_dir(&worktree_path)
        .env("WORKTRUNK_CONFIG_PATH", repo.test_config_path())
        .output()
        .expect("Failed to execute wt remove");

    assert!(
        output.status.success(),
        "wt remove should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify {{ branch }} expanded to "HEAD" (fallback for detached HEAD state)
    let content =
        std::fs::read_to_string(&branch_file).expect("Hook should have created the branch file");
    assert_eq!(
        content.trim(),
        "branch=HEAD",
        "{{ branch }} should expand to 'HEAD' for detached HEAD worktrees"
    );
}
