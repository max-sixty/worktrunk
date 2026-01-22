//! Integration tests for `wt step promote`

use crate::common::{TestRepo, make_snapshot_cmd, repo, setup_snapshot_settings};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::fs;

/// Helper to get the current branch in a directory
fn get_branch(repo: &TestRepo, dir: &std::path::Path) -> String {
    let output = repo
        .git_command()
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(dir)
        .output()
        .unwrap();
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
    repo.wt_command()
        .args(["step", "promote", "feature"])
        .output()
        .unwrap();

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
    repo.wt_command()
        .args(["step", "promote", "feature"])
        .output()
        .unwrap();

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
    repo.wt_command()
        .args(["step", "promote", "feature"])
        .output()
        .unwrap();

    // List should show mismatch indicators
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "list",
        &[],
        Some(repo.root_path()),
    ));
}
