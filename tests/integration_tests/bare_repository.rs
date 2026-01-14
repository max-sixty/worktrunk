use crate::common::{
    BareRepoTest, TestRepo, TestRepoBase, canonicalize, configure_directive_file, directive_file,
    repo, setup_temp_snapshot_settings, wait_for_file, wt_command,
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
    for _ in 0..50 {
        if !feature_worktree.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(
        !feature_worktree.exists(),
        "Feature worktree should be removed after merge"
    );

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
    // in bare repos (bare_repo/wt-logs/ instead of worktree/.git/wt-logs/)
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
    let log_path = test.bare_repo_path().join("wt-logs/feature-remove.log");
    wait_for_file(&log_path);

    // Verify it's NOT in the worktree's .git directory (which doesn't exist for linked worktrees)
    let wrong_path = main_worktree.join(".git/wt-logs/feature-remove.log");
    assert!(
        !wrong_path.exists(),
        "Log should NOT be in worktree's .git directory"
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
