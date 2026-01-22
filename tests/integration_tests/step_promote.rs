//! Integration tests for `wt step promote`

use crate::common::{TestRepo, make_snapshot_cmd, repo, setup_snapshot_settings, wt_command};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::fs;
use std::process::Command;

/// Helper to get the current branch in a directory
fn get_branch(repo: &TestRepo, dir: &std::path::Path) -> String {
    let output = repo
        .git_command()
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git rev-parse failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Test promoting from another worktree (no argument)
#[rstest]
fn test_promote_from_worktree(mut repo: TestRepo) {
    let _settings_guard = setup_snapshot_settings(&repo).bind_to_scope();
    let feature_path = repo.add_worktree("feature");

    // Run promote from the worktree (no argument needed)
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["promote"],
        Some(&feature_path),
    ));

    // Verify branches were exchanged
    assert_eq!(
        get_branch(&repo, repo.root_path()),
        "feature",
        "main worktree should now have feature"
    );
    assert_eq!(
        get_branch(&repo, &feature_path),
        "main",
        "other worktree should now have main"
    );
}

/// Test promoting by specifying branch name
#[rstest]
fn test_promote_with_branch_argument(mut repo: TestRepo) {
    let _settings_guard = setup_snapshot_settings(&repo).bind_to_scope();
    let feature_path = repo.add_worktree("feature");

    // Run promote from main worktree, specifying the branch
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["promote", "feature"],
        Some(repo.root_path()),
    ));

    // Verify branches were exchanged
    assert_eq!(
        get_branch(&repo, repo.root_path()),
        "feature",
        "main worktree should now have feature"
    );
    assert_eq!(
        get_branch(&repo, &feature_path),
        "main",
        "other worktree should now have main"
    );
}

/// Test restoring canonical state
#[rstest]
fn test_promote_restore(mut repo: TestRepo) {
    let _settings_guard = setup_snapshot_settings(&repo).bind_to_scope();
    let feature_path = repo.add_worktree("feature");

    // First promote: feature to main worktree
    let output = repo
        .wt_command()
        .args(["step", "promote", "feature"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "first promote failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify first promote worked
    assert_eq!(get_branch(&repo, repo.root_path()), "feature");

    // Restore: promote main back (now 'main' is in the other worktree)
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["promote", "main"],
        Some(repo.root_path()),
    ));

    // Verify canonical state restored
    assert_eq!(
        get_branch(&repo, repo.root_path()),
        "main",
        "main worktree should have main again"
    );
    assert_eq!(
        get_branch(&repo, &feature_path),
        "feature",
        "other worktree should have feature again"
    );
}

/// Test when branch is already in main worktree
#[rstest]
fn test_promote_already_in_main(repo: TestRepo) {
    let _settings_guard = setup_snapshot_settings(&repo).bind_to_scope();
    // 'main' is already in main worktree
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["promote", "main"],
        Some(repo.root_path()),
    ));
}

/// Test auto-restore with no argument from main worktree (after prior promote)
#[rstest]
fn test_promote_auto_restore(mut repo: TestRepo) {
    let _settings_guard = setup_snapshot_settings(&repo).bind_to_scope();
    let feature_path = repo.add_worktree("feature");

    // First promote: feature to main worktree (creates mismatch)
    let output = repo
        .wt_command()
        .args(["step", "promote", "feature"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "first promote failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify first promote worked
    assert_eq!(get_branch(&repo, repo.root_path()), "feature");
    assert_eq!(get_branch(&repo, &feature_path), "main");

    // Auto-restore: no argument from main worktree restores default branch
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["promote"],
        Some(repo.root_path()),
    ));

    // Verify canonical state restored
    assert_eq!(
        get_branch(&repo, repo.root_path()),
        "main",
        "main worktree should have main again"
    );
    assert_eq!(
        get_branch(&repo, &feature_path),
        "feature",
        "other worktree should have feature again"
    );
}

/// Test auto-restore when no argument from main worktree (already canonical)
#[rstest]
fn test_promote_no_arg_from_main(repo: TestRepo) {
    let _settings_guard = setup_snapshot_settings(&repo).bind_to_scope();
    // From main worktree with no arg: restores default branch (already there)
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["promote"],
        Some(repo.root_path()),
    ));
}

/// Test error when branch has no worktree
#[rstest]
fn test_promote_branch_not_in_worktree(repo: TestRepo) {
    let _settings_guard = setup_snapshot_settings(&repo).bind_to_scope();
    // Create a branch but don't make a worktree for it
    repo.run_git(&["branch", "orphan"]);

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["promote", "orphan"],
        Some(repo.root_path()),
    ));
}

/// Test error when main worktree is dirty
#[rstest]
fn test_promote_dirty_main(mut repo: TestRepo) {
    let _settings_guard = setup_snapshot_settings(&repo).bind_to_scope();
    let _feature_path = repo.add_worktree("feature");

    // Make main worktree dirty
    fs::write(repo.root_path().join("dirty.txt"), "uncommitted").unwrap();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["promote", "feature"],
        Some(repo.root_path()),
    ));
}

/// Test error when target worktree is dirty
#[rstest]
fn test_promote_dirty_target(mut repo: TestRepo) {
    let _settings_guard = setup_snapshot_settings(&repo).bind_to_scope();
    let feature_path = repo.add_worktree("feature");

    // Make target worktree dirty
    fs::write(feature_path.join("dirty.txt"), "uncommitted").unwrap();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["promote", "feature"],
        Some(repo.root_path()),
    ));
}

/// Test that wt list shows mismatch indicator after promote
#[rstest]
fn test_promote_shows_mismatch_in_list(mut repo: TestRepo) {
    let _settings_guard = setup_snapshot_settings(&repo).bind_to_scope();
    let _feature_path = repo.add_worktree("feature");

    // Promote
    let output = repo
        .wt_command()
        .args(["step", "promote", "feature"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "promote failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // List should show mismatch indicators
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "list",
        &[],
        Some(repo.root_path()),
    ));
}

/// Test error when run in a bare repository (no worktrees)
#[test]
fn test_promote_bare_repo_no_worktrees() {
    let temp_dir = tempfile::tempdir().unwrap();
    let bare_repo = temp_dir.path().join("bare.git");

    // Create a bare repository
    Command::new("git")
        .args(["init", "--bare"])
        .arg(&bare_repo)
        .output()
        .unwrap();

    // Try to run promote in the bare repo - fails with "No worktrees found"
    let output = wt_command()
        .args(["step", "promote", "feature"])
        .current_dir(&bare_repo)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(
        stderr.contains("No worktrees found"),
        "Expected no worktrees error, got: {stderr}"
    );
}

/// Test error when run in a bare repository with worktrees
#[test]
fn test_promote_bare_repo_with_worktrees() {
    let temp_dir = tempfile::tempdir().unwrap();
    let bare_repo = temp_dir.path().join("bare.git");
    let worktree_path = temp_dir.path().join("worktree");
    let temp_clone = temp_dir.path().join("temp");

    // Create a bare repository
    Command::new("git")
        .args(["init", "--bare", "--initial-branch=main"])
        .arg(&bare_repo)
        .output()
        .unwrap();

    // Create a commit via a temporary clone
    Command::new("git")
        .args([
            "clone",
            bare_repo.to_str().unwrap(),
            temp_clone.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(&temp_clone)
        .output()
        .unwrap();

    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(&temp_clone)
        .output()
        .unwrap();

    Command::new("git")
        .args(["commit", "--allow-empty", "-m", "init"])
        .current_dir(&temp_clone)
        .output()
        .unwrap();

    Command::new("git")
        .args(["push", "origin", "main"])
        .current_dir(&temp_clone)
        .output()
        .unwrap();

    // Add a worktree to the bare repo
    Command::new("git")
        .args([
            "--git-dir",
            bare_repo.to_str().unwrap(),
            "worktree",
            "add",
            worktree_path.to_str().unwrap(),
            "main",
        ])
        .output()
        .unwrap();

    // Try to run promote in the bare repo - should fail with bare repo error
    let output = wt_command()
        .args(["step", "promote", "feature"])
        .current_dir(&bare_repo)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(
        stderr.contains("bare repositories"),
        "Expected bare repo error, got: {stderr}"
    );
}

/// Test error when main worktree has detached HEAD
#[rstest]
fn test_promote_detached_head_main(mut repo: TestRepo) {
    let _settings_guard = setup_snapshot_settings(&repo).bind_to_scope();
    let _feature_path = repo.add_worktree("feature");

    // Detach HEAD in main worktree
    let sha = repo
        .git_command()
        .args(["rev-parse", "HEAD"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();
    let sha = String::from_utf8_lossy(&sha.stdout).trim().to_string();

    repo.git_command()
        .args(["checkout", "--detach", &sha])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    // Promote should fail due to detached HEAD
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["promote", "feature"],
        Some(repo.root_path()),
    ));
}

/// Test error when linked worktree has detached HEAD (no-arg promote)
#[rstest]
fn test_promote_detached_head_linked(mut repo: TestRepo) {
    let _settings_guard = setup_snapshot_settings(&repo).bind_to_scope();
    let feature_path = repo.add_worktree("feature");

    // Detach HEAD in the linked worktree
    let sha = repo
        .git_command()
        .args(["rev-parse", "HEAD"])
        .current_dir(&feature_path)
        .output()
        .unwrap();
    let sha = String::from_utf8_lossy(&sha.stdout).trim().to_string();

    repo.git_command()
        .args(["checkout", "--detach", &sha])
        .current_dir(&feature_path)
        .output()
        .unwrap();

    // No-arg promote from linked worktree should fail due to detached HEAD
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["promote"],
        Some(&feature_path),
    ));
}
