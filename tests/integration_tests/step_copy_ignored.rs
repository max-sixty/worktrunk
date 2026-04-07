//! Integration tests for `wt step copy-ignored`

use crate::common::{TestRepo, make_snapshot_cmd, make_snapshot_cmd_with_global_flags, repo};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::fs;
use std::path::{Path, PathBuf};
const CUSTOM_COPY_IGNORED_EXCLUDE_CONFIG: &str = r#"[step.copy-ignored]
exclude = [".custom-cache/"]
"#;

fn run_copy_ignored(repo: &TestRepo, feature_path: &Path) -> std::process::Output {
    let output = repo
        .wt_command()
        .args(["step", "copy-ignored"])
        .current_dir(feature_path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "copy-ignored should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn run_copy_ignored_single_entry(repo: &TestRepo, feature_path: &Path) {
    let output = run_copy_ignored(repo, feature_path);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Copied 1 file"),
        "expected one copied file: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_worktree_project_config(worktree_path: &Path, contents: &str) {
    let config_dir = worktree_path.join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(config_dir.join("wt.toml"), contents).unwrap();
}

fn assert_copy_ignored_excluded(feature_path: &Path, excluded_dirs: &[&str], source: &str) {
    assert!(
        feature_path.join(".env").exists(),
        ".env should still be copied"
    );
    for excluded_dir in excluded_dirs {
        assert!(
            !feature_path.join(excluded_dir).exists(),
            "{} should be excluded by {}",
            excluded_dir,
            source
        );
    }
}

fn setup_copy_ignored_exclude_fixture(repo: &mut TestRepo) -> PathBuf {
    let feature_path = repo.add_worktree("feature");

    let ignored_entries = ".env\n.custom-cache/\n";

    fs::create_dir_all(repo.root_path().join(".custom-cache")).unwrap();
    fs::write(repo.root_path().join(".env"), "SECRET=value").unwrap();
    fs::write(
        repo.root_path().join(".custom-cache").join("state.json"),
        "{}",
    )
    .unwrap();
    fs::write(repo.root_path().join(".gitignore"), ignored_entries).unwrap();
    fs::write(repo.root_path().join(".worktreeinclude"), ignored_entries).unwrap();

    feature_path
}

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

#[rstest]
fn test_copy_ignored_excludes_project_config(mut repo: TestRepo) {
    let feature_path = setup_copy_ignored_exclude_fixture(&mut repo);
    repo.write_project_config(CUSTOM_COPY_IGNORED_EXCLUDE_CONFIG);
    write_worktree_project_config(&feature_path, CUSTOM_COPY_IGNORED_EXCLUDE_CONFIG);

    run_copy_ignored_single_entry(&repo, &feature_path);

    assert_copy_ignored_excluded(&feature_path, &[".custom-cache"], "project config");
}

#[rstest]
fn test_copy_ignored_excludes_user_config(mut repo: TestRepo) {
    let feature_path = setup_copy_ignored_exclude_fixture(&mut repo);
    repo.write_test_config(CUSTOM_COPY_IGNORED_EXCLUDE_CONFIG);

    run_copy_ignored_single_entry(&repo, &feature_path);

    assert_copy_ignored_excluded(&feature_path, &[".custom-cache"], "user config");
}

#[rstest]
fn test_copy_ignored_skips_built_in_excluded_dirs(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");
    let ignored_entries = ".conductor/\n.entire/\n.pi/\n.env\n";

    for dir in [".conductor", ".entire", ".pi"] {
        fs::create_dir_all(repo.root_path().join(dir)).unwrap();
        fs::write(repo.root_path().join(dir).join("state.json"), dir).unwrap();
    }
    fs::write(repo.root_path().join(".env"), "SECRET=value").unwrap();
    fs::write(repo.root_path().join(".gitignore"), ignored_entries).unwrap();
    fs::write(repo.root_path().join(".worktreeinclude"), ignored_entries).unwrap();

    run_copy_ignored_single_entry(&repo, &feature_path);

    assert_copy_ignored_excluded(
        &feature_path,
        &[".conductor", ".entire", ".pi"],
        "built-in excludes",
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
    run_copy_ignored(&repo, &feature_path);

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
    run_copy_ignored(&repo, &feature_path);
    run_copy_ignored(&repo, &feature_path);

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
    run_copy_ignored(&repo, &feature_path);

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

    run_copy_ignored(&repo, &feature_path);

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

    run_copy_ignored(&repo, &feature_path);

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

    run_copy_ignored(&repo, &feature_path);

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
        .run()
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
        .run()
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
    run_copy_ignored(&repo, &feature_path);

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

/// Test --force overwrites existing files in destination
#[rstest]
fn test_copy_ignored_force_overwrites(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    // Create .env in main with original content
    fs::write(repo.root_path().join(".env"), "NEW_SECRET=updated").unwrap();
    fs::write(repo.root_path().join(".gitignore"), ".env\n").unwrap();
    fs::write(repo.root_path().join(".worktreeinclude"), ".env\n").unwrap();

    // Create existing .env in feature with different content (e.g., generated by env:setup)
    fs::write(feature_path.join(".env"), "OLD_SECRET=stale").unwrap();

    // Without --force: existing file should NOT be overwritten
    repo.wt_command()
        .args(["step", "copy-ignored"])
        .current_dir(&feature_path)
        .output()
        .unwrap();
    assert_eq!(
        fs::read_to_string(feature_path.join(".env")).unwrap(),
        "OLD_SECRET=stale",
        "Without --force, existing file should not be overwritten"
    );

    // With --force: existing file SHOULD be overwritten
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["copy-ignored", "--force"],
        Some(&feature_path),
    ));
    assert_eq!(
        fs::read_to_string(feature_path.join(".env")).unwrap(),
        "NEW_SECRET=updated",
        "With --force, existing file should be overwritten"
    );
}

/// Test --force works when destination file doesn't exist yet (no-op removal)
#[rstest]
fn test_copy_ignored_force_no_existing(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    // Create .env in main only — feature has no .env
    fs::write(repo.root_path().join(".env"), "SECRET=value").unwrap();
    fs::write(repo.root_path().join(".gitignore"), ".env\n").unwrap();
    fs::write(repo.root_path().join(".worktreeinclude"), ".env\n").unwrap();

    // --force on a fresh worktree should still copy successfully
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["copy-ignored", "--force"],
        Some(&feature_path),
    ));
    assert_eq!(
        fs::read_to_string(feature_path.join(".env")).unwrap(),
        "SECRET=value",
        "With --force, file should be copied even when dest doesn't exist"
    );
}

/// Test --force overwrites files and symlinks inside directories
#[rstest]
fn test_copy_ignored_force_directory(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    // Create target directory with a file and a symlink in main
    let target_dir = repo.root_path().join("target");
    fs::create_dir_all(target_dir.join("debug")).unwrap();
    fs::write(target_dir.join("debug").join("output"), "new content").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("output", target_dir.join("debug").join("link")).unwrap();

    fs::write(repo.root_path().join(".gitignore"), "target/\n").unwrap();
    fs::write(repo.root_path().join(".worktreeinclude"), "target/\n").unwrap();

    // Create existing file and symlink in feature with different content/target
    fs::create_dir_all(feature_path.join("target").join("debug")).unwrap();
    fs::write(
        feature_path.join("target").join("debug").join("output"),
        "old content",
    )
    .unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        "old_target",
        feature_path.join("target").join("debug").join("link"),
    )
    .unwrap();

    // With --force: files and symlinks inside directory should be overwritten
    repo.wt_command()
        .args(["step", "copy-ignored", "--force"])
        .current_dir(&feature_path)
        .output()
        .unwrap();

    assert_eq!(
        fs::read_to_string(feature_path.join("target").join("debug").join("output")).unwrap(),
        "new content",
        "With --force, files inside directories should be overwritten"
    );
    #[cfg(unix)]
    assert_eq!(
        fs::read_link(feature_path.join("target").join("debug").join("link")).unwrap(),
        std::path::PathBuf::from("output"),
        "With --force, symlinks inside directories should be overwritten"
    );
}

/// Test --verbose shows entries being copied (GitHub issue #1084)
#[rstest]
fn test_copy_ignored_verbose(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    // Create gitignored files
    fs::write(repo.root_path().join(".env"), "SECRET=value").unwrap();
    fs::write(repo.root_path().join(".gitignore"), ".env\n").unwrap();

    // Run with -v (global verbose flag)
    assert_cmd_snapshot!(make_snapshot_cmd_with_global_flags(
        &repo,
        "step",
        &["copy-ignored"],
        Some(&feature_path),
        &["-v"],
    ));

    // Verify file was still copied
    assert!(feature_path.join(".env").exists());
}

/// Test --verbose with directory entries (GitHub issue #1084)
#[rstest]
fn test_copy_ignored_verbose_directory(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    // Create target directory with files
    let target_dir = repo.root_path().join("target");
    fs::create_dir_all(target_dir.join("debug")).unwrap();
    fs::write(target_dir.join("debug").join("output"), "binary").unwrap();
    fs::write(repo.root_path().join(".gitignore"), "target/\n").unwrap();

    assert_cmd_snapshot!(make_snapshot_cmd_with_global_flags(
        &repo,
        "step",
        &["copy-ignored"],
        Some(&feature_path),
        &["-v"],
    ));

    assert!(
        feature_path
            .join("target")
            .join("debug")
            .join("output")
            .exists()
    );
}

#[rstest]
fn test_copy_ignored_counts_files_not_entries(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    // Create a directory with multiple files — the summary should count
    // individual files, not top-level entries.
    let target_dir = repo.root_path().join("target");
    fs::create_dir_all(target_dir.join("debug/deps")).unwrap();
    fs::write(target_dir.join("debug/output"), "bin1").unwrap();
    fs::write(target_dir.join("debug/deps/libfoo.rlib"), "lib").unwrap();
    fs::write(target_dir.join("debug/deps/libbar.rlib"), "lib").unwrap();
    fs::write(repo.root_path().join(".gitignore"), "target/\n").unwrap();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["copy-ignored"],
        Some(&feature_path),
    ));
}

/// Test idempotent behavior with broken symlinks after interrupted copy (GitHub issue #1084)
///
/// When ctrl+c interrupts a copy, broken symlinks may remain at the destination.
/// exists() follows symlinks and returns false for broken ones, so a naive check
/// would try to create a new symlink and fail with EEXIST.
#[cfg(unix)]
#[rstest]
fn test_copy_ignored_broken_symlink_idempotent(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    // Create directory with a symlink in main
    let target_dir = repo.root_path().join("target");
    fs::create_dir_all(&target_dir).unwrap();
    std::os::unix::fs::symlink("nonexistent", target_dir.join("link")).unwrap();

    fs::write(repo.root_path().join(".gitignore"), "target/\n").unwrap();

    // Simulate interrupted copy: create destination with a broken symlink
    let dest_target = feature_path.join("target");
    fs::create_dir_all(&dest_target).unwrap();
    std::os::unix::fs::symlink("old_target", dest_target.join("link")).unwrap();

    // Verify the broken symlink exists (symlink_metadata succeeds, but exists() returns false)
    assert!(dest_target.join("link").symlink_metadata().is_ok());

    // Run copy-ignored — should succeed (skip existing symlink), not fail with EEXIST
    let output = repo
        .wt_command()
        .args(["step", "copy-ignored"])
        .current_dir(&feature_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "copy-ignored should succeed with broken symlink at destination: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Test that non-regular files (sockets) inside directories are skipped (GitHub issue #1084)
///
/// node_modules and similar directories can contain sockets or FIFOs.
/// These should be silently skipped instead of failing with
/// "source path is not an existing regular file".
#[cfg(unix)]
#[rstest]
fn test_copy_ignored_skips_non_regular_files(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    // Create target directory with a socket and a regular file
    let target_dir = repo.root_path().join("target");
    fs::create_dir_all(&target_dir).unwrap();
    let socket_path = target_dir.join("test.sock");
    let _listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
    fs::write(target_dir.join("data.txt"), "content").unwrap();

    fs::write(repo.root_path().join(".gitignore"), "target/\n").unwrap();

    // Should succeed, skipping the socket
    let output = repo
        .wt_command()
        .args(["step", "copy-ignored"])
        .current_dir(&feature_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "copy-ignored should succeed with socket in directory: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Regular file should be copied, socket should NOT be copied
    assert!(feature_path.join("target").join("data.txt").exists());
    assert!(!feature_path.join("target").join("test.sock").exists());
}

/// Test that top-level symlinks are copied as symlinks, not as regular files (GitHub issue #1488)
///
/// When `git ls-files` lists a symlink as a top-level entry (not inside a directory),
/// it should be recreated as a symlink in the destination, not copied as a regular file.
#[cfg(unix)]
#[rstest]
fn test_copy_ignored_preserves_top_level_symlinks(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    // Create a regular file and a symlink to it, both gitignored
    fs::write(repo.root_path().join("test_file"), "content").unwrap();
    std::os::unix::fs::symlink("test_file", repo.root_path().join("symlink_to_test_file")).unwrap();

    fs::write(
        repo.root_path().join(".gitignore"),
        "test_file\nsymlink_to_test_file\n",
    )
    .unwrap();

    let output = repo
        .wt_command()
        .args(["step", "copy-ignored"])
        .current_dir(&feature_path)
        .output()
        .unwrap();

    assert!(output.status.success());

    // The symlink should be preserved as a symlink, not copied as a regular file
    let dest_symlink = feature_path.join("symlink_to_test_file");
    assert!(
        dest_symlink
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink(),
        "symlink_to_test_file should be a symlink, not a regular file"
    );
    assert_eq!(
        fs::read_link(&dest_symlink).unwrap(),
        std::path::PathBuf::from("test_file"),
        "symlink target should be preserved"
    );

    // The regular file should also be copied
    assert!(feature_path.join("test_file").exists());

    // Run again to exercise the idempotent skip path (symlink already exists)
    let output2 = repo
        .wt_command()
        .args(["step", "copy-ignored"])
        .current_dir(&feature_path)
        .output()
        .unwrap();

    assert!(output2.status.success());

    // Symlink should still be intact
    assert!(
        dest_symlink
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink(),
        "symlink should survive idempotent re-run"
    );
}

/// Test that symlinks inside directories are copied correctly
#[cfg(unix)]
#[rstest]
fn test_copy_ignored_directory_with_symlinks(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    // Create a gitignored directory containing a symlink
    let target_dir = repo.root_path().join("target");
    fs::create_dir_all(&target_dir).unwrap();
    fs::write(target_dir.join("data.txt"), "content").unwrap();
    std::os::unix::fs::symlink("data.txt", target_dir.join("link.txt")).unwrap();

    fs::write(repo.root_path().join(".gitignore"), "target/\n").unwrap();

    let output = repo
        .wt_command()
        .args(["step", "copy-ignored"])
        .current_dir(&feature_path)
        .output()
        .unwrap();

    assert!(output.status.success());

    // Both the regular file and the symlink should be copied
    assert!(feature_path.join("target").join("data.txt").exists());
    let link = feature_path.join("target").join("link.txt");
    assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
    assert_eq!(fs::read_link(&link).unwrap().to_str().unwrap(), "data.txt");
}

/// Test that copy errors include file paths in the message (GitHub issue #1084)
///
/// Tests both the directory recursive copy error path (copy_dir_recursive_fallback)
/// and the top-level file copy error path (step_copy_ignored main loop).
#[cfg(unix)]
#[rstest]
fn test_copy_ignored_error_includes_path_directory(mut repo: TestRepo) {
    use std::os::unix::fs::PermissionsExt;

    let feature_path = repo.add_worktree("feature");

    // Create target directory with files
    let target_dir = repo.root_path().join("target");
    fs::create_dir_all(target_dir.join("sub")).unwrap();
    fs::write(target_dir.join("sub").join("file.txt"), "content").unwrap();

    fs::write(repo.root_path().join(".gitignore"), "target/\n").unwrap();

    // Create destination target/sub as read-only so file copy fails
    let dest_sub = feature_path.join("target").join("sub");
    fs::create_dir_all(&dest_sub).unwrap();
    fs::set_permissions(&dest_sub, fs::Permissions::from_mode(0o555)).unwrap();

    // Copy should fail — error message should mention the file path
    let output = repo
        .wt_command()
        .args(["step", "copy-ignored"])
        .current_dir(&feature_path)
        .output()
        .unwrap();

    // Restore permissions for cleanup
    fs::set_permissions(&dest_sub, fs::Permissions::from_mode(0o755)).unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("copying"),
        "Error should mention the file path, got: {stderr}"
    );
}

/// Test that top-level file copy errors include file paths (GitHub issue #1084)
///
/// This exercises the error path in the main copy loop (not copy_dir_recursive_fallback).
#[cfg(unix)]
#[rstest]
fn test_copy_ignored_error_includes_path_file(mut repo: TestRepo) {
    use std::os::unix::fs::PermissionsExt;

    let feature_path = repo.add_worktree("feature");

    // Create a top-level file that's gitignored
    fs::write(repo.root_path().join(".env"), "SECRET=value").unwrap();
    fs::write(repo.root_path().join(".gitignore"), ".env\n").unwrap();

    // Make the destination worktree root read-only so reflink_or_copy fails
    // with PermissionDenied (not AlreadyExists)
    fs::set_permissions(&feature_path, fs::Permissions::from_mode(0o555)).unwrap();

    let output = repo
        .wt_command()
        .args(["step", "copy-ignored"])
        .current_dir(&feature_path)
        .output()
        .unwrap();

    // Restore permissions for cleanup
    fs::set_permissions(&feature_path, fs::Permissions::from_mode(0o755)).unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("copying") && stderr.contains(".env"),
        "Error should mention the file path, got: {stderr}"
    );
}

/// Test that nested file copy errors mention the parent directory creation failure
///
/// Exercises the `create_dir_all(parent)` error path in the top-level file copy loop.
/// The existing `test_copy_ignored_error_includes_path_file` test uses a top-level file,
/// so it doesn't reach `create_dir_all`.
#[cfg(unix)]
#[rstest]
fn test_copy_ignored_error_nested_file_parent_creation(mut repo: TestRepo) {
    use std::os::unix::fs::PermissionsExt;

    let feature_path = repo.add_worktree("feature");

    // Create a nested file that's individually gitignored (not as a directory)
    let cache_dir = repo.root_path().join("cache");
    fs::create_dir_all(&cache_dir).unwrap();
    fs::write(cache_dir.join("data.json"), "content").unwrap();

    // Gitignore the specific file (not the directory) so git ls-files returns it as a file
    fs::write(repo.root_path().join(".gitignore"), "cache/data.json\n").unwrap();
    fs::write(
        repo.root_path().join(".worktreeinclude"),
        "cache/data.json\n",
    )
    .unwrap();

    // Make feature worktree root read-only so create_dir_all("cache/") fails
    fs::set_permissions(&feature_path, fs::Permissions::from_mode(0o555)).unwrap();

    let output = repo
        .wt_command()
        .args(["step", "copy-ignored"])
        .current_dir(&feature_path)
        .output()
        .unwrap();

    // Restore permissions for cleanup
    fs::set_permissions(&feature_path, fs::Permissions::from_mode(0o755)).unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("creating directory for") && stderr.contains("cache"),
        "Error should mention parent directory creation, got: {stderr}"
    );
}

/// Test that broken symlinks at the destination don't prevent copying regular files (GitHub #1547)
///
/// When copy-ignored runs and the destination already has a broken symlink where a
/// regular file would be copied, reflink_or_copy follows the symlink and gets ENOENT
/// instead of EEXIST. This should be treated as "already exists" (skip), not an error.
#[cfg(unix)]
#[rstest]
fn test_copy_ignored_broken_symlink_at_dest_for_regular_file(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    // Create a regular file in main that's gitignored
    fs::write(repo.root_path().join(".env"), "SECRET=value").unwrap();
    fs::write(repo.root_path().join(".gitignore"), ".env\n").unwrap();

    // Create a broken symlink at the destination where the file would be copied.
    // The target path has a non-existent parent dir so fs::copy fails with ENOENT.
    std::os::unix::fs::symlink("/nonexistent/path/file", feature_path.join(".env")).unwrap();

    // Verify it's a broken symlink (symlink_metadata succeeds, but exists() returns false)
    assert!(feature_path.join(".env").symlink_metadata().is_ok());
    assert!(!feature_path.join(".env").exists());

    // copy-ignored should succeed, skipping the broken symlink
    let output = repo
        .wt_command()
        .args(["step", "copy-ignored"])
        .current_dir(&feature_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "copy-ignored should succeed when destination has broken symlink: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Test that broken symlinks inside directories don't prevent copying (GitHub #1547)
#[cfg(unix)]
#[rstest]
fn test_copy_ignored_broken_symlink_in_dir_at_dest(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    // Create target directory with a regular file in main
    let target_dir = repo.root_path().join("target");
    fs::create_dir_all(&target_dir).unwrap();
    fs::write(target_dir.join("data.txt"), "content").unwrap();
    fs::write(repo.root_path().join(".gitignore"), "target/\n").unwrap();

    // Create destination target dir with a broken symlink where data.txt would go
    let dest_target = feature_path.join("target");
    fs::create_dir_all(&dest_target).unwrap();
    std::os::unix::fs::symlink("/nonexistent/path/file", dest_target.join("data.txt")).unwrap();

    // copy-ignored should succeed, skipping the broken symlink
    let output = repo
        .wt_command()
        .args(["step", "copy-ignored"])
        .current_dir(&feature_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "copy-ignored should succeed when dir entry has broken symlink at dest: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Test that directory permissions are preserved during copy (GitHub issue #1589)
///
/// When copying gitignored directories, the destination directories should have the
/// same permissions as the source directories. For example, a directory with mode 0700
/// should not become 0755 in the destination.
#[cfg(unix)]
#[rstest]
fn test_copy_ignored_preserves_directory_permissions(mut repo: TestRepo) {
    use std::os::unix::fs::PermissionsExt;

    let feature_path = repo.add_worktree("feature");

    // Create a directory with restrictive permissions (0700)
    let test_dir = repo.root_path().join("test");
    fs::create_dir_all(&test_dir).unwrap();
    fs::write(test_dir.join("file"), "content").unwrap();
    fs::set_permissions(&test_dir, fs::Permissions::from_mode(0o700)).unwrap();

    // Add to .gitignore
    fs::write(repo.root_path().join(".gitignore"), "test\n").unwrap();

    // Run copy-ignored
    let output = repo
        .wt_command()
        .args(["step", "copy-ignored"])
        .current_dir(&feature_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "copy-ignored should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify directory was copied
    let dest_dir = feature_path.join("test");
    assert!(dest_dir.exists(), "test directory should be copied");
    assert!(dest_dir.join("file").exists(), "test/file should be copied");

    // Verify directory permissions are preserved (0700, not default 0755)
    let dest_mode = fs::metadata(&dest_dir).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        dest_mode, 0o700,
        "Directory permissions should be preserved (expected 0700, got {dest_mode:04o})"
    );

    // Also verify read-only directories (0o555) are handled correctly.
    // Permissions must be set AFTER copying contents, otherwise the destination
    // becomes read-only before files are copied into it.
    let readonly_dir = repo.root_path().join("readonly");
    fs::create_dir_all(&readonly_dir).unwrap();
    fs::write(readonly_dir.join("data"), "content").unwrap();
    fs::set_permissions(&readonly_dir, fs::Permissions::from_mode(0o555)).unwrap();
    fs::write(repo.root_path().join(".gitignore"), "test\nreadonly\n").unwrap();

    let output = repo
        .wt_command()
        .args(["step", "copy-ignored"])
        .current_dir(&feature_path)
        .output()
        .unwrap();
    let dest_readonly = feature_path.join("readonly");
    assert!(
        output.status.success(),
        "copy-ignored should handle read-only source dirs: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Check permissions BEFORE restoring for cleanup
    let dest_readonly_mode = fs::metadata(&dest_readonly).unwrap().permissions().mode() & 0o777;
    // Restore for cleanup
    fs::set_permissions(&readonly_dir, fs::Permissions::from_mode(0o755)).unwrap();
    if dest_readonly.exists() {
        fs::set_permissions(&dest_readonly, fs::Permissions::from_mode(0o755)).unwrap();
    }
    assert_eq!(
        dest_readonly_mode, 0o555,
        "Read-only directory permissions should be preserved (expected 0555, got {dest_readonly_mode:04o})"
    );
}

/// Test that file executable permissions are preserved during copy (GitHub issue #1936)
///
/// When copying gitignored files, the destination files should have the same
/// permissions as the source files. For example, a file with mode 0755 (executable)
/// should not become 0644 in the destination.
#[cfg(unix)]
#[rstest]
fn test_copy_ignored_preserves_file_executable_permissions(mut repo: TestRepo) {
    use std::os::unix::fs::PermissionsExt;

    let feature_path = repo.add_worktree("feature");

    // Create an ignored directory with an executable file (simulates node_modules binaries)
    let bin_dir = repo.root_path().join("node_modules/.bin");
    fs::create_dir_all(&bin_dir).unwrap();
    fs::write(bin_dir.join("playwright"), "#!/bin/sh\necho playwright").unwrap();
    fs::set_permissions(
        bin_dir.join("playwright"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();

    // Also create a non-executable file for comparison
    fs::write(bin_dir.join("config.json"), r#"{"key": "value"}"#).unwrap();

    // Create a top-level ignored executable file (exercises the individual file copy
    // path in step_commands.rs, separate from the recursive directory copy in copy.rs)
    fs::write(
        repo.root_path().join("run-tests.sh"),
        "#!/bin/sh\necho running tests",
    )
    .unwrap();
    fs::set_permissions(
        repo.root_path().join("run-tests.sh"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();

    // Add to .gitignore
    fs::write(
        repo.root_path().join(".gitignore"),
        "node_modules\nrun-tests.sh\n",
    )
    .unwrap();

    // Run copy-ignored
    let output = repo
        .wt_command()
        .args(["step", "copy-ignored"])
        .current_dir(&feature_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "copy-ignored should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify executable file inside directory was copied with permissions preserved
    // (exercises copy.rs copy_dir_recursive_inner path)
    let dest_exec = feature_path.join("node_modules/.bin/playwright");
    assert!(dest_exec.exists(), "executable file should be copied");
    let dest_mode = fs::metadata(&dest_exec).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        dest_mode, 0o755,
        "Executable file permissions should be preserved (expected 0755, got {dest_mode:04o})"
    );

    // Verify non-executable file retains its permissions too
    let dest_config = feature_path.join("node_modules/.bin/config.json");
    assert!(dest_config.exists(), "non-executable file should be copied");
    let config_mode = fs::metadata(&dest_config).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        config_mode, 0o644,
        "Non-executable file permissions should be preserved (expected 0644, got {config_mode:04o})"
    );

    // Verify top-level executable file was copied with permissions preserved
    // (exercises step_commands.rs individual file copy path)
    let dest_script = feature_path.join("run-tests.sh");
    assert!(
        dest_script.exists(),
        "top-level executable should be copied"
    );
    let script_mode = fs::metadata(&dest_script).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        script_mode, 0o755,
        "Top-level executable permissions should be preserved (expected 0755, got {script_mode:04o})"
    );
}

/// Test that VCS metadata directories are excluded from copy-ignored (GitHub issue #1249)
///
/// VCS metadata directories like `.jj` (Jujutsu), `.hg` (Mercurial) contain internal
/// state tied to a specific working directory. Copying them between worktrees breaks
/// the colocated VCS.
#[rstest]
fn test_copy_ignored_skips_vcs_metadata_dirs(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    // Create VCS metadata directories that are gitignored
    let jj_dir = repo.root_path().join(".jj");
    fs::create_dir_all(jj_dir.join("repo")).unwrap();
    fs::write(jj_dir.join("repo/store"), "jj internal state").unwrap();

    let hg_dir = repo.root_path().join(".hg");
    fs::create_dir_all(&hg_dir).unwrap();
    fs::write(hg_dir.join("dirstate"), "hg internal state").unwrap();

    // Also create a regular ignored file that SHOULD be copied
    fs::write(repo.root_path().join(".env"), "SECRET=value").unwrap();

    fs::write(repo.root_path().join(".gitignore"), ".jj/\n.hg/\n.env\n").unwrap();

    // Run copy-ignored
    run_copy_ignored(&repo, &feature_path);

    // Verify: .env was copied (regular ignored file)
    assert!(
        feature_path.join(".env").exists(),
        ".env should be copied to destination"
    );

    // Verify: .jj was NOT copied (VCS metadata)
    assert!(
        !feature_path.join(".jj").exists(),
        ".jj directory should NOT be copied (VCS metadata)"
    );

    // Verify: .hg was NOT copied (VCS metadata)
    assert!(
        !feature_path.join(".hg").exists(),
        ".hg directory should NOT be copied (VCS metadata)"
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
        .run()
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

/// Regression test for EMFILE ("too many open files") when copying many ignored
/// directories concurrently. The fix ensures step_copy_ignored creates a single
/// shared thread pool that copy_dir_recursive reuses, rather than each directory
/// spawning its own pool.
#[rstest]
fn test_copy_ignored_many_directories_no_emfile(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    // Create enough ignored directories to trigger concurrent pool pressure.
    // 200 directories × 10 files = 2000 files exercises the parallel copy path.
    let mut gitignore_entries = String::new();
    let mut worktreeinclude_entries = String::new();
    for i in 0..200 {
        let dir_name = format!("ignored-dir-{i}/");
        let dir = repo.root_path().join(format!("ignored-dir-{i}"));
        fs::create_dir_all(&dir).unwrap();
        for j in 0..10 {
            fs::write(
                dir.join(format!("file-{j}.txt")),
                format!("content {i}-{j}"),
            )
            .unwrap();
        }
        gitignore_entries.push_str(&dir_name);
        gitignore_entries.push('\n');
        worktreeinclude_entries.push_str(&dir_name);
        worktreeinclude_entries.push('\n');
    }
    fs::write(repo.root_path().join(".gitignore"), &gitignore_entries).unwrap();
    fs::write(
        repo.root_path().join(".worktreeinclude"),
        &worktreeinclude_entries,
    )
    .unwrap();

    run_copy_ignored(&repo, &feature_path);

    // Spot-check a sample of copied directories
    for i in [0, 99, 199] {
        for j in [0, 5, 9] {
            let dst_file = feature_path.join(format!("ignored-dir-{i}/file-{j}.txt"));
            assert!(
                dst_file.exists(),
                "ignored-dir-{i}/file-{j}.txt should be copied"
            );
            assert_eq!(
                fs::read_to_string(&dst_file).unwrap(),
                format!("content {i}-{j}"),
                "ignored-dir-{i}/file-{j}.txt content should match"
            );
        }
    }
}
