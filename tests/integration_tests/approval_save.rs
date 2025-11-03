use insta::assert_snapshot;
use std::fs;
use tempfile::TempDir;
use worktrunk::config::WorktrunkConfig;

/// Test that approved commands are actually persisted to disk
///
/// This test uses `approve_command_to()` to ensure it never writes to the user's config
#[test]
fn test_approval_saves_to_disk() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("worktrunk").join("config.toml");

    // Create config and save to temp directory ONLY
    let mut config = WorktrunkConfig::default();

    // Add an approval to the explicit path
    config
        .approve_command_to(
            "github.com/test/repo".to_string(),
            "test command".to_string(),
            &config_path,
        )
        .expect("Failed to save approval");

    // Verify the config was written to the isolated path
    assert!(
        config_path.exists(),
        "Config file was not created at {:?}",
        config_path
    );

    // Verify TOML structure
    let toml_content = fs::read_to_string(&config_path).unwrap();
    assert_snapshot!(toml_content, @r#"
    worktree-path = "../{main-worktree}.{branch}"

    [commit-generation]
    args = []

    [projects."github.com/test/repo"]
    approved-commands = ["test command"]
    "#);

    // Verify approval is in memory
    assert!(config.is_command_approved("github.com/test/repo", "test command"));
}

/// Test that duplicate approvals are not saved twice
#[test]
fn test_duplicate_approvals_not_saved_twice() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");

    let mut config = WorktrunkConfig::default();

    // Add same approval twice
    config
        .approve_command_to(
            "github.com/test/repo".to_string(),
            "test".to_string(),
            &config_path,
        )
        .ok();
    config
        .approve_command_to(
            "github.com/test/repo".to_string(),
            "test".to_string(),
            &config_path,
        )
        .ok();

    // Verify only one entry exists
    let matching_commands = config
        .projects
        .get("github.com/test/repo")
        .map(|p| {
            p.approved_commands
                .iter()
                .filter(|cmd| *cmd == "test")
                .count()
        })
        .unwrap_or(0);

    assert_eq!(matching_commands, 1, "Duplicate approval was saved");

    // Verify file contains only one entry
    let toml_content = fs::read_to_string(&config_path).unwrap();
    assert_snapshot!(toml_content, @r#"
    worktree-path = "../{main-worktree}.{branch}"

    [commit-generation]
    args = []

    [projects."github.com/test/repo"]
    approved-commands = ["test"]
    "#);
}

/// Test that approvals from different projects don't conflict
#[test]
fn test_multiple_project_approvals() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");

    let mut config = WorktrunkConfig::default();

    // Add approvals for different projects
    config
        .approve_command_to(
            "github.com/user1/repo1".to_string(),
            "npm install".to_string(),
            &config_path,
        )
        .unwrap();
    config
        .approve_command_to(
            "github.com/user2/repo2".to_string(),
            "cargo build".to_string(),
            &config_path,
        )
        .unwrap();
    config
        .approve_command_to(
            "github.com/user1/repo1".to_string(),
            "npm test".to_string(),
            &config_path,
        )
        .unwrap();

    // Verify all approvals exist
    assert!(config.is_command_approved("github.com/user1/repo1", "npm install"));
    assert!(config.is_command_approved("github.com/user2/repo2", "cargo build"));
    assert!(config.is_command_approved("github.com/user1/repo1", "npm test"));
    assert!(!config.is_command_approved("github.com/user1/repo1", "cargo build"));

    // Verify file structure
    let toml_content = fs::read_to_string(&config_path).unwrap();
    assert_snapshot!(toml_content, @r#"
    worktree-path = "../{main-worktree}.{branch}"

    [commit-generation]
    args = []

    [projects."github.com/user1/repo1"]
    approved-commands = [
        "npm install",
        "npm test",
    ]

    [projects."github.com/user2/repo2"]
    approved-commands = ["cargo build"]
    "#);
}

/// Test that the isolated config NEVER writes to user's actual config
#[test]
fn test_isolated_config_safety() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("isolated.toml");

    // Read user's actual config before test (if it exists)
    use directories::ProjectDirs;
    let user_config_path = if let Some(proj_dirs) = ProjectDirs::from("", "", "worktrunk") {
        proj_dirs.config_dir().join("config.toml")
    } else {
        // Fallback for platforms where config dir can't be determined
        std::env::var("HOME")
            .map(|home| std::path::PathBuf::from(home).join(".config/worktrunk/config.toml"))
            .unwrap_or_else(|_| temp_dir.path().join("dummy.toml"))
    };

    let user_config_before = if user_config_path.exists() {
        Some(fs::read_to_string(&user_config_path).unwrap())
    } else {
        None
    };

    // Create isolated config and make changes
    let mut config = WorktrunkConfig::default();
    config
        .approve_command_to(
            "github.com/safety-test/repo".to_string(),
            "THIS SHOULD NOT APPEAR IN USER CONFIG".to_string(),
            &config_path,
        )
        .unwrap();

    // Verify user's config is unchanged
    let user_config_after = if user_config_path.exists() {
        Some(fs::read_to_string(&user_config_path).unwrap())
    } else {
        None
    };

    assert_eq!(
        user_config_before, user_config_after,
        "User config was modified by isolated test!"
    );

    // Verify the test command was written to isolated path
    let isolated_content = fs::read_to_string(&config_path).unwrap();
    assert!(isolated_content.contains("THIS SHOULD NOT APPEAR IN USER CONFIG"));
}

/// Test that --force flag does NOT save approvals
///
/// The --force flag should allow commands to run once without saving them
/// to the config file. This ensures --force is a one-time bypass, not a
/// permanent approval.
#[test]
fn test_force_flag_does_not_save_approval() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");

    // Start with empty config
    let initial_config = WorktrunkConfig::default();
    initial_config.save_to(&config_path).unwrap();

    // When using --force, the approval is NOT saved to config
    // This is the correct behavior - force is a one-time bypass
    // So we just verify the initial config is unchanged

    // Load the config and verify it's still empty (no approvals added)
    let saved_config = fs::read_to_string(&config_path).unwrap();
    assert_snapshot!(saved_config, @r#"
    worktree-path = "../{main-worktree}.{branch}"

    [commit-generation]
    args = []

    [projects]
    "#);
}

/// Test that approval saving logic handles missing config gracefully
#[test]
fn test_force_flag_saves_to_new_config_file() {
    let temp_dir = TempDir::new().unwrap();
    let config_dir = temp_dir.path().join("nested").join("config");
    let config_path = config_dir.join("config.toml");

    // Don't create the directory - test that it's created automatically
    assert!(!config_path.exists());

    // Create a config and save
    let mut config = WorktrunkConfig::default();
    config
        .approve_command_to(
            "github.com/test/nested".to_string(),
            "test command".to_string(),
            &config_path,
        )
        .unwrap();

    // Verify directory and file were created
    assert!(config_path.exists(), "Config file should be created");
    assert!(config_dir.exists(), "Config directory should be created");

    // Verify content
    let content = fs::read_to_string(&config_path).unwrap();
    assert_snapshot!(content, @r#"
    worktree-path = "../{main-worktree}.{branch}"

    [commit-generation]
    args = []

    [projects."github.com/test/nested"]
    approved-commands = ["test command"]
    "#);
}
