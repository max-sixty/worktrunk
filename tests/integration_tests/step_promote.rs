//! Integration tests for `wt step promote`

use crate::common::{TestRepo, make_snapshot_cmd, repo, setup_snapshot_settings, wt_command};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::fs;
use std::process::Command;

/// Helper to get the current branch in a directory
fn get_branch(repo: &TestRepo, dir: &std::path::Path) -> String {
    let output = repo
        .git_command()
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git rev-parse failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Test promoting from another worktree (no argument)
#[rstest]
fn test_promote_from_worktree(mut repo: TestRepo) {
    let _settings_guard = setup_snapshot_settings(&repo).bind_to_scope();
    let feature_path = repo.add_worktree("feature");

    // Run promote from the worktree (no argument needed)
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["promote"],
        Some(&feature_path),
    ));

    // Verify branches were exchanged
    assert_eq!(
        get_branch(&repo, repo.root_path()),
        "feature",
        "main worktree should now have feature"
    );
    assert_eq!(
        get_branch(&repo, &feature_path),
        "main",
        "other worktree should now have main"
    );
}

/// Test promoting by specifying branch name
#[rstest]
fn test_promote_with_branch_argument(mut repo: TestRepo) {
    let _settings_guard = setup_snapshot_settings(&repo).bind_to_scope();
    let feature_path = repo.add_worktree("feature");

    // Run promote from main worktree, specifying the branch
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["promote", "feature"],
        Some(repo.root_path()),
    ));

    // Verify branches were exchanged
    assert_eq!(
        get_branch(&repo, repo.root_path()),
        "feature",
        "main worktree should now have feature"
    );
    assert_eq!(
        get_branch(&repo, &feature_path),
        "main",
        "other worktree should now have main"
    );
}

/// Test restoring canonical state
#[rstest]
fn test_promote_restore(mut repo: TestRepo) {
    let _settings_guard = setup_snapshot_settings(&repo).bind_to_scope();
    let feature_path = repo.add_worktree("feature");

    // First promote: feature to main worktree
    let output = repo
        .wt_command()
        .args(["step", "promote", "feature"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "first promote failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify first promote worked
    assert_eq!(get_branch(&repo, repo.root_path()), "feature");

    // Restore: promote main back (now 'main' is in the other worktree)
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["promote", "main"],
        Some(repo.root_path()),
    ));

    // Verify canonical state restored
    assert_eq!(
        get_branch(&repo, repo.root_path()),
        "main",
        "main worktree should have main again"
    );
    assert_eq!(
        get_branch(&repo, &feature_path),
        "feature",
        "other worktree should have feature again"
    );
}

/// Test when branch is already in main worktree
#[rstest]
fn test_promote_already_in_main(repo: TestRepo) {
    let _settings_guard = setup_snapshot_settings(&repo).bind_to_scope();
    // 'main' is already in main worktree
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["promote", "main"],
        Some(repo.root_path()),
    ));
}

/// Test auto-restore with no argument from main worktree (after prior promote)
#[rstest]
fn test_promote_auto_restore(mut repo: TestRepo) {
    let _settings_guard = setup_snapshot_settings(&repo).bind_to_scope();
    let feature_path = repo.add_worktree("feature");

    // First promote: feature to main worktree (creates mismatch)
    let output = repo
        .wt_command()
        .args(["step", "promote", "feature"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "first promote failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify first promote worked
    assert_eq!(get_branch(&repo, repo.root_path()), "feature");
    assert_eq!(get_branch(&repo, &feature_path), "main");

    // Auto-restore: no argument from main worktree restores default branch
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["promote"],
        Some(repo.root_path()),
    ));

    // Verify canonical state restored
    assert_eq!(
        get_branch(&repo, repo.root_path()),
        "main",
        "main worktree should have main again"
    );
    assert_eq!(
        get_branch(&repo, &feature_path),
        "feature",
        "other worktree should have feature again"
    );
}

/// Test auto-restore when no argument from main worktree (already canonical)
#[rstest]
fn test_promote_no_arg_from_main(repo: TestRepo) {
    let _settings_guard = setup_snapshot_settings(&repo).bind_to_scope();
    // From main worktree with no arg: restores default branch (already there)
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["promote"],
        Some(repo.root_path()),
    ));
}

/// Test error when branch has no worktree
#[rstest]
fn test_promote_branch_not_in_worktree(repo: TestRepo) {
    let _settings_guard = setup_snapshot_settings(&repo).bind_to_scope();
    // Create a branch but don't make a worktree for it
    repo.run_git(&["branch", "orphan"]);

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["promote", "orphan"],
        Some(repo.root_path()),
    ));
}

/// Test error when main worktree is dirty
#[rstest]
fn test_promote_dirty_main(mut repo: TestRepo) {
    let _settings_guard = setup_snapshot_settings(&repo).bind_to_scope();
    let _feature_path = repo.add_worktree("feature");

    // Make main worktree dirty
    fs::write(repo.root_path().join("dirty.txt"), "uncommitted").unwrap();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["promote", "feature"],
        Some(repo.root_path()),
    ));
}

/// Test error when target worktree is dirty
#[rstest]
fn test_promote_dirty_target(mut repo: TestRepo) {
    let _settings_guard = setup_snapshot_settings(&repo).bind_to_scope();
    let feature_path = repo.add_worktree("feature");

    // Make target worktree dirty
    fs::write(feature_path.join("dirty.txt"), "uncommitted").unwrap();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["promote", "feature"],
        Some(repo.root_path()),
    ));
}

/// Test that wt list shows mismatch indicator after promote
#[rstest]
fn test_promote_shows_mismatch_in_list(mut repo: TestRepo) {
    let _settings_guard = setup_snapshot_settings(&repo).bind_to_scope();
    let _feature_path = repo.add_worktree("feature");

    // Promote
    let output = repo
        .wt_command()
        .args(["step", "promote", "feature"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "promote failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // List should show mismatch indicators
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "list",
        &[],
        Some(repo.root_path()),
    ));
}

/// Test error when run in a bare repository (no worktrees)
#[test]
fn test_promote_bare_repo_no_worktrees() {
    let temp_dir = tempfile::tempdir().unwrap();
    let bare_repo = temp_dir.path().join("bare.git");

    // Create a bare repository
    Command::new("git")
        .args(["init", "--bare"])
        .arg(&bare_repo)
        .output()
        .unwrap();

    // Try to run promote in the bare repo - fails with "No worktrees found"
    let output = wt_command()
        .args(["step", "promote", "feature"])
        .current_dir(&bare_repo)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(
        stderr.contains("No worktrees found"),
        "Expected no worktrees error, got: {stderr}"
    );
}

/// Test error when run in a bare repository with worktrees
#[test]
fn test_promote_bare_repo_with_worktrees() {
    let temp_dir = tempfile::tempdir().unwrap();
    let bare_repo = temp_dir.path().join("bare.git");
    let worktree_path = temp_dir.path().join("worktree");
    let temp_clone = temp_dir.path().join("temp");

    // Create a bare repository
    Command::new("git")
        .args(["init", "--bare", "--initial-branch=main"])
        .arg(&bare_repo)
        .output()
        .unwrap();

    // Create a commit via a temporary clone
    Command::new("git")
        .args([
            "clone",
            bare_repo.to_str().unwrap(),
            temp_clone.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(&temp_clone)
        .output()
        .unwrap();

    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(&temp_clone)
        .output()
        .unwrap();

    Command::new("git")
        .args(["commit", "--allow-empty", "-m", "init"])
        .current_dir(&temp_clone)
        .output()
        .unwrap();

    Command::new("git")
        .args(["push", "origin", "main"])
        .current_dir(&temp_clone)
        .output()
        .unwrap();

    // Add a worktree to the bare repo
    Command::new("git")
        .args([
            "--git-dir",
            bare_repo.to_str().unwrap(),
            "worktree",
            "add",
            worktree_path.to_str().unwrap(),
            "main",
        ])
        .output()
        .unwrap();

    // Try to run promote in the bare repo - should fail with bare repo error
    let output = wt_command()
        .args(["step", "promote", "feature"])
        .current_dir(&bare_repo)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(
        stderr.contains("bare repositories"),
        "Expected bare repo error, got: {stderr}"
    );
}

/// Test error when main worktree has detached HEAD
#[rstest]
fn test_promote_detached_head_main(mut repo: TestRepo) {
    let _settings_guard = setup_snapshot_settings(&repo).bind_to_scope();
    let _feature_path = repo.add_worktree("feature");

    // Detach HEAD in main worktree
    let sha = repo
        .git_command()
        .args(["rev-parse", "HEAD"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();
    let sha = String::from_utf8_lossy(&sha.stdout).trim().to_string();

    repo.git_command()
        .args(["checkout", "--detach", &sha])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    // Promote should fail due to detached HEAD
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["promote", "feature"],
        Some(repo.root_path()),
    ));
}

/// Helper: commit a .gitignore in a worktree
fn commit_gitignore(repo: &TestRepo, dir: &std::path::Path, content: &str) {
    fs::write(dir.join(".gitignore"), content).unwrap();
    repo.run_git_in(dir, &["add", ".gitignore"]);
    repo.run_git_in(dir, &["commit", "-m", "add gitignore"]);
}

// ─── Swap tests ───────────────────────────────────────────────────────────

/// Both worktrees have gitignored files — verify complete bidirectional swap
#[rstest]
fn test_promote_swap_bidirectional(mut repo: TestRepo) {
    let _settings_guard = setup_snapshot_settings(&repo).bind_to_scope();
    let feature_path = repo.add_worktree("feature");

    let gitignore = "build/\n*.log\n";
    commit_gitignore(&repo, repo.root_path(), gitignore);
    commit_gitignore(&repo, &feature_path, gitignore);

    // Main: build/main-artifact, app.log
    fs::create_dir_all(repo.root_path().join("build")).unwrap();
    fs::write(repo.root_path().join("build/main-artifact"), "main build").unwrap();
    fs::write(repo.root_path().join("app.log"), "main log").unwrap();

    // Feature: build/feature-artifact, debug.log
    fs::create_dir_all(feature_path.join("build")).unwrap();
    fs::write(feature_path.join("build/feature-artifact"), "feature build").unwrap();
    fs::write(feature_path.join("debug.log"), "feature log").unwrap();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["promote", "feature"],
        Some(repo.root_path()),
    ));

    // Main worktree (now feature) should have feature's files
    assert_eq!(
        fs::read_to_string(repo.root_path().join("build/feature-artifact")).unwrap(),
        "feature build"
    );
    assert_eq!(
        fs::read_to_string(repo.root_path().join("debug.log")).unwrap(),
        "feature log"
    );
    assert!(!repo.root_path().join("build/main-artifact").exists());
    assert!(!repo.root_path().join("app.log").exists());

    // Feature worktree (now main) should have main's files
    assert_eq!(
        fs::read_to_string(feature_path.join("build/main-artifact")).unwrap(),
        "main build"
    );
    assert_eq!(
        fs::read_to_string(feature_path.join("app.log")).unwrap(),
        "main log"
    );
    assert!(!feature_path.join("build/feature-artifact").exists());
    assert!(!feature_path.join("debug.log").exists());
}

/// Only main worktree has gitignored files — they should move to the other worktree
#[rstest]
fn test_promote_swap_only_main_has_ignored(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    let gitignore = "build/\n*.log\n";
    commit_gitignore(&repo, repo.root_path(), gitignore);
    commit_gitignore(&repo, &feature_path, gitignore);

    // Only main has ignored files
    fs::create_dir_all(repo.root_path().join("build")).unwrap();
    fs::write(repo.root_path().join("build/artifact"), "main artifact").unwrap();
    fs::write(repo.root_path().join("app.log"), "main log").unwrap();

    let output = repo
        .wt_command()
        .args(["step", "promote", "feature"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Main worktree should be clean (no ignored files)
    assert!(!repo.root_path().join("build").exists());
    assert!(!repo.root_path().join("app.log").exists());

    // Feature worktree should have main's files
    assert_eq!(
        fs::read_to_string(feature_path.join("build/artifact")).unwrap(),
        "main artifact"
    );
    assert_eq!(
        fs::read_to_string(feature_path.join("app.log")).unwrap(),
        "main log"
    );
}

/// Only feature worktree has gitignored files — they should move to the main worktree
#[rstest]
fn test_promote_swap_only_feature_has_ignored(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    let gitignore = "build/\n";
    commit_gitignore(&repo, repo.root_path(), gitignore);
    commit_gitignore(&repo, &feature_path, gitignore);

    // Only feature has ignored files
    fs::create_dir_all(feature_path.join("build")).unwrap();
    fs::write(feature_path.join("build/artifact"), "feature artifact").unwrap();

    let output = repo
        .wt_command()
        .args(["step", "promote", "feature"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Main worktree should have feature's files
    assert_eq!(
        fs::read_to_string(repo.root_path().join("build/artifact")).unwrap(),
        "feature artifact"
    );

    // Feature worktree should be clean
    assert!(!feature_path.join("build").exists());
}

/// Neither worktree has gitignored files — promote should succeed without swap message
#[rstest]
fn test_promote_swap_no_ignored_files(mut repo: TestRepo) {
    let _settings_guard = setup_snapshot_settings(&repo).bind_to_scope();
    let _feature_path = repo.add_worktree("feature");

    // No .gitignore, no ignored files — just promote
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["promote", "feature"],
        Some(repo.root_path()),
    ));
}

/// Directories with nested structure are swapped correctly (deep nesting)
#[rstest]
fn test_promote_swap_nested_directories(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    let gitignore = "build/\n";
    commit_gitignore(&repo, repo.root_path(), gitignore);
    commit_gitignore(&repo, &feature_path, gitignore);

    // Main: deep nested structure
    fs::create_dir_all(repo.root_path().join("build/debug/x86/obj")).unwrap();
    fs::write(
        repo.root_path().join("build/debug/x86/obj/main.o"),
        "main object",
    )
    .unwrap();
    fs::write(repo.root_path().join("build/debug/main.bin"), "main binary").unwrap();

    // Feature: different nested structure
    fs::create_dir_all(feature_path.join("build/release/arm64")).unwrap();
    fs::write(
        feature_path.join("build/release/arm64/feature.bin"),
        "feature binary",
    )
    .unwrap();

    let output = repo
        .wt_command()
        .args(["step", "promote", "feature"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Main worktree should have feature's structure
    assert_eq!(
        fs::read_to_string(repo.root_path().join("build/release/arm64/feature.bin")).unwrap(),
        "feature binary"
    );
    assert!(!repo.root_path().join("build/debug").exists());

    // Feature worktree should have main's structure
    assert_eq!(
        fs::read_to_string(feature_path.join("build/debug/x86/obj/main.o")).unwrap(),
        "main object"
    );
    assert_eq!(
        fs::read_to_string(feature_path.join("build/debug/main.bin")).unwrap(),
        "main binary"
    );
    assert!(!feature_path.join("build/release").exists());
}

/// Same-named ignored files with different content are swapped (not clobbered)
#[rstest]
fn test_promote_swap_same_filename_different_content(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    let gitignore = "build/\n";
    commit_gitignore(&repo, repo.root_path(), gitignore);
    commit_gitignore(&repo, &feature_path, gitignore);

    // Both have build/output.bin with different content
    fs::create_dir_all(repo.root_path().join("build")).unwrap();
    fs::write(repo.root_path().join("build/output.bin"), "main output").unwrap();

    fs::create_dir_all(feature_path.join("build")).unwrap();
    fs::write(feature_path.join("build/output.bin"), "feature output").unwrap();

    let output = repo
        .wt_command()
        .args(["step", "promote", "feature"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Content should be swapped, not lost
    assert_eq!(
        fs::read_to_string(repo.root_path().join("build/output.bin")).unwrap(),
        "feature output",
        "main worktree should have feature's content"
    );
    assert_eq!(
        fs::read_to_string(feature_path.join("build/output.bin")).unwrap(),
        "main output",
        "feature worktree should have main's content"
    );
}

/// Promote and restore round-trip preserves gitignored files
#[rstest]
fn test_promote_swap_roundtrip(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    let gitignore = "build/\n*.log\n";
    commit_gitignore(&repo, repo.root_path(), gitignore);
    commit_gitignore(&repo, &feature_path, gitignore);

    // Create ignored files
    fs::create_dir_all(repo.root_path().join("build")).unwrap();
    fs::write(repo.root_path().join("build/artifact"), "main build").unwrap();
    fs::write(repo.root_path().join("app.log"), "main log").unwrap();

    fs::create_dir_all(feature_path.join("build")).unwrap();
    fs::write(feature_path.join("build/artifact"), "feature build").unwrap();
    fs::write(feature_path.join("debug.log"), "feature log").unwrap();

    // Promote feature → main
    let output = repo
        .wt_command()
        .args(["step", "promote", "feature"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Restore: promote main back
    let output = repo
        .wt_command()
        .args(["step", "promote", "main"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // After round-trip: everything should be back to original state
    assert_eq!(get_branch(&repo, repo.root_path()), "main");
    assert_eq!(get_branch(&repo, &feature_path), "feature");

    assert_eq!(
        fs::read_to_string(repo.root_path().join("build/artifact")).unwrap(),
        "main build",
        "main worktree should have its original build artifact back"
    );
    assert_eq!(
        fs::read_to_string(repo.root_path().join("app.log")).unwrap(),
        "main log"
    );
    assert_eq!(
        fs::read_to_string(feature_path.join("build/artifact")).unwrap(),
        "feature build",
        "feature worktree should have its original build artifact back"
    );
    assert_eq!(
        fs::read_to_string(feature_path.join("debug.log")).unwrap(),
        "feature log"
    );
}

/// .worktreeinclude limits which ignored files are swapped
#[rstest]
fn test_promote_swap_respects_worktreeinclude(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    // .worktreeinclude only includes build/ — .env and *.log should NOT be swapped
    let gitignore = "build/\n*.log\n.env\n";
    commit_gitignore(&repo, repo.root_path(), gitignore);
    commit_gitignore(&repo, &feature_path, gitignore);

    // Commit .worktreeinclude on both branches (must be tracked for clean worktree)
    fs::write(repo.root_path().join(".worktreeinclude"), "build/\n").unwrap();
    repo.run_git(&["add", ".worktreeinclude"]);
    repo.run_git(&["commit", "-m", "add worktreeinclude"]);

    fs::write(feature_path.join(".worktreeinclude"), "build/\n").unwrap();
    repo.run_git_in(&feature_path, &["add", ".worktreeinclude"]);
    repo.run_git_in(&feature_path, &["commit", "-m", "add worktreeinclude"]);

    // Main: build/ + .env + app.log
    fs::create_dir_all(repo.root_path().join("build")).unwrap();
    fs::write(repo.root_path().join("build/main-bin"), "main binary").unwrap();
    fs::write(repo.root_path().join(".env"), "MAIN_SECRET=1").unwrap();
    fs::write(repo.root_path().join("app.log"), "main log").unwrap();

    // Feature: build/ + .env + debug.log
    fs::create_dir_all(feature_path.join("build")).unwrap();
    fs::write(feature_path.join("build/feature-bin"), "feature binary").unwrap();
    fs::write(feature_path.join(".env"), "FEATURE_SECRET=1").unwrap();
    fs::write(feature_path.join("debug.log"), "feature log").unwrap();

    let output = repo
        .wt_command()
        .args(["step", "promote", "feature"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // build/ should be swapped (matches .worktreeinclude)
    assert_eq!(
        fs::read_to_string(repo.root_path().join("build/feature-bin")).unwrap(),
        "feature binary"
    );
    assert_eq!(
        fs::read_to_string(feature_path.join("build/main-bin")).unwrap(),
        "main binary"
    );

    // .env and *.log should NOT be swapped (excluded by .worktreeinclude)
    assert_eq!(
        fs::read_to_string(repo.root_path().join(".env")).unwrap(),
        "MAIN_SECRET=1",
        ".env should stay in place"
    );
    assert_eq!(
        fs::read_to_string(repo.root_path().join("app.log")).unwrap(),
        "main log",
        "app.log should stay in place"
    );
    assert_eq!(
        fs::read_to_string(feature_path.join(".env")).unwrap(),
        "FEATURE_SECRET=1",
        ".env should stay in place"
    );
    assert_eq!(
        fs::read_to_string(feature_path.join("debug.log")).unwrap(),
        "feature log",
        "debug.log should stay in place"
    );
}

/// No stale staging directory left after successful promote
#[rstest]
fn test_promote_swap_no_staging_leftover(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    let gitignore = "build/\n";
    commit_gitignore(&repo, repo.root_path(), gitignore);
    commit_gitignore(&repo, &feature_path, gitignore);

    fs::create_dir_all(repo.root_path().join("build")).unwrap();
    fs::write(repo.root_path().join("build/artifact"), "data").unwrap();

    let output = repo
        .wt_command()
        .args(["step", "promote", "feature"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The staging directory should be cleaned up
    let git_dir = repo.root_path().join(".git");
    assert!(
        !git_dir.join("wt-promote-staging").exists(),
        "staging directory should be cleaned up after promote"
    );
}

/// Tracked files are not affected by the swap
#[rstest]
fn test_promote_swap_does_not_touch_tracked_files(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    let gitignore = "build/\n";
    commit_gitignore(&repo, repo.root_path(), gitignore);
    commit_gitignore(&repo, &feature_path, gitignore);

    // Create a tracked file on main (committed, not ignored)
    fs::write(repo.root_path().join("src.txt"), "main source").unwrap();
    repo.run_git(&["add", "src.txt"]);
    repo.run_git(&["commit", "-m", "add source"]);

    // Create a tracked file on feature
    fs::write(feature_path.join("feat.txt"), "feature source").unwrap();
    repo.run_git_in(&feature_path, &["add", "feat.txt"]);
    repo.run_git_in(&feature_path, &["commit", "-m", "add feat source"]);

    // Create ignored files
    fs::create_dir_all(repo.root_path().join("build")).unwrap();
    fs::write(repo.root_path().join("build/main.o"), "main obj").unwrap();
    fs::create_dir_all(feature_path.join("build")).unwrap();
    fs::write(feature_path.join("build/feat.o"), "feat obj").unwrap();

    let output = repo
        .wt_command()
        .args(["step", "promote", "feature"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Tracked files should follow their branches (via git switch, not our swap)
    // Main worktree (now on feature) should have feat.txt from git switch
    assert_eq!(
        fs::read_to_string(repo.root_path().join("feat.txt")).unwrap(),
        "feature source"
    );
    // Feature worktree (now on main) should have src.txt from git switch
    assert_eq!(
        fs::read_to_string(feature_path.join("src.txt")).unwrap(),
        "main source"
    );

    // Ignored files should be swapped by our code
    assert_eq!(
        fs::read_to_string(repo.root_path().join("build/feat.o")).unwrap(),
        "feat obj"
    );
    assert_eq!(
        fs::read_to_string(feature_path.join("build/main.o")).unwrap(),
        "main obj"
    );
}

/// Test that promote bails with guidance when stale staging directory exists
#[rstest]
fn test_promote_stale_staging_blocks_with_guidance(mut repo: TestRepo) {
    let _settings_guard = setup_snapshot_settings(&repo).bind_to_scope();
    let feature_path = repo.add_worktree("feature");

    // Need gitignored files so stage_ignored is actually called
    let gitignore = "build/\n";
    commit_gitignore(&repo, repo.root_path(), gitignore);
    commit_gitignore(&repo, &feature_path, gitignore);
    fs::create_dir_all(repo.root_path().join("build")).unwrap();
    fs::write(repo.root_path().join("build/artifact"), "main build").unwrap();

    // Create a fake leftover staging directory (as if previous promote was interrupted)
    let git_dir = repo.root_path().join(".git");
    let staging_dir = git_dir.join("wt-promote-staging");
    fs::create_dir_all(staging_dir.join("a")).unwrap();
    fs::write(staging_dir.join("a/leftover"), "leftover data").unwrap();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["promote", "feature"],
        None
    ));

    // Staging dir should still exist (not destroyed)
    assert!(staging_dir.exists(), "staging dir should be preserved");
    assert_eq!(
        fs::read_to_string(staging_dir.join("a/leftover")).unwrap(),
        "leftover data"
    );
}

/// Test error when linked worktree has detached HEAD (no-arg promote)
#[rstest]
fn test_promote_detached_head_linked(mut repo: TestRepo) {
    let _settings_guard = setup_snapshot_settings(&repo).bind_to_scope();
    let feature_path = repo.add_worktree("feature");

    // Detach HEAD in the linked worktree
    let sha = repo
        .git_command()
        .args(["rev-parse", "HEAD"])
        .current_dir(&feature_path)
        .output()
        .unwrap();
    let sha = String::from_utf8_lossy(&sha.stdout).trim().to_string();

    repo.git_command()
        .args(["checkout", "--detach", &sha])
        .current_dir(&feature_path)
        .output()
        .unwrap();

    // No-arg promote from linked worktree should fail due to detached HEAD
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["promote"],
        Some(&feature_path),
    ));
}
