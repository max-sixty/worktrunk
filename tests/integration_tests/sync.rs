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

/// Rebase conflict stops sync and shows resolution instructions.
#[rstest]
fn test_sync_rebase_conflict(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&["worktree", "prune"]);
    repo.commit("initial");

    // Create a worktree that modifies the same file as main
    let pr1 = repo.add_worktree("pr1");
    repo.commit_in_worktree(&pr1, "shared.txt", "feature", "pr1: modify shared");

    // Advance main with a conflicting change to the same file
    std::fs::write(repo.root_path().join("shared.txt"), "main-v2").unwrap();
    repo.run_git(&["add", "shared.txt"]);
    repo.run_git(&["commit", "-m", "main: modify shared"]);

    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "sync", &[], Some(&pr1)));

    // Clean up the in-progress rebase so temp dir removal doesn't fail
    repo.run_git_in(&pr1, &["rebase", "--abort"]);
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
