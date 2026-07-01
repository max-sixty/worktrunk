//! Integration tests for `wt step relocate`

use crate::common::{
    TestRepo, configure_directive_files, directive_files, make_snapshot_cmd, repo,
};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::fs;
use std::path::Path;

/// Get the parent directory of the repo (where worktrees are created)
fn worktree_parent(repo: &TestRepo) -> std::path::PathBuf {
    repo.root_path().parent().unwrap().to_path_buf()
}

/// Test with no mismatched worktrees
#[rstest]
fn test_relocate_no_mismatches(mut repo: TestRepo) {
    // Create a worktree at the expected location
    repo.add_worktree("feature");

    // All worktrees should be at expected paths
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "step", &["relocate"], None));
}

/// Test relocating a single mismatched worktree
#[rstest]
fn test_relocate_single_mismatch(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // Create a worktree manually at a non-standard location
    let wrong_path = parent.join("wrong-location");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "feature",
        wrong_path.to_str().unwrap(),
    ]);

    // Relocate should move it to the expected path
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "step", &["relocate"], None));

    // Verify the worktree was moved to expected location
    let expected_path = parent.join("repo.feature");
    assert!(
        expected_path.exists(),
        "Worktree should be at expected path: {}",
        expected_path.display()
    );
    assert!(
        !wrong_path.exists(),
        "Old worktree path should no longer exist: {}",
        wrong_path.display()
    );
}

/// Test dry run shows what would be moved
#[rstest]
fn test_relocate_dry_run(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // Create a worktree at a non-standard location
    let wrong_path = parent.join("wrong-location");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "feature",
        wrong_path.to_str().unwrap(),
    ]);

    // Dry run should show what would be moved without actually moving
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["relocate", "--dry-run"],
        None
    ));

    // Verify the worktree was NOT moved
    assert!(
        wrong_path.exists(),
        "Worktree should still be at wrong path in dry run: {}",
        wrong_path.display()
    );
}

/// Test that locked worktrees are skipped
#[rstest]
fn test_relocate_locked_worktree(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // Create a worktree at a non-standard location and lock it
    let wrong_path = parent.join("wrong-location");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "feature",
        wrong_path.to_str().unwrap(),
    ]);
    repo.run_git(&["worktree", "lock", wrong_path.to_str().unwrap()]);

    // Relocate should skip locked worktree
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "step", &["relocate"], None));

    // Verify the worktree was NOT moved
    assert!(
        wrong_path.exists(),
        "Locked worktree should not be moved: {}",
        wrong_path.display()
    );
}

/// Test mixed success and skip (covers "Relocated X, skipped Y" output)
#[rstest]
fn test_relocate_mixed_success_and_skip(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // Create one worktree that can be moved
    let wrong_path1 = parent.join("wrong-location-1");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "feature1",
        wrong_path1.to_str().unwrap(),
    ]);

    // Create another worktree that is locked (will be skipped)
    let wrong_path2 = parent.join("wrong-location-2");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "feature2",
        wrong_path2.to_str().unwrap(),
    ]);
    repo.run_git(&["worktree", "lock", wrong_path2.to_str().unwrap()]);

    // Relocate should move feature1 and skip feature2
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "step", &["relocate"], None));

    // Verify feature1 was moved
    let expected_path1 = parent.join("repo.feature1");
    assert!(
        expected_path1.exists(),
        "feature1 should be at expected path: {}",
        expected_path1.display()
    );

    // Verify feature2 was NOT moved (locked)
    assert!(
        wrong_path2.exists(),
        "Locked feature2 should not be moved: {}",
        wrong_path2.display()
    );
}

/// Test that existing target path causes skip
#[rstest]
fn test_relocate_target_exists(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // Create a worktree at a non-standard location
    let wrong_path = parent.join("wrong-location");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "feature",
        wrong_path.to_str().unwrap(),
    ]);

    // Create a directory at the expected location
    let expected_path = parent.join("repo.feature");
    fs::create_dir_all(&expected_path).unwrap();
    fs::write(expected_path.join("existing-file.txt"), "existing").unwrap();

    // Relocate should skip because target exists
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "step", &["relocate"], None));

    // Verify the worktree was NOT moved
    assert!(
        wrong_path.exists(),
        "Worktree should not be moved when target exists: {}",
        wrong_path.display()
    );
}

/// Test that dirty linked worktrees relocate cleanly without --commit.
///
/// `git worktree move` carries modified-tracked and untracked files along
/// with the worktree, so there's no reason to require a clean state. Issue
/// #3103.
#[rstest]
fn test_relocate_dirty_without_commit(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // Create a worktree at a non-standard location
    let wrong_path = parent.join("wrong-location");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "feature",
        wrong_path.to_str().unwrap(),
    ]);

    // Make uncommitted changes - both modified and untracked
    fs::write(wrong_path.join("dirty.txt"), "untracked file").unwrap();
    // Modify a tracked file too (initial test commit creates README.md or similar).
    let tracked = wrong_path.join("modified-tracked.txt");
    fs::write(&tracked, "first").unwrap();
    repo.git_command()
        .args([
            "-C",
            wrong_path.to_str().unwrap(),
            "add",
            "modified-tracked.txt",
        ])
        .run()
        .unwrap();
    repo.git_command()
        .args([
            "-C",
            wrong_path.to_str().unwrap(),
            "commit",
            "-m",
            "add tracked",
        ])
        .run()
        .unwrap();
    fs::write(&tracked, "second").unwrap();

    // Relocate should move the dirty worktree
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "step", &["relocate"], None));

    // Verify the worktree was moved to its expected location, carrying both
    // the untracked and modified-tracked files with it.
    let expected_path = parent.join("repo.feature");
    assert!(
        expected_path.exists(),
        "Dirty worktree should be moved: {}",
        expected_path.display()
    );
    assert!(
        !wrong_path.exists(),
        "Old worktree path should no longer exist: {}",
        wrong_path.display()
    );
    assert!(
        expected_path.join("dirty.txt").exists(),
        "Untracked file should travel with the worktree",
    );
    assert_eq!(
        fs::read_to_string(expected_path.join("modified-tracked.txt")).unwrap(),
        "second",
        "Modified tracked file content should travel with the worktree",
    );
}

/// Test that a dirty main worktree is still skipped — its relocation runs
/// `git checkout <default-branch>` which refuses to switch over uncommitted
/// changes.
#[rstest]
fn test_relocate_dirty_main_worktree_skipped(repo: TestRepo) {
    let parent = worktree_parent(&repo);
    let repo_path = repo.root_path().to_path_buf();

    // Switch main worktree to a feature branch so it becomes a relocation
    // candidate (expected path = repo.feature, not repo).
    repo.run_git(&["checkout", "-b", "feature"]);

    // Make uncommitted changes in main worktree
    fs::write(repo_path.join("dirty.txt"), "uncommitted").unwrap();

    // Relocate should skip the dirty main worktree
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "step", &["relocate"], None));

    // Main worktree stays put
    let expected_path = parent.join("repo.feature");
    assert!(
        !expected_path.exists(),
        "Dirty main worktree should not be relocated: {}",
        expected_path.display()
    );
}

/// Test that --commit auto-commits dirty worktrees before relocating
#[rstest]
fn test_relocate_dirty_with_commit(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // Create a worktree at a non-standard location
    let wrong_path = parent.join("wrong-location");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "feature",
        wrong_path.to_str().unwrap(),
    ]);

    // Make uncommitted changes
    fs::write(wrong_path.join("dirty.txt"), "uncommitted changes").unwrap();

    // Configure mock LLM command via config file
    let worktrunk_config = r#"
[commit.generation]
command = "cat >/dev/null && echo 'chore: auto-commit before relocate'"
"#;
    fs::write(repo.test_config_path(), worktrunk_config).unwrap();

    // Relocate with --commit should commit then move
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["relocate", "--commit"],
        None
    ));

    // Verify the worktree was moved to expected location
    let expected_path = parent.join("repo.feature");
    assert!(
        expected_path.exists(),
        "Worktree should be at expected path after commit: {}",
        expected_path.display()
    );
    assert!(
        !wrong_path.exists(),
        "Old worktree path should no longer exist: {}",
        wrong_path.display()
    );
}

/// Test that --clobber backs up non-worktree paths at target locations
#[rstest]
fn test_relocate_clobber_backs_up(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // Create a worktree at a non-standard location
    let wrong_path = parent.join("wrong-location");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "feature",
        wrong_path.to_str().unwrap(),
    ]);

    // Create a directory at the expected location (non-worktree blocker)
    let expected_path = parent.join("repo.feature");
    fs::create_dir_all(&expected_path).unwrap();
    fs::write(expected_path.join("existing-file.txt"), "existing content").unwrap();

    // Relocate with --clobber should backup and move
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["relocate", "--clobber"],
        None
    ));

    // Verify the worktree was moved
    assert!(
        expected_path.exists(),
        "Worktree should be at expected location: {}",
        expected_path.display()
    );
    assert!(
        !wrong_path.exists(),
        "Original path should no longer exist: {}",
        wrong_path.display()
    );

    // Verify backup exists (with timestamp suffix)
    let backup_exists = fs::read_dir(&parent)
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.starts_with("repo.feature.bak.")
        });
    assert!(backup_exists, "Backup directory should exist");
}

/// Regression: when the computed backup path already exists, relocate
/// --clobber falls back to the next free `-N` name rather than overwriting it.
/// (Matches the `wt switch --clobber` contract — see
/// test_switch_clobber_falls_back_when_backup_taken.)
#[rstest]
fn test_relocate_clobber_falls_back_when_backup_taken(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // Create a worktree at a non-standard location.
    let wrong_path = parent.join("wrong-location");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "feature",
        wrong_path.to_str().unwrap(),
    ]);

    // Blocker file at the expected destination.
    let expected_path = parent.join("repo.feature");
    fs::write(&expected_path, "blocker contents").unwrap();

    // Pre-create the backup path relocate would compute. TEST_EPOCH pins the
    // timestamp suffix so this name is deterministic.
    // TEST_EPOCH=1735776000 -> 2025-01-02 00:00:00 UTC
    let taken = parent.join("repo.feature.bak.20250102-000000");
    fs::write(&taken, "existing backup").unwrap();

    let output = make_snapshot_cmd(&repo, "step", &["relocate", "--clobber"], None)
        .output()
        .expect("relocate should run");
    assert!(
        output.status.success(),
        "relocate must fall back to a free backup name; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The worktree was moved to the expected location.
    assert!(
        expected_path.is_dir(),
        "Worktree should be at expected location: {}",
        expected_path.display()
    );

    // The pre-existing backup is untouched; the blocker moved to the -2 name.
    assert_eq!(
        fs::read_to_string(&taken).unwrap(),
        "existing backup",
        "existing backup must not be overwritten"
    );
    let fallback = parent.join("repo.feature.bak.20250102-000000-2");
    assert_eq!(
        fs::read_to_string(&fallback).unwrap(),
        "blocker contents",
        "blocker file must move to the -2 fallback name"
    );
}

/// Test that --clobber refuses to clobber an existing worktree
#[rstest]
fn test_relocate_clobber_refuses_worktree(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // Create worktree alpha at a non-standard location
    let wrong_path = parent.join("wrong-location");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "alpha",
        wrong_path.to_str().unwrap(),
    ]);

    // Create another worktree beta at alpha's expected location
    let expected_path = parent.join("repo.alpha");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "beta",
        expected_path.to_str().unwrap(),
    ]);

    // Relocate with --clobber should still skip (can't clobber a worktree)
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["relocate", "--clobber", "alpha"],
        None
    ));

    // Verify alpha was NOT moved (beta still occupies the target)
    assert!(
        wrong_path.exists(),
        "alpha should still be at wrong location: {}",
        wrong_path.display()
    );
}

/// Test relocating specific worktrees by branch name
#[rstest]
fn test_relocate_specific_branch(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // Create two worktrees at non-standard locations
    let wrong_path1 = parent.join("wrong-location-1");
    let wrong_path2 = parent.join("wrong-location-2");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "feature1",
        wrong_path1.to_str().unwrap(),
    ]);
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "feature2",
        wrong_path2.to_str().unwrap(),
    ]);

    // Relocate only feature1
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["relocate", "feature1"],
        None
    ));

    // Verify only feature1 was moved
    let expected_path1 = parent.join("repo.feature1");
    assert!(
        expected_path1.exists(),
        "feature1 should be at expected path: {}",
        expected_path1.display()
    );
    assert!(
        wrong_path2.exists(),
        "feature2 should still be at wrong path: {}",
        wrong_path2.display()
    );
}

/// Test relocating main worktree with non-default branch (create + switch)
#[rstest]
fn test_relocate_main_worktree(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // Switch main worktree to a feature branch
    repo.run_git(&["checkout", "-b", "feature"]);

    // Relocate should create worktree for feature and switch main to default branch
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "step", &["relocate"], None));

    // Verify new worktree was created
    let expected_path = parent.join("repo.feature");
    assert!(
        expected_path.exists(),
        "Feature worktree should be created at: {}",
        expected_path.display()
    );

    // Verify main worktree is now on default branch
    let output = repo
        .git_command()
        .args(["branch", "--show-current"])
        .run()
        .unwrap();
    let current_branch = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        current_branch.trim(),
        "main",
        "Main worktree should be on default branch"
    );
}

/// Regression: a branch literally named `-foo` (creatable via `git
/// update-ref refs/heads/-foo HEAD`) must round-trip through main worktree
/// relocation without `git worktree add` parsing the ref as a flag.
/// Without `--end-of-options`, the `worktree add` call would fail with
/// `unknown switch 'o'`.
#[rstest]
fn test_relocate_main_worktree_hyphen_prefixed_branch(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // `git checkout -b -- -foo` is rejected by modern git, but `update-ref`
    // happily writes the ref, then `symbolic-ref` moves HEAD onto it.
    repo.run_git(&["update-ref", "refs/heads/-foo", "HEAD"]);
    repo.run_git(&["symbolic-ref", "HEAD", "refs/heads/-foo"]);

    let output = repo
        .wt_command()
        .args(["step", "relocate"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "relocate must succeed with a hyphen-prefixed branch; \
         stdout: {stdout}\nstderr: {stderr}"
    );

    let expected_path = parent.join("repo.-foo");
    assert!(
        expected_path.exists(),
        "worktree for `-foo` should be created at: {}",
        expected_path.display()
    );
}

/// Test swap scenario: two worktrees at each other's expected locations
///
/// When alpha is at repo.beta and beta is at repo.alpha, relocate
/// automatically handles the swap via a temporary location.
#[rstest]
fn test_relocate_swap(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // Create worktrees at each other's expected locations
    // alpha at repo.beta (where beta should go)
    // beta at repo.alpha (where alpha should go)
    let path_for_beta = parent.join("repo.beta");
    let path_for_alpha = parent.join("repo.alpha");

    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "alpha",
        path_for_beta.to_str().unwrap(), // alpha at beta's expected location
    ]);
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "beta",
        path_for_alpha.to_str().unwrap(), // beta at alpha's expected location
    ]);

    // Relocate resolves the swap via temp location
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "step", &["relocate"], None));

    // Verify both are now at their expected locations
    assert!(path_for_alpha.exists(), "alpha should be at repo.alpha");
    assert!(path_for_beta.exists(), "beta should be at repo.beta");
}

/// Test relocating multiple worktrees shows compact output
#[rstest]
fn test_relocate_multiple(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // Create 5 worktrees at non-standard locations
    for i in 1..=5 {
        let wrong_path = parent.join(format!("wrong-{i}"));
        repo.run_git(&[
            "worktree",
            "add",
            "-b",
            &format!("feature-{i}"),
            wrong_path.to_str().unwrap(),
        ]);
    }

    // Relocate all
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "step", &["relocate"], None));

    // Verify all were moved
    for i in 1..=5 {
        let expected_path = parent.join(format!("repo.feature-{i}"));
        assert!(
            expected_path.exists(),
            "feature-{i} should be at expected path: {}",
            expected_path.display()
        );
    }
}

/// Test that two worktrees targeting the same path doesn't panic
///
/// Before the fix, this would panic with "existing target must be a tracked worktree"
/// because after the first worktree moved, the second would find an occupied target
/// that wasn't in the tracking map.
#[rstest]
fn test_relocate_same_target_no_panic(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // Create two worktrees at non-standard locations
    let wrong_path1 = parent.join("wrong-location-1");
    let wrong_path2 = parent.join("wrong-location-2");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "alpha",
        wrong_path1.to_str().unwrap(),
    ]);
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "beta",
        wrong_path2.to_str().unwrap(),
    ]);

    // Configure a template that maps BOTH branches to the same path
    // This creates the "same target" scenario
    let worktrunk_config = r#"
worktree-path = "{{ repo }}.shared"
"#;
    fs::write(repo.test_config_path(), worktrunk_config).unwrap();

    // Relocate only alpha and beta (exclude any other branches from prior tests)
    // Previously this would panic
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["relocate", "alpha", "beta"],
        None
    ));

    // Verify first worktree moved to shared location
    // Note: {{ repo }} in template uses repo NAME, so path is inside repo root
    let shared_path = repo.root_path().join("repo.shared");
    assert!(
        shared_path.exists(),
        "First worktree should be at shared path: {}",
        shared_path.display()
    );

    // Second worktree should still be at its original location (skipped)
    // It was skipped because the target was occupied after first moved there
    assert!(
        wrong_path1.exists() || wrong_path2.exists(),
        "One worktree should remain at original location (skipped)"
    );
}

/// Test that template expansion errors are reported gracefully
#[rstest]
fn test_relocate_template_error(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // Create a worktree at a non-standard location
    let wrong_path = parent.join("wrong-location");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "feature",
        wrong_path.to_str().unwrap(),
    ]);

    // Configure an invalid template with a non-existent variable
    let worktrunk_config = r#"
worktree-path = "{{ nonexistent_variable }}"
"#;
    fs::write(repo.test_config_path(), worktrunk_config).unwrap();

    // Relocate should warn about template error and skip
    // Filter to just "feature" to avoid noise from other worktrees
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["relocate", "feature"],
        None
    ));

    // Verify the worktree was NOT moved (skipped due to template error)
    assert!(
        wrong_path.exists(),
        "Worktree should not be moved when template fails: {}",
        wrong_path.display()
    );
}

/// Regression test: main worktree relocation must surface a failed
/// `git checkout <default_branch>` rather than silently claiming success.
///
/// Setup engineers a state where `worktrunk.default-branch` is set to a
/// branch that does not exist locally. `Repository::default_branch()`
/// trusts the persisted value (validation happens downstream), so
/// `wt step relocate` proceeds into `move_main_worktree`, which tries
/// `git checkout <nonexistent-branch>`. Before the fix, `Cmd::run()`
/// returned `Ok(Output { status: non-zero, .. })` and the `?` operator
/// didn't propagate it, so relocate printed "Relocated main ..." even
/// though nothing happened.
///
/// After the fix: non-zero exit bails with the git stderr, exit code is
/// non-zero, and the main worktree stays at its original path.
#[rstest]
fn test_relocate_main_worktree_checkout_failure_surfaces(repo: TestRepo) {
    let parent = worktree_parent(&repo);
    let repo_path = repo.root_path().to_path_buf();

    // Switch main worktree to a non-default branch so it becomes a
    // relocation candidate (expected path = repo.feature, not repo).
    repo.run_git(&["checkout", "-b", "feature"]);

    // Point worktrunk's default-branch cache at a branch that doesn't
    // resolve locally. `default_branch()` now returns this value without
    // validating it, so relocate's preflight does NOT bail and the main
    // worktree code path runs `git checkout nonexistent-branch-xyz`.
    repo.run_git(&[
        "config",
        "worktrunk.default-branch",
        "nonexistent-branch-xyz",
    ]);

    let output = repo
        .wt_command()
        .args(["step", "relocate"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "relocate must fail when checkout of default branch fails; \
         stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        !stderr.contains("Relocated"),
        "relocate must not claim success after a failed checkout; \
         stdout: {stdout}\nstderr: {stderr}"
    );

    // Main worktree is untouched - still at repo_path, still on feature.
    assert!(
        repo_path.exists(),
        "main worktree path should still exist: {}",
        repo_path.display()
    );
    let expected_path = parent.join("repo.feature");
    assert!(
        !expected_path.exists(),
        "relocate must not create the new worktree path after checkout \
         failure: {}",
        expected_path.display()
    );

    let branch_output = repo
        .git_command()
        .args(["branch", "--show-current"])
        .run()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&branch_output.stdout).trim(),
        "feature",
        "main worktree branch should be unchanged after failed checkout"
    );
}

/// Test that empty default branch is detected early with actionable error.
///
/// Engineers a state where detection genuinely fails (no remote, no
/// standard branch names, no init.defaultBranch) so `default_branch()`
/// returns None — relocate's preflight bails with a clear setup hint.
#[rstest]
fn test_relocate_empty_default_branch(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // Create a worktree at a non-standard location on a branch with a
    // non-standard name, then rename `main` to another non-standard name
    // and remove the remote. With no remote, no main/master/develop/trunk,
    // and no init.defaultBranch, detection has nothing to go on.
    let wrong_path = parent.join("wrong-location");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "feature",
        wrong_path.to_str().unwrap(),
    ]);
    repo.run_git(&["branch", "-m", "main", "trunk-a"]);
    repo.run_git(&["remote", "remove", "origin"]);

    // Relocate should fail early with helpful error
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "step", &["relocate"], None));
}

/// `step relocate --dry-run --format=json` lists planned moves with from/to paths.
#[rstest]
fn test_relocate_dry_run_json(repo: TestRepo) {
    let parent = worktree_parent(&repo);
    let wrong_path = parent.join("wrong-location");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "feature",
        wrong_path.to_str().unwrap(),
    ]);

    let output = repo
        .wt_command()
        .args(["step", "relocate", "--dry-run", "--format=json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "relocate dry-run JSON should succeed"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");

    assert_eq!(parsed["dry_run"], true);
    let entries = parsed["entries"].as_array().expect("entries array");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["branch"], "feature");
    assert!(
        entries[0]["from"]
            .as_str()
            .unwrap()
            .ends_with("wrong-location")
    );
    assert!(entries[0]["to"].as_str().unwrap().ends_with("repo.feature"));

    assert_eq!(
        parsed["skipped"].as_array().expect("skipped array").len(),
        0
    );

    // Dry run did not move
    assert!(wrong_path.exists());
}

/// `step relocate --format=json` after execution emits per-branch records and
/// distinguishes relocated vs skipped (with stable `reason` codes).
#[rstest]
fn test_relocate_json_with_skip(repo: TestRepo) {
    let repo_path = repo.root_path().to_path_buf();
    // Switch the main worktree to a feature branch so it becomes a relocation
    // candidate, then dirty it. A dirty main worktree is skipped because its
    // relocation runs `git checkout`, which won't switch over dirty state.
    repo.run_git(&["checkout", "-b", "feature"]);
    fs::write(repo_path.join("dirty.txt"), "uncommitted").unwrap();

    let output = repo
        .wt_command()
        .args(["step", "relocate", "--format=json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "relocate JSON should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");

    assert_eq!(parsed["dry_run"], false);
    let entries = parsed["entries"].as_array().expect("entries array");
    assert_eq!(entries.len(), 0);

    let skipped = parsed["skipped"].as_array().expect("skipped array");
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0]["branch"], "feature");
    assert_eq!(skipped[0]["reason"], "uncommitted");
}

/// `step relocate --format=json` after a successful execution emits the
/// per-branch `entries` array with `from` / `to` paths.
#[rstest]
fn test_relocate_executes_json(repo: TestRepo) {
    let parent = worktree_parent(&repo);
    let wrong_path = parent.join("wrong-location");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "feature",
        wrong_path.to_str().unwrap(),
    ]);

    let output = repo
        .wt_command()
        .args(["step", "relocate", "--format=json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "relocate JSON should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");

    assert_eq!(parsed["dry_run"], false);
    let entries = parsed["entries"].as_array().expect("entries array");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["branch"], "feature");
    assert!(
        entries[0]["from"]
            .as_str()
            .unwrap()
            .ends_with("wrong-location")
    );
    assert!(entries[0]["to"].as_str().unwrap().ends_with("repo.feature"));

    // The actual move should have happened.
    assert!(!wrong_path.exists());
    assert!(parent.join("repo.feature").exists());
}

/// `step relocate --format=json` surfaces template-expansion failures as
/// `skipped` entries with `reason: "template_error"` rather than silently
/// reporting an empty success — automation needs to detect a broken config.
#[rstest]
fn test_relocate_template_error_json(repo: TestRepo) {
    let parent = worktree_parent(&repo);
    // Add a worktree so something exists to evaluate the template against.
    let wrong_path = parent.join("wrong-location");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "feature",
        wrong_path.to_str().unwrap(),
    ]);

    // Reference an undefined template variable to force expansion failure.
    let worktrunk_config = r#"
worktree-path = "../{{ undefined_var }}.{{ branch }}"
"#;
    fs::write(repo.test_config_path(), worktrunk_config).unwrap();

    let output = repo
        .wt_command()
        .args(["step", "relocate", "--format=json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "relocate JSON should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");

    let skipped = parsed["skipped"].as_array().expect("skipped array");
    assert!(
        skipped.iter().any(|s| s["reason"] == "template_error"),
        "template_error skip missing from JSON: {parsed}"
    );
}

/// Relocating a worktree the user is standing inside preserves their
/// subdirectory position, routing the `cd` through the same
/// `resolve_subdir_in_target` helper as `switch`/`remove` (issue #3343 unify).
#[rstest]
fn test_relocate_preserves_subdir(repo: TestRepo) {
    let parent = worktree_parent(&repo);
    let (cd_path, exec_path, _guard) = directive_files();

    // Create a worktree at a non-standard location, with a subdirectory the
    // user is working in.
    let wrong_path = parent.join("wrong-location");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "feature",
        wrong_path.to_str().unwrap(),
    ]);
    let subdir = Path::new("apps").join("gateway");
    fs::create_dir_all(wrong_path.join(&subdir)).unwrap();

    let mut cmd = repo.wt_command();
    configure_directive_files(&mut cmd, &cd_path, &exec_path);
    cmd.args(["step", "relocate"])
        .current_dir(wrong_path.join(&subdir));

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "wt step relocate failed: {output:?}"
    );

    // The cd directive should land in the equivalent subdirectory of the
    // worktree's new location, not at its root.
    let cd_content = fs::read_to_string(&cd_path).unwrap_or_default();
    let expected_subdir = parent.join("repo.feature").join(&subdir);
    let expected_str = expected_subdir.to_string_lossy();
    assert!(
        cd_content.contains(&*expected_str),
        "CD file should contain relocated subdirectory path {expected_str}, got: {cd_content}"
    );
}
