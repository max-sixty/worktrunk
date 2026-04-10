//! Integration tests for `wt sync`

use crate::common::{TestRepo, make_snapshot_cmd, repo};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;

/// Helper: create a worktree for a branch that starts from another branch.
///
/// `add_worktree` always branches from HEAD of the main worktree (main). To
/// create a stacked branch (pr2 on top of pr1), we create the branch at the
/// desired starting point, then add a worktree for it.
fn add_stacked_worktree(
    repo: &mut TestRepo,
    branch: &str,
    start_point: &str,
) -> std::path::PathBuf {
    let safe = branch.replace('/', "-");
    let worktree_path = repo
        .root_path()
        .parent()
        .unwrap()
        .join(format!("repo.{safe}"));
    let worktree_str = worktree_path.to_str().unwrap();
    repo.run_git(&["worktree", "add", "-b", branch, worktree_str, start_point]);
    worktree_path
}

/// Set up a linear stack: main -> pr1 -> pr2 where the dependency tree is
/// unambiguous. Returns (pr1_path, pr2_path).
fn setup_linear_stack(repo: &mut TestRepo) -> (std::path::PathBuf, std::path::PathBuf) {
    let pr1 = repo.add_worktree("pr1");
    repo.commit_in_worktree(&pr1, "pr1.txt", "pr1 content", "pr1 commit");

    let pr2 = add_stacked_worktree(repo, "pr2", "pr1");
    repo.commit_in_worktree(&pr2, "pr2.txt", "pr2 content", "pr2 commit");

    (pr1, pr2)
}

/// Set up a 3-level stack: main -> pr1 -> pr2 -> pr3.
/// Returns (pr1_path, pr2_path, pr3_path).
fn setup_deep_stack(
    repo: &mut TestRepo,
) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let pr1 = repo.add_worktree("pr1");
    repo.commit_in_worktree(&pr1, "pr1.txt", "pr1 content", "pr1 commit");

    let pr2 = add_stacked_worktree(repo, "pr2", "pr1");
    repo.commit_in_worktree(&pr2, "pr2.txt", "pr2 content", "pr2 commit");

    let pr3 = add_stacked_worktree(repo, "pr3", "pr2");
    repo.commit_in_worktree(&pr3, "pr3.txt", "pr3 content", "pr3 commit");

    (pr1, pr2, pr3)
}

/// main -> pr1 -> pr2, new commit on main. Dry-run shows tree and planned
/// rebases.
#[rstest]
fn test_sync_dry_run_linear_stack(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&["worktree", "prune"]);
    repo.commit("initial");
    let (pr1, _pr2) = setup_linear_stack(&mut repo);

    // Advance main
    repo.commit("advance main");

    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "sync", &["--dry-run"], Some(&pr1)));
}

/// main -> pr1 -> pr2, commit on main, sync rebases in order.
#[rstest]
fn test_sync_main_advances(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&["worktree", "prune"]);
    repo.commit("initial");
    let (pr1, _pr2) = setup_linear_stack(&mut repo);

    // Advance main with a non-conflicting file
    repo.commit("advance main");

    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "sync", &[], Some(&pr1)));
}

/// main -> pr1 -> pr2, extra commit on pr1, sync rebases pr2 onto pr1.
#[rstest]
fn test_sync_mid_stack_change(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&["worktree", "prune"]);
    repo.commit("initial");
    let (pr1, _pr2) = setup_linear_stack(&mut repo);

    // Add another commit on pr1 (not yet in pr2)
    repo.commit_in_worktree(&pr1, "pr1b.txt", "pr1b content", "pr1 second commit");

    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "sync", &[], Some(&pr1)));
}

/// Already-synced single branch reports up-to-date.
#[rstest]
fn test_sync_up_to_date(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&["worktree", "prune"]);
    repo.commit("initial");

    let pr1 = repo.add_worktree("pr1");
    repo.commit_in_worktree(&pr1, "pr1.txt", "pr1 content", "pr1 commit");

    // No changes — everything is already synced
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "sync", &[], Some(&pr1)));
}

/// Dirty worktree blocks sync.
#[rstest]
fn test_sync_dirty_worktree_aborts(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&["worktree", "prune"]);
    repo.commit("initial");

    let pr1 = repo.add_worktree("pr1");
    repo.commit_in_worktree(&pr1, "pr1.txt", "pr1 content", "pr1 commit");

    // Advance main so there's something to sync
    repo.commit("advance main");

    // Make pr1 dirty
    std::fs::write(pr1.join("dirty.txt"), "uncommitted").unwrap();

    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "sync", &[], Some(&pr1)));
}

/// Two independent stacks, default syncs both.
#[rstest]
fn test_sync_default_syncs_all(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&["worktree", "prune"]);
    repo.commit("initial");

    // Stack A: main -> pr-a1
    let pr_a1 = repo.add_worktree("pr-a1");
    repo.commit_in_worktree(&pr_a1, "a1.txt", "a1 content", "a1 commit");

    // Stack B: main -> pr-b1
    let pr_b1 = repo.add_worktree("pr-b1");
    repo.commit_in_worktree(&pr_b1, "b1.txt", "b1 content", "b1 commit");

    // Advance main
    repo.commit("advance main");

    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "sync", &[], Some(&pr_a1)));
}

/// Two independent stacks, --stack syncs only current stack.
#[rstest]
fn test_sync_stack_flag(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&["worktree", "prune"]);
    repo.commit("initial");

    // Stack A: main -> pr-a1
    let pr_a1 = repo.add_worktree("pr-a1");
    repo.commit_in_worktree(&pr_a1, "a1.txt", "a1 content", "a1 commit");

    // Stack B: main -> pr-b1
    let pr_b1 = repo.add_worktree("pr-b1");
    repo.commit_in_worktree(&pr_b1, "b1.txt", "b1 content", "b1 commit");

    // Advance main
    repo.commit("advance main");

    // Sync from pr-a1 with --stack — should only sync stack A
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "sync", &["--stack"], Some(&pr_a1)));
}

// =========================================================================
// Plan scenarios (3-level stacks matching plan.md exactly)
// =========================================================================

/// Plan scenario 1: Update all branches after main changes.
///
/// main ─ A ─ X (new)
///        └── PR1 ─ D
///                   └── PR2 ─ F
///                              └── PR3 ─ H
///
/// `wt sync` should rebase PR1 onto main, PR2 onto PR1, PR3 onto PR2.
#[rstest]
fn test_sync_scenario1_main_advances_deep_stack(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&["worktree", "prune"]);
    repo.commit("initial");
    let (pr1, _pr2, _pr3) = setup_deep_stack(&mut repo);

    // Advance main
    repo.commit("advance main");

    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "sync", &[], Some(&pr1)));
}

/// Plan scenario 2: Commit in the middle, update the rest.
///
/// main ─ A
///        └── PR1 ─ D ─ Z (new fix)
///                   └── PR2 ─ F
///                              └── PR3 ─ H
///
/// `wt sync` detects PR2 is behind PR1, rebases PR2 and PR3.
#[rstest]
fn test_sync_scenario2_mid_stack_change_deep(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&["worktree", "prune"]);
    repo.commit("initial");
    let (pr1, _pr2, _pr3) = setup_deep_stack(&mut repo);

    // Add a new commit on pr1 (PR2 and PR3 are now stale)
    repo.commit_in_worktree(&pr1, "pr1-fix.txt", "fix content", "pr1 fix");

    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "sync", &[], Some(&pr1)));
}

/// Plan scenario 3: PR merged to main — reparent children with rebase --onto.
///
/// main ─ A ─ [PR1 squashed]
///        └── PR1 ─ D  (integrated)
///                   └── PR2 ─ F
///                              └── PR3 ─ H
///
/// `wt sync` should detect PR1 is integrated, reparent PR2 onto main using
/// `rebase --onto main pr1 pr2`, then rebase PR3 onto PR2.
#[rstest]
fn test_sync_scenario3_pr_merged_to_main(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&["worktree", "prune"]);
    repo.commit("initial");
    let (_pr1, _pr2, _pr3) = setup_deep_stack(&mut repo);

    // Simulate squash-merge of PR1 into main: apply PR1's changes onto main
    // so that integration detection sees PR1 as integrated.
    repo.run_git(&["checkout", "main"]);
    std::fs::write(repo.root_path().join("pr1.txt"), "pr1 content").unwrap();
    repo.run_git(&["add", "pr1.txt"]);
    repo.run_git(&["commit", "-m", "squash-merge pr1"]);

    // Run sync from pr2's worktree
    let pr2_path = repo.root_path().parent().unwrap().join("repo.pr2");
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "sync", &[], Some(&pr2_path)));
}

// =========================================================================
// Optional phases: --fetch, --push, --prune
// =========================================================================

/// --fetch runs git fetch before syncing.
#[rstest]
fn test_sync_fetch(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&["worktree", "prune"]);
    repo.commit("initial");
    let (pr1, _pr2) = setup_linear_stack(&mut repo);

    // Advance main so there's something to sync
    repo.commit("advance main");

    // --fetch should run git fetch first, then rebase
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "sync", &["--fetch"], Some(&pr1)));
}

/// --push force-pushes rebased branches that have an upstream.
#[rstest]
fn test_sync_push(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&["worktree", "prune"]);
    repo.commit("initial");
    let (pr1, _pr2) = setup_linear_stack(&mut repo);

    // Push pr1 with upstream tracking
    repo.run_git(&["push", "-u", "origin", "pr1"]);

    // Advance main so there's something to sync
    repo.commit("advance main");

    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "sync", &["--push"], Some(&pr1)));
}

/// --push skips branches whose remote branch was deleted.
#[rstest]
fn test_sync_push_skips_deleted_remote(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&["worktree", "prune"]);
    repo.commit("initial");
    let (pr1, _pr2) = setup_linear_stack(&mut repo);

    // Push both branches with upstream tracking
    repo.run_git(&["push", "-u", "origin", "pr1"]);
    repo.run_git(&["push", "-u", "origin", "pr2"]);

    // Delete pr2's remote branch (simulates branch deleted on GitHub after merge)
    repo.run_git(&["push", "origin", "--delete", "pr2"]);
    repo.run_git(&["fetch", "--prune"]);

    // Advance main so there's something to sync
    repo.commit("advance main");

    // Sync with --push: pr1 should push, pr2 should be skipped (no upstream)
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "sync", &["--push"], Some(&pr1)));
}

// =========================================================================
// Stack file tests
// =========================================================================

/// Sync always creates the stack file (.git/wt/stack).
#[rstest]
fn test_sync_creates_stack_file(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&["worktree", "prune"]);
    repo.commit("initial");
    let (_pr1, _pr2, _pr3) = setup_deep_stack(&mut repo);

    // Dry-run still creates the stack file (tree is built regardless)
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "sync",
        &["--dry-run"],
        Some(&_pr1)
    ));

    // Verify the stack file was created with correct content
    let stack_file = repo.root_path().join(".git").join("wt").join("stack");
    assert!(stack_file.exists(), "stack file should exist");
    let content = std::fs::read_to_string(&stack_file).unwrap();
    assert!(content.contains("pr1"), "stack file should contain pr1");
    assert!(content.contains("pr2"), "stack file should contain pr2");
    assert!(content.contains("pr3"), "stack file should contain pr3");
}

/// Stack file overrides inference: scenario 2 (mid-stack commit) works
/// correctly when the stack file defines the tree.
#[rstest]
fn test_sync_stack_file_fixes_scenario2(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&["worktree", "prune"]);
    repo.commit("initial");
    let (pr1, _pr2, _pr3) = setup_deep_stack(&mut repo);

    // First sync to rebase everything (auto-creates stack file)
    let mut cmd = make_snapshot_cmd(&repo, "sync", &[], Some(&pr1));
    cmd.output().unwrap();

    // Now add a mid-stack commit on pr1 (scenario 2)
    repo.commit_in_worktree(&pr1, "pr1-fix.txt", "fix content", "pr1 fix");

    // With stack file, pr2 should rebase onto pr1 (not main)
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "sync", &[], Some(&pr1)));
}

/// PR merged into non-default branch: PR2 squash-merged into PR1 (not main).
///
/// main ─ A
///        └── PR1 ─ D ─ [PR2 squashed into PR1]
///                   └── PR2 ─ F  (integrated into PR1)
///                              └── PR3 ─ H
///
/// With a stack file, `wt sync` should detect PR2 is integrated into PR1 and
/// reparent PR3 onto PR1 (not main).
#[rstest]
fn test_sync_pr_merged_into_non_default_branch(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&["worktree", "prune"]);
    repo.commit("initial");
    let (pr1, _pr2, _pr3) = setup_deep_stack(&mut repo);

    // First sync auto-creates the stack file
    let mut cmd = make_snapshot_cmd(&repo, "sync", &[], Some(&pr1));
    cmd.output().unwrap();

    // Simulate squash-merge of PR2 into PR1: apply PR2's changes onto PR1
    std::fs::write(pr1.join("pr2.txt"), "pr2 content").unwrap();
    repo.run_git_in(&pr1, &["add", "pr2.txt"]);
    repo.run_git_in(&pr1, &["commit", "-m", "squash-merge pr2 into pr1"]);

    // Run sync — should detect PR2 is integrated into PR1 and reparent PR3 onto PR1
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "sync", &[], Some(&pr1)));
}

/// PR3 squash-merged into its parent PR2 (mid-stack, not main).
///
/// main ─ A
///        └── PR1
///              └── PR2 ─ [PR3 squashed into PR2]
///                    └── PR3 (integrated into PR2)
///                          └── PR4
///
/// PR3's parent is PR2 (not main). After PR3 is squash-merged into PR2,
/// `wt sync` should detect PR3 is integrated and reparent PR4 onto PR2.
#[rstest]
fn test_sync_mid_stack_pr_merged_into_parent(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&["worktree", "prune"]);
    repo.commit("initial");

    // Build 4-level stack: main -> pr1 -> pr2 -> pr3 -> pr4
    let pr1 = repo.add_worktree("pr1");
    repo.commit_in_worktree(&pr1, "pr1.txt", "pr1 content", "pr1 commit");
    let pr2 = add_stacked_worktree(&mut repo, "pr2", "pr1");
    repo.commit_in_worktree(&pr2, "pr2.txt", "pr2 content", "pr2 commit");
    let pr3 = add_stacked_worktree(&mut repo, "pr3", "pr2");
    repo.commit_in_worktree(&pr3, "pr3.txt", "pr3 content", "pr3 commit");
    let pr4 = add_stacked_worktree(&mut repo, "pr4", "pr3");
    repo.commit_in_worktree(&pr4, "pr4.txt", "pr4 content", "pr4 commit");

    // First sync to auto-create stack file
    let mut cmd = make_snapshot_cmd(&repo, "sync", &[], Some(&pr1));
    cmd.output().unwrap();

    // Simulate squash-merge of PR3 into PR2
    std::fs::write(pr2.join("pr3.txt"), "pr3 content").unwrap();
    repo.run_git_in(&pr2, &["add", "pr3.txt"]);
    repo.run_git_in(&pr2, &["commit", "-m", "squash-merge pr3 into pr2"]);

    // Sync should detect PR3 integrated into PR2, reparent PR4 onto PR2
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "sync", &[], Some(&pr1)));
}

/// Cascading merge: PR2 merged into PR1, then PR1 merged into main.
///
/// Step 1: PR2 squash-merged into PR1 → PR3 reparents onto PR1
/// Step 2: PR1 squash-merged into main → PR3 reparents onto main
///
/// Tests that sync handles sequential merges correctly.
#[rstest]
fn test_sync_cascading_merges(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&["worktree", "prune"]);
    repo.commit("initial");
    let (pr1, _pr2, _pr3) = setup_deep_stack(&mut repo);

    // First sync to auto-create stack file
    let mut cmd = make_snapshot_cmd(&repo, "sync", &[], Some(&pr1));
    cmd.output().unwrap();

    // Step 1: Squash-merge PR2 into PR1
    std::fs::write(pr1.join("pr2.txt"), "pr2 content").unwrap();
    repo.run_git_in(&pr1, &["add", "pr2.txt"]);
    repo.run_git_in(&pr1, &["commit", "-m", "squash-merge pr2 into pr1"]);

    // Sync after first merge — PR3 should reparent onto PR1
    let mut cmd = make_snapshot_cmd(&repo, "sync", &[], Some(&pr1));
    cmd.output().unwrap();

    // Step 2: Squash-merge PR1 into main
    repo.run_git(&["checkout", "main"]);
    std::fs::write(repo.root_path().join("pr1.txt"), "pr1 content").unwrap();
    std::fs::write(repo.root_path().join("pr2.txt"), "pr2 content").unwrap();
    repo.run_git(&["add", "."]);
    repo.run_git(&["commit", "-m", "squash-merge pr1 into main"]);

    // Sync after second merge — PR3 should reparent onto main
    let pr3_path = repo.root_path().parent().unwrap().join("repo.pr3");
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "sync", &[], Some(&pr3_path)));
}

/// --prune removes integrated worktrees after syncing.
#[rstest]
fn test_sync_prune(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&["worktree", "prune"]);
    repo.commit("initial");
    let (_pr1, _pr2, _pr3) = setup_deep_stack(&mut repo);

    // Simulate squash-merge of PR1 into main
    repo.run_git(&["checkout", "main"]);
    std::fs::write(repo.root_path().join("pr1.txt"), "pr1 content").unwrap();
    repo.run_git(&["add", "pr1.txt"]);
    repo.run_git(&["commit", "-m", "squash-merge pr1"]);

    let pr2_path = repo.root_path().parent().unwrap().join("repo.pr2");
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "sync",
        &["--prune"],
        Some(&pr2_path)
    ));
}
