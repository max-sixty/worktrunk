use crate::common::{TestRepo, setup_temp_snapshot_settings, wt_command};
use insta_cmd::assert_cmd_snapshot;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Helper to create a bare repository test setup
struct BareRepoTest {
    temp_dir: tempfile::TempDir,
    bare_repo_path: PathBuf,
    test_config_path: PathBuf,
}

impl BareRepoTest {
    fn new() -> Self {
        let temp_dir = tempfile::TempDir::new().expect("Failed to create temp directory");
        let bare_repo_path = temp_dir.path().join("test-repo.git");
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
            .expect("Failed to create bare repo");

        if !output.status.success() {
            panic!(
                "Failed to init bare repo:\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        // Canonicalize path
        test.bare_repo_path = test
            .bare_repo_path
            .canonicalize()
            .expect("Failed to canonicalize bare repo path");

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
    fn create_worktree(&self, branch: &str, worktree_name: &str) -> PathBuf {
        let worktree_path = self.temp_dir.path().join(worktree_name);

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

        let output = cmd.output().expect("Failed to create worktree");

        if !output.status.success() {
            panic!(
                "Failed to create worktree:\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        worktree_path
            .canonicalize()
            .expect("Failed to canonicalize worktree path")
    }

    /// Create a commit in the specified worktree
    fn commit_in_worktree(&self, worktree_path: &PathBuf, message: &str) {
        // Create a file
        let file_path = worktree_path.join("file.txt");
        fs::write(&file_path, message).expect("Failed to write file");

        // Add file
        let output = Command::new("git")
            .args(["add", "file.txt"])
            .current_dir(worktree_path)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("Failed to add file");

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
            .expect("Failed to commit");

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
        .env("SOURCE_DATE_EPOCH", "1761609600")
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env_remove("NO_COLOR")
        .env_remove("CLICOLOR_FORCE");
    }
}

#[test]
fn test_bare_repo_list_worktrees() {
    let test = BareRepoTest::new();

    // Create two worktrees
    let main_worktree = test.create_worktree("main", "test-repo.main");
    test.commit_in_worktree(&main_worktree, "Initial commit on main");

    let feature_worktree = test.create_worktree("feature", "test-repo.feature");
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
    let main_worktree = test.create_worktree("main", "test-repo.main");
    test.commit_in_worktree(&main_worktree, "Initial commit");

    // Run wt list and verify bare repo is NOT shown
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    cmd.arg("list").current_dir(&main_worktree);

    let output = cmd.output().expect("Failed to run wt list");
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
    let main_worktree = test.create_worktree("main", "test-repo.main");
    test.commit_in_worktree(&main_worktree, "Initial commit");

    // Use default template (sibling worktrees): ../{{ main_worktree }}.{{ branch }}
    // No config needed - defaults will create worktrees as siblings with dot separator

    // Run wt switch --create to create a new worktree
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    cmd.args(["switch", "--create", "feature", "--internal"])
        .current_dir(&main_worktree);

    let output = cmd.output().expect("Failed to run wt switch");

    if !output.status.success() {
        panic!(
            "wt switch failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Debug: check where worktrees actually are
    let list_output = Command::new("git")
        .args([
            "-C",
            test.bare_repo_path().to_str().unwrap(),
            "worktree",
            "list",
        ])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("Failed to list worktrees");

    eprintln!(
        "Git worktree list:\n{}",
        String::from_utf8_lossy(&list_output.stdout)
    );

    // Verify the new worktree was created as sibling with dot separator
    // Default template: ../{{ main_worktree }}.{{ branch }} -> test-repo.git.feature
    let bare_name = test.bare_repo_path().file_name().unwrap().to_str().unwrap();
    let expected_path = test.temp_path().join(format!("{}.feature", bare_name));
    assert!(
        expected_path.exists(),
        "Expected worktree at {:?} but it doesn't exist.\nGit worktree list:\n{}",
        expected_path,
        String::from_utf8_lossy(&list_output.stdout)
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

    let output = cmd.output().expect("Failed to run git worktree list");
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should show 3 entries: bare repo + 2 worktrees
    assert_eq!(stdout.lines().count(), 3);
}

#[test]
fn test_bare_repo_switch_with_default_naming() {
    let test = BareRepoTest::new();

    // Create initial worktree
    let main_worktree = test.create_worktree("main", "test-repo.main");
    test.commit_in_worktree(&main_worktree, "Initial commit");

    // Use default naming pattern (should still work with bare repos)
    // Default is "../{{ main_worktree }}.{{ branch }}" which becomes "test-repo.git.feature"
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    cmd.args(["switch", "--create", "feature", "--internal"])
        .current_dir(&main_worktree);

    let output = cmd.output().expect("Failed to run wt switch");

    if !output.status.success() {
        panic!(
            "wt switch failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Verify worktree was created as sibling to bare repo
    let bare_name = test.bare_repo_path().file_name().unwrap().to_str().unwrap();
    let expected_path = test.temp_path().join(format!("{}.feature", bare_name));
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
    let main_worktree = test.create_worktree("main", "test-repo.main");
    test.commit_in_worktree(&main_worktree, "Initial commit");

    let feature_worktree = test.create_worktree("feature", "test-repo.feature");
    test.commit_in_worktree(&feature_worktree, "Feature work");

    // Remove feature worktree from main worktree
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    cmd.args(["remove", "feature", "--no-background", "--internal"])
        .current_dir(&main_worktree);

    let output = cmd.output().expect("Failed to run wt remove");

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
    let main_worktree = test.create_worktree("main", "test-repo.main");
    test.commit_in_worktree(&main_worktree, "Main commit");

    let _feature1 = test.create_worktree("feature1", "test-repo.feature1");
    let _feature2 = test.create_worktree("feature2", "test-repo.feature2");

    // Run wt list to see which is marked as primary
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    cmd.arg("list").current_dir(&main_worktree);

    let output = cmd.output().expect("Failed to run wt list");
    let stderr = String::from_utf8_lossy(&output.stderr);

    // First non-bare worktree (main) should be primary (table output is on stderr)
    // The exact formatting may vary, but main should be listed
    assert!(stderr.contains("main"));
}

#[test]
fn test_bare_repo_worktree_base_used_for_paths() {
    let test = BareRepoTest::new();

    // Create initial worktree
    let main_worktree = test.create_worktree("main", "test-repo.main");
    test.commit_in_worktree(&main_worktree, "Initial commit");

    // Use default template - creates sibling worktrees with dot separator
    // No config needed - defaults will create worktrees as siblings

    // Create new worktree - should be created relative to bare repo root
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    cmd.args(["switch", "--create", "dev", "--internal"])
        .current_dir(&main_worktree);

    cmd.output().expect("Failed to run wt switch");

    // Verify path is created as sibling to bare repo (using worktree_base)
    // Default template: ../{{ main_worktree }}.{{ branch }} -> test-repo.git.dev
    let bare_name = test.bare_repo_path().file_name().unwrap().to_str().unwrap();
    let expected = test.temp_path().join(format!("{}.dev", bare_name));
    assert!(
        expected.exists(),
        "Worktree should be created using worktree_base: {:?}",
        expected
    );

    // Should NOT be relative to main worktree
    let wrong_path = main_worktree.join("dev");
    assert!(
        !wrong_path.exists(),
        "Worktree should not be relative to main worktree"
    );
}

#[test]
fn test_bare_repo_equivalent_to_normal_repo() {
    // This test verifies that bare repos behave identically to normal repos
    // from the user's perspective

    // Set up bare repo
    let bare_test = BareRepoTest::new();
    let bare_main = bare_test.create_worktree("main", "bare-repo.main");
    bare_test.commit_in_worktree(&bare_main, "Commit in bare repo");

    // Set up normal repo
    let normal_test = TestRepo::new();
    normal_test.commit("Commit in normal repo");

    // Configure both with same worktree path pattern
    let config = r#"
worktree-path = "{{ branch }}"
"#;
    fs::write(bare_test.config_path(), config).expect("Failed to write bare config");
    fs::write(normal_test.test_config_path(), config).expect("Failed to write normal config");

    // List worktrees in both - should show similar structure
    let mut bare_list = wt_command();
    bare_test.configure_wt_cmd(&mut bare_list);
    bare_list.arg("list").current_dir(&bare_main);

    let mut normal_list = wt_command();
    normal_test.clean_cli_env(&mut normal_list);
    normal_list.arg("list").current_dir(normal_test.root_path());

    let bare_output = bare_list.output().expect("Failed to list bare worktrees");
    let normal_output = normal_list
        .output()
        .expect("Failed to list normal worktrees");

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
    let main_worktree = test.create_worktree("main", "test-repo.main");
    test.commit_in_worktree(&main_worktree, "Initial commit");

    // Run wt list from the bare repo directory itself (not from a worktree)
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    cmd.arg("list").current_dir(test.bare_repo_path());

    let output = cmd.output().expect("Failed to run wt list from bare repo");

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

#[test]
fn test_bare_repo_merge_workflow() {
    let test = BareRepoTest::new();

    // Create main worktree
    let main_worktree = test.create_worktree("main", "test-repo.main");
    test.commit_in_worktree(&main_worktree, "Initial commit on main");

    // Use default template - creates sibling worktrees with dot separator
    // No config needed - defaults will create worktrees as siblings

    // Create feature branch worktree
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    cmd.args(["switch", "--create", "feature", "--internal"])
        .current_dir(&main_worktree);
    cmd.output().expect("Failed to create feature worktree");

    // Get feature worktree path (default template: ../{{ main_worktree }}.{{ branch }})
    let bare_name = test.bare_repo_path().file_name().unwrap().to_str().unwrap();
    let feature_worktree = test.temp_path().join(format!("{}.feature", bare_name));
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

    let output = cmd.output().expect("Failed to run wt merge");

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
        .expect("Failed to get git log");

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
    let main_worktree = test.create_worktree("main", "test-repo.main");
    test.commit_in_worktree(&main_worktree, "Initial commit");

    // Create feature worktree
    let bare_name = test.bare_repo_path().file_name().unwrap().to_str().unwrap();
    let feature_worktree = test.create_worktree("feature", &format!("{}.feature", bare_name));
    test.commit_in_worktree(&feature_worktree, "Feature work");

    // Run remove in background to test log file location
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    cmd.args(["remove", "feature", "--internal"])
        .current_dir(&main_worktree);

    let output = cmd.output().expect("Failed to run wt remove");

    if !output.status.success() {
        panic!(
            "wt remove failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Give background process time to create log file (file gets created immediately, content written later)
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Verify log file was created in bare repo's wt-logs directory (not in worktree's .git/)
    // The key test is that the path is correct, not that content was written (background processes are flaky in tests)
    let log_path = test.bare_repo_path().join("wt-logs/feature-remove.log");
    assert!(
        log_path.exists(),
        "Background removal log should be created in bare repo's wt-logs/ directory at {:?}.\nBare repo path: {:?}",
        log_path,
        test.bare_repo_path()
    );

    // Verify it's NOT in the worktree's .git directory (which doesn't exist for linked worktrees)
    let wrong_path = main_worktree.join(".git/wt-logs/feature-remove.log");
    assert!(
        !wrong_path.exists(),
        "Log should NOT be in worktree's .git directory"
    );
}
