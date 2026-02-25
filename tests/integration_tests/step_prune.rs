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

/// Prune skips detached HEAD worktrees
#[rstest]
fn test_prune_skips_detached(mut repo: TestRepo) {
    repo.commit("initial");

    // Merged worktree — should be pruned
    repo.add_worktree("merged-branch");

    // Unmerged worktree with detached HEAD — should be skipped entirely
    // (branch has a unique commit so won't be picked up by orphan scan either)
    repo.add_worktree_with_commit("detached-branch", "d.txt", "data", "detached commit");
    repo.detach_head_in_worktree("detached-branch");

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["prune", "--dry-run", "--min-age=0s"],
        None
    ));

    // Both worktrees still exist (dry run)
    let parent = repo.root_path().parent().unwrap();
    assert!(parent.join("repo.merged-branch").exists());
    assert!(parent.join("repo.detached-branch").exists());
}

/// Prune skips locked worktrees
#[rstest]
fn test_prune_skips_locked(mut repo: TestRepo) {
    repo.commit("initial");

    // Merged worktree — should be pruned
    repo.add_worktree("merged-branch");

    // Locked worktree at same commit — should be skipped
    repo.add_worktree("locked-branch");
    repo.lock_worktree("locked-branch", Some("in use"));

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["prune", "--yes", "--min-age=0s"],
        None
    ));

    // Merged removed, locked remains
    let parent = repo.root_path().parent().unwrap();
    assert!(!parent.join("repo.merged-branch").exists());
    assert!(
        parent.join("repo.locked-branch").exists(),
        "Locked worktree should be skipped"
    );
}

/// Prune deletes orphan branches (integrated branches without worktrees)
#[rstest]
fn test_prune_orphan_branches(mut repo: TestRepo) {
    repo.commit("initial");

    // Create a branch at HEAD (integrated) without a worktree
    repo.create_branch("orphan-integrated");

    // Create an unmerged branch (has a unique commit via worktree, then remove worktree)
    repo.add_worktree_with_commit("unmerged-orphan", "u.txt", "data", "unique commit");

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["prune", "--yes", "--min-age=0s"],
        None
    ));
}

/// Prune from a merged worktree removes it last (CandidateKind::Current)
#[rstest]
fn test_prune_current_worktree(mut repo: TestRepo) {
    repo.commit("initial");

    // Create a worktree at same commit as main
    let wt_path = repo.add_worktree("current-merged");

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["prune", "--yes", "--min-age=0s"],
        Some(&wt_path)
    ));

    // Current worktree was removed
    assert!(
        !wt_path.exists(),
        "Current merged worktree should be removed"
    );
}

/// Prune handles stale/prunable worktrees (directory deleted but git metadata remains)
#[rstest]
fn test_prune_stale_worktree(mut repo: TestRepo) {
    repo.commit("initial");

    // Create a worktree at same commit (integrated), then delete its directory
    let wt_path = repo.add_worktree("stale-branch");
    std::fs::remove_dir_all(&wt_path).unwrap();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["prune", "--yes", "--min-age=0s"],
        None
    ));
}

/// Min-age check passes when worktrees are old enough.
///
/// Uses a far-future epoch (2030) so real worktrees (created Feb 2026) appear
/// ~4 years old, passing the default 1h min-age. This exercises the age
/// fall-through path that `--min-age=0s` bypasses entirely.
#[rstest]
fn test_prune_min_age_passes(mut repo: TestRepo) {
    repo.commit("initial");

    repo.add_worktree("old-merged");

    // Far-future epoch: worktrees appear ~4 years old
    let mut cmd = make_snapshot_cmd(&repo, "step", &["prune", "--dry-run"], None);
    cmd.env("WORKTRUNK_TEST_EPOCH", "1893456000"); // 2030-01-01

    assert_cmd_snapshot!(cmd);
}

/// Stale candidate + young worktrees: shows both the candidate and skipped count.
///
/// A stale worktree (directory deleted) bypasses the age check because it goes
/// through the `is_prunable()` path. A regular merged worktree with the default
/// epoch appears young and is skipped. This exercises the "N skipped" message
/// alongside candidates (lines that require both skipped_young > 0 and
/// non-empty candidates).
#[rstest]
fn test_prune_stale_plus_young(mut repo: TestRepo) {
    repo.commit("initial");

    // Stale worktree: directory deleted, but git metadata remains → candidate
    let wt_path = repo.add_worktree("stale-branch");
    std::fs::remove_dir_all(&wt_path).unwrap();

    // Regular merged worktree: with default epoch it appears "young"
    repo.add_worktree("young-branch");

    // Default min-age (1h) — young-branch is skipped, stale-branch is a candidate
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["prune", "--dry-run"],
        None
    ));
}
