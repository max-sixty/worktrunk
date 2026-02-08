use crate::common::{TestRepo, make_snapshot_cmd, repo};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::process::Stdio;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

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
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "prune",
        &["--pattern=feature/*", "--dry-run"],
        None
    ));
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
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "prune",
        &["--exclude=*keep*", "--dry-run"],
        None
    ));
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
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "prune",
        &["--dry-run"],
        Some(&worktree_path)
    ));
}

#[rstest]
fn test_prune_skips_default_branch(repo: TestRepo) {
    // This should always return no candidates since main is the default branch
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "prune", &["--dry-run"], None));
}

#[rstest]
fn test_prune_executes_removal(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature/to-remove");
    repo.commit_in_worktree(&worktree_path, "f.txt", "content", "Add feature");
    repo.run_git(&["switch", "main"]);
    repo.run_git(&["merge", "--ff-only", "feature/to-remove"]);

    // Execute actual removal with --yes to skip prompt
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "prune", &["--yes"], None));
}

#[rstest]
fn test_prune_force_removes_unmerged(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature/unmerged");
    repo.commit_in_worktree(&worktree_path, "f.txt", "content", "Unmerged work");
    repo.run_git(&["switch", "main"]);

    // Force removal without merging
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "prune",
        &["--force", "--yes"],
        None
    ));
}

#[rstest]
fn test_prune_combined_integrated_and_prunable(mut repo: TestRepo) {
    // Create one integrated branch (normal removal)
    let wt1 = repo.add_worktree("feature/integrated");
    repo.commit_in_worktree(&wt1, "f1.txt", "content", "Integrated feature");
    repo.run_git(&["switch", "main"]);
    repo.run_git(&["merge", "--ff-only", "feature/integrated"]);

    // Create another integrated branch and delete its directory (prunable + integrated)
    let wt2 = repo.add_worktree("feature/prunable");
    repo.commit_in_worktree(&wt2, "f2.txt", "content", "Prunable feature");
    repo.run_git(&["switch", "main"]);
    repo.run_git(&["merge", "--ff-only", "feature/prunable"]);
    std::fs::remove_dir_all(&wt2).unwrap();

    // This should handle both: normal integrated removal + prunable worktree cleanup
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "prune", &["--yes"], None));
}

#[rstest]
fn test_prune_non_interactive_error(mut repo: TestRepo) {
    // Create an integrated branch
    let worktree_path = repo.add_worktree("feature/test");
    repo.commit_in_worktree(&worktree_path, "f.txt", "content", "Test feature");
    repo.run_git(&["switch", "main"]);
    repo.run_git(&["merge", "--ff-only", "feature/test"]);

    // Run without --yes and with stdin closed (non-interactive)
    let mut cmd = repo.wt_command();
    cmd.args(["prune"]).stdin(Stdio::null());

    assert_cmd_snapshot!(cmd);
}

#[rstest]
fn test_prune_trees_match_reason(mut repo: TestRepo) {
    // Create feature branch with changes
    let wt = repo.add_worktree("feature/squashed");
    repo.commit_in_worktree(&wt, "f1.txt", "content1", "Add f1");
    repo.commit_in_worktree(&wt, "f2.txt", "content2", "Add f2");

    // Switch to main and squash merge (creates new commit, trees match)
    repo.run_git(&["switch", "main"]);
    repo.run_git(&["merge", "--squash", "feature/squashed"]);
    repo.run_git(&["commit", "-m", "Squash merge feature"]);

    // TreesMatch: branch tree == target tree, different commit history
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "prune", &["--dry-run"], None));
}

#[rstest]
fn test_prune_no_added_changes_reason(mut repo: TestRepo) {
    // Create feature branch with an empty commit (no file changes)
    let wt = repo.add_worktree("feature/empty-commits");
    repo.run_git_in(&wt, &["commit", "--allow-empty", "-m", "Empty work 1"]);
    repo.run_git_in(&wt, &["commit", "--allow-empty", "-m", "Empty work 2"]);

    // Switch to main and add a commit there (so feature is not an ancestor)
    repo.run_git(&["switch", "main"]);
    repo.commit_in_worktree(repo.root_path(), "mainfile.txt", "main", "Main work");

    // NoAddedChanges: branch has commits but no file changes from merge-base
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "prune", &["--dry-run"], None));
}

#[rstest]
fn test_prune_merge_adds_nothing_reason(mut repo: TestRepo) {
    // Create common base first
    repo.commit_in_worktree(repo.root_path(), "base.txt", "base", "Base file");

    // Create feature branch that adds and removes a file
    let wt = repo.add_worktree("feature/reverted");
    repo.commit_in_worktree(&wt, "temp.txt", "temporary", "Add temp file");
    repo.run_git_in(&wt, &["rm", "temp.txt"]);
    repo.run_git_in(&wt, &["commit", "-m", "Remove temp file"]);
    // Also modify base file
    repo.commit_in_worktree(&wt, "base.txt", "modified", "Modify base");

    // Switch to main and apply the same net change (modify base, no temp file)
    repo.run_git(&["switch", "main"]);
    repo.commit_in_worktree(
        repo.root_path(),
        "base.txt",
        "modified",
        "Modify base differently",
    );

    // MergeAddsNothing: trees differ but merge would produce target's tree
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "prune", &["--dry-run"], None));
}

#[rstest]
fn test_prune_pattern_filters_in_non_matching(mut repo: TestRepo) {
    // Create multiple integrated branches
    let wt1 = repo.add_worktree("feature/keep");
    repo.commit_in_worktree(&wt1, "f1.txt", "c1", "Keep this");

    let wt2 = repo.add_worktree("bugfix/integrated");
    repo.commit_in_worktree(&wt2, "b1.txt", "b1", "Bugfix");

    repo.run_git(&["switch", "main"]);
    repo.run_git(&["merge", "--no-ff", "--no-edit", "feature/keep"]);
    repo.run_git(&["merge", "--no-ff", "--no-edit", "bugfix/integrated"]);

    // Pattern "bugfix/*" should filter IN only bugfix branches
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "prune",
        &["--pattern=bugfix/*", "--dry-run"],
        None
    ));
}

#[rstest]
fn test_prune_exclude_filters_out_matches(mut repo: TestRepo) {
    let wt1 = repo.add_worktree("feature/wip");
    repo.commit_in_worktree(&wt1, "f1.txt", "c1", "WIP feature");

    let wt2 = repo.add_worktree("feature/done");
    repo.commit_in_worktree(&wt2, "f2.txt", "c2", "Done feature");

    repo.run_git(&["switch", "main"]);
    repo.run_git(&["merge", "--no-ff", "--no-edit", "feature/wip"]);
    repo.run_git(&["merge", "--no-ff", "--no-edit", "feature/done"]);

    // Exclude "*wip*" should filter OUT wip branches
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "prune",
        &["--exclude=*wip*", "--dry-run"],
        None
    ));
}

#[rstest]
fn test_prune_prunable_with_missing_directory(mut repo: TestRepo) {
    // Create unmerged branch and remove directory
    let wt = repo.add_worktree("feature/stale-dir");
    repo.commit_in_worktree(&wt, "f.txt", "content", "Unmerged work");
    std::fs::remove_dir_all(&wt).unwrap();

    // Prunable worktrees show even without --force (they're safe to prune)
    repo.run_git(&["switch", "main"]);
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "prune", &["--dry-run"], None));
}

#[rstest]
fn test_prune_integrated_only_no_prunable(mut repo: TestRepo) {
    // Multiple integrated branches, no prunable worktrees
    let wt1 = repo.add_worktree("feature/one");
    repo.commit_in_worktree(&wt1, "f1.txt", "c1", "Feature 1");

    let wt2 = repo.add_worktree("feature/two");
    repo.commit_in_worktree(&wt2, "f2.txt", "c2", "Feature 2");

    repo.run_git(&["switch", "main"]);
    repo.run_git(&["merge", "--no-ff", "--no-edit", "feature/one"]);
    repo.run_git(&["merge", "--no-ff", "--no-edit", "feature/two"]);

    // Only integrated, no prunable
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "prune", &["--dry-run"], None));
}

#[rstest]
fn test_prune_execute_shows_confirmation_output(mut repo: TestRepo) {
    // Create both integrated and prunable candidates
    let wt1 = repo.add_worktree("feature/integrated");
    repo.commit_in_worktree(&wt1, "f1.txt", "c1", "Feature 1");

    let wt2 = repo.add_worktree("feature/prunable");
    repo.commit_in_worktree(&wt2, "f2.txt", "c2", "Feature 2");
    std::fs::remove_dir_all(&wt2).unwrap();

    repo.run_git(&["switch", "main"]);
    repo.run_git(&["merge", "--ff-only", "feature/integrated"]);

    // Execute with --yes to see confirmation display and success reporting
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "prune", &["--yes"], None));
}

#[rstest]
fn test_prune_execute_only_integrated(mut repo: TestRepo) {
    // Only integrated branches (tests integrated-only confirmation path)
    let wt1 = repo.add_worktree("feature/one");
    repo.commit_in_worktree(&wt1, "f1.txt", "c1", "Feature 1");

    let wt2 = repo.add_worktree("feature/two");
    repo.commit_in_worktree(&wt2, "f2.txt", "c2", "Feature 2");

    repo.run_git(&["switch", "main"]);
    repo.run_git(&["merge", "--no-ff", "--no-edit", "feature/one"]);
    repo.run_git(&["merge", "--no-ff", "--no-edit", "feature/two"]);

    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "prune", &["--yes"], None));
}

#[rstest]
fn test_prune_execute_only_prunable(mut repo: TestRepo) {
    // Only prunable worktrees (tests prunable-only confirmation path)
    let wt1 = repo.add_worktree("feature/stale1");
    let wt2 = repo.add_worktree("feature/stale2");

    std::fs::remove_dir_all(&wt1).unwrap();
    std::fs::remove_dir_all(&wt2).unwrap();

    repo.run_git(&["switch", "main"]);
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "prune", &["--yes"], None));
}

#[rstest]
fn test_prune_execute_prunable_integrated(mut repo: TestRepo) {
    // Prunable worktree that's also integrated (tests branch deletion after prune)
    let wt = repo.add_worktree("feature/both");
    repo.commit_in_worktree(&wt, "f.txt", "content", "Feature work");

    // Merge it
    repo.run_git(&["switch", "main"]);
    repo.run_git(&["merge", "--ff-only", "feature/both"]);

    // Remove directory to make it prunable
    std::fs::remove_dir_all(&wt).unwrap();

    // This should prune AND delete the branch (it's both prunable and integrated)
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "prune", &["--yes"], None));
}

#[rstest]
fn test_prune_execute_with_hook_failure(mut repo: TestRepo) {
    // Create integrated branches
    let wt1 = repo.add_worktree("feature/success");
    repo.commit_in_worktree(&wt1, "f1.txt", "c1", "Success");

    let wt2 = repo.add_worktree("feature/will-fail");
    repo.commit_in_worktree(&wt2, "f2.txt", "c2", "Will fail");

    repo.run_git(&["switch", "main"]);
    repo.run_git(&["merge", "--no-ff", "--no-edit", "feature/success"]);
    repo.run_git(&["merge", "--no-ff", "--no-edit", "feature/will-fail"]);

    // Add a pre-remove hook that fails for feature/will-fail
    let hooks_dir = repo.root_path().join(".git").join("hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();
    let hook_path = hooks_dir.join("worktrunk-pre-remove");

    #[cfg(unix)]
    {
        std::fs::write(
            &hook_path,
            "#!/bin/bash\nif [ \"$1\" = \"feature/will-fail\" ]; then exit 1; fi\n",
        )
        .unwrap();
        std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(windows)]
    {
        std::fs::write(
            hook_path.with_extension("bat"),
            "@echo off\nif \"%1\"==\"feature/will-fail\" exit 1\n",
        )
        .unwrap();
    }

    // Execute - one should succeed, one should fail due to hook
    let mut cmd = repo.wt_command();
    cmd.args(["prune", "--yes"]);
    assert_cmd_snapshot!(cmd);
}
