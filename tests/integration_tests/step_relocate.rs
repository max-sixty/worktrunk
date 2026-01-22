//! Integration tests for `wt step relocate`

use crate::common::{TestRepo, make_snapshot_cmd, repo};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::fs;

/// Get the parent directory of the repo (where worktrees are created)
fn worktree_parent(repo: &TestRepo) -> std::path::PathBuf {
    repo.root_path().parent().unwrap().to_path_buf()
}

/// Test with no mismatched worktrees
#[rstest]
fn test_relocate_no_mismatches(mut repo: TestRepo) {
    // Create a worktree at the expected location
    repo.add_worktree("feature");

    // All worktrees should be at expected paths
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "step", &["relocate"], None));
}

/// Test relocating a single mismatched worktree
#[rstest]
fn test_relocate_single_mismatch(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // Create a worktree manually at a non-standard location
    let wrong_path = parent.join("wrong-location");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "feature",
        wrong_path.to_str().unwrap(),
    ]);

    // Relocate should move it to the expected path
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "step", &["relocate"], None));

    // Verify the worktree was moved to expected location
    let expected_path = parent.join("repo.feature");
    assert!(
        expected_path.exists(),
        "Worktree should be at expected path: {}",
        expected_path.display()
    );
    assert!(
        !wrong_path.exists(),
        "Old worktree path should no longer exist: {}",
        wrong_path.display()
    );
}

/// Test dry run shows what would be moved
#[rstest]
fn test_relocate_dry_run(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // Create a worktree at a non-standard location
    let wrong_path = parent.join("wrong-location");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "feature",
        wrong_path.to_str().unwrap(),
    ]);

    // Dry run should show what would be moved without actually moving
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["relocate", "--dry-run"],
        None
    ));

    // Verify the worktree was NOT moved
    assert!(
        wrong_path.exists(),
        "Worktree should still be at wrong path in dry run: {}",
        wrong_path.display()
    );
}

/// Test that locked worktrees are skipped
#[rstest]
fn test_relocate_locked_worktree(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // Create a worktree at a non-standard location and lock it
    let wrong_path = parent.join("wrong-location");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "feature",
        wrong_path.to_str().unwrap(),
    ]);
    repo.run_git(&["worktree", "lock", wrong_path.to_str().unwrap()]);

    // Relocate should skip locked worktree
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "step", &["relocate"], None));

    // Verify the worktree was NOT moved
    assert!(
        wrong_path.exists(),
        "Locked worktree should not be moved: {}",
        wrong_path.display()
    );
}

/// Test mixed success and skip (covers "Relocated X, skipped Y" output)
#[rstest]
fn test_relocate_mixed_success_and_skip(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // Create one worktree that can be moved
    let wrong_path1 = parent.join("wrong-location-1");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "feature1",
        wrong_path1.to_str().unwrap(),
    ]);

    // Create another worktree that is locked (will be skipped)
    let wrong_path2 = parent.join("wrong-location-2");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "feature2",
        wrong_path2.to_str().unwrap(),
    ]);
    repo.run_git(&["worktree", "lock", wrong_path2.to_str().unwrap()]);

    // Relocate should move feature1 and skip feature2
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "step", &["relocate"], None));

    // Verify feature1 was moved
    let expected_path1 = parent.join("repo.feature1");
    assert!(
        expected_path1.exists(),
        "feature1 should be at expected path: {}",
        expected_path1.display()
    );

    // Verify feature2 was NOT moved (locked)
    assert!(
        wrong_path2.exists(),
        "Locked feature2 should not be moved: {}",
        wrong_path2.display()
    );
}

/// Test that existing target path causes skip
#[rstest]
fn test_relocate_target_exists(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // Create a worktree at a non-standard location
    let wrong_path = parent.join("wrong-location");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "feature",
        wrong_path.to_str().unwrap(),
    ]);

    // Create a directory at the expected location
    let expected_path = parent.join("repo.feature");
    fs::create_dir_all(&expected_path).unwrap();
    fs::write(expected_path.join("existing-file.txt"), "existing").unwrap();

    // Relocate should skip because target exists
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "step", &["relocate"], None));

    // Verify the worktree was NOT moved
    assert!(
        wrong_path.exists(),
        "Worktree should not be moved when target exists: {}",
        wrong_path.display()
    );
}

/// Test that dirty worktrees are skipped without --commit
#[rstest]
fn test_relocate_dirty_without_commit(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // Create a worktree at a non-standard location
    let wrong_path = parent.join("wrong-location");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "feature",
        wrong_path.to_str().unwrap(),
    ]);

    // Make uncommitted changes
    fs::write(wrong_path.join("dirty.txt"), "uncommitted changes").unwrap();

    // Relocate should skip dirty worktree
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "step", &["relocate"], None));

    // Verify the worktree was NOT moved
    assert!(
        wrong_path.exists(),
        "Dirty worktree should not be moved: {}",
        wrong_path.display()
    );
}

/// Test that --commit auto-commits dirty worktrees before relocating
#[rstest]
fn test_relocate_dirty_with_commit(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // Create a worktree at a non-standard location
    let wrong_path = parent.join("wrong-location");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "feature",
        wrong_path.to_str().unwrap(),
    ]);

    // Make uncommitted changes
    fs::write(wrong_path.join("dirty.txt"), "uncommitted changes").unwrap();

    // Configure mock LLM command via config file
    let worktrunk_config = r#"
[commit-generation]
command = "sh"
args = ["-c", "cat >/dev/null && echo 'chore: auto-commit before relocate'"]
"#;
    fs::write(repo.test_config_path(), worktrunk_config).unwrap();

    // Relocate with --commit should commit then move
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["relocate", "--commit"],
        None
    ));

    // Verify the worktree was moved to expected location
    let expected_path = parent.join("repo.feature");
    assert!(
        expected_path.exists(),
        "Worktree should be at expected path after commit: {}",
        expected_path.display()
    );
    assert!(
        !wrong_path.exists(),
        "Old worktree path should no longer exist: {}",
        wrong_path.display()
    );
}

/// Test that --clobber backs up non-worktree paths at target locations
#[rstest]
fn test_relocate_clobber_backs_up(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // Create a worktree at a non-standard location
    let wrong_path = parent.join("wrong-location");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "feature",
        wrong_path.to_str().unwrap(),
    ]);

    // Create a directory at the expected location (non-worktree blocker)
    let expected_path = parent.join("repo.feature");
    fs::create_dir_all(&expected_path).unwrap();
    fs::write(expected_path.join("existing-file.txt"), "existing content").unwrap();

    // Relocate with --clobber should backup and move
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["relocate", "--clobber"],
        None
    ));

    // Verify the worktree was moved
    assert!(
        expected_path.exists(),
        "Worktree should be at expected location: {}",
        expected_path.display()
    );
    assert!(
        !wrong_path.exists(),
        "Original path should no longer exist: {}",
        wrong_path.display()
    );

    // Verify backup exists (with timestamp suffix)
    let backup_exists = fs::read_dir(&parent)
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.starts_with("repo.feature.bak-")
        });
    assert!(backup_exists, "Backup directory should exist");
}

/// Test that --clobber refuses to clobber an existing worktree
#[rstest]
fn test_relocate_clobber_refuses_worktree(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // Create worktree alpha at a non-standard location
    let wrong_path = parent.join("wrong-location");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "alpha",
        wrong_path.to_str().unwrap(),
    ]);

    // Create another worktree beta at alpha's expected location
    let expected_path = parent.join("repo.alpha");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "beta",
        expected_path.to_str().unwrap(),
    ]);

    // Relocate with --clobber should still skip (can't clobber a worktree)
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["relocate", "--clobber", "alpha"],
        None
    ));

    // Verify alpha was NOT moved (beta still occupies the target)
    assert!(
        wrong_path.exists(),
        "alpha should still be at wrong location: {}",
        wrong_path.display()
    );
}

/// Test relocating specific worktrees by branch name
#[rstest]
fn test_relocate_specific_branch(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // Create two worktrees at non-standard locations
    let wrong_path1 = parent.join("wrong-location-1");
    let wrong_path2 = parent.join("wrong-location-2");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "feature1",
        wrong_path1.to_str().unwrap(),
    ]);
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "feature2",
        wrong_path2.to_str().unwrap(),
    ]);

    // Relocate only feature1
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["relocate", "feature1"],
        None
    ));

    // Verify only feature1 was moved
    let expected_path1 = parent.join("repo.feature1");
    assert!(
        expected_path1.exists(),
        "feature1 should be at expected path: {}",
        expected_path1.display()
    );
    assert!(
        wrong_path2.exists(),
        "feature2 should still be at wrong path: {}",
        wrong_path2.display()
    );
}

/// Test relocating main worktree with non-default branch (create + switch)
#[rstest]
fn test_relocate_main_worktree(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // Switch main worktree to a feature branch
    repo.run_git(&["checkout", "-b", "feature"]);

    // Relocate should create worktree for feature and switch main to default branch
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "step", &["relocate"], None));

    // Verify new worktree was created
    let expected_path = parent.join("repo.feature");
    assert!(
        expected_path.exists(),
        "Feature worktree should be created at: {}",
        expected_path.display()
    );

    // Verify main worktree is now on default branch
    let output = repo
        .git_command()
        .args(["branch", "--show-current"])
        .output()
        .unwrap();
    let current_branch = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        current_branch.trim(),
        "main",
        "Main worktree should be on default branch"
    );
}

/// Test swap scenario: two worktrees at each other's expected locations
///
/// When alpha is at repo.beta and beta is at repo.alpha, relocate
/// automatically handles the swap via a temporary location.
#[rstest]
fn test_relocate_swap(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // Create worktrees at each other's expected locations
    // alpha at repo.beta (where beta should go)
    // beta at repo.alpha (where alpha should go)
    let path_for_beta = parent.join("repo.beta");
    let path_for_alpha = parent.join("repo.alpha");

    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "alpha",
        path_for_beta.to_str().unwrap(), // alpha at beta's expected location
    ]);
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "beta",
        path_for_alpha.to_str().unwrap(), // beta at alpha's expected location
    ]);

    // Relocate resolves the swap via temp location
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "step", &["relocate"], None));

    // Verify both are now at their expected locations
    assert!(path_for_alpha.exists(), "alpha should be at repo.alpha");
    assert!(path_for_beta.exists(), "beta should be at repo.beta");
}

/// Test relocating multiple worktrees shows compact output
#[rstest]
fn test_relocate_multiple(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // Create 5 worktrees at non-standard locations
    for i in 1..=5 {
        let wrong_path = parent.join(format!("wrong-{i}"));
        repo.run_git(&[
            "worktree",
            "add",
            "-b",
            &format!("feature-{i}"),
            wrong_path.to_str().unwrap(),
        ]);
    }

    // Relocate all
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "step", &["relocate"], None));

    // Verify all were moved
    for i in 1..=5 {
        let expected_path = parent.join(format!("repo.feature-{i}"));
        assert!(
            expected_path.exists(),
            "feature-{i} should be at expected path: {}",
            expected_path.display()
        );
    }
}
