//! Integration tests for `wt step relocate`

use crate::common::{TestRepo, configure_directive_file, directive_file, make_snapshot_cmd, repo};
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

/// Create a worktree for `branch` at `path`, stopped on an unresolved
/// conflict: `git merge side` leaves the index at stages 1–3 with `<<<<<<<` on
/// disk — the state `git add -A` would silently resolve.
fn add_conflicted_worktree(repo: &TestRepo, branch: &str, path: &Path) {
    // A `side` branch that edits the same line, to conflict against.
    fs::write(repo.root_path().join("conflict.txt"), "base\n").unwrap();
    repo.run_git(&["add", "conflict.txt"]);
    repo.run_git(&["commit", "-m", "Base edit"]);
    repo.run_git(&["checkout", "-b", "side"]);
    fs::write(repo.root_path().join("conflict.txt"), "side\n").unwrap();
    repo.run_git(&["commit", "-am", "Conflicting edit on side"]);
    repo.run_git(&["checkout", "main"]);

    repo.run_git(&["worktree", "add", "-b", branch, path.to_str().unwrap()]);
    let git = |args: &[&str]| {
        repo.git_command()
            .current_dir(path)
            .args(args.iter().copied())
            .run()
            .unwrap()
    };
    fs::write(path.join("conflict.txt"), "theirs\n").unwrap();
    git(&["commit", "-am", "Conflicting edit on branch"]);
    // Conflicts: both sides changed the same line.
    git(&["merge", "side"]);

    let unmerged = git(&["diff", "--name-only", "--diff-filter=U"]);
    assert_eq!(
        String::from_utf8_lossy(&unmerged.stdout).trim(),
        "conflict.txt",
        "setup must leave an unresolved conflict in the index"
    );
}

/// Regression: `--commit` stages with `git add -A` before committing, and
/// `git add -A` collapses an unmerged path's index stages — resolving the
/// conflict as far as the index is concerned while `<<<<<<<` is still on disk,
/// and taking git's own refusal to commit with it. So relocate would commit
/// the markers and leave a broken merge looking finished.
#[rstest]
fn test_relocate_refuses_unmerged_paths(repo: TestRepo) {
    let parent = worktree_parent(&repo);
    let wrong_path = parent.join("wrong-location");
    add_conflicted_worktree(&repo, "feature", &wrong_path);
    let head_before = repo.head_sha_in(&wrong_path);

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["relocate", "--commit"],
        None
    ));

    assert_eq!(
        repo.head_sha_in(&wrong_path),
        head_before,
        "the refusal must leave HEAD alone; committing here would commit the conflict markers"
    );
    assert!(
        wrong_path.exists() && !parent.join("repo.feature").exists(),
        "the conflicted worktree must stay put rather than move on an uncommitted conflict"
    );
    assert!(
        fs::read_to_string(wrong_path.join("conflict.txt"))
            .unwrap()
            .contains("<<<<<<<"),
        "the conflict must be left for the user to resolve, markers intact"
    );
}

/// An unresolved conflict is a per-worktree blocker the user must fix by hand,
/// like a locked worktree — so it skips that worktree with a stable JSON
/// `reason` and relocates the rest, rather than failing the whole run.
#[rstest]
fn test_relocate_unmerged_skips_only_that_worktree(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // One worktree stopped on a conflict, one merely dirty (index clean).
    let conflicted_path = parent.join("wrong-conflicted");
    add_conflicted_worktree(&repo, "conflicted", &conflicted_path);
    let clean_path = parent.join("wrong-clean");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "clean",
        clean_path.to_str().unwrap(),
    ]);
    fs::write(clean_path.join("dirty.txt"), "uncommitted").unwrap();

    fs::write(
        repo.test_config_path(),
        r#"
[commit.generation]
command = "cat >/dev/null && echo 'chore: auto-commit before relocate'"
"#,
    )
    .unwrap();

    let output = repo
        .wt_command()
        .args(["step", "relocate", "--commit", "--format=json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "one conflicted worktree must not fail the whole run; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid JSON");

    let skipped = parsed["skipped"].as_array().expect("skipped array");
    assert!(
        skipped
            .iter()
            .any(|s| s["branch"] == "conflicted" && s["reason"] == "unmerged"),
        "conflicted worktree should be skipped with reason \"unmerged\": {parsed}"
    );
    let entries = parsed["entries"].as_array().expect("entries array");
    assert!(
        entries.iter().any(|e| e["branch"] == "clean"),
        "the dirty-but-committable sibling should still relocate: {parsed}"
    );

    // The conflicted worktree stayed put with its conflict; the sibling moved.
    assert!(
        conflicted_path.exists(),
        "conflicted worktree should not move"
    );
    assert!(
        parent.join("repo.clean").exists() && !clean_path.exists(),
        "clean worktree should have relocated"
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

/// A worktree whose target is occupied by a *blocked* worktree must itself be
/// skipped, not temp-moved.
///
/// Regression: `beta` sits at `alpha`'s target, so `alpha` depends on `beta`
/// vacating — but `beta`'s own target is a plain non-worktree file with no
/// `--clobber`, so `beta` is blocked and never moves. Previously the no-progress
/// branch treated `alpha` as a cycle, temp-moved it into the staging dir, and
/// `finalize` then failed moving it into the still-occupied target — erroring
/// out and stranding `alpha` in `.git/wt/staging/relocate/`.
#[rstest]
fn test_relocate_blocked_occupant_skips_dependent(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // beta occupies alpha's expected path (repo.alpha).
    let path_alpha = parent.join("repo.alpha");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "beta",
        path_alpha.to_str().unwrap(),
    ]);

    // alpha lives at a non-standard location and wants repo.alpha.
    let wrong_alpha = parent.join("wrong-alpha");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "alpha",
        wrong_alpha.to_str().unwrap(),
    ]);

    // Block beta's target (repo.beta) with a plain, non-worktree directory.
    let path_beta = parent.join("repo.beta");
    fs::create_dir_all(&path_beta).unwrap();
    fs::write(path_beta.join("blocker.txt"), "blocker").unwrap();

    // Both are skipped; the command must succeed and strand nothing.
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["relocate", "alpha", "beta"],
        None
    ));

    // alpha stays at its original location (not stranded in staging).
    assert!(
        wrong_alpha.exists(),
        "alpha should remain at its original location: {}",
        wrong_alpha.display()
    );
    // beta stays where it was (still occupying repo.alpha).
    assert!(path_alpha.exists(), "beta should remain at repo.alpha");
    // Nothing left behind in the staging dir.
    let stranded = repo.root_path().join(".git/wt/staging/relocate/alpha");
    assert!(
        !stranded.exists(),
        "alpha must not be stranded in the staging dir: {}",
        stranded.display()
    );
}

/// A blocked occupant must propagate transitively down a chain of dependents,
/// across multiple resolution passes.
///
/// Extends `test_relocate_blocked_occupant_skips_dependent` to a 3-level chain
/// (`alpha → beta → gamma-blocked`) that specifically exercises the loop's
/// `made_progress` re-drive. `gamma`'s target is a plain non-worktree directory
/// (no `--clobber`), so `gamma` is blocked at construction; `beta` occupies
/// `gamma`'s dependency (sits at repo.beta) and `alpha` occupies `beta`'s (sits
/// at repo.alpha), so the block can only reach `alpha` one pass after it reaches
/// `beta`.
///
/// The re-drive is only load-bearing when a dependent is *iterated before* its
/// occupant within a pass. Worktrees are processed in `git worktree list` order
/// (git sorts linked worktrees by registration id ≈ path basename), independent
/// of the argument order — so `alpha` is parked at `aaa-alpha`, whose basename
/// sorts before `beta`'s `repo.alpha`. That makes pass 1 visit `alpha` while
/// `beta` is still pending (no block yet → `alpha` stays pending), then block
/// `beta` (occupant `gamma` already blocked). Only the `made_progress` re-drive
/// runs a pass 2 that sees `beta` blocked and blocks `alpha` in turn. Drop the
/// re-drive and pass 1 falls straight into `break_cycle`, which temp-moves the
/// still-pending `alpha` and `finalize` then misplaces it into the occupied
/// `repo.alpha` — the exact bug this guards. (Parking `alpha` at a path that
/// sorts *after* `repo.alpha` collapses the chain into a single pass and no
/// longer tests the re-drive; see the 2-level test.)
#[rstest]
fn test_relocate_blocked_occupant_skips_chain(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // alpha lives at aaa-alpha (basename sorts before repo.alpha, so alpha is
    // iterated before its occupant beta) and wants repo.alpha.
    let wrong_alpha = parent.join("aaa-alpha");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "alpha",
        wrong_alpha.to_str().unwrap(),
    ]);

    // beta occupies alpha's expected path (repo.alpha) and wants repo.beta.
    let path_alpha = parent.join("repo.alpha");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "beta",
        path_alpha.to_str().unwrap(),
    ]);

    // gamma occupies beta's expected path (repo.beta) and wants repo.gamma.
    let path_beta = parent.join("repo.beta");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "gamma",
        path_beta.to_str().unwrap(),
    ]);

    // Block gamma's target (repo.gamma) with a plain, non-worktree directory.
    let path_gamma = parent.join("repo.gamma");
    fs::create_dir_all(&path_gamma).unwrap();
    fs::write(path_gamma.join("blocker.txt"), "blocker").unwrap();

    // All three are skipped; the command must succeed and strand nothing.
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["relocate", "alpha", "beta", "gamma"],
        None
    ));

    // Every worktree stays put — none stranded in staging or misplaced into an
    // occupied target.
    assert!(
        wrong_alpha.exists(),
        "alpha should remain at its original location: {}",
        wrong_alpha.display()
    );
    assert!(path_alpha.exists(), "beta should remain at repo.alpha");
    assert!(path_beta.exists(), "gamma should remain at repo.beta");
    let stranded = repo.root_path().join(".git/wt/staging/relocate/alpha");
    assert!(
        !stranded.exists(),
        "alpha must not be stranded in the staging dir: {}",
        stranded.display()
    );
    let misplaced = path_alpha.join("alpha");
    assert!(
        !misplaced.exists(),
        "alpha must not be misplaced inside beta's worktree: {}",
        misplaced.display()
    );
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

/// Regression test: the human-readable summary count must include
/// template-error branches, matching the `--format=json` skip set.
///
/// When a valid candidate and a template-error branch coexist, the JSON path
/// folds template errors into `all_skipped` but the human summary previously
/// counted only validation/executor skips, undercounting by the number of
/// template errors. The template here fails only for branch `bad` (via a
/// branch-gated undefined variable) while `good` expands cleanly, so relocate
/// moves `good` and skips `bad`.
#[rstest]
fn test_relocate_template_error_counted_in_summary(repo: TestRepo) {
    let parent = worktree_parent(&repo);

    // `good`: a mismatched worktree that will relocate successfully.
    let good_wrong = parent.join("good-wrong");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "good",
        good_wrong.to_str().unwrap(),
    ]);
    // `bad`: a worktree whose template expansion fails.
    let bad_path = parent.join("bad-loc");
    repo.run_git(&["worktree", "add", "-b", "bad", bad_path.to_str().unwrap()]);

    // Template errors only for branch `bad`; `good` renders the standard path.
    let worktrunk_config = "worktree-path = \"{% if branch == 'bad' %}{{ undefined_var }}{% endif %}{{ repo_path }}/../{{ repo }}.{{ branch }}\"\n";
    fs::write(repo.test_config_path(), worktrunk_config).unwrap();

    let output = repo
        .wt_command()
        .args(["step", "relocate"])
        .output()
        .unwrap();
    assert!(output.status.success(), "relocate should succeed");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Relocated 1 worktree, skipped 1 worktree"),
        "summary must count the template-error branch as skipped; stderr was:\n{stderr}"
    );

    // `good` relocated to its expected sibling path; `bad` untouched.
    assert!(
        parent.join("repo.good").exists(),
        "good should have relocated"
    );
    assert!(
        bad_path.exists(),
        "bad should be untouched (template error)"
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
///
/// Ignored on Windows: subdir preservation only fires when the cwd is inside the
/// moving worktree — which is exactly when `git worktree move` (a directory
/// rename) fails with a sharing violation, because a live process holds that cwd.
/// Unlike `remove` (where shell integration cds to main before removing), a real
/// Windows user hits this too: their shell holds the cwd across the move. So the
/// preservation path is reachable, and testable, only on Unix.
#[rstest]
#[cfg_attr(windows, ignore)]
fn test_relocate_preserves_subdir(repo: TestRepo) {
    let parent = worktree_parent(&repo);
    let (cd_path, _guard) = directive_file();

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
    configure_directive_file(&mut cmd, &cd_path);
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

/// A shell using the retired single-file protocol cannot follow a
/// relocated current worktree. The relocation still succeeds, but it must
/// explain that the wrapper is stale and how to repair it rather than silently
/// leaving the shell in the renamed-away directory.
///
/// Ignored on Windows for the same reason as the adjacent subdirectory test:
/// relocating a worktree while this process holds its cwd there fails with a
/// sharing violation before the directive-warning path is reachable.
#[rstest]
#[cfg_attr(windows, ignore)]
fn test_relocate_current_with_retired_wrapper_warns(repo: TestRepo) {
    let parent = worktree_parent(&repo);
    let wrong_path = parent.join("wrong-location");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "feature",
        wrong_path.to_str().unwrap(),
    ]);

    let directive_dir = tempfile::TempDir::new().unwrap();
    let retired_path = directive_dir.path().join("directive");
    fs::write(&retired_path, "").unwrap();

    let output = repo
        .wt_command()
        .env("WORKTRUNK_DIRECTIVE_FILE", &retired_path)
        .args(["step", "relocate"])
        .current_dir(&wrong_path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "relocation should still succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!wrong_path.exists(), "the old worktree path should be gone");
    assert!(
        parent.join("repo.feature").exists(),
        "the worktree should reach its expected path"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("shell wrapper is out of date"),
        "the stale wrapper must not fail silently:\n{stderr}"
    );
    assert!(
        stderr.contains("wt config shell install"),
        "the warning must include the repair action:\n{stderr}"
    );
    assert_eq!(
        fs::read_to_string(&retired_path).unwrap(),
        "",
        "wt must never write to the retired directive file"
    );
}

/// An argument naming no worktree is an error. Matching it against branch names
/// alone left a typo filtering everything out, and the empty result rendered as
/// "All worktrees are at expected paths" — a success message for a no-op.
#[rstest]
fn step_relocate_rejects_unknown_worktree(repo: TestRepo) {
    let output = repo
        .wt_command()
        .args(["step", "relocate", "--dry-run", "no-such-worktree"])
        .output()
        .unwrap();

    assert!(!output.status.success(), "an unknown argument should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("No branch or worktree named"),
        "expected an unmatched-selector error, got: {stderr}"
    );
}

/// A detached worktree can be named by path but has no expected path to move
/// to — the `worktree-path` template is written over the branch name. Naming
/// one is an error rather than an empty filter reported as success.
#[rstest]
fn step_relocate_rejects_detached_worktree(mut repo: TestRepo) {
    repo.add_worktree("feature-detached");
    repo.detach_head_in_worktree("feature-detached");

    let output = repo
        .wt_command()
        .args(["step", "relocate", "--dry-run", "../repo.feature-detached"])
        .output()
        .unwrap();

    assert!(!output.status.success(), "a detached worktree should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("detached"),
        "expected a detached-worktree error, got: {stderr}"
    );
}

/// A worktree whose directory is gone still resolves by branch, but git has
/// marked it prunable and there is nothing left to move. Naming one is an error
/// rather than an empty filter reported as success.
#[rstest]
fn step_relocate_rejects_prunable_worktree(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature-gone");
    fs::remove_dir_all(&worktree_path).unwrap();

    let output = repo
        .wt_command()
        .args(["step", "relocate", "--dry-run", "feature-gone"])
        .output()
        .unwrap();

    assert!(!output.status.success(), "a prunable worktree should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("directory is gone"),
        "expected a prunable-worktree error, got: {stderr}"
    );
}
