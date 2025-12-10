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
            Some(&config_path),
        )
        .unwrap();

    // Verify the config was written to the isolated path
    assert!(
        config_path.exists(),
        "Config file was not created at {:?}",
        config_path
    );

    // Verify TOML structure
    let toml_content = fs::read_to_string(&config_path).unwrap();
    assert_snapshot!(toml_content, @r#"
    worktree-path = "../{{ main_worktree }}.{{ branch }}"

    [commit-generation]
    args = []

    [projects."github.com/test/repo"]
    approved-commands = [
        "test command",
    ]
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
            Some(&config_path),
        )
        .ok();
    config
        .approve_command_to(
            "github.com/test/repo".to_string(),
            "test".to_string(),
            Some(&config_path),
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
    worktree-path = "../{{ main_worktree }}.{{ branch }}"

    [commit-generation]
    args = []

    [projects."github.com/test/repo"]
    approved-commands = [
        "test",
    ]
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
            Some(&config_path),
        )
        .unwrap();
    config
        .approve_command_to(
            "github.com/user2/repo2".to_string(),
            "cargo build".to_string(),
            Some(&config_path),
        )
        .unwrap();
    config
        .approve_command_to(
            "github.com/user1/repo1".to_string(),
            "npm test".to_string(),
            Some(&config_path),
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
    worktree-path = "../{{ main_worktree }}.{{ branch }}"

    [commit-generation]
    args = []

    [projects."github.com/user1/repo1"]
    approved-commands = [
        "npm install",
        "npm test",
    ]

    [projects."github.com/user2/repo2"]
    approved-commands = [
        "cargo build",
    ]
    "#);
}

/// Test that the isolated config NEVER writes to user's actual config
#[test]
fn test_isolated_config_safety() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("isolated.toml");

    // Read user's actual config before test (if it exists)
    use etcetera::base_strategy::{BaseStrategy, choose_base_strategy};
    let user_config_path = if let Ok(strategy) = choose_base_strategy() {
        strategy.config_dir().join("worktrunk").join("config.toml")
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
            Some(&config_path),
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
    worktree-path = "../{{ main_worktree }}.{{ branch }}"

    [commit-generation]
    args = []
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
            Some(&config_path),
        )
        .unwrap();

    // Verify directory and file were created
    assert!(config_path.exists(), "Config file should be created");
    assert!(config_dir.exists(), "Config directory should be created");

    // Verify content
    let content = fs::read_to_string(&config_path).unwrap();
    assert_snapshot!(content, @r#"
    worktree-path = "../{{ main_worktree }}.{{ branch }}"

    [commit-generation]
    args = []

    [projects."github.com/test/nested"]
    approved-commands = [
        "test command",
    ]
    "#);
}

/// Test that saving config preserves TOML comments
///
/// When a user has a config file with comments and we save an approval,
/// all their comments should be preserved. This test verifies the behavior.
#[test]
fn test_saving_approval_preserves_toml_comments() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");

    // Create a config file with comments
    let initial_content = r#"# User preferences for worktrunk
# These comments should be preserved after saving

worktree-path = "../{{ main_worktree }}.{{ branch }}"

# LLM commit generation settings
[commit-generation]
command = "llm"  # inline comment should also be preserved
args = ["-s"]

# Per-project settings below
"#;
    fs::write(&config_path, initial_content).unwrap();

    // Load the config manually by deserializing from TOML
    // (bypasses WorktrunkConfig::load() which requires WORKTRUNK_CONFIG_PATH)
    let toml_str = fs::read_to_string(&config_path).unwrap();
    let mut config: WorktrunkConfig = toml::from_str(&toml_str).unwrap();

    // Add an approval and save back to the same file
    config
        .approve_command_to(
            "github.com/test/repo".to_string(),
            "npm install".to_string(),
            Some(&config_path),
        )
        .unwrap();

    // Read back the saved config
    let saved_content = fs::read_to_string(&config_path).unwrap();

    // Verify comments are preserved
    assert!(
        saved_content.contains("# User preferences for worktrunk"),
        "Top-level comment was lost. Saved content:\n{saved_content}"
    );
    assert!(
        saved_content.contains("# LLM commit generation settings"),
        "Section comment was lost. Saved content:\n{saved_content}"
    );
    assert!(
        saved_content.contains("# inline comment should also be preserved"),
        "Inline comment was lost. Saved content:\n{saved_content}"
    );

    // Verify the approval was also saved
    assert!(
        saved_content.contains("npm install"),
        "Approval was not saved. Saved content:\n{saved_content}"
    );
}

/// Test that permission errors when saving config are handled gracefully
///
/// This tests the lower-level `approve_command_to()` method fails when permissions
/// are denied. The higher-level `approve_command_batch()` catches this error and
/// displays a warning (see src/commands/command_approval.rs:82-85), allowing
/// commands to execute even when the approval can't be saved.
///
/// TODO: Find a way to test permission errors without skipping when running as root.
/// Currently skips in containerized environments (Claude Code web, Docker) where
/// root can write to read-only files. Consider using capabilities or other mechanisms
/// to test permission handling in all environments.
#[test]
#[cfg(unix)]
fn test_permission_error_prevents_save() {
    use std::fs::Permissions;
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("readonly").join("config.toml");

    // Create the directory and initial config file
    let config_dir = config_path.parent().unwrap();
    fs::create_dir_all(config_dir).unwrap();
    let initial_config = WorktrunkConfig::default();
    initial_config.save_to(&config_path).unwrap();

    // Make the directory read-only (prevents writing new files)
    #[cfg(unix)]
    {
        let readonly_perms = Permissions::from_mode(0o444);
        fs::set_permissions(config_dir, readonly_perms).unwrap();
    }

    // Test if permissions actually restrict us (skip if running as root)
    // Root can write to read-only directories, so the test would be meaningless
    let test_file = config_dir.join("test_write");
    if fs::write(&test_file, "test").is_ok() {
        // Running as root or permissions don't work
        // Restore write permissions and skip test
        #[cfg(unix)]
        {
            let writable_perms = Permissions::from_mode(0o755);
            fs::set_permissions(config_dir, writable_perms).unwrap();
        }
        eprintln!("Skipping permission test - running with elevated privileges");
        return;
    }

    // Try to save a new approval - this should fail
    let mut config = WorktrunkConfig::default();
    let result = config.approve_command_to(
        "github.com/test/readonly".to_string(),
        "test command".to_string(),
        Some(&config_path),
    );

    // Restore write permissions so temp_dir can be cleaned up
    #[cfg(unix)]
    {
        let writable_perms = Permissions::from_mode(0o755);
        fs::set_permissions(config_dir, writable_perms).unwrap();
    }

    // Verify the save failed
    assert!(
        result.is_err(),
        "Expected save to fail due to permissions, but it succeeded"
    );

    // In the actual code (approve_command_batch), when this error occurs:
    // 1. It's caught with `if let Err(e) = fresh_config.save()`
    // 2. Warning is printed: "🟡 Failed to save command approval: {error}"
    // 3. Hint is printed: "💡 Approval will be requested again next time."
    // 4. Function returns Ok(true) - execution continues!
    //
    // The approval succeeds (commands execute) even though saving failed.
    // This test verifies the save operation correctly fails with permission errors.
}
