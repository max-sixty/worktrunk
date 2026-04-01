//! Integration tests for `wt step prune`

use crate::common::{
    BareRepoTest, TestRepo, make_snapshot_cmd, repo, setup_temp_snapshot_settings,
};
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

/// Prune dry-run shows merged worktrees.
///
/// Two worktrees exercise the "N worktrees" plural path in the dry-run hint.
#[rstest]
fn test_prune_dry_run(mut repo: TestRepo) {
    repo.commit("initial");

    // Create worktrees at same commit as main (look merged)
    repo.add_worktree("merged-a");
    repo.add_worktree("merged-b");

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["prune", "--dry-run", "--min-age=0s"],
        None
    ));

    // Verify worktrees still exist (dry run)
    let parent = repo.root_path().parent().unwrap();
    assert!(
        parent.join("repo.merged-a").exists(),
        "Dry run should not remove worktrees"
    );
    assert!(
        parent.join("repo.merged-b").exists(),
        "Dry run should not remove worktrees"
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

    // Verify worktree was removed (non-current removal — no placeholder created)
    let worktree_path = repo
        .root_path()
        .parent()
        .unwrap()
        .join("repo.merged-branch");
    assert!(!worktree_path.exists(), "Worktree should be fully removed");
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

    // Merged worktree removed (non-current — no placeholder)
    let merged_path = repo.root_path().parent().unwrap().join("repo.merged-one");
    assert!(
        !merged_path.exists(),
        "Merged worktree should be fully removed"
    );

    // Unmerged worktree still exists
    let unmerged_path = repo.root_path().parent().unwrap().join("repo.unmerged");
    assert!(unmerged_path.exists(), "Unmerged worktree should remain");
}

/// Min-age guard: worktrees younger than threshold are skipped.
///
/// With test epoch (Jan 2025) and real file creation (Feb 2026), epoch_now()
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

    // All merged worktrees removed (non-current — no placeholders)
    let parent = repo.root_path().parent().unwrap();
    assert!(
        !parent.join("repo.merged-a").exists(),
        "merged-a should be fully removed"
    );
    assert!(
        !parent.join("repo.merged-b").exists(),
        "merged-b should be fully removed"
    );
    assert!(
        !parent.join("repo.merged-c").exists(),
        "merged-c should be fully removed"
    );
}

/// Prune skips unmerged detached HEAD worktrees
#[rstest]
fn test_prune_skips_unmerged_detached(mut repo: TestRepo) {
    repo.commit("initial");

    // Merged worktree — should be pruned
    repo.add_worktree("merged-branch");

    // Unmerged worktree with detached HEAD — should be skipped (not integrated)
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

/// Prune removes integrated detached HEAD worktrees
#[rstest]
fn test_prune_removes_integrated_detached(mut repo: TestRepo) {
    repo.commit("initial");

    // Worktree at same commit as main, then detach — integrated and detached
    repo.add_worktree("detached-integrated");
    repo.detach_head_in_worktree("detached-integrated");

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["prune", "--yes", "--min-age=0s"],
        None
    ));

    // Worktree was removed (non-current — no placeholder)
    let parent = repo.root_path().parent().unwrap();
    assert!(
        !parent.join("repo.detached-integrated").exists(),
        "Worktree should be fully removed"
    );
}

/// Prune removes multiple integrated detached HEAD worktrees (exercises plural "worktrees")
#[rstest]
fn test_prune_removes_multiple_detached(mut repo: TestRepo) {
    repo.commit("initial");

    // Two worktrees at same commit as main, then detach both
    repo.add_worktree("detached-a");
    repo.detach_head_in_worktree("detached-a");
    repo.add_worktree("detached-b");
    repo.detach_head_in_worktree("detached-b");

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["prune", "--yes", "--min-age=0s"],
        None
    ));

    let parent = repo.root_path().parent().unwrap();
    assert!(
        !parent.join("repo.detached-a").exists(),
        "detached-a should be fully removed"
    );
    assert!(
        !parent.join("repo.detached-b").exists(),
        "detached-b should be fully removed"
    );
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

    // Merged removed (non-current — no placeholder), locked remains
    let parent = repo.root_path().parent().unwrap();
    assert!(
        !parent.join("repo.merged-branch").exists(),
        "Merged worktree should be fully removed"
    );
    assert!(
        parent.join("repo.locked-branch").exists(),
        "Locked worktree should be skipped"
    );
}

/// Prune deletes orphan branches (integrated branches without worktrees).
///
/// Two orphan branches exercise the "N branches" plural path in the summary.
/// Uses a far-future epoch so branches pass the reflog age guard through the
/// normal age-check path (rather than bypassing with --min-age=0s).
#[rstest]
fn test_prune_orphan_branches(mut repo: TestRepo) {
    repo.commit("initial");

    // Create two branches at HEAD (integrated) without worktrees
    repo.create_branch("orphan-a");
    repo.create_branch("orphan-b");

    // Create an unmerged branch (has a unique commit via worktree, then remove worktree)
    repo.add_worktree_with_commit("unmerged-orphan", "u.txt", "data", "unique commit");

    // Far-future epoch: branches appear ~5 years old, passing the default 1h guard
    let mut cmd = make_snapshot_cmd(&repo, "step", &["prune", "--yes"], None);
    cmd.env("WORKTRUNK_TEST_EPOCH", "1893456000"); // 2030-01-01

    assert_cmd_snapshot!(cmd);
}

/// Orphan branches (no worktree) respect the min-age guard via reflog timestamps.
///
/// GIT_COMMITTER_DATE=2025-01-01T00:00:00Z makes the branch reflog timestamp
/// epoch 1735689600. Setting TEST_EPOCH to 30 minutes later (1735691400) means
/// the branch appears 30 minutes old, which is younger than the default 1h.
#[rstest]
fn test_prune_orphan_branch_min_age(repo: TestRepo) {
    repo.commit("initial");

    // Create a branch at HEAD (integrated) without a worktree
    repo.create_branch("orphan-integrated");

    // Epoch 30 minutes after GIT_COMMITTER_DATE → branch appears 30min old, < 1h
    let mut cmd = make_snapshot_cmd(&repo, "step", &["prune", "--yes"], None);
    cmd.env("WORKTRUNK_TEST_EPOCH", "1735691400"); // 2025-01-01T00:30:00Z

    assert_cmd_snapshot!(cmd);
}

/// Prune can remove a mix of branch-only and worktree candidates in one run.
#[rstest]
fn test_prune_mixed_worktree_and_orphan_branch(mut repo: TestRepo) {
    repo.commit("initial");

    // Branch-only candidate: integrated orphan branch without a worktree.
    repo.create_branch("orphan-mixed");

    // Worktree candidate: integrated worktree at the same commit as main.
    repo.add_worktree("merged-mixed");

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["prune", "--yes", "--min-age=0s"],
        None
    ));

    let parent = repo.root_path().parent().unwrap();
    assert!(
        !parent.join("repo.merged-mixed").exists(),
        "Worktree should be fully removed"
    );
}

/// Prune from a merged worktree removes it last (CandidateKind::Current).
///
/// Skipped on Windows: Windows locks the current working directory, preventing
/// `git worktree remove` from deleting it.
#[rstest]
#[cfg(not(target_os = "windows"))]
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
    crate::common::assert_worktree_removed(&wt_path);
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

/// Prune skips worktrees with uncommitted changes
#[rstest]
fn test_prune_skips_dirty(mut repo: TestRepo) {
    repo.commit("initial");

    // Merged worktree with uncommitted changes — should be skipped
    let wt_path = repo.add_worktree("dirty-merged");
    std::fs::write(wt_path.join("scratch.txt"), "wip").unwrap();

    // Clean merged worktree — should be pruned
    repo.add_worktree("clean-merged");

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["prune", "--yes", "--min-age=0s"],
        None
    ));

    // Dirty worktree still exists
    assert!(wt_path.exists(), "Dirty worktree should be skipped");

    // Clean worktree removed (non-current — no placeholder)
    let clean_path = repo.root_path().parent().unwrap().join("repo.clean-merged");
    assert!(
        !clean_path.exists(),
        "Clean worktree should be fully removed"
    );
}

/// Dry-run with mixed worktrees + orphan branches shows both counts.
///
/// Exercises the "N worktrees, M branches would be removed (dry run)" path
/// where the summary must distinguish worktree candidates from branch-only
/// candidates.
#[rstest]
fn test_prune_dry_run_mixed_worktrees_and_branches(mut repo: TestRepo) {
    repo.commit("initial");

    // Two worktrees at same commit as main (integrated)
    repo.add_worktree("merged-a");
    repo.add_worktree("merged-b");

    // One orphan branch (integrated, no worktree)
    repo.create_branch("orphan-integrated");

    // Far-future epoch so everything passes the age guard
    let mut cmd = make_snapshot_cmd(&repo, "step", &["prune", "--dry-run"], None);
    cmd.env("WORKTRUNK_TEST_EPOCH", "1893456000"); // 2030-01-01

    assert_cmd_snapshot!(cmd);
}

/// Prune works when the current worktree is mid-rebase.
///
/// During an interactive rebase, the worktree is in detached HEAD state.
/// `git branch --format=%(refname:lstrip=2)` includes a synthetic entry like
/// `(no branch, rebasing feature)` which isn't a valid ref. The orphan branch
/// scan must not pass this to `integration_reason`.
#[rstest]
fn test_prune_during_rebase(mut repo: TestRepo) {
    repo.commit("initial");

    // Create a merged worktree (same commit as main)
    repo.add_worktree("merged-wt");

    // Create a feature worktree with commits to rebase
    let feature_path = repo.add_worktree_with_commit("rebasing", "r.txt", "v1", "commit 1");
    repo.commit_in_worktree(&feature_path, "r.txt", "v2", "commit 2");

    // Start an interactive rebase that pauses (exec false fails)
    let git_status = repo
        .git_command()
        .args(["rebase", "-i", "--exec", "false", "main"])
        .current_dir(&feature_path)
        .env("GIT_SEQUENCE_EDITOR", "true")
        .run()
        .unwrap();
    // The rebase should pause (exec false fails), leaving us in rebase state
    assert!(!git_status.status.success(), "rebase should be paused");

    // Run prune from the rebasing worktree — should succeed, not error on
    // "(no branch, rebasing ...)" being used as a git revision
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["prune", "--yes", "--min-age=0s"],
        Some(&feature_path)
    ));
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

/// Prune detects squash-merged branches when target later modified the same files (#1818).
///
/// When `git merge-tree --write-tree` conflicts because the branch and target both
/// changed the same files, the patch-id fallback detects the squash merge.
#[rstest]
fn test_prune_squash_merged_same_files_modified(mut repo: TestRepo) {
    repo.commit("initial");

    // Create worktree, make changes to a file
    let wt_path = repo.add_worktree("feature-squash");
    std::fs::write(wt_path.join("shared.txt"), "feature content").unwrap();
    repo.run_git_in(&wt_path, &["add", "shared.txt"]);
    repo.run_git_in(&wt_path, &["commit", "-m", "Add feature"]);

    // Back on main: simulate squash merge (same content), then advance the same file
    std::fs::write(repo.root_path().join("shared.txt"), "feature content").unwrap();
    repo.run_git(&["add", "shared.txt"]);
    repo.run_git(&["commit", "-m", "Squash merge feature"]);

    std::fs::write(
        repo.root_path().join("shared.txt"),
        "feature content\nmore main changes",
    )
    .unwrap();
    repo.run_git(&["add", "shared.txt"]);
    repo.run_git(&["commit", "-m", "Advance same file on main"]);

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["prune", "--dry-run", "--min-age=0s"],
        None
    ));
}

/// Default branch without a worktree should not be pruned despite being
/// trivially "integrated" into itself (tautological SameCommit).
#[test]
fn test_prune_skips_default_branch_orphan() {
    use crate::common::TestRepoBase;

    let test = BareRepoTest::new();

    // Create main worktree with a commit, then remove it so main becomes orphan
    let main_wt = test.create_worktree("main", "main");
    test.commit_in(&main_wt, "initial commit");
    std::fs::remove_dir_all(&main_wt).unwrap();
    test.git_command(test.bare_repo_path())
        .args(["worktree", "prune"])
        .run()
        .unwrap();

    // Create a feature branch (integrated, at same commit as main)
    let feature_wt = test.create_worktree("feature", "feature");

    let settings = setup_temp_snapshot_settings(test.temp_path());
    settings.bind(|| {
        let mut cmd = test.wt_command();
        cmd.args(["step", "prune", "--yes"])
            .current_dir(&feature_wt)
            // Far-future epoch: branches appear old enough to pass min-age guard
            .env("WORKTRUNK_TEST_EPOCH", "1893456000");

        assert_cmd_snapshot!("prune_skips_default_branch_orphan", cmd);
    });

    // Verify main branch still exists
    let output = test
        .git_command(test.bare_repo_path())
        .args(["branch", "--list", "main"])
        .run()
        .unwrap();
    let branches = String::from_utf8_lossy(&output.stdout);
    assert!(
        branches.contains("main"),
        "Default branch 'main' should not have been pruned"
    );
}
