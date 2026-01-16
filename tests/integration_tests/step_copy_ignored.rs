//! Integration tests for `wt step copy-ignored`

use crate::common::{TestRepo, make_snapshot_cmd, repo};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::fs;

/// Test with no .worktreeinclude file and no gitignored files
#[rstest]
fn test_copy_ignored_no_worktreeinclude(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");
    // No .worktreeinclude file and no gitignored files → nothing to copy
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["copy-ignored"],
        Some(&feature_path),
    ));
}

/// Test default behavior: copies all gitignored files when no .worktreeinclude exists
#[rstest]
fn test_copy_ignored_default_copies_all(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    // Create gitignored files but NO .worktreeinclude
    fs::write(repo.root_path().join(".env"), "SECRET=value").unwrap();
    fs::write(repo.root_path().join("cache.db"), "cached data").unwrap();
    fs::write(repo.root_path().join(".gitignore"), ".env\ncache.db\n").unwrap();

    // Without .worktreeinclude, all gitignored files should be copied
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["copy-ignored"],
        Some(&feature_path),
    ));

    // Verify both files were copied
    assert!(
        feature_path.join(".env").exists(),
        ".env should be copied without .worktreeinclude"
    );
    assert!(
        feature_path.join("cache.db").exists(),
        "cache.db should be copied without .worktreeinclude"
    );
}

/// Test error handling when .worktreeinclude has invalid syntax
#[rstest]
fn test_copy_ignored_invalid_worktreeinclude(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    // Create invalid .worktreeinclude (unclosed brace in alternate group)
    fs::write(repo.root_path().join(".worktreeinclude"), "{unclosed\n").unwrap();

    // Should fail with parse error
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["copy-ignored"],
        Some(&feature_path),
    ));
}

/// Test with .worktreeinclude but nothing ignored
#[rstest]
fn test_copy_ignored_empty_intersection(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");
    // Create .worktreeinclude with a pattern
    fs::write(repo.root_path().join(".worktreeinclude"), ".env\n").unwrap();
    // But don't create .gitignore or .env file

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["copy-ignored"],
        Some(&feature_path),
    ));
}

/// Test that files in .worktreeinclude but NOT in .gitignore are not copied
#[rstest]
fn test_copy_ignored_not_ignored_file(mut repo: TestRepo) {
    // Create feature worktree
    let feature_path = repo.add_worktree("feature");

    // Create .env file in main but it's not in .gitignore
    fs::write(repo.root_path().join(".env"), "SECRET=value").unwrap();

    // Create .worktreeinclude listing .env
    fs::write(repo.root_path().join(".worktreeinclude"), ".env\n").unwrap();

    // Run from feature worktree
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["copy-ignored"],
        Some(&feature_path),
    ));
}

/// Test basic file copy: .env in both .gitignore and .worktreeinclude
#[rstest]
fn test_copy_ignored_basic(mut repo: TestRepo) {
    // Create feature worktree
    let feature_path = repo.add_worktree("feature");

    // Create .env file in main
    fs::write(repo.root_path().join(".env"), "SECRET=value").unwrap();

    // Add .env to .gitignore
    fs::write(repo.root_path().join(".gitignore"), ".env\n").unwrap();

    // Create .worktreeinclude listing .env
    fs::write(repo.root_path().join(".worktreeinclude"), ".env\n").unwrap();

    // Run from feature worktree
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["copy-ignored"],
        Some(&feature_path),
    ));

    // Verify file was copied
    let copied_env = feature_path.join(".env");
    assert!(
        copied_env.exists(),
        ".env should be copied to feature worktree"
    );
    assert_eq!(
        fs::read_to_string(&copied_env).unwrap(),
        "SECRET=value",
        ".env content should match"
    );
}

/// Test idempotent behavior: running twice should succeed (skips existing files)
#[rstest]
fn test_copy_ignored_idempotent(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    // Setup: .env file that matches both patterns
    fs::write(repo.root_path().join(".env"), "SECRET=value").unwrap();
    fs::write(repo.root_path().join(".gitignore"), ".env\n").unwrap();
    fs::write(repo.root_path().join(".worktreeinclude"), ".env\n").unwrap();

    // Run copy-ignored twice - second run should succeed (skip existing)
    let output1 = repo
        .wt_command()
        .args(["step", "copy-ignored"])
        .current_dir(&feature_path)
        .output()
        .unwrap();
    assert!(output1.status.success(), "First copy should succeed");

    let output2 = repo
        .wt_command()
        .args(["step", "copy-ignored"])
        .current_dir(&feature_path)
        .output()
        .unwrap();
    assert!(
        output2.status.success(),
        "Second copy should succeed (idempotent)"
    );

    // File should still exist with original content
    assert_eq!(
        fs::read_to_string(feature_path.join(".env")).unwrap(),
        "SECRET=value"
    );
}

/// Test copying a single file in a subdirectory (creates parent dirs)
#[rstest]
fn test_copy_ignored_nested_file(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    // Create a nested file that's gitignored
    let cache_dir = repo.root_path().join("cache");
    fs::create_dir_all(&cache_dir).unwrap();
    fs::write(cache_dir.join("data.json"), r#"{"key": "value"}"#).unwrap();

    // Gitignore the specific file (not the directory)
    fs::write(repo.root_path().join(".gitignore"), "cache/data.json\n").unwrap();

    // Worktreeinclude the specific file
    fs::write(
        repo.root_path().join(".worktreeinclude"),
        "cache/data.json\n",
    )
    .unwrap();

    // Run from feature worktree
    let output = repo
        .wt_command()
        .args(["step", "copy-ignored"])
        .current_dir(&feature_path)
        .output()
        .unwrap();
    assert!(output.status.success());

    // Verify file was copied (parent dir should be created)
    let copied_file = feature_path.join("cache").join("data.json");
    assert!(copied_file.exists(), "Nested file should be copied");
    assert_eq!(
        fs::read_to_string(&copied_file).unwrap(),
        r#"{"key": "value"}"#
    );
}

/// Test --dry-run shows what would be copied without copying
#[rstest]
fn test_copy_ignored_dry_run(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    // Setup: .env file that matches both patterns
    fs::write(repo.root_path().join(".env"), "SECRET=value").unwrap();
    fs::write(repo.root_path().join(".gitignore"), ".env\n").unwrap();
    fs::write(repo.root_path().join(".worktreeinclude"), ".env\n").unwrap();

    // Run with --dry-run
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["copy-ignored", "--dry-run"],
        Some(&feature_path),
    ));

    // Verify file was NOT copied
    let copied_env = feature_path.join(".env");
    assert!(
        !copied_env.exists(),
        ".env should NOT be copied in dry-run mode"
    );
}

/// Test copying a directory (e.g., target/)
#[rstest]
fn test_copy_ignored_directory(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    // Create target directory with some files
    let target_dir = repo.root_path().join("target");
    fs::create_dir_all(target_dir.join("debug")).unwrap();
    fs::write(target_dir.join("debug").join("output"), "binary content").unwrap();
    fs::write(target_dir.join("CACHEDIR.TAG"), "cache tag").unwrap();

    // Add target to .gitignore
    fs::write(repo.root_path().join(".gitignore"), "target/\n").unwrap();

    // Create .worktreeinclude listing target
    fs::write(repo.root_path().join(".worktreeinclude"), "target/\n").unwrap();

    // Run from feature worktree
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["copy-ignored"],
        Some(&feature_path),
    ));

    // Verify directory was copied with contents
    let copied_target = feature_path.join("target");
    assert!(copied_target.exists(), "target should be copied");
    assert!(
        copied_target.join("debug").join("output").exists(),
        "target/debug/output should be copied"
    );
    assert_eq!(
        fs::read_to_string(copied_target.join("debug").join("output")).unwrap(),
        "binary content"
    );
}

/// Test glob patterns: .env.*
#[rstest]
fn test_copy_ignored_glob_pattern(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    // Create multiple .env files
    fs::write(repo.root_path().join(".env"), "base").unwrap();
    fs::write(repo.root_path().join(".env.local"), "local").unwrap();
    fs::write(repo.root_path().join(".env.test"), "test").unwrap();

    // .gitignore with .env*
    fs::write(repo.root_path().join(".gitignore"), ".env*\n").unwrap();

    // .worktreeinclude with same pattern
    fs::write(repo.root_path().join(".worktreeinclude"), ".env*\n").unwrap();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["copy-ignored"],
        Some(&feature_path),
    ));

    // Verify all were copied
    assert!(feature_path.join(".env").exists());
    assert!(feature_path.join(".env.local").exists());
    assert!(feature_path.join(".env.test").exists());
}

/// Test same worktree source and destination
#[rstest]
fn test_copy_ignored_same_worktree(repo: TestRepo) {
    // Setup files
    fs::write(repo.root_path().join(".env"), "value").unwrap();
    fs::write(repo.root_path().join(".gitignore"), ".env\n").unwrap();
    fs::write(repo.root_path().join(".worktreeinclude"), ".env\n").unwrap();

    // Run from main worktree (source = dest = main)
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "step", &["copy-ignored"], None,));
}

/// Test --from flag to specify source worktree
#[rstest]
fn test_copy_ignored_from_flag(mut repo: TestRepo) {
    // Create two worktrees
    let feature_a = repo.add_worktree("feature-a");
    let feature_b = repo.add_worktree("feature-b");

    // Create .env in feature-a (not in main)
    fs::write(feature_a.join(".env"), "from-feature-a").unwrap();

    // Add .env to .gitignore in feature-a (source worktree)
    fs::write(feature_a.join(".gitignore"), ".env\n").unwrap();

    // Create .worktreeinclude in feature-a
    fs::write(feature_a.join(".worktreeinclude"), ".env\n").unwrap();

    // Run from feature-b, copying from feature-a
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["copy-ignored", "--from", "feature-a"],
        Some(&feature_b),
    ));

    // Verify file was copied
    assert!(feature_b.join(".env").exists());
    assert_eq!(
        fs::read_to_string(feature_b.join(".env")).unwrap(),
        "from-feature-a"
    );
}

/// Test that COW copies are independent (modifying one doesn't affect the other)
#[rstest]
fn test_copy_ignored_cow_independence(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    // Create file in main
    fs::write(repo.root_path().join(".env"), "original").unwrap();
    fs::write(repo.root_path().join(".gitignore"), ".env\n").unwrap();
    fs::write(repo.root_path().join(".worktreeinclude"), ".env\n").unwrap();

    // Copy to feature
    repo.wt_command()
        .args(["step", "copy-ignored"])
        .current_dir(&feature_path)
        .output()
        .expect("copy-ignored should succeed");

    // Modify the copy in feature
    fs::write(feature_path.join(".env"), "modified").unwrap();

    // Original should be unchanged
    assert_eq!(
        fs::read_to_string(repo.root_path().join(".env")).unwrap(),
        "original",
        "Original file should be unchanged after modifying copy"
    );
}

/// Test deep file patterns: **/.claude/settings.local.json
#[rstest]
fn test_copy_ignored_deep_pattern(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    // Create nested .claude directory with settings
    let claude_dir = repo.root_path().join(".claude");
    fs::create_dir_all(&claude_dir).unwrap();
    fs::write(claude_dir.join("settings.local.json"), r#"{"key":"value"}"#).unwrap();

    // Add to .gitignore
    fs::write(
        repo.root_path().join(".gitignore"),
        "**/.claude/settings.local.json\n",
    )
    .unwrap();

    // Add to .worktreeinclude
    fs::write(
        repo.root_path().join(".worktreeinclude"),
        "**/.claude/settings.local.json\n",
    )
    .unwrap();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["copy-ignored"],
        Some(&feature_path),
    ));

    // Verify the nested file was copied
    assert!(
        feature_path
            .join(".claude")
            .join("settings.local.json")
            .exists()
    );
}

/// Test that nested .gitignore files are respected (not just root .gitignore)
#[rstest]
fn test_copy_ignored_nested_gitignore(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    // Create a subdirectory with its own .gitignore
    let subdir = repo.root_path().join("config");
    fs::create_dir_all(&subdir).unwrap();

    // Create a file ignored by the nested .gitignore (not root)
    fs::write(subdir.join("local.json"), r#"{"local":true}"#).unwrap();

    // Add .gitignore ONLY in the subdirectory
    fs::write(subdir.join(".gitignore"), "local.json\n").unwrap();

    // Root .worktreeinclude should match the file
    fs::write(
        repo.root_path().join(".worktreeinclude"),
        "config/local.json\n",
    )
    .unwrap();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["copy-ignored"],
        Some(&feature_path),
    ));

    // Verify the file was copied (nested .gitignore was respected)
    assert!(
        feature_path.join("config").join("local.json").exists(),
        "File ignored by nested .gitignore should be copied"
    );
}

/// Test --to flag to specify destination worktree
#[rstest]
fn test_copy_ignored_to_flag(mut repo: TestRepo) {
    // Create two worktrees
    let feature_a = repo.add_worktree("feature-a");
    let feature_b = repo.add_worktree("feature-b");

    // Create .env in main
    fs::write(repo.root_path().join(".env"), "from-main").unwrap();

    // Add .env to .gitignore in main
    fs::write(repo.root_path().join(".gitignore"), ".env\n").unwrap();

    // Create .worktreeinclude in main
    fs::write(repo.root_path().join(".worktreeinclude"), ".env\n").unwrap();

    // Run from feature-a, copying from main (default) to feature-b (explicit)
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["copy-ignored", "--to", "feature-b"],
        Some(&feature_a),
    ));

    // Verify file was copied to feature-b (not feature-a)
    assert!(feature_b.join(".env").exists());
    assert!(!feature_a.join(".env").exists());
    assert_eq!(
        fs::read_to_string(feature_b.join(".env")).unwrap(),
        "from-main"
    );
}

/// Test --from with a branch that has no worktree
#[rstest]
fn test_copy_ignored_from_nonexistent_worktree(repo: TestRepo) {
    // Create a branch without a worktree
    repo.git_command()
        .args(["branch", "orphan-branch"])
        .output()
        .unwrap();

    // Try to copy from a branch with no worktree
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["copy-ignored", "--from", "orphan-branch"],
        None,
    ));
}

/// Test --to with a branch that has no worktree
#[rstest]
fn test_copy_ignored_to_nonexistent_worktree(repo: TestRepo) {
    // Create a branch without a worktree
    repo.git_command()
        .args(["branch", "orphan-branch"])
        .output()
        .unwrap();

    // Setup a file to copy
    fs::write(repo.root_path().join(".env"), "value").unwrap();
    fs::write(repo.root_path().join(".gitignore"), ".env\n").unwrap();

    // Try to copy to a branch with no worktree
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["copy-ignored", "--to", "orphan-branch"],
        None,
    ));
}

/// Test copy-ignored when default branch has no worktree
///
/// When the default branch (main) has no worktree, copy-ignored falls back to
/// the main worktree (the original clone directory) for non-bare repos.
#[rstest]
fn test_copy_ignored_no_default_branch_worktree(mut repo: TestRepo) {
    // Create a feature worktree and switch main worktree to a different branch
    let feature_path = repo.add_worktree("feature");
    repo.switch_primary_to("develop"); // main worktree is now on 'develop', not 'main'

    // Set up ignored file in the main worktree (which is now on 'develop')
    fs::write(repo.root_path().join(".env"), "SECRET=value").unwrap();
    fs::write(repo.root_path().join(".gitignore"), ".env\n").unwrap();
    fs::write(repo.root_path().join(".worktreeinclude"), ".env\n").unwrap();

    // Copy from feature - should use main worktree as source (primary_worktree fallback)
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["copy-ignored"],
        Some(&feature_path),
    ));

    // Verify file was copied from main worktree
    assert!(
        feature_path.join(".env").exists(),
        ".env should be copied from main worktree"
    );
}

/// Test copy-ignored in a bare repository setup
///
/// This test reproduces GitHub issue #598: `wt step copy-ignored` fails in bare repo
/// with error "git ls-files failed: fatal: this operation must be run in a work tree"
#[test]
fn test_copy_ignored_bare_repo() {
    use crate::common::{BareRepoTest, TestRepoBase, setup_temp_snapshot_settings, wt_command};

    let test = BareRepoTest::new();

    // Create main worktree (default branch)
    let main_worktree = test.create_worktree("main", "main");
    test.commit_in(&main_worktree, "Initial commit on main");

    // Create a feature worktree
    let feature_worktree = test.create_worktree("feature", "feature");
    test.commit_in(&feature_worktree, "Feature work");

    // Create .env file in main (source worktree)
    fs::write(main_worktree.join(".env"), "SECRET=value").unwrap();

    // Add .env to .gitignore in main
    fs::write(main_worktree.join(".gitignore"), ".env\n").unwrap();

    // Create .worktreeinclude in main
    fs::write(main_worktree.join(".worktreeinclude"), ".env\n").unwrap();

    // Run copy-ignored from feature worktree (copies from main by default)
    let settings = setup_temp_snapshot_settings(test.temp_path());
    settings.bind(|| {
        let mut cmd = wt_command();
        test.configure_wt_cmd(&mut cmd);
        cmd.args(["step", "copy-ignored"])
            .current_dir(&feature_worktree);

        insta_cmd::assert_cmd_snapshot!(cmd);
    });

    // Verify file was copied
    assert!(
        feature_worktree.join(".env").exists(),
        ".env should be copied to feature worktree"
    );
}

/// Test that worktrees nested inside the source are not copied (GitHub issue #641)
///
/// When worktree-path is configured to place worktrees inside the primary worktree
/// (e.g., `.worktrees/{{ branch | sanitize }}`), copy-ignored should NOT copy
/// those nested worktrees, as this would cause recursive copying.
#[rstest]
fn test_copy_ignored_skips_nested_worktrees(mut repo: TestRepo) {
    // Create a .worktrees directory inside the main repo (simulating worktree-path = ".worktrees/...")
    let nested_worktrees_dir = repo.root_path().join(".worktrees");
    fs::create_dir_all(&nested_worktrees_dir).unwrap();

    // Create a worktree inside .worktrees/ using raw git commands
    let nested_worktree_path = nested_worktrees_dir.join("feature-nested");
    repo.git_command()
        .args([
            "worktree",
            "add",
            "-b",
            "feature-nested",
            nested_worktree_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    // Add .worktrees to .gitignore (typical for this setup)
    fs::write(repo.root_path().join(".gitignore"), ".worktrees/\n").unwrap();

    // Also create a regular ignored file that SHOULD be copied
    fs::write(repo.root_path().join(".env"), "SECRET=value").unwrap();
    fs::write(repo.root_path().join(".gitignore"), ".worktrees/\n.env\n").unwrap();

    // Create another worktree (outside the main repo, using default path)
    let dest_path = repo.add_worktree("destination");

    // Run copy-ignored
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["copy-ignored"],
        Some(&dest_path),
    ));

    // Verify: .env was copied (regular ignored file)
    assert!(
        dest_path.join(".env").exists(),
        ".env should be copied to destination"
    );

    // Verify: .worktrees was NOT copied (contains a worktree)
    assert!(
        !dest_path.join(".worktrees").exists(),
        ".worktrees directory should NOT be copied (contains nested worktree)"
    );
}
