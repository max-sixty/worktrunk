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

    // Default min-age (1d) — worktree appears "young" due to test epoch
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

/// Prune removes an integrated detached-HEAD worktree through the synchronous
/// fallback when the rename-into-trash fast path is blocked.
///
/// A detached worktree has no branch, so the fallback's branch deletion is a
/// no-op — this covers that arm of `delete_branch_in_synchronous_fallback`.
#[rstest]
fn test_prune_detached_worktree_rename_fallback(mut repo: TestRepo) {
    repo.commit("initial");
    let wt_path = repo.add_worktree("detached-fallback");
    repo.detach_head_in_worktree("detached-fallback");

    // Pre-create a file at the computed staged path so `std::fs::rename`
    // fails and prune takes the synchronous non-current fallback.
    let trash_dir = crate::common::resolve_git_common_dir(repo.root_path()).join("wt/trash");
    std::fs::create_dir_all(&trash_dir).unwrap();
    let staged_path = trash_dir.join(format!(
        "{}-{}",
        wt_path.file_name().unwrap().to_string_lossy(),
        crate::common::TEST_EPOCH
    ));
    std::fs::write(&staged_path, "blocking file to force fallback").unwrap();

    let output = repo
        .wt_command()
        .args(["step", "prune", "--yes", "--min-age=0s"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "prune should remove a detached worktree via the fallback; stderr:\n{stderr}"
    );
    assert!(
        !wt_path.exists(),
        "the detached worktree should be removed before prune exits"
    );

    let _ = std::fs::remove_file(&staged_path);
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

/// Regression for #2936: an unborn worktree (`git worktree add --orphan`) has
/// HEAD = `0000…`, so its branch name doesn't resolve via `git rev-parse`.
/// Prune used to abort with `fatal: Needed a single revision` while checking
/// branch integration. The conservative fix is to skip such worktrees from
/// the prune candidate set — they have no commits to integrate.
#[rstest]
fn test_prune_skips_unborn_worktree(mut repo: TestRepo) {
    repo.commit("initial");

    // Merged worktree — should still be pruned alongside the unborn one.
    repo.add_worktree("merged-branch");

    let orphan_path = repo.root_path().parent().unwrap().join("repo.orphan");
    let out = repo
        .git_command()
        .args([
            "worktree",
            "add",
            "--orphan",
            "-b",
            "orphan",
            orphan_path.to_str().unwrap(),
        ])
        .run()
        .unwrap();
    assert!(
        out.status.success(),
        "git worktree add --orphan failed: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let output = repo
        .wt_command()
        .args(["step", "prune", "--yes", "--min-age=0s"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("Needed a single revision"),
        "wt step prune should not bail on unborn worktrees, got stderr:\n{stderr}"
    );
    assert!(
        output.status.success(),
        "wt step prune should succeed; stderr:\n{stderr}"
    );

    let parent = repo.root_path().parent().unwrap();
    assert!(
        !parent.join("repo.merged-branch").exists(),
        "merged worktree should still be pruned"
    );
    assert!(
        orphan_path.exists(),
        "unborn worktree is skipped (not a prune candidate), so it should remain"
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

    // Far-future epoch: branches appear ~5 years old, passing the default 1d guard
    let mut cmd = make_snapshot_cmd(&repo, "step", &["prune", "--yes"], None);
    cmd.env("WORKTRUNK_TEST_EPOCH", "1893456000"); // 2030-01-01
    cmd.env("RAYON_NUM_THREADS", "1"); // deterministic output order

    assert_cmd_snapshot!(cmd);
}

/// Orphan branches (no worktree) respect the min-age guard via reflog timestamps.
///
/// GIT_COMMITTER_DATE=2025-01-01T00:00:00Z makes the branch reflog timestamp
/// epoch 1735689600. Setting TEST_EPOCH to 30 minutes later (1735691400) means
/// the branch appears 30 minutes old, which is younger than the default 1d.
#[rstest]
fn test_prune_orphan_branch_min_age(repo: TestRepo) {
    repo.commit("initial");

    // Create a branch at HEAD (integrated) without a worktree
    repo.create_branch("orphan-integrated");

    // Epoch 30 minutes after GIT_COMMITTER_DATE → branch appears 30min old, < 1d
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

/// Extract the `worktree <path>` line for the entry whose path ends with
/// `dir_name` from `git worktree list --porcelain` output.
///
/// Returns git's own path string verbatim — the exact form `wt` emits in its
/// JSON `path` field, since `WorktreeInfo::path` is `PathBuf::from(this)`.
/// Deriving the expected value this way avoids the Windows mismatch where
/// `std::fs::canonicalize` yields a `\\?\` verbatim, backslash-separated path
/// while git reports a forward-slash one.
fn porcelain_worktree_path<'a>(porcelain: &'a str, dir_name: &str) -> &'a str {
    porcelain
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .find(|path| {
            std::path::Path::new(path)
                .file_name()
                .is_some_and(|name| name == dir_name)
        })
        .unwrap_or_else(|| panic!("no worktree ending in {dir_name} in:\n{porcelain}"))
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
    repo.run_git(&[
        "worktree",
        "add",
        "--detach",
        wt_path.to_str().unwrap(),
        "HEAD",
    ]);
    let branches_before = repo.git_output(&["branch", "--format=%(refname:short)"]);

    std::fs::remove_dir_all(&wt_path).unwrap();
    let list_before = repo.git_output(&["worktree", "list", "--porcelain"]);
    assert!(
        list_before.contains("prunable"),
        "Git should report stale detached worktree metadata before prune"
    );
    // Use git's own path string for the expectation — `wt`'s JSON `path` is
    // `PathBuf::from` of exactly this, with no re-canonicalization.
    let wt_path_str = porcelain_worktree_path(&list_before, "repo.stale-detached");

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
/// ~4 years old, passing the default 1d min-age. This exercises the age
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

    // Epoch 30 minutes after GIT_COMMITTER_DATE → orphan branch appears 30min old, < 1d
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

    // Default min-age (1d) — young-branch is skipped, stale-branch is removed
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

/// Branch a worktree, advance the default branch past it (so it's integrated
/// and prunable), and put a `pre-remove` hook in the invoking worktree (cwd) —
/// the config `wt step prune` resolves against. Returns the worktree path and
/// the marker file the hook writes.
fn prune_pre_remove_setup(repo: &mut TestRepo) -> (std::path::PathBuf, std::path::PathBuf) {
    use path_slash::PathExt as _;

    let wt_path = repo.add_worktree("merged");
    // Advance the default branch so `merged` is strictly an ancestor — prune
    // treats it as integrated and removable.
    repo.commit("Advance default branch");
    // The `pre-remove` hook lives in the invoking worktree (cwd), uncommitted.
    let marker = repo.root_path().join("prune-pre-remove-ran.txt");
    repo.write_project_config(&format!(
        r#"pre-remove = "echo ran > {}""#,
        marker.to_slash_lossy()
    ));
    (wt_path, marker)
}

/// `wt step prune` never prompts inline — streaming removals would deadlock
/// against a prompt. Instead a candidate whose `pre-remove` (resolved from the
/// invoking worktree's config) isn't yet approved is SKIPPED with
/// `(approval required)`, with a hint pointing at `wt config approvals add`.
/// Skipping is non-fatal — exit 0 — so other candidates with already-approved
/// (or no) hooks still get pruned.
#[rstest]
fn test_prune_pre_remove_needs_approval(mut repo: TestRepo) {
    let (wt_path, marker) = prune_pre_remove_setup(&mut repo);

    let output = repo
        .wt_command()
        .args(["step", "prune", "--foreground", "--min-age=0s"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "prune should skip the unapproved candidate, not abort; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("(approval required)"),
        "prune should report the candidate as skipped for approval; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("wt config approvals add"),
        "prune should hint at how to pre-approve; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("pre-remove: echo ran >"),
        "hint should list the unapproved template grouped by hook; stderr:\n{stderr}"
    );
    // The hint runs the path through `format_path_for_display`, so the
    // substring lands as `~/…` rather than the raw tempdir prefix.
    let wt_basename = wt_path.file_name().unwrap().to_string_lossy();
    assert!(
        stderr.contains("wt -C ~/") && stderr.contains(&format!("{wt_basename} remove")),
        "hint should offer a per-worktree `wt -C ~/…/{wt_basename} remove` alternative; stderr:\n{stderr}"
    );
    // `prune_pre_remove_setup` writes `.config/wt.toml` only in the invoking
    // worktree, so the candidate's `.config/wt.toml` doesn't exist — the
    // byte-compare flags the candidate as having different hooks on branch.
    assert!(
        stderr.contains("(different hooks on branch)"),
        "candidate without its own .config/wt.toml should be flagged as differing; stderr:\n{stderr}"
    );
    assert!(
        wt_path.exists(),
        "the worktree must not be removed when its hooks aren't approved"
    );
    assert!(
        !marker.exists(),
        "the pre-remove hook must not run without approval"
    );
}

/// An unmerged worktree is outside prune's removal set, so the `pre-remove` it
/// would run is never part of the approval gate.
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

/// With `--yes`, `wt step prune` runs the `pre-remove` hook from the invoking
/// worktree's `.config/wt.toml` for each pruned worktree.
#[rstest]
fn test_prune_runs_pre_remove_hook(mut repo: TestRepo) {
    use crate::common::wait_for_file_content;

    let (wt_path, marker) = prune_pre_remove_setup(&mut repo);

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

/// A declined orphan deletion removed nothing, so nothing is counted.
///
/// The scan selects both `carrier` (worktree, integrated) and `orphan`
/// (branch-only, integrated). `carrier`'s `pre-remove` hook then points
/// `orphan` at a commit main doesn't contain, so when the worker reaches
/// `orphan`, the SafeDelete re-check declines — and a candidate that removed
/// nothing must not appear in the summary as either "1 branch" (the plan's
/// intent) or "1 worktree" (the fate-counting of a stale entry it never had).
///
/// `RAYON_NUM_THREADS=1` makes the ordering causal, not raced: the serial
/// scan queues `carrier` (worktree entries precede orphans in `check_items`)
/// before `orphan`, and the single FIFO worker runs `carrier`'s removal — the
/// hook — before `orphan`'s deletion.
#[rstest]
fn test_prune_excludes_declined_orphan_deletion_from_summary(mut repo: TestRepo) {
    let carrier_wt = repo.add_worktree("carrier");
    repo.run_git(&["branch", "orphan"]);
    // Advance the default branch so both are ancestors — integrated at scan.
    repo.commit("Advance default branch");
    // Runs in `carrier`: commit a file, hand that commit to `orphan`, then
    // step `carrier` back to its integrated tip with a clean tree.
    repo.write_project_config(
        r#"pre-remove = "printf raced > raced.txt && git add raced.txt && git commit -m raced && git update-ref refs/heads/orphan HEAD && git reset --hard HEAD~1""#,
    );

    let mut cmd = repo.wt_command();
    cmd.args(["step", "prune", "--foreground", "--yes", "--min-age=0s"])
        .env("RAYON_NUM_THREADS", "1");
    let output = cmd.output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr)
        .ansi_strip()
        .into_owned();
    assert!(output.status.success(), "prune should succeed:\n{stderr}");

    // The exact line, so a counted orphan fails whichever way it's counted:
    // as its plan's intent ("…, 1 branch") or as a phantom worktree
    // ("…, 1 worktree").
    assert!(
        stderr.contains("Pruned 1 worktree & branch\n"),
        "the summary must count only carrier; the no-op orphan contributes nothing:\n{stderr}",
    );
    assert!(!carrier_wt.exists(), "carrier should be removed");
    repo.run_git(&["rev-parse", "--verify", "refs/heads/orphan"]);
    let tip = repo.git_output(&["show", "--format=", "--name-only", "refs/heads/orphan"]);
    assert!(
        tip.lines().any(|line| line == "raced.txt"),
        "the hook's commit must be orphan's tip, or the divergence never happened:\n{tip}",
    );
}

/// The summary counts the executed outcome, not the scan-time plan.
///
/// The scan selects `merged` as integrated and plans to take its branch with
/// it. Its `pre-remove` hook then commits, so the SafeDelete re-check against
/// fresh refs declines and only the worktree goes. Counting the plan would
/// announce a branch the run left standing.
#[rstest]
fn test_prune_summary_counts_declined_deletion_as_worktree_only(mut repo: TestRepo) {
    let wt_path = repo.add_worktree("merged");
    // Advance the default branch so `merged` is an ancestor — prune's scan
    // treats it as integrated and plans to delete the branch too.
    repo.commit("Advance default branch");
    // Runs in the worktree being removed and leaves it clean, so the removal
    // still succeeds while the branch it was going to delete has moved on.
    repo.write_project_config(
        r#"pre-remove = "printf raced > raced.txt && git add raced.txt && git commit -m raced""#,
    );

    let output = repo
        .wt_command()
        .args(["step", "prune", "--foreground", "--yes", "--min-age=0s"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr)
        .ansi_strip()
        .into_owned();
    assert!(output.status.success(), "prune should succeed:\n{stderr}");

    assert!(
        stderr.contains("Pruned 1 worktree") && !stderr.contains("worktree & branch"),
        "summary must count the worktree alone, not the branch prune kept:\n{stderr}",
    );
    assert!(!wt_path.exists(), "the merged worktree should be removed");
    repo.run_git(&["rev-parse", "--verify", "refs/heads/merged"]);
    let tip = repo.git_output(&["show", "--format=", "--name-only", "refs/heads/merged"]);
    assert!(
        tip.lines().any(|line| line == "raced.txt"),
        "the hook's commit must be the branch tip, or the divergence never happened:\n{tip}",
    );
}

/// Canary for `wt step prune` removals overlapping `.git/config` readers,
/// and regression test for synchronous fallback completion.
///
/// Prune's hook-free removals — the rename-failure fallback included — run
/// concurrently with the parallel `integration_reason` readers on the read
/// side of `check_lock` (`src/commands/step/prune.rs`). That is safe because
/// the chain's branch deletion is a CAS `git update-ref -d`, which never
/// rewrites `.git/config`; a deletion mechanism that rewrites config via
/// lockfile+rename (as `git branch -D` does — the original Windows race,
/// #2801) would collide with those readers again. Each branch here gets a
/// `[branch "<name>"]` section so any such regression has a section to
/// rewrite, and the assertion watches for the Windows
/// `unable to access '.git/config'` failure.
///
/// The fallback must also complete synchronously
/// (`SynchronousForNonCurrent`): this forces it for one non-current
/// integrated worktree (by pre-blocking its staged path, like
/// `test_remove_background_fallback_on_rename_failure`) while several other
/// integrated worktrees keep the parallel fan-out running. `wt step prune`
/// cannot exit before the fallback removal and branch deletion finish — the
/// regression assertion (`blocked` worktree and branch gone the instant prune
/// returns) holds on every platform.
///
/// On Unix a `git` shim on `PATH` additionally stalls the fallback's `git
/// branch -d` for two seconds and records that it ran: proof prune *waits*
/// for it rather than racing ahead. The shim is Unix-only because Rust's
/// `Command` resolves a bare program name through `CreateProcess`, which
/// appends only `.exe` and never finds a `git.cmd`/`git.bat` — the same
/// reason `mock_commands` links a real `.exe` mock on Windows. Windows
/// still exercises the fallback (the pre-blocked staged path) and the
/// synchronous-completion assertion.
#[rstest]
fn test_prune_fallback_config_race_canary(mut repo: TestRepo) {
    repo.commit("initial");

    // Several integrated worktrees → a real parallel integration-check
    // fan-out. `add_worktree` puts each branch at `main` HEAD, so all are
    // same-commit integrated and will be pruned. Each branch gets a
    // `[branch "<name>"]` section so a config-rewriting deletion (a
    // regression from the CAS `update-ref -d` back toward `git branch -d`,
    // which removes the section via lockfile+rename) has a racing write to
    // make. (No remote needed: the same-commit local check yields
    // "integrated" before upstream is consulted.)
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
    // fails. Pick one in the middle so integration checks for later refs are
    // still in flight while the fallback runs.
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

    // Parallel fan-out is the point — do NOT pin RAYON_NUM_THREADS=1.
    let mut cmd = repo.wt_command();

    // Unix only: a `git` shim that delays the fallback's branch deletion of
    // `<blocked>` by two seconds and records that it ran. Before the fix that
    // deletion ran in a detached shell, so prune could exit while the branch
    // still existed; the shim proves the fixed path waits for it.
    #[cfg(unix)]
    let branch_delete_marker = repo.home_path().join("fallback-branch-delete-started");
    #[cfg(unix)]
    {
        let git_wrapper_dir = repo.home_path().join("git-wrapper");
        std::fs::create_dir_all(&git_wrapper_dir).unwrap();
        write_delaying_git_wrapper(&git_wrapper_dir, &which::which("git").unwrap());
        prepend_path(&mut cmd, &git_wrapper_dir);
        cmd.env("WT_PRUNE_DELAY_BRANCH", &blocked);
        cmd.env("WT_PRUNE_BRANCH_DELETE_STARTED", &branch_delete_marker);
    }

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
    #[cfg(unix)]
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

/// A stale worktree entry whose metadata prune fails at execution surfaces
/// the failure rather than pretending the candidate was removed: the scan
/// records the prune in the plan (`prune_entry`), execution runs it before
/// the branch deletion, and its error fails the run — the branch and the
/// entry both survive intact. A `git` shim failing `worktree remove` is the
/// deterministic trigger. Unix-only for the same `CreateProcess` reason as
/// the canary shim above.
#[cfg(unix)]
#[rstest]
fn test_prune_surfaces_failing_metadata_prune(mut repo: TestRepo) {
    repo.commit("initial");

    // Integrated branch whose worktree directory is deleted out-of-band → a
    // stale (prunable) worktree entry, prune's `Prunable` check source.
    repo.add_worktree("stale-merged");
    std::fs::remove_dir_all(repo.worktree_path("stale-merged")).unwrap();

    let mut cmd = repo.wt_command();
    let git_wrapper_dir = repo.home_path().join("git-wrapper");
    std::fs::create_dir_all(&git_wrapper_dir).unwrap();
    write_failing_worktree_remove_wrapper(&git_wrapper_dir, &which::which("git").unwrap());
    prepend_path(&mut cmd, &git_wrapper_dir);
    let prune_failed_marker = repo.home_path().join("worktree-remove-failed");
    cmd.env("WT_TEST_WORKTREE_REMOVE_FAILED", &prune_failed_marker);

    let output = cmd
        .args(["step", "prune", "--yes", "--min-age=0s"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "a failing metadata prune is a failed removal, not a silent skip.\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("stale-merged"),
        "the error must name the failed candidate.\nstderr:\n{stderr}"
    );
    assert!(
        prune_failed_marker.exists(),
        "the shim never fired — the metadata prune was not exercised"
    );
    // Nothing half-done: the entry is still registered and the branch intact.
    let list = repo.git_output(&["worktree", "list", "--porcelain"]);
    assert!(
        list.contains("prunable"),
        "the failed prune must leave the entry registered; worktrees:\n{list}"
    );
    let branches = repo.git_output(&["branch", "--format=%(refname:short)"]);
    assert!(
        branches.lines().any(|branch| branch == "stale-merged"),
        "the failed candidate's branch must survive; branches:\n{branches}"
    );
}

/// `--dry-run` mutates nothing, even though the scan now plans stale entries
/// through `prepare_worktree_removal` like every other source: planning is
/// pure — the metadata prune rides the plan and only execution performs it.
/// A regression here (a mutation creeping back into planning) would make the
/// preview destructive.
#[rstest]
fn test_prune_dry_run_leaves_stale_entry_registered(mut repo: TestRepo) {
    repo.commit("initial");

    repo.add_worktree("stale-branch");
    std::fs::remove_dir_all(repo.worktree_path("stale-branch")).unwrap();

    let output = repo
        .wt_command()
        .args(["step", "prune", "--dry-run", "--min-age=0s"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("would be removed"),
        "the stale entry must be previewed as a candidate:\n{stdout}"
    );
    let list = repo.git_output(&["worktree", "list", "--porcelain"]);
    assert!(
        list.contains("prunable"),
        "dry run must leave the stale entry registered; worktrees:\n{list}"
    );
    let branches = repo.git_output(&["branch", "--format=%(refname:short)"]);
    assert!(
        branches.lines().any(|branch| branch == "stale-branch"),
        "dry run must leave the branch; branches:\n{branches}"
    );
}

/// Hook-free removals execute concurrently (the read side of `check_lock`),
/// not one at a time.
///
/// Two integrated orphan branches are removed through a `git` shim whose
/// `update-ref -d` arms barrier on each other: each records that it started,
/// then waits for the *other* branch's deletion to start before proceeding.
/// The barrier only resolves if both removals are in flight at once — under
/// serialized removals the first deletion would wait out the shim's 15 s
/// timeout and drop a sentinel file, which the test asserts absent. Causally
/// driven, so it runs at barrier speed when concurrency works; the timeout is
/// only the safety net. Unix-only for the same `CreateProcess` shim reason as
/// the canary above.
#[cfg(unix)]
#[rstest]
fn test_prune_removals_run_concurrently(repo: TestRepo) {
    repo.commit("initial");

    // Orphan branches at main HEAD: same-commit integrated, no worktree, so
    // each becomes a hook-free BranchOnly candidate on the parallel path.
    repo.create_branch("para-a");
    repo.create_branch("para-b");

    let mut cmd = repo.wt_command();
    // The removal pool is sized from the rayon thread count; pin it to two so
    // the barrier can resolve even on a single-core runner (the workers block
    // in subprocess waits, so two threads don't need two CPUs).
    cmd.env("RAYON_NUM_THREADS", "2");
    let git_wrapper_dir = repo.home_path().join("git-wrapper");
    std::fs::create_dir_all(&git_wrapper_dir).unwrap();
    write_barrier_git_wrapper(&git_wrapper_dir, &which::which("git").unwrap());
    prepend_path(&mut cmd, &git_wrapper_dir);
    let barrier_dir = repo.home_path().join("barrier");
    std::fs::create_dir_all(&barrier_dir).unwrap();
    cmd.env("WT_TEST_BARRIER_DIR", &barrier_dir);

    let output = cmd
        .args(["step", "prune", "--yes", "--min-age=0s"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "prune should succeed:\n{stderr}");
    for name in ["para-a", "para-b"] {
        assert!(
            barrier_dir.join(format!("started-{name}")).exists(),
            "the shim never fired for {name} — its CAS delete was not exercised"
        );
    }
    let timeouts: Vec<String> = std::fs::read_dir(&barrier_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("timeout-"))
        .collect();
    assert!(
        timeouts.is_empty(),
        "a deletion waited out the barrier — removals ran serially: {timeouts:?}"
    );
    let branches = repo.git_output(&["branch", "--format=%(refname:short)"]);
    for name in ["para-a", "para-b"] {
        assert!(
            !branches.lines().any(|branch| branch == name),
            "{name} should have been deleted; branches:\n{branches}"
        );
    }
}

/// The removals that unregister stale worktree metadata serialize through the
/// repository registry lock — one `git worktree remove` teardown at a time.
///
/// Four stale entries: two carrying a branch (`BranchOnly` plans whose
/// `prune_entry` executes the prune) and two detached (`StaleDetached`, which
/// prune in place of a removal). Each unregisters its own metadata with
/// `git worktree remove <path>`, which enumerates every sibling's `commondir`
/// as it resolves its target — so two overlapping teardowns can read an entry
/// another worker is mid-deleting and fail (issue #3661). The shim probes for
/// that overlap with an atomic `mkdir` lock held across a fixed window around
/// each teardown; serialized, no two ever hold it at once, so the test asserts
/// no `overlap-` sentinel appears (and every entry still pruned). If the lock
/// regressed, all four teardowns would fire at once and three would collide in
/// the window. Unix-only for the same `CreateProcess` shim reason as the canary
/// above.
///
/// `--foreground` runs both ways. It reserves `check_lock`'s write side for the
/// TTY trash-cleanup spinner, which only a worktree removal paints; every
/// candidate here plans a branch deletion or a bare prune, so the flag changes
/// nothing — the registry teardowns serialize inside `Repository` regardless.
#[cfg(unix)]
#[rstest]
fn test_prune_metadata_removals_serialize(
    mut repo: TestRepo,
    #[values(false, true)] foreground: bool,
) {
    repo.commit("initial");

    // At main HEAD, so every entry is same-commit integrated.
    let mut stale = vec![repo.add_worktree("stale-a"), repo.add_worktree("stale-b")];
    for name in ["det-a", "det-b"] {
        let path = repo
            .root_path()
            .parent()
            .unwrap()
            .join(format!("repo.{name}"));
        repo.run_git(&[
            "worktree",
            "add",
            "--detach",
            path.to_str().unwrap(),
            "HEAD",
        ]);
        stale.push(path);
    }
    for path in &stale {
        std::fs::remove_dir_all(path).unwrap();
    }

    let mut cmd = repo.wt_command();
    // The removal pool is sized from the rayon thread count; pin it to four so
    // all four teardowns would run at once if the registry lock regressed (the
    // workers block in subprocess waits, so four threads don't need four CPUs).
    cmd.env("RAYON_NUM_THREADS", "4");
    let git_wrapper_dir = repo.home_path().join("git-wrapper");
    std::fs::create_dir_all(&git_wrapper_dir).unwrap();
    write_overlap_probe_worktree_remove_wrapper(&git_wrapper_dir, &which::which("git").unwrap());
    prepend_path(&mut cmd, &git_wrapper_dir);
    let barrier_dir = repo.home_path().join("barrier");
    std::fs::create_dir_all(&barrier_dir).unwrap();
    cmd.env("WT_TEST_BARRIER_DIR", &barrier_dir);

    cmd.args(["step", "prune", "--yes", "--min-age=0s"]);
    if foreground {
        cmd.arg("--foreground");
    }
    let output = cmd.output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "prune should succeed:\n{stderr}");
    let sentinels: Vec<String> = std::fs::read_dir(&barrier_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    let started: Vec<&String> = sentinels
        .iter()
        .filter(|n| n.starts_with("started-"))
        .collect();
    assert_eq!(
        started.len(),
        stale.len(),
        "every stale entry should have pruned its own metadata: {started:?}"
    );
    let overlaps: Vec<&String> = sentinels
        .iter()
        .filter(|n| n.starts_with("overlap-"))
        .collect();
    assert!(
        overlaps.is_empty(),
        "two `git worktree remove` teardowns overlapped — the repository registry \
         lock did not serialize them (issue #3661): {overlaps:?}"
    );
    let list = repo.git_output(&["worktree", "list", "--porcelain"]);
    assert!(
        !list.contains("prunable"),
        "every stale entry should be unregistered; worktrees:\n{list}"
    );
    let branches = repo.git_output(&["branch", "--format=%(refname:short)"]);
    for name in ["stale-a", "stale-b"] {
        assert!(
            !branches.lines().any(|branch| branch == name),
            "{name} should have been deleted; branches:\n{branches}"
        );
    }
}

/// The first failing removal aborts the rest of the queue.
///
/// Three integrated orphan branches scan in sorted order (`abort-a` first)
/// with a single worker (`RAYON_NUM_THREADS=1`), so the jobs run FIFO. A
/// `git` shim makes `abort-a`'s CAS delete fail *after* deleting the ref
/// (so `cas_delete_branch_outcome`'s re-check finds it gone and propagates
/// the error rather than reporting `RetainedRaced`). The failure must flip
/// the abort flag: the queued `abort-b`/`abort-c` removals never execute,
/// their branches survive, no summary prints, and prune exits non-zero —
/// the serial loop's abort-on-first-error, preserved across the fan-out.
/// Unix-only for the same `CreateProcess` shim reason as the canary above.
#[cfg(unix)]
#[rstest]
fn test_prune_removal_failure_aborts_remaining_queue(repo: TestRepo) {
    repo.commit("initial");

    repo.create_branch("abort-a");
    repo.create_branch("abort-b");
    repo.create_branch("abort-c");

    let mut cmd = repo.wt_command();
    cmd.env("RAYON_NUM_THREADS", "1"); // FIFO: abort-a dispatches first
    let git_wrapper_dir = repo.home_path().join("git-wrapper");
    std::fs::create_dir_all(&git_wrapper_dir).unwrap();
    write_failing_branch_delete_wrapper(&git_wrapper_dir, &which::which("git").unwrap());
    prepend_path(&mut cmd, &git_wrapper_dir);
    cmd.env("WT_TEST_FAIL_DELETE_BRANCH", "abort-a");

    let output = cmd
        .args(["step", "prune", "--yes", "--min-age=0s"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "a failed removal must fail the run:\n{stderr}"
    );
    assert!(
        stderr.contains("removing branch abort-a"),
        "the error should carry the failing candidate's context:\n{stderr}"
    );
    assert!(
        !stderr.contains("Pruned "),
        "no summary after an aborted run:\n{stderr}"
    );
    let branches = repo.git_output(&["branch", "--format=%(refname:short)"]);
    assert!(
        !branches.lines().any(|b| b == "abort-a"),
        "the shim deleted abort-a's ref; branches:\n{branches}"
    );
    for name in ["abort-b", "abort-c"] {
        assert!(
            branches.lines().any(|b| b == name),
            "{name} was queued behind the failure and must survive; branches:\n{branches}"
        );
    }
}

/// Concurrent failures: the first error is reported, the rest stay quiet.
///
/// Two failing removals rendezvous inside the shim (the barrier from
/// `test_prune_removals_run_concurrently`) so both are in flight before
/// either error lands — exercising the drain's duplicate-failure arm, which
/// logs at debug rather than printing a second error.
#[cfg(unix)]
#[rstest]
fn test_prune_concurrent_removal_failures_report_first(repo: TestRepo) {
    repo.commit("initial");

    repo.create_branch("dupe-a");
    repo.create_branch("dupe-b");

    let mut cmd = repo.wt_command();
    cmd.env("RAYON_NUM_THREADS", "2"); // both jobs in flight at once
    let git_wrapper_dir = repo.home_path().join("git-wrapper");
    std::fs::create_dir_all(&git_wrapper_dir).unwrap();
    write_barrier_failing_delete_wrapper(&git_wrapper_dir, &which::which("git").unwrap());
    prepend_path(&mut cmd, &git_wrapper_dir);
    let barrier_dir = repo.home_path().join("barrier");
    std::fs::create_dir_all(&barrier_dir).unwrap();
    cmd.env("WT_TEST_BARRIER_DIR", &barrier_dir);

    let output = cmd
        .args(["step", "prune", "--yes", "--min-age=0s"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "failed removals must fail the run:\n{stderr}"
    );
    for name in ["dupe-a", "dupe-b"] {
        assert!(
            barrier_dir.join(format!("started-{name}")).exists(),
            "the shim never fired for {name}"
        );
    }
    // Exactly one candidate's failure reaches the terminal.
    assert_eq!(
        stderr.matches("removing branch dupe-").count(),
        1,
        "exactly one of the concurrent failures should be reported:\n{stderr}"
    );
}

/// A `git` shim that deletes `refs/heads/$WT_TEST_FAIL_DELETE_BRANCH` for
/// real and then reports failure when prune's CAS delete targets it —
/// making `cas_delete_branch_outcome` propagate an error (ref gone on
/// re-check) instead of `RetainedRaced` (ref still present).
#[cfg(unix)]
fn write_failing_branch_delete_wrapper(dir: &std::path::Path, real_git: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    let real_git = shell_escape::unix::escape(real_git.to_string_lossy());
    let script = format!(
        r#"#!/bin/sh
if [ "$1" = "update-ref" ] && [ "$2" = "-d" ] && [ "$3" = "refs/heads/$WT_TEST_FAIL_DELETE_BRANCH" ]; then
  {real_git} update-ref -d "$3" || true
  exit 1
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

/// The barrier shim (see `write_barrier_git_wrapper`) with a failing tail:
/// both `dupe-{a,b}` deletions rendezvous, delete their ref for real, then
/// report failure — two concurrent removal errors.
#[cfg(unix)]
fn write_barrier_failing_delete_wrapper(dir: &std::path::Path, real_git: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    let real_git = shell_escape::unix::escape(real_git.to_string_lossy());
    let script = format!(
        r#"#!/bin/sh
case "$1 $2 $3" in
  "update-ref -d refs/heads/dupe-a") own=dupe-a; other=dupe-b ;;
  "update-ref -d refs/heads/dupe-b") own=dupe-b; other=dupe-a ;;
  *) exec {real_git} "$@" ;;
esac
: > "$WT_TEST_BARRIER_DIR/started-$own"
i=0
while [ ! -e "$WT_TEST_BARRIER_DIR/started-$other" ]; do
  i=$((i+1))
  if [ "$i" -gt 300 ]; then
    break
  fi
  sleep 0.05
done
{real_git} update-ref -d "$3" || true
exit 1
"#
    );
    let path = dir.join("git");
    std::fs::write(&path, script).unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&path, permissions).unwrap();
}

/// A `git` shim whose `worktree remove` arms a probe for overlap (see
/// `test_prune_metadata_removals_serialize`): each records that it started,
/// then takes an atomic `mkdir` lock for a fixed window. Under the registry
/// serialization this is testing, no two teardowns ever hold it at once, so a
/// failed `mkdir` — a concurrent teardown mid-window — drops an `overlap-`
/// sentinel the test asserts absent. Everything else passes through to the real
/// git.
#[cfg(unix)]
fn write_overlap_probe_worktree_remove_wrapper(dir: &std::path::Path, real_git: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    let real_git = shell_escape::unix::escape(real_git.to_string_lossy());
    let script = format!(
        r#"#!/bin/sh
case "$1 $2" in
  "worktree remove") ;;
  *) exec {real_git} "$@" ;;
esac
own=$(basename "$3")
: > "$WT_TEST_BARRIER_DIR/started-$own"
if mkdir "$WT_TEST_BARRIER_DIR/active" 2>/dev/null; then
  sleep 0.2
  rmdir "$WT_TEST_BARRIER_DIR/active"
else
  : > "$WT_TEST_BARRIER_DIR/overlap-$own"
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

/// A `git` shim whose `update-ref -d refs/heads/para-{a,b}` arms rendezvous
/// with each other (see `test_prune_removals_run_concurrently`); everything
/// else passes through to the real git.
#[cfg(unix)]
fn write_barrier_git_wrapper(dir: &std::path::Path, real_git: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    let real_git = shell_escape::unix::escape(real_git.to_string_lossy());
    let script = format!(
        r#"#!/bin/sh
case "$1 $2 $3" in
  "update-ref -d refs/heads/para-a") own=para-a; other=para-b ;;
  "update-ref -d refs/heads/para-b") own=para-b; other=para-a ;;
  *) exec {real_git} "$@" ;;
esac
: > "$WT_TEST_BARRIER_DIR/started-$own"
i=0
while [ ! -e "$WT_TEST_BARRIER_DIR/started-$other" ]; do
  i=$((i+1))
  if [ "$i" -gt 300 ]; then
    : > "$WT_TEST_BARRIER_DIR/timeout-$own"
    break
  fi
  sleep 0.05
done
exec {real_git} "$@"
"#
    );
    let path = dir.join("git");
    std::fs::write(&path, script).unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&path, permissions).unwrap();
}

/// A `git` shim that fails every `git worktree remove` and passes everything
/// else through to the real git.
#[cfg(unix)]
fn write_failing_worktree_remove_wrapper(dir: &std::path::Path, real_git: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    let real_git = shell_escape::unix::escape(real_git.to_string_lossy());
    let script = format!(
        r#"#!/bin/sh
if [ "$1" = "worktree" ] && [ "$2" = "remove" ]; then
  : > "$WT_TEST_WORKTREE_REMOVE_FAILED"
  echo "shim: worktree remove disabled" >&2
  exit 1
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

#[cfg(unix)]
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
    // Match every deletion shape this prune might take. `delete_branch_if_safe`
    // emits one of:
    //   - `branch -D <branch>` (the force path, and the fallback when there's
    //     no snapshot SHA to compare-and-swap against)
    //   - `update-ref -d refs/heads/<branch> <expected-sha>` (the CAS path it
    //     takes when the branch is integrated)
    // The `-d` arm below is also matched defensively for the plain-delete form;
    // production no longer emits it.
    let script = format!(
        r#"#!/bin/sh
if [ "$1" = "branch" ] && {{ [ "$2" = "-d" ] || [ "$2" = "-D" ]; }} && [ "$3" = "$WT_PRUNE_DELAY_BRANCH" ]; then
  : > "$WT_PRUNE_BRANCH_DELETE_STARTED"
  sleep 2
elif [ "$1" = "update-ref" ] && [ "$2" = "-d" ] && [ "$3" = "refs/heads/$WT_PRUNE_DELAY_BRANCH" ]; then
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

/// `wt step prune` sweeps stale worktrees unattended, so it's the worst place to
/// delete a branch another worktree still has checked out — the user isn't
/// watching, and the survivor is left at a null OID with an unresolvable `HEAD`.
/// It shares `prepare_worktree_removal` with `wt remove`, and this pins that it
/// keeps sharing the guard.
#[rstest]
fn test_prune_retains_branch_checked_out_in_another_worktree(mut repo: TestRepo) {
    let survivor = repo.add_worktree("feature");

    // A `--force` duplicate whose directory then disappears out-of-band: prune
    // finds a stale entry whose branch is still live in `survivor`.
    let dup = repo.root_path().parent().unwrap().join("repo.feature-dup");
    repo.run_git(&[
        "worktree",
        "add",
        "--force",
        dup.to_str().unwrap(),
        "feature",
    ]);
    std::fs::remove_dir_all(&dup).unwrap();

    // Default `min-age`, deliberately: it holds `survivor` back as too young
    // while the stale entry is pruned regardless of age. That asymmetry is what
    // leaves a live checkout standing when the branch deletion runs — with
    // `--min-age=0s` prune would remove both worktrees, and the branch would be
    // free to delete by the time it did.
    let output = repo
        .wt_command()
        .args(["step", "prune", "--yes"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr)
        .ansi_strip()
        .into_owned();
    assert!(output.status.success(), "prune should succeed:\n{stderr}");

    let branch_exists = repo
        .git_command()
        .args(["show-ref", "--verify", "--quiet", "refs/heads/feature"])
        .run()
        .unwrap()
        .status
        .success();
    assert!(
        branch_exists,
        "prune must retain a branch another worktree still has checked out:\n{stderr}",
    );

    assert!(
        repo.git_command()
            .args(["rev-parse", "--verify", "HEAD"])
            .current_dir(&survivor)
            .run()
            .unwrap()
            .status
            .success(),
        "survivor must resolve HEAD to a commit, not a deleted branch:\n{stderr}",
    );

    // All the run took is the stale worktree entry, so that is what the summary
    // counts. Counting the candidate's kind instead would announce a branch the
    // run deliberately kept, contradicting the retention line above it.
    assert!(
        stderr.contains("Pruned 1 worktree") && !stderr.contains("Pruned 1 branch"),
        "summary must count the pruned entry, not the retained branch:\n{stderr}",
    );
}
