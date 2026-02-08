use crate::common::{make_snapshot_cmd, repo, TestRepo};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;

#[rstest]
fn test_prune_no_candidates(repo: TestRepo) {
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "prune", &["--dry-run"], None));
}

#[rstest]
fn test_prune_integrated(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature/merged");
    repo.commit_in_worktree(&worktree_path, "f.txt", "content", "Add feature");
    repo.run_git(&["switch", "main"]);
    repo.run_git(&["merge", "--ff-only", "feature/merged"]);
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "prune", &["--dry-run"], None));
}

#[rstest]
fn test_prune_prunable_worktrees(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature/stale");
    // Remove the worktree directory to make it prunable
    std::fs::remove_dir_all(&worktree_path).unwrap();
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "prune", &["--dry-run"], None));
}

#[rstest]
fn test_prune_respects_locks(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature/locked");
    repo.commit_in_worktree(&worktree_path, "f.txt", "content", "Add feature");
    repo.run_git(&["switch", "main"]);
    repo.run_git(&["merge", "--ff-only", "feature/locked"]);

    // Lock the worktree (use the worktree path, not branch name)
    let lock_path = worktree_path.to_string_lossy();
    repo.run_git(&["worktree", "lock", &lock_path]);

    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "prune", &["--dry-run"], None));
}

#[rstest]
fn test_prune_pattern_filtering(mut repo: TestRepo) {
    // Create multiple branches all from the same starting point
    // This way they can be fast-forward merged independently
    let feature1 = repo.add_worktree("feature/auth");
    repo.commit_in_worktree(&feature1, "f1.txt", "content", "Add auth");

    let feature2 = repo.add_worktree("feature/ui");
    repo.commit_in_worktree(&feature2, "f2.txt", "content", "Add UI");

    let hotfix = repo.add_worktree("hotfix/critical");
    repo.commit_in_worktree(&hotfix, "h.txt", "content", "Fix bug");

    // Switch to main and merge all three
    repo.run_git(&["switch", "main"]);
    repo.run_git(&["merge", "--no-ff", "--no-edit", "feature/auth"]);
    repo.run_git(&["merge", "--no-ff", "--no-edit", "feature/ui"]);
    repo.run_git(&["merge", "--no-ff", "--no-edit", "hotfix/critical"]);

    // Test pattern filtering - should show only feature/* branches
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "prune", &["--pattern=feature/*", "--dry-run"], None));
}

#[rstest]
fn test_prune_exclude_patterns(mut repo: TestRepo) {
    let feature1 = repo.add_worktree("feature/keep");
    repo.commit_in_worktree(&feature1, "f1.txt", "content", "Keep this");

    let feature2 = repo.add_worktree("feature/remove");
    repo.commit_in_worktree(&feature2, "f2.txt", "content", "Remove this");

    // Merge both
    repo.run_git(&["switch", "main"]);
    repo.run_git(&["merge", "--no-ff", "--no-edit", "feature/keep"]);
    repo.run_git(&["merge", "--no-ff", "--no-edit", "feature/remove"]);

    // Should show only feature/remove, excluding feature/keep
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "prune", &["--exclude=*keep*", "--dry-run"], None));
}

#[rstest]
fn test_prune_skips_current_branch(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature/current");
    repo.commit_in_worktree(&worktree_path, "f.txt", "content", "Add feature");

    // Switch to main and merge
    repo.run_git(&["switch", "main"]);
    repo.run_git(&["merge", "--ff-only", "feature/current"]);

    // Prune from the merged branch's worktree (not via switch, which would fail)
    // The current branch protection should be based on the worktree we're running from
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "prune", &["--dry-run"], Some(&worktree_path)));
}

#[rstest]
fn test_prune_skips_default_branch(repo: TestRepo) {
    // This should always return no candidates since main is the default branch
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "prune", &["--dry-run"], None));
}
