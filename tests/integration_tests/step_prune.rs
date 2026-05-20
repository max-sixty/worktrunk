//! Integration tests for `wt step prune`

use crate::common::{
    BareRepoTest, TestRepo, make_snapshot_cmd, repo, repo_with_remote, setup_temp_snapshot_settings,
};
use ansi_str::AnsiStr;
use insta::assert_snapshot;
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

    let mut cmd = make_snapshot_cmd(&repo, "step", &["prune", "--yes", "--min-age=0s"], None);
    cmd.env("RAYON_NUM_THREADS", "1"); // deterministic output order
    assert_cmd_snapshot!(cmd);

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

    let mut cmd = make_snapshot_cmd(
        &repo,
        "step",
        &["prune", "--yes", "--min-age=0s", "--foreground"],
        None,
    );
    cmd.env("RAYON_NUM_THREADS", "1"); // deterministic output order
    assert_cmd_snapshot!(cmd);

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

    let mut cmd = make_snapshot_cmd(&repo, "step", &["prune", "--yes", "--min-age=0s"], None);
    cmd.env("RAYON_NUM_THREADS", "1"); // deterministic output order
    assert_cmd_snapshot!(cmd);

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
    cmd.env("RAYON_NUM_THREADS", "1"); // deterministic output order

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

    let mut cmd = make_snapshot_cmd(&repo, "step", &["prune", "--yes", "--min-age=0s"], None);
    cmd.env("RAYON_NUM_THREADS", "1"); // deterministic output order
    assert_cmd_snapshot!(cmd);

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

/// Prune handles stale detached metadata without deleting any branch.
#[rstest]
fn test_prune_stale_detached_worktree(repo: TestRepo) {
    repo.commit("initial");

    let wt_path = repo
        .root_path()
        .parent()
        .unwrap()
        .join("repo.stale-detached");
    let wt_path_str = wt_path.to_str().unwrap();
    repo.run_git(&["worktree", "add", "--detach", wt_path_str, "HEAD"]);
    let wt_path = std::fs::canonicalize(wt_path).unwrap();
    let wt_path_str = wt_path.to_str().unwrap();
    let branches_before = repo.git_output(&["branch", "--format=%(refname:short)"]);

    std::fs::remove_dir_all(&wt_path).unwrap();
    let list_before = repo.git_output(&["worktree", "list", "--porcelain"]);
    assert!(
        list_before.contains("prunable"),
        "Git should report stale detached worktree metadata before prune"
    );

    let output = repo
        .wt_command()
        .args([
            "step",
            "prune",
            "--yes",
            "--min-age=0s",
            "--format=json",
            "--foreground",
        ])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr)
        .ansi_strip()
        .into_owned();
    assert!(output.status.success(), "prune failed\nstderr:\n{stderr}");

    let items: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        items.len(),
        1,
        "expected one pruned item\nstderr:\n{stderr}"
    );
    assert!(items[0]["branch"].is_null());
    assert_eq!(items[0]["kind"].as_str(), Some("stale_worktree"));
    assert_eq!(items[0]["path"].as_str(), Some(wt_path_str));

    let list_after = repo.git_output(&["worktree", "list", "--porcelain"]);
    assert!(
        !list_after.contains(wt_path_str),
        "Stale detached worktree metadata should be pruned"
    );
    let branches_after = repo.git_output(&["branch", "--format=%(refname:short)"]);
    assert_eq!(
        branches_after, branches_before,
        "Pruning stale detached metadata should not delete branches"
    );
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

    // Orphan branch (no worktree) at HEAD: integrated but appears young
    repo.create_branch("young-orphan");

    // Epoch 30 minutes after GIT_COMMITTER_DATE → orphan branch appears 30min old, < 1h
    let mut cmd = make_snapshot_cmd(&repo, "step", &["prune", "--dry-run"], None);
    cmd.env("WORKTRUNK_TEST_EPOCH", "1735691400");
    assert_cmd_snapshot!(cmd);
}

/// Non-dry-run variant of `test_prune_stale_plus_young`: exercises the skipped_young
/// message in the non-dry-run removal path.
#[rstest]
fn test_prune_stale_plus_young_non_dry_run(mut repo: TestRepo) {
    repo.commit("initial");

    // Stale worktree: directory deleted, but git metadata remains → candidate
    let wt_path = repo.add_worktree("stale-branch");
    std::fs::remove_dir_all(&wt_path).unwrap();

    // Regular merged worktree: with default epoch it appears "young"
    repo.add_worktree("young-branch");

    // Default min-age (1h) — young-branch is skipped, stale-branch is removed
    let mut cmd = make_snapshot_cmd(&repo, "step", &["prune", "--yes"], None);
    cmd.env("RAYON_NUM_THREADS", "1"); // deterministic output order
    assert_cmd_snapshot!(cmd);
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

// ============================================================================
// --format=json
// ============================================================================

#[rstest]
fn test_prune_dry_run_json(mut repo: TestRepo) {
    repo.commit("initial");
    repo.add_worktree("merged-a");

    let output = repo
        .wt_command()
        .args([
            "step",
            "prune",
            "--dry-run",
            "--min-age=0s",
            "--format=json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());

    let mut settings = insta::Settings::clone_current();
    settings.add_filter(r#""path": "[^"]*""#, r#""path": "<PATH>""#);
    settings.bind(|| {
        assert_snapshot!(String::from_utf8_lossy(&output.stdout));
    });
}

#[rstest]
fn test_prune_dry_run_json_empty(mut repo: TestRepo) {
    repo.commit("initial");
    repo.add_worktree_with_commit("feature", "f.txt", "content", "feature commit");

    let output = repo
        .wt_command()
        .args([
            "step",
            "prune",
            "--dry-run",
            "--min-age=0s",
            "--format=json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stdout), @"[]");
}

#[rstest]
fn test_prune_json_actual_removal(mut repo: TestRepo) {
    repo.commit("initial");
    repo.add_worktree("merged-a");

    let output = repo
        .wt_command()
        .args([
            "step",
            "prune",
            "--min-age=0s",
            "--format=json",
            "--yes",
            "--foreground",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());

    let mut settings = insta::Settings::clone_current();
    settings.add_filter(r#""path": "[^"]*""#, r#""path": "<PATH>""#);
    settings.bind(|| {
        assert_snapshot!(String::from_utf8_lossy(&output.stdout));
    });
}

#[cfg(not(target_os = "windows"))]
#[rstest]
fn test_prune_dry_run_json_current_worktree(mut repo: TestRepo) {
    repo.commit("initial");
    let wt_path = repo.add_worktree("current-merged");

    let output = repo
        .wt_command()
        .args([
            "step",
            "prune",
            "--dry-run",
            "--min-age=0s",
            "--format=json",
        ])
        .current_dir(&wt_path)
        .output()
        .unwrap();
    assert!(output.status.success());

    let mut settings = insta::Settings::clone_current();
    settings.add_filter(r#""path": "[^"]*""#, r#""path": "<PATH>""#);
    settings.bind(|| {
        assert_snapshot!(String::from_utf8_lossy(&output.stdout));
    });
}

#[rstest]
fn test_prune_dry_run_json_orphan_branch(repo: TestRepo) {
    repo.commit("initial");
    // Orphan branch: integrated but no worktree
    repo.create_branch("orphan-integrated");

    let output = repo
        .wt_command()
        .args([
            "step",
            "prune",
            "--dry-run",
            "--min-age=0s",
            "--format=json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());

    assert_snapshot!(String::from_utf8_lossy(&output.stdout));
}

#[cfg(not(target_os = "windows"))]
#[rstest]
fn test_prune_json_current_worktree(mut repo: TestRepo) {
    repo.commit("initial");
    let wt_path = repo.add_worktree("current-merged");

    let output = repo
        .wt_command()
        .args([
            "step",
            "prune",
            "--min-age=0s",
            "--format=json",
            "--yes",
            "--foreground",
        ])
        .current_dir(&wt_path)
        .output()
        .unwrap();
    assert!(output.status.success());

    let mut settings = insta::Settings::clone_current();
    settings.add_filter(r#""path": "[^"]*""#, r#""path": "<PATH>""#);
    settings.bind(|| {
        assert_snapshot!(String::from_utf8_lossy(&output.stdout));
    });
}

#[rstest]
fn test_prune_json_orphan_branch(repo: TestRepo) {
    repo.commit("initial");
    repo.create_branch("orphan-integrated");

    let output = repo
        .wt_command()
        .args([
            "step",
            "prune",
            "--min-age=0s",
            "--format=json",
            "--yes",
            "--foreground",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());

    assert_snapshot!(String::from_utf8_lossy(&output.stdout));
}

/// Regression: `wt step prune` ORs over local AND upstream like `wt remove` /
/// `wt list`. A worktree merged into LOCAL `main` must still be pruned when
/// `main` and `origin/main` have diverged. Mirrors
/// `test_remove_merged_locally_when_upstream_diverged` in `remove.rs`.
#[rstest]
fn test_prune_locally_merged_when_upstream_diverged(#[from(repo_with_remote)] mut repo: TestRepo) {
    let remote_path = repo.remote_path().unwrap().to_path_buf();

    // Advance origin/main with a remote-only commit so local and upstream diverge.
    let github_sim = repo.home_path().join("github-sim-prune-local-merge");
    repo.run_git_in(
        repo.home_path(),
        &[
            "clone",
            remote_path.to_str().unwrap(),
            "github-sim-prune-local-merge",
        ],
    );
    std::fs::write(github_sim.join("remote-only.txt"), "remote only").unwrap();
    repo.run_git_in(&github_sim, &["add", "remote-only.txt"]);
    repo.run_git_in(&github_sim, &["commit", "-m", "Remote-only main commit"]);
    repo.run_git_in(&github_sim, &["push", "origin", "main"]);

    // Merge a feature into local main so local main contains the feature commit.
    repo.add_worktree("feature-prune-local");
    let feature_path = repo.worktree_path("feature-prune-local");
    std::fs::write(feature_path.join("feature.txt"), "feature").unwrap();
    repo.run_git_in(feature_path, &["add", "feature.txt"]);
    repo.run_git_in(feature_path, &["commit", "-m", "Add feature"]);
    repo.run_git(&[
        "merge",
        "--no-ff",
        "-m",
        "Merge feature",
        "feature-prune-local",
    ]);

    repo.run_git(&["fetch", "origin"]);

    let local_main = repo.git_output(&["rev-parse", "main"]);
    let origin_main = repo.git_output(&["rev-parse", "origin/main"]);
    assert_ne!(
        local_main, origin_main,
        "main and origin/main should differ"
    );
    assert!(
        !repo
            .git_command()
            .args(["merge-base", "--is-ancestor", "main", "origin/main"])
            .run()
            .unwrap()
            .status
            .success(),
        "local main must not be an ancestor of origin/main",
    );
    assert!(
        !repo
            .git_command()
            .args(["merge-base", "--is-ancestor", "origin/main", "main"])
            .run()
            .unwrap()
            .status
            .success(),
        "origin/main must not be an ancestor of local main",
    );

    let output = make_snapshot_cmd(&repo, "step", &["prune", "--yes", "--min-age=0s"], None)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr)
        .ansi_strip()
        .into_owned();

    assert!(
        output.status.success(),
        "prune should succeed\nstderr:\n{stderr}",
    );

    let worktree_path = repo
        .root_path()
        .parent()
        .unwrap()
        .join("repo.feature-prune-local");
    assert!(
        !worktree_path.exists(),
        "locally-merged worktree should be pruned even when origin/main has diverged\nstderr:\n{stderr}",
    );

    let branch_still_exists = repo
        .git_command()
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            "refs/heads/feature-prune-local",
        ])
        .run()
        .unwrap()
        .status
        .success();
    assert!(
        !branch_still_exists,
        "locally-merged branch should be deleted alongside its worktree\nstderr:\n{stderr}",
    );
}

/// Regression companion: a worktree squash-merged on `origin/main` is pruned
/// when local `main` has its own unique commits. Mirrors
/// `test_remove_squash_merged_on_remote_when_local_main_diverged`.
#[rstest]
fn test_prune_squash_merged_on_remote_when_local_diverged(
    #[from(repo_with_remote)] mut repo: TestRepo,
) {
    let remote_path = repo.remote_path().unwrap().to_path_buf();

    // Build, push, and remote-squash-merge a feature branch.
    repo.add_worktree("feature-prune-remote-squash");
    let feature_path = repo.worktree_path("feature-prune-remote-squash");
    std::fs::write(feature_path.join("feature-remote.txt"), "initial").unwrap();
    repo.run_git_in(feature_path, &["add", "feature-remote.txt"]);
    repo.run_git_in(feature_path, &["commit", "-m", "Add feature"]);
    std::fs::write(feature_path.join("feature-remote.txt"), "final").unwrap();
    repo.run_git_in(feature_path, &["add", "feature-remote.txt"]);
    repo.run_git_in(feature_path, &["commit", "-m", "Finalize feature"]);
    repo.run_git_in(
        feature_path,
        &["push", "-u", "origin", "feature-prune-remote-squash"],
    );

    let github_sim = repo.home_path().join("github-sim-prune-remote-squash");
    repo.run_git_in(
        repo.home_path(),
        &[
            "clone",
            remote_path.to_str().unwrap(),
            "github-sim-prune-remote-squash",
        ],
    );
    repo.run_git_in(
        &github_sim,
        &["merge", "--squash", "origin/feature-prune-remote-squash"],
    );
    repo.run_git_in(&github_sim, &["commit", "-m", "Add feature (#1)"]);
    repo.run_git_in(&github_sim, &["push", "origin", "main"]);

    // Fetch the remote squash; advance local main with a unique commit so local
    // and upstream diverge.
    repo.run_git(&["fetch", "origin"]);
    std::fs::write(repo.root_path().join("local-only.txt"), "local only").unwrap();
    repo.run_git(&["add", "local-only.txt"]);
    repo.run_git(&["commit", "-m", "Local-only main commit"]);

    let local_main = repo.git_output(&["rev-parse", "main"]);
    let origin_main = repo.git_output(&["rev-parse", "origin/main"]);
    assert_ne!(
        local_main, origin_main,
        "local main should diverge from origin/main"
    );

    let output = make_snapshot_cmd(&repo, "step", &["prune", "--yes", "--min-age=0s"], None)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr)
        .ansi_strip()
        .into_owned();

    assert!(
        output.status.success(),
        "prune should succeed\nstderr:\n{stderr}",
    );

    let worktree_path = repo
        .root_path()
        .parent()
        .unwrap()
        .join("repo.feature-prune-remote-squash");
    assert!(
        !worktree_path.exists(),
        "remotely-squash-merged worktree should be pruned when local main has diverged\nstderr:\n{stderr}",
    );
}

/// Hook announcements during prune include the branch name for disambiguation
#[rstest]
fn test_prune_hook_announcements_include_branch(mut repo: TestRepo) {
    repo.commit("initial");

    // Use branch names that don't collide with the fixture's feature-a/b/c
    repo.add_worktree("merged-x");
    repo.add_worktree("merged-y");

    repo.write_test_config(
        r#"[post-remove]
cleanup = "echo done"
"#,
    );

    let mut cmd = make_snapshot_cmd(&repo, "step", &["prune", "--yes", "--min-age=0s"], None);
    cmd.env("RAYON_NUM_THREADS", "1");
    assert_cmd_snapshot!(cmd);
}

/// Commit a `pre-remove` hook on the default branch, branch a worktree off that
/// commit (so it carries the hook in its own `.config/wt.toml`), then drop the
/// hook from the default branch and advance past the worktree's commit. The
/// worktree is now integrated (an ancestor of the default branch) and clean,
/// while the cwd config no longer mentions `pre-remove` — the situation where
/// prune would otherwise approve against the wrong config. Returns the
/// worktree path and the marker file the hook writes.
fn prune_branch_local_pre_remove_setup(
    repo: &mut TestRepo,
) -> (std::path::PathBuf, std::path::PathBuf) {
    use path_slash::PathExt as _;

    let marker = repo.root_path().join("prune-pre-remove-ran.txt");
    repo.write_project_config(&format!(
        r#"pre-remove = "echo ran > {}""#,
        marker.to_slash_lossy()
    ));
    repo.commit("Add pre-remove hook");
    let wt_path = repo.add_worktree("merged-with-hook");
    repo.write_project_config("# no hooks on the default branch\n");
    repo.commit("Drop pre-remove hook");
    (wt_path, marker)
}

/// `wt step prune`'s approval covers each pruned worktree's own `pre-remove`,
/// not just the cwd config's. A hook that lives only in a pruned worktree's
/// branch-local `.config/wt.toml` must be approved before it runs — with no TTY
/// and no prior approval, prune aborts rather than running it silently.
#[rstest]
fn test_prune_branch_local_pre_remove_needs_approval(mut repo: TestRepo) {
    let (wt_path, marker) = prune_branch_local_pre_remove_setup(&mut repo);

    let output = repo
        .wt_command()
        .args(["step", "prune", "--foreground", "--min-age=0s"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "prune should abort when an un-approved branch-local pre-remove is in scope; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("needs approval") && stderr.contains("pre-remove"),
        "prune should surface the pruned worktree's pre-remove for approval; stderr:\n{stderr}"
    );
    assert!(
        wt_path.exists(),
        "the worktree must not be removed when approval fails"
    );
    assert!(
        !marker.exists(),
        "the pre-remove hook must not run without approval"
    );
}

/// An unmerged worktree's branch-local `pre-remove` is outside prune's removal
/// set, so it must not be part of the approval gate.
#[rstest]
fn test_prune_unmerged_pre_remove_is_not_approved(mut repo: TestRepo) {
    repo.write_project_config(r#"pre-remove = "echo unmerged pre-remove""#);
    repo.commit("Add pre-remove hook");
    let wt_path = repo.add_worktree_with_commit(
        "unmerged-with-hook",
        "unmerged.txt",
        "content",
        "unmerged commit",
    );

    let output = repo
        .wt_command()
        .args(["step", "prune", "--foreground", "--min-age=0s"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "prune should not gate on an unmerged worktree's pre-remove; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("No merged worktrees to remove"),
        "prune should report no removable worktrees; stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("needs approval"),
        "unmerged pre-remove must not be requested for approval; stderr:\n{stderr}"
    );
    assert!(wt_path.exists(), "unmerged worktree should remain");
}

/// Removing only non-current worktrees does not switch directories, so the
/// primary worktree's `post-switch` is outside prune's approval gate.
#[rstest]
fn test_prune_non_current_removal_does_not_approve_post_switch(mut repo: TestRepo) {
    repo.write_project_config(r#"post-switch = "echo primary post-switch""#);
    repo.commit("Add post-switch hook");
    let wt_path = repo.add_worktree("merged-no-current");

    let output = repo
        .wt_command()
        .args(["step", "prune", "--foreground", "--min-age=0s"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "prune should not gate on post-switch for non-current removals; stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("needs approval"),
        "primary post-switch must not be requested for approval; stderr:\n{stderr}"
    );
    assert!(
        !wt_path.exists(),
        "the merged non-current worktree should be removed"
    );
}

/// With `--yes`, `wt step prune` runs the approved `pre-remove` hook from each
/// pruned worktree's own `.config/wt.toml`.
#[rstest]
fn test_prune_runs_branch_local_pre_remove_hook(mut repo: TestRepo) {
    use crate::common::wait_for_file_content;

    let (wt_path, marker) = prune_branch_local_pre_remove_setup(&mut repo);

    let output = repo
        .wt_command()
        .args(["step", "prune", "--foreground", "--yes", "--min-age=0s"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "wt step prune failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    wait_for_file_content(&marker);
    assert_eq!(std::fs::read_to_string(&marker).unwrap().trim(), "ran");
    assert!(!wt_path.exists(), "the merged worktree should be removed");
}

/// Regression test for serialized `wt step prune` fallback removals.
///
/// B-prime (the `check_lock` RwLock in `src/commands/step/prune.rs`)
/// serializes the parallel `integration_reason` `.git/config` readers against
/// the config-rewriting branch deletion writer. That includes the
/// cross-filesystem / `.gitmodules` / Windows-file-lock fallback: prune runs
/// the non-current fallback removal and branch deletion synchronously under
/// the write guard instead of spawning a detached `git worktree remove && git
/// branch -d`. A branch with a `[branch "<name>"]` section makes deletion
/// rewrite `.git/config` via lockfile+rename.
///
/// This forces that fallback for one non-current integrated worktree (by
/// pre-blocking its staged path, like
/// `test_remove_background_fallback_on_rename_failure`) while several other
/// integrated worktrees keep the parallel integration-check fan-out running,
/// so the deferred config-rewriting `git branch -D` overlaps live
/// `.git/config` readers.
///
/// On POSIX this is a deterministic fallback-path smoke test: a concurrent
/// `.git/config` read and rename never collide, so it always passes. On
/// Windows the same overlap can hit the documented residual gap — `git`
/// failing with `unable to access '.git/config': Permission denied`,
/// panicking prune. It is left in the normal suite (not a dedicated stress
/// lane) deliberately: a Windows flake here is empirical proof the fallback
/// path needs closing rather than the structurally-estimated "small" gap
/// (issue #2801).
#[rstest]
fn test_prune_fallback_config_race_canary(mut repo: TestRepo) {
    repo.commit("initial");

    // Several integrated worktrees → a real parallel integration-check
    // fan-out. `add_worktree` puts each branch at `main` HEAD, so all are
    // same-commit integrated and will be pruned. Each branch gets a
    // `[branch "<name>"]` section so its `git branch -D` rewrites
    // `.git/config` — the racing write. (No remote needed: `git branch -D`
    // removes the section regardless of whether `origin` resolves, and the
    // same-commit local check yields "integrated" before upstream is
    // consulted.)
    let names: Vec<String> = (0..6).map(|i| format!("merged-canary-{i}")).collect();
    for name in &names {
        repo.add_worktree(name);
        repo.run_git(&["config", &format!("branch.{name}.remote"), "origin"]);
        repo.run_git(&[
            "config",
            &format!("branch.{name}.merge"),
            &format!("refs/heads/{name}"),
        ]);
    }

    // Force the fallback for one *non-current* worktree by pre-creating a
    // file at its computed staged path so `std::fs::rename(worktree → trash)`
    // fails. Pick one in the middle so integration checks for later refs would
    // still be in flight if fallback branch deletion escaped the write guard.
    let blocked = names[3].clone();
    let blocked_wt_path = repo.worktree_path(&blocked).to_path_buf();
    let trash_dir = crate::common::resolve_git_common_dir(repo.root_path()).join("wt/trash");
    std::fs::create_dir_all(&trash_dir).unwrap();
    let staged_path = trash_dir.join(format!(
        "{}-{}",
        blocked_wt_path.file_name().unwrap().to_string_lossy(),
        crate::common::TEST_EPOCH
    ));
    std::fs::write(&staged_path, "blocking file to force fallback").unwrap();

    // Delay the fallback's `git branch -d <blocked>` call. Before the fix, that
    // branch deletion ran in a detached shell, so `wt step prune` could exit
    // while the branch still existed. The fixed path waits for it under the
    // prune write lock.
    let git_wrapper_dir = repo.home_path().join("git-wrapper");
    std::fs::create_dir_all(&git_wrapper_dir).unwrap();
    let real_git = which::which("git").unwrap();
    let branch_delete_marker = repo.home_path().join("fallback-branch-delete-started");
    write_delaying_git_wrapper(&git_wrapper_dir, &real_git);

    // Parallel fan-out is the point — do NOT pin RAYON_NUM_THREADS=1.
    let mut cmd = repo.wt_command();
    prepend_path(&mut cmd, &git_wrapper_dir);
    cmd.env("WT_PRUNE_DELAY_BRANCH", &blocked);
    cmd.env("WT_PRUNE_BRANCH_DELETE_STARTED", &branch_delete_marker);
    let output = cmd
        .args(["step", "prune", "--yes", "--min-age=0s"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "prune should succeed; the old Windows fallback-path race \
         failed it here with a `.git/config` permission error \
         (issue #2801).\nstderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("unable to access '.git/config'"),
        "fallback-path `.git/config` race fired — the fallback's \
         branch deletion collided with a live integration-check \
         reader (issue #2801).\nstderr:\n{stderr}"
    );
    assert!(
        branch_delete_marker.exists(),
        "delayed fallback branch deletion did not run"
    );
    assert!(
        !blocked_wt_path.exists(),
        "fallback worktree removal should finish before prune exits"
    );
    let branches = repo.git_output(&["branch", "--format=%(refname:short)"]);
    assert!(
        !branches.lines().any(|branch| branch == blocked),
        "fallback branch deletion should finish before prune exits; branches:\n{branches}"
    );

    let _ = std::fs::remove_file(&staged_path);
}

fn prepend_path(cmd: &mut std::process::Command, dir: &std::path::Path) {
    let (path_var_name, current_path) = std::env::vars_os()
        .find(|(key, _)| key.eq_ignore_ascii_case("PATH"))
        .map(|(key, value)| (key, Some(value)))
        .unwrap_or_else(|| ("PATH".into(), None));
    let mut paths: Vec<std::path::PathBuf> = current_path
        .as_deref()
        .map(std::env::split_paths)
        .into_iter()
        .flatten()
        .collect();
    paths.insert(0, dir.to_path_buf());
    cmd.env(path_var_name, std::env::join_paths(paths).unwrap());
}

#[cfg(unix)]
fn write_delaying_git_wrapper(dir: &std::path::Path, real_git: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    let real_git = shell_escape::unix::escape(real_git.to_string_lossy());
    let script = format!(
        r#"#!/bin/sh
if [ "$1" = "branch" ] && [ "$2" = "-d" ] && [ "$3" = "$WT_PRUNE_DELAY_BRANCH" ]; then
  : > "$WT_PRUNE_BRANCH_DELETE_STARTED"
  sleep 2
fi
exec {real_git} "$@"
"#
    );
    let path = dir.join("git");
    std::fs::write(&path, script).unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&path, permissions).unwrap();
}

#[cfg(windows)]
fn write_delaying_git_wrapper(dir: &std::path::Path, real_git: &std::path::Path) {
    let script = format!(
        r#"@echo off
if "%~1"=="branch" if "%~2"=="-d" if "%~3"=="%WT_PRUNE_DELAY_BRANCH%" (
  type nul > "%WT_PRUNE_BRANCH_DELETE_STARTED%"
  ping -n 3 127.0.0.1 > nul
)
"{}" %*
"#,
        real_git.display()
    );
    std::fs::write(dir.join("git.cmd"), script).unwrap();
}
