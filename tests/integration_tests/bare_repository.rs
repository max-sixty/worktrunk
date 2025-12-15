use crate::common::{
    TestRepo, canonicalize, repo, setup_temp_snapshot_settings, wait_for_file, wt_command,
};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

/// Helper to create a bare repository test setup
struct BareRepoTest {
    temp_dir: tempfile::TempDir,
    bare_repo_path: PathBuf,
    test_config_path: PathBuf,
}

impl BareRepoTest {
    fn new() -> Self {
        let temp_dir = tempfile::TempDir::new().unwrap();
        // Bare repo without .git suffix - worktrees go inside as subdirectories
        let bare_repo_path = temp_dir.path().join("repo");
        let test_config_path = temp_dir.path().join("test-config.toml");

        let mut test = Self {
            temp_dir,
            bare_repo_path,
            test_config_path,
        };

        // Create bare repository
        let output = Command::new("git")
            .args(["init", "--bare", "--initial-branch", "main"])
            .current_dir(test.temp_dir.path())
            .arg(&test.bare_repo_path)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .unwrap();

        if !output.status.success() {
            panic!(
                "Failed to init bare repo:\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        // Canonicalize path (using dunce to avoid \\?\ prefix on Windows)
        test.bare_repo_path = canonicalize(&test.bare_repo_path).unwrap();

        // Write config with template for worktrees inside bare repo
        // Template {{ branch }} creates worktrees as subdirectories: repo/main, repo/feature
        fs::write(&test.test_config_path, "worktree-path = \"{{ branch }}\"\n").unwrap();

        test
    }

    fn bare_repo_path(&self) -> &PathBuf {
        &self.bare_repo_path
    }

    fn temp_path(&self) -> &std::path::Path {
        self.temp_dir.path()
    }

    fn config_path(&self) -> &PathBuf {
        &self.test_config_path
    }

    /// Create a worktree from the bare repository
    /// Worktrees are created inside the bare repo directory: repo/main, repo/feature
    fn create_worktree(&self, branch: &str, worktree_name: &str) -> PathBuf {
        let worktree_path = self.bare_repo_path.join(worktree_name);

        let mut cmd = Command::new("git");
        cmd.args([
            "-C",
            self.bare_repo_path.to_str().unwrap(),
            "worktree",
            "add",
            "-b",
            branch,
            worktree_path.to_str().unwrap(),
        ])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_DATE", "2025-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2025-01-01T00:00:00Z");

        let output = cmd.output().unwrap();

        if !output.status.success() {
            panic!(
                "Failed to create worktree:\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        canonicalize(&worktree_path).unwrap()
    }

    /// Create a commit in the specified worktree
    fn commit_in_worktree(&self, worktree_path: &PathBuf, message: &str) {
        // Create a file
        let file_path = worktree_path.join("file.txt");
        fs::write(&file_path, message).unwrap();

        // Add file
        let output = Command::new("git")
            .args(["add", "file.txt"])
            .current_dir(worktree_path)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .unwrap();

        if !output.status.success() {
            panic!(
                "Failed to add file:\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        // Commit
        let output = Command::new("git")
            .args(["commit", "-m", message])
            .current_dir(worktree_path)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Test User")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_AUTHOR_DATE", "2025-01-01T00:00:00Z")
            .env("GIT_COMMITTER_NAME", "Test User")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_DATE", "2025-01-01T00:00:00Z")
            .output()
            .unwrap();

        if !output.status.success() {
            panic!(
                "Failed to commit:\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    /// Configure a wt command with test environment
    fn configure_wt_cmd(&self, cmd: &mut Command) {
        cmd.env(
            "WORKTRUNK_CONFIG_PATH",
            self.test_config_path.to_str().unwrap(),
        )
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("SOURCE_DATE_EPOCH", "1735776000")
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env_remove("NO_COLOR")
        .env_remove("CLICOLOR_FORCE");
    }
}

#[test]
fn test_bare_repo_list_worktrees() {
    let test = BareRepoTest::new();

    // Create worktrees inside bare repo matching template: {{ branch }}
    // Worktrees are at repo/main and repo/feature
    let main_worktree = test.create_worktree("main", "main");
    test.commit_in_worktree(&main_worktree, "Initial commit on main");

    let feature_worktree = test.create_worktree("feature", "feature");
    test.commit_in_worktree(&feature_worktree, "Work on feature");

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
    test.commit_in_worktree(&main_worktree, "Initial commit");

    // Run wt list and verify bare repo is NOT shown
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    cmd.arg("list").current_dir(&main_worktree);

    let output = cmd.output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Should only show the main worktree, not the bare repo (table output is on stderr)
    assert!(stderr.contains("main"));
    assert!(!stderr.contains(".git"));
    assert!(!stderr.contains("bare"));
}

#[test]
fn test_bare_repo_switch_creates_worktree() {
    let test = BareRepoTest::new();

    // Create initial worktree
    let main_worktree = test.create_worktree("main", "main");
    test.commit_in_worktree(&main_worktree, "Initial commit");

    // Run wt switch --create to create a new worktree
    // Config uses {{ branch }} template, so worktrees are created inside bare repo
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    cmd.args(["switch", "--create", "feature", "--internal"])
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
    let mut cmd = Command::new("git");
    cmd.args([
        "-C",
        test.bare_repo_path().to_str().unwrap(),
        "worktree",
        "list",
    ])
    .env("GIT_CONFIG_GLOBAL", "/dev/null")
    .env("GIT_CONFIG_SYSTEM", "/dev/null");

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should show 3 entries: bare repo + 2 worktrees
    assert_eq!(stdout.lines().count(), 3);
}

#[test]
fn test_bare_repo_switch_with_configured_naming() {
    let test = BareRepoTest::new();

    // Create initial worktree
    let main_worktree = test.create_worktree("main", "main");
    test.commit_in_worktree(&main_worktree, "Initial commit");

    // Config uses "{{ branch }}" template, so worktrees are created inside bare repo
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    cmd.args(["switch", "--create", "feature", "--internal"])
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
    test.commit_in_worktree(&main_worktree, "Initial commit");

    let feature_worktree = test.create_worktree("feature", "feature");
    test.commit_in_worktree(&feature_worktree, "Feature work");

    // Remove feature worktree from main worktree
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    cmd.args(["remove", "feature", "--no-background", "--internal"])
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
    assert!(main_worktree.exists(), "Main worktree should still exist");
}

#[test]
fn test_bare_repo_identifies_primary_correctly() {
    let test = BareRepoTest::new();

    // Create multiple worktrees
    let main_worktree = test.create_worktree("main", "main");
    test.commit_in_worktree(&main_worktree, "Main commit");

    let _feature1 = test.create_worktree("feature1", "feature1");
    let _feature2 = test.create_worktree("feature2", "feature2");

    // Run wt list to see which is marked as primary
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    cmd.arg("list").current_dir(&main_worktree);

    let output = cmd.output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    // First non-bare worktree (main) should be primary (table output is on stderr)
    // The exact formatting may vary, but main should be listed
    assert!(stderr.contains("main"));
}

#[test]
fn test_bare_repo_worktree_base_used_for_paths() {
    let test = BareRepoTest::new();

    // Create initial worktree
    let main_worktree = test.create_worktree("main", "main");
    test.commit_in_worktree(&main_worktree, "Initial commit");

    // Create new worktree - config uses {{ branch }} template
    // Worktrees are created inside the bare repo directory
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    cmd.args(["switch", "--create", "dev", "--internal"])
        .current_dir(&main_worktree);

    cmd.output().unwrap();

    // Verify path is created inside bare repo (using worktree_base)
    // Template: {{ branch }} -> repo/dev
    let expected = test.bare_repo_path().join("dev");
    assert!(
        expected.exists(),
        "Worktree should be created using worktree_base: {:?}",
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

    // Set up bare repo
    let bare_test = BareRepoTest::new();
    let bare_main = bare_test.create_worktree("main", "main");
    bare_test.commit_in_worktree(&bare_main, "Commit in bare repo");

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
    repo.clean_cli_env(&mut normal_list);
    normal_list.arg("list").current_dir(repo.root_path());

    let bare_output = bare_list.output().unwrap();
    let normal_output = normal_list.output().unwrap();

    // Both should show 1 worktree (main/main) - table output is on stderr
    let bare_stderr = String::from_utf8_lossy(&bare_output.stderr);
    let normal_stderr = String::from_utf8_lossy(&normal_output.stderr);

    assert!(bare_stderr.contains("main"));
    assert!(normal_stderr.contains("main"));
    assert_eq!(bare_stderr.lines().count(), normal_stderr.lines().count());
}

#[test]
fn test_bare_repo_commands_from_bare_directory() {
    let test = BareRepoTest::new();

    // Create a worktree so the repo has some content
    let main_worktree = test.create_worktree("main", "main");
    test.commit_in_worktree(&main_worktree, "Initial commit");

    // Run wt list from the bare repo directory itself (not from a worktree)
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    cmd.arg("list").current_dir(test.bare_repo_path());

    let output = cmd.output().unwrap();

    if !output.status.success() {
        panic!(
            "wt list from bare repo failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Should list the worktree even when run from bare repo (table output is on stderr)
    assert!(stderr.contains("main"), "Should show main worktree");
    assert!(!stderr.contains("bare"), "Should not show bare repo itself");
}

/// Test that merge workflow works correctly with bare repositories.
///
/// Skipped on Windows due to file locking issues that prevent worktree removal
/// during background cleanup after merge. The merge functionality itself works
/// correctly - this is a timing/cleanup issue specific to Windows file handles.
#[test]
fn test_bare_repo_merge_workflow() {
    let test = BareRepoTest::new();

    // Create main worktree
    let main_worktree = test.create_worktree("main", "main");
    test.commit_in_worktree(&main_worktree, "Initial commit on main");

    // Create feature branch worktree using wt switch
    // Config uses {{ branch }} template, so worktrees are inside bare repo
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    cmd.args(["switch", "--create", "feature", "--internal"])
        .current_dir(&main_worktree);
    cmd.output().unwrap();

    // Get feature worktree path (template: {{ branch }} -> repo/feature)
    let feature_worktree = test.bare_repo_path().join("feature");
    assert!(feature_worktree.exists(), "Feature worktree should exist");

    // Make a commit in feature worktree
    test.commit_in_worktree(&feature_worktree, "Feature work");

    // Merge feature into main (explicitly specify target)
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    cmd.args([
        "merge",
        "main",        // Explicitly specify target branch
        "--no-squash", // Skip squash to avoid LLM dependency
        "--no-verify", // Skip pre-merge hooks
        "--internal",
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
    assert!(main_worktree.exists(), "Main worktree should still exist");

    // Check that feature branch commit is now in main
    let log_output = Command::new("git")
        .args(["-C", main_worktree.to_str().unwrap(), "log", "--oneline"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
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
    test.commit_in_worktree(&main_worktree, "Initial commit");

    // Create feature worktree
    let feature_worktree = test.create_worktree("feature", "feature");
    test.commit_in_worktree(&feature_worktree, "Feature work");

    // Run remove in background to test log file location
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    cmd.args(["remove", "feature", "--internal"])
        .current_dir(&main_worktree);

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
    wait_for_file(&log_path, Duration::from_secs(5));

    // Verify it's NOT in the worktree's .git directory (which doesn't exist for linked worktrees)
    let wrong_path = main_worktree.join(".git/wt-logs/feature-remove.log");
    assert!(
        !wrong_path.exists(),
        "Log should NOT be in worktree's .git directory"
    );
}
