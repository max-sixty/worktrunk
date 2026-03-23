use crate::common::{
    BareRepoTest, TestRepo, TestRepoBase, canonicalize, configure_directive_file,
    configure_git_cmd, directive_file, repo, setup_temp_snapshot_settings, wait_for,
    wait_for_file_content, wait_for_file_count, wt_command,
};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[test]
fn test_bare_repo_list_worktrees() {
    let test = BareRepoTest::new();

    // Create worktrees inside bare repo matching template: {{ branch }}
    // Worktrees are at repo/main and repo/feature
    let main_worktree = test.create_worktree("main", "main");
    test.commit_in(&main_worktree, "Initial commit on main");

    let feature_worktree = test.create_worktree("feature", "feature");
    test.commit_in(&feature_worktree, "Work on feature");

    let settings = setup_temp_snapshot_settings(test.temp_path());
    settings.bind(|| {
        // Run wt list from the main worktree
        let mut cmd = wt_command();
        test.configure_wt_cmd(&mut cmd);
        cmd.arg("list").current_dir(&main_worktree);

        assert_cmd_snapshot!(cmd);
    });
}

#[test]
fn test_bare_repo_list_shows_no_bare_entry() {
    let test = BareRepoTest::new();

    // Create one worktree
    let main_worktree = test.create_worktree("main", "main");
    test.commit_in(&main_worktree, "Initial commit");

    // Run wt list and verify bare repo is NOT shown (only main worktree appears)
    let settings = setup_temp_snapshot_settings(test.temp_path());
    settings.bind(|| {
        let mut cmd = wt_command();
        test.configure_wt_cmd(&mut cmd);
        cmd.arg("list").current_dir(&main_worktree);

        assert_cmd_snapshot!(cmd);
    });
}

#[test]
fn test_bare_repo_switch_creates_worktree() {
    let test = BareRepoTest::new();

    // Create initial worktree
    let main_worktree = test.create_worktree("main", "main");
    test.commit_in(&main_worktree, "Initial commit");

    // Run wt switch --create to create a new worktree
    // Config uses {{ branch }} template, so worktrees are created inside bare repo
    let (directive_path, _guard) = directive_file();
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    configure_directive_file(&mut cmd, &directive_path);
    cmd.args(["switch", "--create", "feature"])
        .current_dir(&main_worktree);

    let output = cmd.output().unwrap();

    if !output.status.success() {
        panic!(
            "wt switch failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Verify the new worktree was created inside the bare repo
    // Template: {{ branch }} -> repo/feature
    let expected_path = test.bare_repo_path().join("feature");
    assert!(
        expected_path.exists(),
        "Expected worktree at {:?}",
        expected_path
    );

    // Verify git worktree list shows both worktrees (but not bare repo)
    let output = test
        .git_command(test.bare_repo_path())
        .args(["worktree", "list"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should show 3 entries: bare repo + 2 worktrees
    assert_eq!(stdout.lines().count(), 3);
}

#[test]
fn test_bare_repo_switch_with_configured_naming() {
    let test = BareRepoTest::new();

    // Create initial worktree
    let main_worktree = test.create_worktree("main", "main");
    test.commit_in(&main_worktree, "Initial commit");

    // Config uses "{{ branch }}" template, so worktrees are created inside bare repo
    let (directive_path, _guard) = directive_file();
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    configure_directive_file(&mut cmd, &directive_path);
    cmd.args(["switch", "--create", "feature"])
        .current_dir(&main_worktree);

    let output = cmd.output().unwrap();

    if !output.status.success() {
        panic!(
            "wt switch failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Verify worktree was created inside bare repo
    let expected_path = test.bare_repo_path().join("feature");
    assert!(
        expected_path.exists(),
        "Expected worktree at {:?}",
        expected_path
    );
}

#[test]
fn test_bare_repo_remove_worktree() {
    let test = BareRepoTest::new();

    // Create two worktrees
    let main_worktree = test.create_worktree("main", "main");
    test.commit_in(&main_worktree, "Initial commit");

    let feature_worktree = test.create_worktree("feature", "feature");
    test.commit_in(&feature_worktree, "Feature work");

    // Remove feature worktree from main worktree
    let (directive_path, _guard) = directive_file();
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    configure_directive_file(&mut cmd, &directive_path);
    cmd.args(["remove", "feature", "--foreground"])
        .current_dir(&main_worktree);

    let output = cmd.output().unwrap();

    if !output.status.success() {
        panic!(
            "wt remove failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Verify feature worktree was removed
    assert!(
        !feature_worktree.exists(),
        "Feature worktree should be removed"
    );

    // Verify main worktree still exists
    assert!(main_worktree.exists());
}

#[test]
fn test_bare_repo_identifies_primary_correctly() {
    let test = BareRepoTest::new();

    // Create multiple worktrees
    let main_worktree = test.create_worktree("main", "main");
    test.commit_in(&main_worktree, "Main commit");

    let _feature1 = test.create_worktree("feature1", "feature1");
    let _feature2 = test.create_worktree("feature2", "feature2");

    // Run wt list to see which is marked as primary
    let settings = setup_temp_snapshot_settings(test.temp_path());
    settings.bind(|| {
        let mut cmd = wt_command();
        test.configure_wt_cmd(&mut cmd);
        cmd.arg("list").current_dir(&main_worktree);

        assert_cmd_snapshot!(cmd);
    });
}

#[test]
fn test_bare_repo_path_used_for_worktree_paths() {
    let test = BareRepoTest::new();

    // Create initial worktree
    let main_worktree = test.create_worktree("main", "main");
    test.commit_in(&main_worktree, "Initial commit");

    // Create new worktree - config uses {{ branch }} template
    // Worktrees are created inside the bare repo directory
    let (directive_path, _guard) = directive_file();
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    configure_directive_file(&mut cmd, &directive_path);
    cmd.args(["switch", "--create", "dev"])
        .current_dir(&main_worktree);

    cmd.output().unwrap();

    // Verify path is created inside bare repo (using repo_path as base)
    // Template: {{ branch }} -> repo/dev
    let expected = test.bare_repo_path().join("dev");
    assert!(
        expected.exists(),
        "Worktree should be created using repo_path: {:?}",
        expected
    );

    // Should NOT be relative to main worktree's directory (as if it were a non-bare repo)
    let wrong_path = main_worktree.parent().unwrap().join("main.dev");
    assert!(
        !wrong_path.exists(),
        "Worktree should not use worktree directory as base"
    );
}

#[test]
fn test_bare_repo_with_repo_path_variable() {
    // Test that {{ repo_path }} resolves correctly in bare repos
    // For bare repos, repo_path should be the bare repo directory itself
    let test = BareRepoTest::new();

    // Override config to use {{ repo_path }} explicitly
    fs::write(
        test.config_path(),
        "worktree-path = \"{{ repo_path }}/../worktrees/{{ branch | sanitize }}\"\n",
    )
    .unwrap();

    // Create initial worktree
    let main_worktree = test.create_worktree("main", "main");
    test.commit_in(&main_worktree, "Initial commit");

    // Create new worktree using wt switch
    let (directive_path, _guard) = directive_file();
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    configure_directive_file(&mut cmd, &directive_path);
    cmd.args(["switch", "--create", "feature/auth"])
        .current_dir(&main_worktree);

    let output = cmd.output().unwrap();

    if !output.status.success() {
        panic!(
            "wt switch failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Verify worktree was created at sibling path using {{ repo_path }}/../worktrees/
    // Bare repo is at /tmp/xxx/repo, so worktree should be at /tmp/xxx/worktrees/feature-auth
    let expected_path = test
        .bare_repo_path()
        .parent()
        .unwrap()
        .join("worktrees")
        .join("feature-auth");
    assert!(
        expected_path.exists(),
        "Expected worktree at {:?} (using repo_path variable)",
        expected_path
    );
}

#[rstest]
fn test_bare_repo_equivalent_to_normal_repo(repo: TestRepo) {
    // This test verifies that bare repos behave identically to normal repos
    // from the user's perspective

    // Remove fixture worktrees to get a clean state with just main
    for branch in &["feature-a", "feature-b", "feature-c"] {
        let worktree_path = repo
            .root_path()
            .parent()
            .unwrap()
            .join(format!("repo.{}", branch));
        if worktree_path.exists() {
            repo.git_command()
                .args([
                    "worktree",
                    "remove",
                    "--force",
                    worktree_path.to_str().unwrap(),
                ])
                .output()
                .unwrap();
        }
    }

    // Set up bare repo
    let bare_test = BareRepoTest::new();
    let bare_main = bare_test.create_worktree("main", "main");
    bare_test.commit_in(&bare_main, "Commit in bare repo");

    // Set up normal repo (using fixture)
    repo.commit("Commit in normal repo");

    // Configure both with same worktree path pattern
    let config = r#"
worktree-path = "{{ branch }}"
"#;
    fs::write(bare_test.config_path(), config).unwrap();
    fs::write(repo.test_config_path(), config).unwrap();

    // List worktrees in both - should show similar structure
    let mut bare_list = wt_command();
    bare_test.configure_wt_cmd(&mut bare_list);
    bare_list.arg("list").current_dir(&bare_main);

    let mut normal_list = wt_command();
    repo.configure_wt_cmd(&mut normal_list);
    normal_list.arg("list").current_dir(repo.root_path());

    let bare_output = bare_list.output().unwrap();
    let normal_output = normal_list.output().unwrap();

    // Both should show 1 worktree (main/main) - table output is on stdout
    let bare_stdout = String::from_utf8_lossy(&bare_output.stdout);
    let normal_stdout = String::from_utf8_lossy(&normal_output.stdout);

    assert!(bare_stdout.contains("main"));
    assert!(normal_stdout.contains("main"));
    assert_eq!(bare_stdout.lines().count(), normal_stdout.lines().count());
}

#[test]
fn test_bare_repo_commands_from_bare_directory() {
    let test = BareRepoTest::new();

    // Create a worktree so the repo has some content
    let main_worktree = test.create_worktree("main", "main");
    test.commit_in(&main_worktree, "Initial commit");

    // Run wt list from the bare repo directory itself (not from a worktree)
    // Should list the worktree even when run from bare repo, not showing bare repo itself
    let settings = setup_temp_snapshot_settings(test.temp_path());
    settings.bind(|| {
        let mut cmd = wt_command();
        test.configure_wt_cmd(&mut cmd);
        cmd.arg("list").current_dir(test.bare_repo_path());

        assert_cmd_snapshot!(cmd);
    });
}

///
/// Skipped on Windows due to file locking issues that prevent worktree removal
/// during background cleanup after merge. The merge functionality itself works
/// correctly - this is a timing/cleanup issue specific to Windows file handles.
#[test]
fn test_bare_repo_merge_workflow() {
    let test = BareRepoTest::new();

    // Create main worktree
    let main_worktree = test.create_worktree("main", "main");
    test.commit_in(&main_worktree, "Initial commit on main");

    // Create feature branch worktree using wt switch
    // Config uses {{ branch }} template, so worktrees are inside bare repo
    let (directive_path, _guard) = directive_file();
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    configure_directive_file(&mut cmd, &directive_path);
    cmd.args(["switch", "--create", "feature"])
        .current_dir(&main_worktree);
    cmd.output().unwrap();

    // Get feature worktree path (template: {{ branch }} -> repo/feature)
    let feature_worktree = test.bare_repo_path().join("feature");
    assert!(feature_worktree.exists());

    // Make a commit in feature worktree
    test.commit_in(&feature_worktree, "Feature work");

    // Merge feature into main (explicitly specify target)
    let (directive_path, _guard) = directive_file();
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    configure_directive_file(&mut cmd, &directive_path);
    cmd.args([
        "merge",
        "main",        // Explicitly specify target branch
        "--no-squash", // Skip squash to avoid LLM dependency
        "--no-verify", // Skip pre-merge hooks
    ])
    .current_dir(&feature_worktree);

    let output = cmd.output().unwrap();

    if !output.status.success() {
        panic!(
            "wt merge failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Wait for background removal to complete
    wait_for("feature worktree removed", || !feature_worktree.exists());

    // Verify main worktree still exists and has the feature commit
    assert!(main_worktree.exists());

    // Check that feature branch commit is now in main
    let log_output = test
        .git_command(&main_worktree)
        .args(["log", "--oneline"])
        .output()
        .unwrap();

    let log = String::from_utf8_lossy(&log_output.stdout);
    assert!(
        log.contains("Feature work"),
        "Main should contain feature commit after merge"
    );
}

#[test]
fn test_bare_repo_background_logs_location() {
    // This test verifies that background operation logs go to the correct location
    // in bare repos (bare_repo/wt/logs/ instead of worktree/.git/wt/logs/)
    let test = BareRepoTest::new();

    // Create main worktree
    let main_worktree = test.create_worktree("main", "main");
    test.commit_in(&main_worktree, "Initial commit");

    // Create feature worktree
    let feature_worktree = test.create_worktree("feature", "feature");
    test.commit_in(&feature_worktree, "Feature work");

    // Run remove in background to test log file location
    let (directive_path, _guard) = directive_file();
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    configure_directive_file(&mut cmd, &directive_path);
    cmd.args(["remove", "feature"]).current_dir(&main_worktree);

    let output = cmd.output().unwrap();

    if !output.status.success() {
        panic!(
            "wt remove failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Wait for background process to create log file (poll instead of fixed sleep)
    // The key test is that the path is correct, not that content was written (background processes are flaky in tests)
    // Log filename has hash suffix: feature-<hash>-remove-<hash>.log
    let log_dir = test.bare_repo_path().join("wt/logs");
    wait_for_file_count(&log_dir, "log", 1);

    // Verify the log file matches expected pattern (feature-*-remove.log)
    // Format: {branch_with_hash}-{op}.log (internal ops don't have hash on suffix)
    let log_files: Vec<_> = std::fs::read_dir(&log_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.starts_with("feature-") && name.ends_with("-remove.log")
        })
        .collect();
    assert_eq!(
        log_files.len(),
        1,
        "Expected exactly one feature-*-remove.log file, found: {:?}",
        log_files
    );

    // Verify it's NOT in the worktree's .git directory (which doesn't exist for linked worktrees)
    let wrong_dir = main_worktree.join(".git/wt/logs");
    assert!(
        !wrong_dir.exists()
            || std::fs::read_dir(&wrong_dir)
                .map(|d| d.count())
                .unwrap_or(0)
                == 0,
        "Log should NOT be in worktree's .git directory"
    );
}

#[test]
fn test_bare_repo_project_config_found_from_bare_root() {
    // Regression test for #1691: project config in the primary worktree should be
    // found when running from the bare repo root directory, not just from within
    // a worktree that contains the config.
    let test = BareRepoTest::new();

    // Create main worktree (the primary worktree for bare repos)
    let main_worktree = test.create_worktree("main", "main");
    test.commit_in(&main_worktree, "Initial commit");

    // Place project config in the primary worktree's .config/wt.toml
    let config_dir = main_worktree.join(".config");
    fs::create_dir_all(&config_dir).unwrap();

    // Use a marker file to prove the hook ran
    let marker_path = test.bare_repo_path().join("hook-ran.marker");
    let marker_str = marker_path.to_str().unwrap().replace('\\', "/");
    fs::write(
        config_dir.join("wt.toml"),
        format!("post-start = \"echo hook-executed > '{}'\"\n", marker_str),
    )
    .unwrap();

    // Commit the config so it's part of the worktree
    let output = test
        .git_command(&main_worktree)
        .args(["add", ".config/wt.toml"])
        .output()
        .unwrap();
    assert!(output.status.success());
    test.commit_in(&main_worktree, "Add project config");

    // Now run `wt switch --create feature` from the bare repo root (NOT from main worktree)
    // This is the scenario described in #1691
    let (directive_path, _guard) = directive_file();
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    configure_directive_file(&mut cmd, &directive_path);
    cmd.args(["switch", "--create", "feature", "--yes"])
        .current_dir(test.bare_repo_path());

    let output = cmd.output().unwrap();

    if !output.status.success() {
        panic!(
            "wt switch failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // The hook from the primary worktree's config should have executed
    wait_for_file_content(&marker_path);
    let content = fs::read_to_string(&marker_path).unwrap();
    assert!(
        content.contains("hook-executed"),
        "Hook from primary worktree config should run when command is invoked from bare root. \
         Marker file content: {:?}",
        content
    );
}

#[test]
fn test_bare_repo_project_config_found_with_dash_c_flag() {
    // Regression test for #1691 (comment): project config in the primary worktree
    // should be found when using `-C <repo>` from an unrelated directory.
    let test = BareRepoTest::new();

    // Create main worktree (the primary worktree for bare repos)
    let main_worktree = test.create_worktree("main", "main");
    test.commit_in(&main_worktree, "Initial commit");

    // Place project config in the primary worktree's .config/wt.toml
    let config_dir = main_worktree.join(".config");
    fs::create_dir_all(&config_dir).unwrap();

    // Use a marker file to prove the hook ran
    let marker_path = test.bare_repo_path().join("hook-ran-c-flag.marker");
    let marker_str = marker_path.to_str().unwrap().replace('\\', "/");
    fs::write(
        config_dir.join("wt.toml"),
        format!("post-start = \"echo hook-executed > '{}'\"\n", marker_str),
    )
    .unwrap();

    // Commit the config so it's part of the worktree
    let output = test
        .git_command(&main_worktree)
        .args(["add", ".config/wt.toml"])
        .output()
        .unwrap();
    assert!(output.status.success());
    test.commit_in(&main_worktree, "Add project config");

    // Run from a completely unrelated directory using -C to point at the bare repo
    let unrelated_dir = tempfile::tempdir().unwrap();
    let (directive_path, _guard) = directive_file();
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    configure_directive_file(&mut cmd, &directive_path);
    cmd.args([
        "-C",
        test.bare_repo_path().to_str().unwrap(),
        "switch",
        "--create",
        "feature-c-flag",
        "--yes",
    ])
    .current_dir(unrelated_dir.path());

    let output = cmd.output().unwrap();

    if !output.status.success() {
        panic!(
            "wt switch -C failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // The hook from the primary worktree's config should have executed
    wait_for_file_content(&marker_path);
    let content = fs::read_to_string(&marker_path).unwrap();
    assert!(
        content.contains("hook-executed"),
        "Hook from primary worktree config should run when using -C flag. \
         Marker file content: {:?}",
        content
    );
}

#[test]
fn test_bare_repo_slashed_branch_with_sanitize() {
    // Test that slashed branch names work with bare repos and the sanitize filter
    // This matches the documented workflow in tips-patterns.md
    let test = BareRepoTest::new();

    // Override config to use sanitize filter (matches documented config)
    fs::write(
        test.config_path(),
        "worktree-path = \"{{ branch | sanitize }}\"\n",
    )
    .unwrap();

    // Create main worktree
    let main_worktree = test.create_worktree("main", "main");
    test.commit_in(&main_worktree, "Initial commit");

    // Create feature branch with slash using wt switch
    let (directive_path, _guard) = directive_file();
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    configure_directive_file(&mut cmd, &directive_path);
    cmd.args(["switch", "--create", "feature/auth"])
        .current_dir(&main_worktree);

    let output = cmd.output().unwrap();

    if !output.status.success() {
        panic!(
            "wt switch failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Verify worktree was created with sanitized name (feature-auth, not feature/auth)
    let expected_path = test.bare_repo_path().join("feature-auth");
    assert!(
        expected_path.exists(),
        "Expected worktree at {:?} (sanitized from feature/auth)",
        expected_path
    );

    // Verify slashed path was NOT created
    let wrong_path = test.bare_repo_path().join("feature/auth");
    assert!(
        !wrong_path.exists(),
        "Should not create nested directory for slashed branch"
    );

    // Verify git branch name is preserved (not sanitized)
    let branch_output = test
        .git_command(&expected_path)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&branch_output.stdout).trim(),
        "feature/auth",
        "Git branch name should be preserved as feature/auth"
    );
}

/// Helper to create a nested bare repository test setup (project/.git pattern)
///
/// This tests the pattern from GitHub issue #313 where users clone with:
/// `git clone --bare <url> project/.git`
struct NestedBareRepoTest {
    temp_dir: tempfile::TempDir,
    /// Path to the parent directory (project/)
    project_path: PathBuf,
    /// Path to the bare repo (project/.git/)
    bare_repo_path: PathBuf,
    test_config_path: PathBuf,
    git_config_path: PathBuf,
}

impl NestedBareRepoTest {
    fn new() -> Self {
        let temp_dir = tempfile::TempDir::new().unwrap();
        // Create project directory
        let project_path = temp_dir.path().join("project");
        fs::create_dir(&project_path).unwrap();

        // Bare repo inside project directory as .git
        let bare_repo_path = project_path.join(".git");
        let test_config_path = temp_dir.path().join("test-config.toml");
        let git_config_path = temp_dir.path().join("test-gitconfig");

        // Write git config with user settings (like TestRepo)
        fs::write(
            &git_config_path,
            "[user]\n\tname = Test User\n\temail = test@example.com\n\
             [advice]\n\tmergeConflict = false\n\tresolveConflict = false\n\
             [init]\n\tdefaultBranch = main\n",
        )
        .unwrap();

        let mut test = Self {
            temp_dir,
            project_path,
            bare_repo_path,
            test_config_path,
            git_config_path,
        };

        // Create bare repository at project/.git
        let mut cmd = Command::new("git");
        cmd.args(["init", "--bare", "--initial-branch", "main"])
            .arg(&test.bare_repo_path);
        test.configure_git_cmd(&mut cmd);
        let output = cmd.output().unwrap();

        if !output.status.success() {
            panic!(
                "Failed to init nested bare repo:\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        // Canonicalize paths
        test.project_path = canonicalize(&test.project_path).unwrap();
        test.bare_repo_path = canonicalize(&test.bare_repo_path).unwrap();

        // Write config with template for worktrees as siblings to .git
        // For nested bare repos (project/.git), we use "../{{ branch }}" to create
        // worktrees at project/main, project/feature (siblings to .git)
        fs::write(
            &test.test_config_path,
            "worktree-path = \"../{{ branch }}\"\n",
        )
        .unwrap();

        test
    }

    fn project_path(&self) -> &PathBuf {
        &self.project_path
    }

    fn bare_repo_path(&self) -> &PathBuf {
        &self.bare_repo_path
    }

    fn config_path(&self) -> &Path {
        &self.test_config_path
    }

    fn temp_path(&self) -> &Path {
        self.temp_dir.path()
    }

    /// Configure a wt command with test environment
    fn configure_wt_cmd(&self, cmd: &mut Command) {
        self.configure_git_cmd(cmd);
        cmd.env("WORKTRUNK_CONFIG_PATH", &self.test_config_path)
            .env_remove("NO_COLOR")
            .env_remove("CLICOLOR_FORCE");
    }

    /// Get test environment variables as a vector for PTY tests.
    #[cfg(all(unix, feature = "shell-integration-tests"))]
    fn test_env_vars(&self) -> Vec<(String, String)> {
        use crate::common::{NULL_DEVICE, STATIC_TEST_ENV_VARS, TEST_EPOCH};

        let mut vars: Vec<(String, String)> = STATIC_TEST_ENV_VARS
            .iter()
            .map(|&(k, v)| (k.to_string(), v.to_string()))
            .collect();

        // HOME and XDG_CONFIG_HOME are needed for config lookups in env_clear'd PTY
        let home = self.temp_dir.path().join("home");
        std::fs::create_dir_all(&home).ok();

        vars.extend([
            (
                "GIT_CONFIG_GLOBAL".to_string(),
                self.git_config_path.display().to_string(),
            ),
            ("GIT_CONFIG_SYSTEM".to_string(), NULL_DEVICE.to_string()),
            (
                "GIT_AUTHOR_DATE".to_string(),
                "2025-01-01T00:00:00Z".to_string(),
            ),
            (
                "GIT_COMMITTER_DATE".to_string(),
                "2025-01-01T00:00:00Z".to_string(),
            ),
            ("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()),
            ("HOME".to_string(), home.display().to_string()),
            (
                "XDG_CONFIG_HOME".to_string(),
                home.join(".config").display().to_string(),
            ),
            ("WORKTRUNK_TEST_EPOCH".to_string(), TEST_EPOCH.to_string()),
            (
                "WORKTRUNK_CONFIG_PATH".to_string(),
                self.test_config_path.display().to_string(),
            ),
            (
                "WORKTRUNK_SYSTEM_CONFIG_PATH".to_string(),
                "/etc/xdg/worktrunk/config.toml".to_string(),
            ),
            (
                "WORKTRUNK_APPROVALS_PATH".to_string(),
                self.temp_dir
                    .path()
                    .join("test-approvals.toml")
                    .display()
                    .to_string(),
            ),
        ]);

        vars
    }
}

impl TestRepoBase for NestedBareRepoTest {
    fn git_config_path(&self) -> &Path {
        &self.git_config_path
    }
}

/// instead of project/.git/ (GitHub issue #313)
#[test]
fn test_nested_bare_repo_worktree_path() {
    let test = NestedBareRepoTest::new();

    // Create first worktree using wt switch --create
    let (directive_path, _guard) = directive_file();
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    configure_directive_file(&mut cmd, &directive_path);
    cmd.args(["switch", "--create", "main"])
        .current_dir(test.bare_repo_path());

    let output = cmd.output().unwrap();

    if !output.status.success() {
        panic!(
            "wt switch --create main failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // CRITICAL: Worktree should be at project/main, NOT project/.git/main
    let expected_path = test.project_path().join("main");
    let wrong_path = test.bare_repo_path().join("main");

    assert!(
        expected_path.exists(),
        "Expected worktree at {:?} (sibling to .git)",
        expected_path
    );
    assert!(
        !wrong_path.exists(),
        "Worktree should NOT be inside .git directory at {:?}",
        wrong_path
    );
}

#[test]
fn test_nested_bare_repo_full_workflow() {
    let test = NestedBareRepoTest::new();

    // Create main worktree
    let (directive_path, _guard) = directive_file();
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    configure_directive_file(&mut cmd, &directive_path);
    cmd.args(["switch", "--create", "main"])
        .current_dir(test.bare_repo_path());
    cmd.output().unwrap();

    let main_worktree = test.project_path().join("main");
    assert!(main_worktree.exists());
    test.commit_in(&main_worktree, "Initial");

    // Create feature worktree
    let (directive_path, _guard) = directive_file();
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    configure_directive_file(&mut cmd, &directive_path);
    cmd.args(["switch", "--create", "feature"])
        .current_dir(&main_worktree);
    cmd.output().unwrap();

    // Feature worktree should be at project/feature
    let feature_worktree = test.project_path().join("feature");
    assert!(
        feature_worktree.exists(),
        "Feature worktree should be at project/feature"
    );

    // List should show both worktrees
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    cmd.arg("list").current_dir(&main_worktree);
    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("main"), "Should list main worktree");
    assert!(stdout.contains("feature"), "Should list feature worktree");

    // Remove feature worktree
    let (directive_path, _guard) = directive_file();
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    configure_directive_file(&mut cmd, &directive_path);
    cmd.args(["remove", "feature", "--foreground"])
        .current_dir(&main_worktree);
    cmd.output().unwrap();

    assert!(
        !feature_worktree.exists(),
        "Feature worktree should be removed"
    );
    assert!(main_worktree.exists());
}

#[test]
fn test_nested_bare_repo_list_snapshot() {
    let test = NestedBareRepoTest::new();

    // Create main worktree
    let (directive_path, _guard) = directive_file();
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    configure_directive_file(&mut cmd, &directive_path);
    cmd.args(["switch", "--create", "main"])
        .current_dir(test.bare_repo_path());
    cmd.output().unwrap();

    let main_worktree = test.project_path().join("main");
    test.commit_in(&main_worktree, "Initial");

    // Create feature worktree
    let (directive_path, _guard) = directive_file();
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    configure_directive_file(&mut cmd, &directive_path);
    cmd.args(["switch", "--create", "feature"])
        .current_dir(&main_worktree);
    cmd.output().unwrap();

    // Take snapshot of list output
    let settings = setup_temp_snapshot_settings(test.temp_path());
    settings.bind(|| {
        let mut cmd = wt_command();
        test.configure_wt_cmd(&mut cmd);
        cmd.arg("list").current_dir(&main_worktree);
        assert_cmd_snapshot!(cmd);
    });
}

#[test]
fn test_bare_repo_bootstrap_first_worktree() {
    // Test that we can create the first worktree in a bare repo using wt switch --create
    // without needing to manually run `git worktree add` first.
    // This tests that load_project_config() returns None for bare repos without worktrees,
    // allowing the bootstrap workflow to proceed.
    let test = BareRepoTest::new();

    // Unlike other tests, we do NOT create any worktrees first.
    // We run wt switch --create directly on the bare repo.

    let (directive_path, _guard) = directive_file();
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    configure_directive_file(&mut cmd, &directive_path);
    cmd.args(["switch", "--create", "main"])
        .current_dir(test.bare_repo_path());

    let output = cmd.output().unwrap();

    if !output.status.success() {
        panic!(
            "wt switch --create main from bare repo with no worktrees failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Verify the worktree was created inside the bare repo
    // Template: {{ branch }} -> repo/main
    let expected_path = test.bare_repo_path().join("main");
    assert!(
        expected_path.exists(),
        "Expected first worktree at {:?}",
        expected_path
    );

    // Verify git worktree list shows the new worktree
    let output = test
        .git_command(test.bare_repo_path())
        .args(["worktree", "list"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should show 2 entries: bare repo + main worktree
    assert_eq!(
        stdout.lines().count(),
        2,
        "Should have bare repo + 1 worktree"
    );
    assert!(stdout.contains("main"), "Should list main worktree");
}

/// Regression test: `wt list` from a `git clone --bare` repo must not run
/// `git status` on the bare entry. Before the fix, this produced:
///   "fatal: this operation must be run in a work tree"
///
/// Uses `git clone --bare` (real-world pattern) rather than `git init --bare`
/// (used by BareRepoTest) to cover the exact reported scenario.
#[test]
fn test_clone_bare_repo_list_no_status_errors() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let git_config_path = temp_dir.path().join("test-gitconfig");
    let test_config_path = temp_dir.path().join("test-config.toml");
    fs::write(
        &git_config_path,
        "[user]\n\tname = Test User\n\temail = test@example.com\n\
         [init]\n\tdefaultBranch = main\n",
    )
    .unwrap();
    fs::write(&test_config_path, "").unwrap();

    let run_git = |dir: &Path, args: &[&str]| {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(dir);
        configure_git_cmd(&mut cmd, &git_config_path);
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    };

    // Create a source repo with a commit (clone --bare needs history)
    let source = temp_dir.path().join("source");
    run_git(
        temp_dir.path(),
        &["init", "--initial-branch", "main", source.to_str().unwrap()],
    );
    fs::write(source.join("file.txt"), "content").unwrap();
    run_git(&source, &["add", "file.txt"]);
    run_git(&source, &["commit", "-m", "Initial commit"]);

    // Clone as bare — the exact pattern from the bug report
    let bare_path = temp_dir.path().join("project.bare");
    run_git(
        temp_dir.path(),
        &[
            "clone",
            "--bare",
            source.to_str().unwrap(),
            bare_path.to_str().unwrap(),
        ],
    );

    // Create linked worktrees (outside the bare dir, matching real usage)
    let main_wt = temp_dir.path().join("main");
    let feature_wt = temp_dir.path().join("feature");
    run_git(
        &bare_path,
        &["worktree", "add", main_wt.to_str().unwrap(), "main"],
    );
    run_git(&bare_path, &["branch", "feature", "main"]);
    run_git(
        &bare_path,
        &["worktree", "add", feature_wt.to_str().unwrap(), "feature"],
    );

    // Run wt list from the bare repo directory (the reported scenario)
    let mut cmd = wt_command();
    configure_git_cmd(&mut cmd, &git_config_path);
    cmd.env("WORKTRUNK_CONFIG_PATH", &test_config_path)
        .arg("list")
        .current_dir(&bare_path);
    let output = cmd.output().unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "wt list should succeed.\nstderr: {stderr}"
    );
    assert!(
        !stderr.contains("must be run in a work tree"),
        "Should not get 'must be run in a work tree' error.\nstderr: {stderr}"
    );
    assert!(
        !stderr.contains("git operations failed"),
        "Should not have git operation failures.\nstderr: {stderr}"
    );
}

/// Regression test for #1618: `wt merge` must not remove the default branch
/// worktree in a bare repo. In bare repos all worktrees are linked, so the
/// `is_linked()` check alone can't protect the primary worktree.
#[test]
fn test_bare_repo_merge_preserves_default_branch_worktree() {
    let test = BareRepoTest::new();

    // Create main (default branch) worktree and a feature worktree at the same commit
    let main_worktree = test.create_worktree("main", "main");
    test.commit_in(&main_worktree, "Initial commit on main");

    // Create feature branch at the same commit as main
    let _feature_worktree = test.create_worktree("feature", "feature");

    // Run `wt merge feature` from the main (default branch) worktree.
    // This attempts to merge main into feature — the important thing is that
    // the main worktree must NOT be removed even though is_linked() returns true.
    let (directive_path, _guard) = directive_file();
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    configure_directive_file(&mut cmd, &directive_path);
    cmd.args([
        "merge",
        "feature",     // Target = feature branch
        "--no-squash", // Skip squash to avoid LLM dependency
        "--no-verify", // Skip hooks
    ])
    .current_dir(&main_worktree);

    let output = cmd.output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    // The merge itself may succeed or show "already up to date", but the key
    // assertion is that the main worktree is preserved (not removed).
    assert!(
        main_worktree.exists(),
        "Default branch worktree must not be removed.\nstderr: {stderr}"
    );

    // Should show "primary worktree" preservation message
    assert!(
        stderr.contains("primary worktree"),
        "Should show primary worktree preservation message.\nstderr: {stderr}"
    );
}

/// Helper: create a NestedBareRepoTest with no worktree-path configured and a main worktree.
///
/// Reuses NestedBareRepoTest's bare repo setup but clears the worktree-path config,
/// so the default template (which references `{{ repo }}`) triggers the bare repo prompt.
fn setup_unconfigured_nested_bare_repo() -> NestedBareRepoTest {
    let test = NestedBareRepoTest::new();

    // Temporarily set worktree-path so the main worktree lands at project/main
    // (without this, the default {{ repo }} template produces .git.main).
    fs::write(
        test.config_path(),
        "worktree-path = \"../{{ branch | sanitize }}\"\n",
    )
    .unwrap();

    // Create main worktree with a commit (needed as a starting point for switch)
    let (directive_path, _guard) = directive_file();
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    configure_directive_file(&mut cmd, &directive_path);
    cmd.args(["switch", "--create", "main", "--yes"])
        .current_dir(test.bare_repo_path());
    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "Failed to create main worktree:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Clear config so the default template applies — the test subject is the bare repo prompt.
    // Skip shell integration prompt so it doesn't interfere (especially in PTY tests).
    fs::write(test.config_path(), "skip-shell-integration-prompt = true\n").unwrap();

    test
}

/// Test that --yes does NOT auto-accept the bare repo config change — it shows
/// the warning and creates the worktree at the unconfigured (bad) path.
#[test]
fn test_bare_repo_worktree_path_prompt_auto_accept() {
    let test = setup_unconfigured_nested_bare_repo();
    let main_worktree = test.project_path().join("main");

    let settings = setup_temp_snapshot_settings(test.temp_path());
    settings.bind(|| {
        let (directive_path, _guard) = directive_file();
        let mut cmd = wt_command();
        test.configure_wt_cmd(&mut cmd);
        configure_directive_file(&mut cmd, &directive_path);
        cmd.args(["switch", "--create", "feature", "--yes"])
            .current_dir(&main_worktree);

        assert_cmd_snapshot!(cmd);
    });

    // Config should NOT have worktree-path — --yes skips the config prompt
    let config_content = fs::read_to_string(test.config_path()).unwrap();
    assert!(
        !config_content.contains("worktree-path"),
        "Config should NOT contain worktree-path — --yes should not auto-configure.\nConfig: {config_content}"
    );

    // Worktree created at the unconfigured path (bad but expected without config)
    let bad_path = test.project_path().join(".git.feature");
    assert!(
        bad_path.exists(),
        "Worktree should be at {:?} (unconfigured default path)",
        bad_path
    );
}

/// Test that non-interactive (piped stdin) shows warning instead of prompt.
#[test]
fn test_bare_repo_worktree_path_prompt_non_interactive_warning() {
    let test = setup_unconfigured_nested_bare_repo();
    let main_worktree = test.project_path().join("main");

    let settings = setup_temp_snapshot_settings(test.temp_path());
    settings.bind(|| {
        let (directive_path, _guard) = directive_file();
        let mut cmd = wt_command();
        test.configure_wt_cmd(&mut cmd);
        configure_directive_file(&mut cmd, &directive_path);
        // No --yes, but stdin is piped (non-interactive) since assert_cmd_snapshot
        // doesn't attach a TTY
        cmd.args(["switch", "--create", "feature"])
            .current_dir(&main_worktree);

        assert_cmd_snapshot!(cmd);
    });
}

// =============================================================================
// PTY-based interactive prompt tests
// =============================================================================

#[cfg(all(unix, feature = "shell-integration-tests"))]
mod bare_repo_prompt_pty {
    use super::*;
    use crate::common::pty::{build_pty_command, exec_cmd_in_pty_prompted};
    use crate::common::{add_pty_binary_path_filters, add_pty_filters, wt_bin};
    use insta::assert_snapshot;

    fn prompt_pty_settings(temp_path: &Path) -> insta::Settings {
        let mut settings = setup_temp_snapshot_settings(temp_path);
        add_pty_filters(&mut settings);
        add_pty_binary_path_filters(&mut settings);
        settings
    }

    #[test]
    fn test_bare_repo_worktree_path_prompt_accept_pty() {
        let test = setup_unconfigured_nested_bare_repo();
        let main_worktree = test.project_path().join("main");
        let env_vars = test.test_env_vars();

        let cmd = build_pty_command(
            wt_bin().to_str().unwrap(),
            &["switch", "--create", "feature"],
            &main_worktree,
            &env_vars,
            None,
        );
        let (output, exit_code) = exec_cmd_in_pty_prompted(cmd, &["y\n"], "[y/N");

        assert_eq!(exit_code, 0);
        prompt_pty_settings(test.temp_path()).bind(|| {
            assert_snapshot!("bare_repo_prompt_accept", &output);
        });

        // Verify config was written
        let config_content = fs::read_to_string(test.config_path()).unwrap();
        assert!(
            config_content.contains("worktree-path"),
            "Config should contain worktree-path override.\nConfig: {config_content}"
        );
    }

    #[test]
    fn test_bare_repo_worktree_path_prompt_decline_pty() {
        let test = setup_unconfigured_nested_bare_repo();
        let main_worktree = test.project_path().join("main");
        let env_vars = test.test_env_vars();

        let cmd = build_pty_command(
            wt_bin().to_str().unwrap(),
            &["switch", "--create", "feature"],
            &main_worktree,
            &env_vars,
            None,
        );
        let (output, exit_code) = exec_cmd_in_pty_prompted(cmd, &["n\n"], "[y/N");

        assert_eq!(exit_code, 0);
        prompt_pty_settings(test.temp_path()).bind(|| {
            assert_snapshot!("bare_repo_prompt_decline", &output);
        });

        // Verify skip flag was saved in git config
        let git_config_output = Command::new("git")
            .args(["config", "worktrunk.skip-bare-repo-prompt"])
            .current_dir(&main_worktree)
            .env("GIT_CONFIG_GLOBAL", test.git_config_path())
            .output()
            .unwrap();
        let value = String::from_utf8_lossy(&git_config_output.stdout);
        assert_eq!(
            value.trim(),
            "true",
            "Skip flag should be saved in git config"
        );
    }

    #[test]
    fn test_bare_repo_worktree_path_prompt_preview_pty() {
        let test = setup_unconfigured_nested_bare_repo();
        let main_worktree = test.project_path().join("main");
        let env_vars = test.test_env_vars();

        let cmd = build_pty_command(
            wt_bin().to_str().unwrap(),
            &["switch", "--create", "feature"],
            &main_worktree,
            &env_vars,
            None,
        );
        // Send ? first to see preview, then n to decline
        let (output, exit_code) = exec_cmd_in_pty_prompted(cmd, &["?\n", "n\n"], "[y/N");

        assert_eq!(exit_code, 0);
        prompt_pty_settings(test.temp_path()).bind(|| {
            assert_snapshot!("bare_repo_prompt_preview", &output);
        });
    }
}
