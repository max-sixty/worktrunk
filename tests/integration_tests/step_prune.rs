//! Integration tests for `wt step prune`

use crate::common::{TestRepo, make_snapshot_cmd, repo};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;

/// No merged worktrees — nothing to prune
#[rstest]
fn test_prune_no_merged(mut repo: TestRepo) {
    repo.commit("initial");

    // Create a worktree with a unique commit (not merged into main)
    repo.add_worktree_with_commit("feature", "f.txt", "content", "feature commit");

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["prune", "--dry-run", "--min-age=0s"],
        None
    ));
}

/// Prune dry-run shows merged worktrees
#[rstest]
fn test_prune_dry_run(mut repo: TestRepo) {
    repo.commit("initial");

    // Create a worktree at same commit as main (looks merged)
    repo.add_worktree("merged-branch");

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["prune", "--dry-run", "--min-age=0s"],
        None
    ));

    // Verify worktree still exists (dry run)
    let worktree_path = repo
        .root_path()
        .parent()
        .unwrap()
        .join("repo.merged-branch");
    assert!(
        worktree_path.exists(),
        "Worktree should still exist after dry run"
    );
}

/// Prune actually removes merged worktrees
#[rstest]
fn test_prune_removes_merged(mut repo: TestRepo) {
    repo.commit("initial");

    // Create a worktree at same commit as main (integrated)
    repo.add_worktree("merged-branch");

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["prune", "--yes", "--min-age=0s"],
        None
    ));

    // Verify worktree was removed
    let worktree_path = repo
        .root_path()
        .parent()
        .unwrap()
        .join("repo.merged-branch");
    assert!(!worktree_path.exists(), "Merged worktree should be removed");
}

/// Prune skips worktrees with unique commits (not merged)
#[rstest]
fn test_prune_skips_unmerged(mut repo: TestRepo) {
    repo.commit("initial");

    // One merged worktree
    repo.add_worktree("merged-one");

    // One unmerged worktree (has a unique commit)
    repo.add_worktree_with_commit("unmerged", "u.txt", "content", "unmerged commit");

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["prune", "--yes", "--min-age=0s"],
        None
    ));

    // Merged worktree removed
    let merged_path = repo.root_path().parent().unwrap().join("repo.merged-one");
    assert!(!merged_path.exists(), "Merged worktree should be removed");

    // Unmerged worktree still exists
    let unmerged_path = repo.root_path().parent().unwrap().join("repo.unmerged");
    assert!(unmerged_path.exists(), "Unmerged worktree should remain");
}

/// Min-age guard: worktrees younger than threshold are skipped.
///
/// With test epoch (Jan 2025) and real file creation (Feb 2026), get_now()
/// returns a time before the file was created, so age is 0 — always younger
/// than any positive threshold. This verifies the guard works.
#[rstest]
fn test_prune_min_age_skips_young(mut repo: TestRepo) {
    repo.commit("initial");

    // Create a worktree at same commit as main (would be pruned without age guard)
    repo.add_worktree("young-branch");

    // Default min-age (1h) — worktree appears "young" due to test epoch
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["prune", "--dry-run"],
        None
    ));

    // Verify worktree still exists
    let worktree_path = repo.root_path().parent().unwrap().join("repo.young-branch");
    assert!(worktree_path.exists(), "Young worktree should be skipped");
}

/// Prune multiple merged worktrees at once
#[rstest]
fn test_prune_multiple(mut repo: TestRepo) {
    repo.commit("initial");

    repo.add_worktree("merged-a");
    repo.add_worktree("merged-b");
    repo.add_worktree("merged-c");

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["prune", "--yes", "--min-age=0s"],
        None
    ));

    // All merged worktrees removed
    let parent = repo.root_path().parent().unwrap();
    assert!(!parent.join("repo.merged-a").exists());
    assert!(!parent.join("repo.merged-b").exists());
    assert!(!parent.join("repo.merged-c").exists());
}
