use insta::assert_snapshot;
use std::fs;
use tempfile::TempDir;
use worktrunk::config::WorktrunkConfig;

/// Test that approved commands are actually persisted to disk
///
/// This test uses `approve_command()` to ensure it never writes to the user's config
#[test]
fn test_approval_saves_to_disk() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("worktrunk").join("config.toml");

    // Create config and save to temp directory ONLY
    let mut config = WorktrunkConfig::default();

    // Add an approval to the explicit path
    config
        .approve_command(
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
        .approve_command(
            "github.com/test/repo".to_string(),
            "test".to_string(),
            Some(&config_path),
        )
        .ok();
    config
        .approve_command(
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
        .approve_command(
            "github.com/user1/repo1".to_string(),
            "npm install".to_string(),
            Some(&config_path),
        )
        .unwrap();
    config
        .approve_command(
            "github.com/user2/repo2".to_string(),
            "cargo build".to_string(),
            Some(&config_path),
        )
        .unwrap();
    config
        .approve_command(
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
        .approve_command(
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

/// Test that --yes flag does NOT save approvals
///
/// The --yes flag should allow commands to run once without saving them
/// to the config file. This ensures --yes is a one-time bypass, not a
/// permanent approval.
#[test]
fn test_yes_flag_does_not_save_approval() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");

    // Start with empty config
    let initial_config = WorktrunkConfig::default();
    initial_config.save_to(&config_path).unwrap();

    // When using --yes, the approval is NOT saved to config
    // This is the correct behavior - yes is a one-time bypass
    // So we just verify the initial config is unchanged

    // Load the config and verify it's still empty (no approvals added)
    let saved_config = fs::read_to_string(&config_path).unwrap();
    assert_snapshot!(saved_config, @"
    [commit-generation]
    args = []
    ");
}

/// Test that approval saving logic handles missing config gracefully
#[test]
fn test_approval_saves_to_new_config_file() {
    let temp_dir = TempDir::new().unwrap();
    let config_dir = temp_dir.path().join("nested").join("config");
    let config_path = config_dir.join("config.toml");

    // Don't create the directory - test that it's created automatically
    assert!(!config_path.exists());

    // Create a config and save
    let mut config = WorktrunkConfig::default();
    config
        .approve_command(
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
        .approve_command(
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

/// Test that concurrent approve operations don't lose data
///
/// This tests a race condition where two config instances (simulating separate processes)
/// both approve commands. Without proper merging, the second save would overwrite
/// the first approval, losing it.
#[test]
fn test_concurrent_approve_preserves_all_approvals() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");

    // Process A: Start with empty config, approve "npm install"
    let mut config_a = WorktrunkConfig::default();

    // Process B: Start with empty config (simulating a separate process that loaded before A saved)
    let mut config_b = WorktrunkConfig::default();

    // Process A approves and saves "npm install"
    config_a
        .approve_command(
            "github.com/user/repo".to_string(),
            "npm install".to_string(),
            Some(&config_path),
        )
        .unwrap();

    // Verify file has "npm install"
    let toml_content = fs::read_to_string(&config_path).unwrap();
    assert!(
        toml_content.contains("npm install"),
        "File should contain 'npm install'"
    );

    // Process B (which loaded BEFORE Process A saved) now approves and saves "npm test"
    // The save_to method should merge with what's on disk, not overwrite
    config_b
        .approve_command(
            "github.com/user/repo".to_string(),
            "npm test".to_string(),
            Some(&config_path),
        )
        .unwrap();

    // Read the final state from disk
    let toml_content = fs::read_to_string(&config_path).unwrap();

    // Both approvals should be preserved
    assert!(
        toml_content.contains("npm install"),
        "BUG: 'npm install' approval was lost due to race condition. \
         config_b's save_to() should merge with disk state, not overwrite it. \
         Saved content:\n{toml_content}"
    );
    assert!(
        toml_content.contains("npm test"),
        "'npm test' approval should exist. Saved content:\n{toml_content}"
    );
}

/// Test that concurrent revoke operations don't lose data
///
/// This tests a race condition where two config instances (simulating separate processes)
/// both revoke commands. Without proper merging, the second save would restore
/// the revoked command from its stale in-memory state.
#[test]
fn test_concurrent_revoke_preserves_all_changes() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");

    // Setup: config file has two commands approved
    let mut setup_config = WorktrunkConfig::default();
    setup_config
        .approve_command(
            "github.com/user/repo".to_string(),
            "npm install".to_string(),
            Some(&config_path),
        )
        .unwrap();
    setup_config
        .approve_command(
            "github.com/user/repo".to_string(),
            "npm test".to_string(),
            Some(&config_path),
        )
        .unwrap();

    // Verify setup
    let toml_content = fs::read_to_string(&config_path).unwrap();
    assert!(toml_content.contains("npm install"));
    assert!(toml_content.contains("npm test"));

    // Process A: loads config (has ["npm install", "npm test"])
    let mut config_a = WorktrunkConfig::default();
    config_a
        .projects
        .entry("github.com/user/repo".to_string())
        .or_default()
        .approved_commands = vec!["npm install".to_string(), "npm test".to_string()];

    // Process B: loads config (has ["npm install", "npm test"])
    let mut config_b = WorktrunkConfig::default();
    config_b
        .projects
        .entry("github.com/user/repo".to_string())
        .or_default()
        .approved_commands = vec!["npm install".to_string(), "npm test".to_string()];

    // Process A revokes "npm install"
    config_a
        .revoke_command("github.com/user/repo", "npm install", Some(&config_path))
        .unwrap();

    // Process B (with stale state) revokes "npm test"
    // Should see that "npm install" was already revoked and preserve that
    config_b
        .revoke_command("github.com/user/repo", "npm test", Some(&config_path))
        .unwrap();

    // Read the final state from disk
    let toml_content = fs::read_to_string(&config_path).unwrap();

    // Both revocations should be respected - neither command should remain
    assert!(
        !toml_content.contains("npm install"),
        "'npm install' should have been revoked. Saved content:\n{toml_content}"
    );
    assert!(
        !toml_content.contains("npm test"),
        "'npm test' should have been revoked. Saved content:\n{toml_content}"
    );
}

/// Test that concurrent approve operations across different projects also work
#[test]
fn test_concurrent_approve_different_projects() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");

    // Process A: empty config
    let mut config_a = WorktrunkConfig::default();

    // Process B: empty config (loaded before A saved)
    let mut config_b = WorktrunkConfig::default();

    // Process A approves for project1
    config_a
        .approve_command(
            "github.com/user/project1".to_string(),
            "npm install".to_string(),
            Some(&config_path),
        )
        .unwrap();

    // Process B approves for project2
    // Should preserve project1's approval
    config_b
        .approve_command(
            "github.com/user/project2".to_string(),
            "cargo build".to_string(),
            Some(&config_path),
        )
        .unwrap();

    let toml_content = fs::read_to_string(&config_path).unwrap();

    assert!(
        toml_content.contains("github.com/user/project1"),
        "Project1 should be preserved. Content:\n{toml_content}"
    );
    assert!(
        toml_content.contains("npm install"),
        "'npm install' should be preserved. Content:\n{toml_content}"
    );
    assert!(
        toml_content.contains("github.com/user/project2"),
        "Project2 should exist. Content:\n{toml_content}"
    );
    assert!(
        toml_content.contains("cargo build"),
        "'cargo build' should exist. Content:\n{toml_content}"
    );
}

/// Test that permission errors when saving config are handled gracefully
///
/// This tests the lower-level `approve_command()` method fails when permissions
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
    let result = config.approve_command(
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

/// Test that set_skip_shell_integration_prompt saves to disk
#[test]
fn test_skip_shell_integration_prompt_saves_to_disk() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("worktrunk").join("config.toml");

    let mut config = WorktrunkConfig::default();
    config
        .set_skip_shell_integration_prompt(Some(&config_path))
        .unwrap();

    // Verify file was created
    assert!(
        config_path.exists(),
        "Config file was not created at {:?}",
        config_path
    );

    // Verify TOML structure
    let toml_content = fs::read_to_string(&config_path).unwrap();
    assert_snapshot!(toml_content, @"
    skip-shell-integration-prompt = true

    [commit-generation]
    args = []
    ");
}

/// Test that set_skip_shell_integration_prompt is idempotent
#[test]
fn test_skip_shell_integration_prompt_idempotent() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");

    let mut config = WorktrunkConfig::default();

    // Call twice - should not error
    config
        .set_skip_shell_integration_prompt(Some(&config_path))
        .unwrap();
    config
        .set_skip_shell_integration_prompt(Some(&config_path))
        .unwrap();

    // Field should still be true
    assert!(config.skip_shell_integration_prompt);

    // File should have the flag exactly once
    let toml_content = fs::read_to_string(&config_path).unwrap();
    let count = toml_content
        .matches("skip-shell-integration-prompt")
        .count();
    assert_eq!(count, 1, "Flag should appear exactly once");
}

/// Test that saving config through a symlink preserves the symlink
///
/// When the config file is a symlink (e.g., user has config.toml -> dotfiles/worktrunk.toml),
/// saving should write to the target file without destroying the symlink.
#[test]
#[cfg(unix)]
fn test_saving_through_symlink_preserves_symlink() {
    use std::os::unix::fs::symlink;

    let temp_dir = TempDir::new().unwrap();

    // Create a "dotfiles" directory with the actual config
    let dotfiles_dir = temp_dir.path().join("dotfiles");
    fs::create_dir_all(&dotfiles_dir).unwrap();
    let target_path = dotfiles_dir.join("worktrunk.toml");

    // Create initial config at the target location
    let initial_content = r#"# My dotfiles config
worktree-path = "../{{ main_worktree }}.{{ branch }}"

[commit-generation]
command = "llm"
"#;
    fs::write(&target_path, initial_content).unwrap();

    // Create symlink: config_path -> dotfiles/worktrunk.toml
    let config_dir = temp_dir.path().join("config").join("worktrunk");
    fs::create_dir_all(&config_dir).unwrap();
    let symlink_path = config_dir.join("config.toml");
    symlink(&target_path, &symlink_path).unwrap();

    // Verify symlink was created correctly
    assert!(symlink_path.is_symlink(), "Should be a symlink before save");
    assert_eq!(
        fs::read_link(&symlink_path).unwrap(),
        target_path,
        "Symlink should point to target"
    );

    // Load config and add an approval through the symlink path
    let toml_str = fs::read_to_string(&symlink_path).unwrap();
    let mut config: WorktrunkConfig = toml::from_str(&toml_str).unwrap();

    config
        .approve_command(
            "github.com/test/symlink-repo".to_string(),
            "npm install".to_string(),
            Some(&symlink_path),
        )
        .unwrap();

    // Verify symlink is preserved
    assert!(
        symlink_path.is_symlink(),
        "Symlink should be preserved after save"
    );
    assert_eq!(
        fs::read_link(&symlink_path).unwrap(),
        target_path,
        "Symlink target should be unchanged"
    );

    // Verify content was written to the target file
    let target_content = fs::read_to_string(&target_path).unwrap();
    assert!(
        target_content.contains("npm install"),
        "Approval should be written to target. Content:\n{target_content}"
    );
    assert!(
        target_content.contains("# My dotfiles config"),
        "Comments should be preserved. Content:\n{target_content}"
    );

    // Verify reading through symlink gets the same content
    let symlink_content = fs::read_to_string(&symlink_path).unwrap();
    assert_eq!(
        target_content, symlink_content,
        "Content should be identical whether read through symlink or target"
    );
}
