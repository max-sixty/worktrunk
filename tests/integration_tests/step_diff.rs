//! Integration tests for `wt step diff`

use crate::common::{TestRepo, make_snapshot_cmd, repo, setup_snapshot_settings};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::fs;
use std::path::Path;

/// Helper: create a feature worktree with a commit ahead of main
fn setup_feature_with_commit(repo: &mut TestRepo) -> std::path::PathBuf {
    let feature_path = repo.add_worktree("feature");
    fs::write(feature_path.join("feature.txt"), "feature content").unwrap();
    repo.run_git_in(&feature_path, &["add", "feature.txt"]);
    repo.run_git_in(&feature_path, &["commit", "-m", "Add feature file"]);
    feature_path
}

/// No changes: worktree identical to merge base
#[rstest]
fn test_step_diff_no_changes(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");
    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["diff"],
        Some(&feature_path),
    ));
}

/// Committed changes show full diff by default
#[rstest]
fn test_step_diff_committed_changes(mut repo: TestRepo) {
    let feature_path = setup_feature_with_commit(&mut repo);
    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["diff"],
        Some(&feature_path),
    ));
}

/// Untracked files appear in diff
#[rstest]
fn test_step_diff_untracked_files(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");
    fs::write(feature_path.join("untracked.txt"), "untracked content").unwrap();
    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["diff"],
        Some(&feature_path),
    ));
}

/// All change types: committed + staged + unstaged + untracked
#[rstest]
fn test_step_diff_all_change_types(mut repo: TestRepo) {
    let feature_path = setup_feature_with_commit(&mut repo);

    // Staged change
    fs::write(feature_path.join("staged.txt"), "staged content").unwrap();
    repo.run_git_in(&feature_path, &["add", "staged.txt"]);

    // Unstaged change (modify a tracked file)
    fs::write(feature_path.join("feature.txt"), "modified content").unwrap();

    // Untracked file
    fs::write(feature_path.join("new.txt"), "new content").unwrap();

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["diff"],
        Some(&feature_path),
    ));
}

/// Stat mode produces diffstat summary
#[rstest]
fn test_step_diff_stat_mode(mut repo: TestRepo) {
    let feature_path = setup_feature_with_commit(&mut repo);
    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["diff", "-s"],
        Some(&feature_path),
    ));
}

/// Stat mode with untracked files
#[rstest]
fn test_step_diff_stat_untracked(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");
    fs::write(feature_path.join("untracked.txt"), "untracked content").unwrap();
    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["diff", "--stat"],
        Some(&feature_path),
    ));
}

/// Real index is unchanged after running diff
#[rstest]
fn test_step_diff_index_unchanged(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");
    fs::write(feature_path.join("untracked.txt"), "content").unwrap();

    // Get index state before
    let index_before = get_git_status(&repo, &feature_path);

    // Run step diff
    let mut cmd = repo.wt_command();
    cmd.args(["step", "diff"]).current_dir(&feature_path);
    let output = cmd.output().unwrap();
    assert!(output.status.success(), "step diff failed");

    // Get index state after
    let index_after = get_git_status(&repo, &feature_path);

    assert_eq!(
        index_before, index_after,
        "Real git index was modified by step diff"
    );
}

/// Explicit target branch
#[rstest]
fn test_step_diff_explicit_target(mut repo: TestRepo) {
    let feature_path = setup_feature_with_commit(&mut repo);
    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["diff", "main"],
        Some(&feature_path),
    ));
}

fn get_git_status(repo: &TestRepo, dir: &Path) -> String {
    let output = repo
        .git_command()
        .args(["status", "--porcelain"])
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).to_string()
}
