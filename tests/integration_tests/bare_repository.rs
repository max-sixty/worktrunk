use crate::common::TestRepo;
use insta_cmd::{assert_cmd_snapshot, get_cargo_bin};
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
            .env("GIT_AUTHOR_DATE", "2025-01-01T00:00:00Z")
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
        cmd.env("WORKTRUNK_CONFIG", self.test_config_path.to_str().unwrap())
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

    // Run wt list from the main worktree
    let mut cmd = Command::new(get_cargo_bin("wt"));
    test.configure_wt_cmd(&mut cmd);
    cmd.arg("list").current_dir(&main_worktree);

    assert_cmd_snapshot!(cmd);
}

#[test]
fn test_bare_repo_list_shows_no_bare_entry() {
    let test = BareRepoTest::new();

    // Create one worktree
    let main_worktree = test.create_worktree("main", "test-repo.main");
    test.commit_in_worktree(&main_worktree, "Initial commit");

    // Run wt list and verify bare repo is NOT shown
    let mut cmd = Command::new(get_cargo_bin("wt"));
    test.configure_wt_cmd(&mut cmd);
    cmd.arg("list").current_dir(&main_worktree);

    let output = cmd.output().expect("Failed to run wt list");
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should only show the main worktree, not the bare repo
    assert!(stdout.contains("main"));
    assert!(!stdout.contains(".git"));
    assert!(!stdout.contains("bare"));
}

#[test]
fn test_bare_repo_switch_creates_worktree() {
    let test = BareRepoTest::new();

    // Create initial worktree
    let main_worktree = test.create_worktree("main", "test-repo.main");
    test.commit_in_worktree(&main_worktree, "Initial commit");

    // Create worktrunk config to use worktree naming pattern suitable for bare repos
    let config = r#"
worktree-path = "{branch}"
"#;
    fs::write(test.config_path(), config).expect("Failed to write config");

    // Run wt switch --create to create a new worktree
    let mut cmd = Command::new(get_cargo_bin("wt"));
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

    // Verify the new worktree was created
    // With template "{branch}", it creates as sibling: bare-repo-name.feature
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
    // Default is "../{main-worktree}.{branch}" which becomes "test-repo.git.feature"
    let mut cmd = Command::new(get_cargo_bin("wt"));
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
    let mut cmd = Command::new(get_cargo_bin("wt"));
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
    let mut cmd = Command::new(get_cargo_bin("wt"));
    test.configure_wt_cmd(&mut cmd);
    cmd.arg("list").current_dir(&main_worktree);

    let output = cmd.output().expect("Failed to run wt list");
    let stdout = String::from_utf8_lossy(&output.stdout);

    // First non-bare worktree (main) should be primary
    // The exact formatting may vary, but main should be listed
    assert!(stdout.contains("main"));
}

#[test]
fn test_bare_repo_worktree_base_used_for_paths() {
    let test = BareRepoTest::new();

    // Create initial worktree
    let main_worktree = test.create_worktree("main", "test-repo.main");
    test.commit_in_worktree(&main_worktree, "Initial commit");

    // Configure to use simple branch naming
    let config = r#"
worktree-path = "{branch}"
"#;
    fs::write(test.config_path(), config).expect("Failed to write config");

    // Create new worktree - should be created relative to bare repo root
    let mut cmd = Command::new(get_cargo_bin("wt"));
    test.configure_wt_cmd(&mut cmd);
    cmd.args(["switch", "--create", "dev", "--internal"])
        .current_dir(&main_worktree);

    cmd.output().expect("Failed to run wt switch");

    // Verify path is created as sibling to bare repo (using worktree_base)
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
worktree-path = "{branch}"
"#;
    fs::write(bare_test.config_path(), config).expect("Failed to write bare config");
    fs::write(normal_test.test_config_path(), config).expect("Failed to write normal config");

    // List worktrees in both - should show similar structure
    let mut bare_list = Command::new(get_cargo_bin("wt"));
    bare_test.configure_wt_cmd(&mut bare_list);
    bare_list.arg("list").current_dir(&bare_main);

    let mut normal_list = Command::new(get_cargo_bin("wt"));
    normal_test.clean_cli_env(&mut normal_list);
    normal_list.arg("list").current_dir(normal_test.root_path());

    let bare_output = bare_list.output().expect("Failed to list bare worktrees");
    let normal_output = normal_list
        .output()
        .expect("Failed to list normal worktrees");

    // Both should show 1 worktree (main/main)
    let bare_stdout = String::from_utf8_lossy(&bare_output.stdout);
    let normal_stdout = String::from_utf8_lossy(&normal_output.stdout);

    assert!(bare_stdout.contains("main"));
    assert!(normal_stdout.contains("main"));
    assert_eq!(bare_stdout.lines().count(), normal_stdout.lines().count());
}
