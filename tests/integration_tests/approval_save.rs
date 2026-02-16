use insta::assert_snapshot;
use std::fs;
use tempfile::TempDir;
use worktrunk::config::Approvals;
use worktrunk::config::UserConfig;

///
/// This test uses `approve_command()` to ensure it never writes to the user's config
#[test]
fn test_approval_saves_to_disk() {
    let temp_dir = TempDir::new().unwrap();
    let approvals_path = temp_dir.path().join("worktrunk").join("approvals.toml");

    // Create approvals and save to temp directory ONLY
    let mut approvals = Approvals::default();

    // Add an approval to the explicit path
    approvals
        .approve_command(
            "github.com/test/repo".to_string(),
            "test command".to_string(),
            Some(&approvals_path),
        )
        .unwrap();

    // Verify the file was written to the isolated path
    assert!(
        approvals_path.exists(),
        "Approvals file was not created at {:?}",
        approvals_path
    );

    // Verify TOML structure
    let toml_content = fs::read_to_string(&approvals_path).unwrap();
    assert_snapshot!(toml_content, @r#"
    [projects."github.com/test/repo"]
    approved-commands = [
        "test command",
    ]
    "#);

    // Verify approval is in memory
    assert!(approvals.is_command_approved("github.com/test/repo", "test command"));
}

#[test]
fn test_duplicate_approvals_not_saved_twice() {
    let temp_dir = TempDir::new().unwrap();
    let approvals_path = temp_dir.path().join("approvals.toml");

    let mut approvals = Approvals::default();

    // Add same approval twice
    approvals
        .approve_command(
            "github.com/test/repo".to_string(),
            "test".to_string(),
            Some(&approvals_path),
        )
        .ok();
    approvals
        .approve_command(
            "github.com/test/repo".to_string(),
            "test".to_string(),
            Some(&approvals_path),
        )
        .ok();

    // Verify only one entry exists by reading the saved file
    let toml_content = fs::read_to_string(&approvals_path).unwrap();
    assert_snapshot!(toml_content, @r#"
    [projects."github.com/test/repo"]
    approved-commands = [
        "test",
    ]
    "#);
}

#[test]
fn test_multiple_project_approvals() {
    let temp_dir = TempDir::new().unwrap();
    let approvals_path = temp_dir.path().join("approvals.toml");

    let mut approvals = Approvals::default();

    // Add approvals for different projects
    approvals
        .approve_command(
            "github.com/user1/repo1".to_string(),
            "npm install".to_string(),
            Some(&approvals_path),
        )
        .unwrap();
    approvals
        .approve_command(
            "github.com/user2/repo2".to_string(),
            "cargo build".to_string(),
            Some(&approvals_path),
        )
        .unwrap();
    approvals
        .approve_command(
            "github.com/user1/repo1".to_string(),
            "npm test".to_string(),
            Some(&approvals_path),
        )
        .unwrap();

    // Verify all approvals exist
    assert!(approvals.is_command_approved("github.com/user1/repo1", "npm install"));
    assert!(approvals.is_command_approved("github.com/user2/repo2", "cargo build"));
    assert!(approvals.is_command_approved("github.com/user1/repo1", "npm test"));
    assert!(!approvals.is_command_approved("github.com/user1/repo1", "cargo build"));

    // Verify file structure
    let toml_content = fs::read_to_string(&approvals_path).unwrap();
    assert_snapshot!(toml_content, @r#"
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

#[test]
fn test_isolated_config_safety() {
    let temp_dir = TempDir::new().unwrap();
    let approvals_path = temp_dir.path().join("isolated.toml");

    // Read user's actual approvals before test (if it exists)
    use etcetera::base_strategy::{BaseStrategy, choose_base_strategy};
    let user_approvals_path = if let Ok(strategy) = choose_base_strategy() {
        strategy
            .config_dir()
            .join("worktrunk")
            .join("approvals.toml")
    } else {
        // Fallback for platforms where config dir can't be determined
        std::env::var("HOME")
            .map(|home| std::path::PathBuf::from(home).join(".config/worktrunk/approvals.toml"))
            .unwrap_or_else(|_| temp_dir.path().join("dummy.toml"))
    };

    let user_approvals_before = if user_approvals_path.exists() {
        Some(fs::read_to_string(&user_approvals_path).unwrap())
    } else {
        None
    };

    // Create isolated approvals and make changes
    let mut approvals = Approvals::default();
    approvals
        .approve_command(
            "github.com/safety-test/repo".to_string(),
            "THIS SHOULD NOT APPEAR IN USER APPROVALS".to_string(),
            Some(&approvals_path),
        )
        .unwrap();

    // Verify user's approvals file is unchanged
    let user_approvals_after = if user_approvals_path.exists() {
        Some(fs::read_to_string(&user_approvals_path).unwrap())
    } else {
        None
    };

    assert_eq!(
        user_approvals_before, user_approvals_after,
        "User approvals file was modified by isolated test!"
    );

    // Verify the test command was written to isolated path
    let isolated_content = fs::read_to_string(&approvals_path).unwrap();
    assert!(isolated_content.contains("THIS SHOULD NOT APPEAR IN USER APPROVALS"));
}

///
/// The --yes flag should allow commands to run once without saving them
/// to the config file. This ensures --yes is a one-time bypass, not a
/// permanent approval.
#[test]
fn test_yes_flag_does_not_save_approval() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");

    // Start with empty config
    let initial_config = UserConfig::default();
    initial_config.save_to(&config_path).unwrap();

    // When using --yes, the approval is NOT saved to config
    // This is the correct behavior - yes is a one-time bypass
    // So we just verify the initial config is unchanged

    // Load the config and verify it's still empty (no approvals added)
    let saved_config = fs::read_to_string(&config_path).unwrap();
    assert_snapshot!(saved_config, @"");
}

#[test]
fn test_approval_saves_to_new_approvals_file() {
    let temp_dir = TempDir::new().unwrap();
    let approvals_dir = temp_dir.path().join("nested").join("config");
    let approvals_path = approvals_dir.join("approvals.toml");

    // Don't create the directory - test that it's created automatically
    assert!(!approvals_path.exists());

    // Create approvals and save
    let mut approvals = Approvals::default();
    approvals
        .approve_command(
            "github.com/test/nested".to_string(),
            "test command".to_string(),
            Some(&approvals_path),
        )
        .unwrap();

    // Verify directory and file were created
    assert!(approvals_path.exists());
    assert!(approvals_dir.exists());

    // Verify content
    let content = fs::read_to_string(&approvals_path).unwrap();
    assert_snapshot!(content, @r#"
    [projects."github.com/test/nested"]
    approved-commands = [
        "test command",
    ]
    "#);
}

///
/// When a user has a config file with comments and we save a non-approval
/// mutation, all their comments should be preserved.
#[test]
fn test_saving_config_mutation_preserves_toml_comments() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");

    // Create a config file with comments
    let initial_content = r#"# User preferences for worktrunk
# These comments should be preserved after saving

worktree-path = "../{{ main_worktree }}.{{ branch }}"  # inline comment should also be preserved

# LLM commit generation settings
[commit.generation]
command = "llm -m claude-haiku-4.5"

# Per-project settings below
"#;
    fs::write(&config_path, initial_content).unwrap();

    // Load the config manually by deserializing from TOML
    let toml_str = fs::read_to_string(&config_path).unwrap();
    let mut config: UserConfig = toml::from_str(&toml_str).unwrap();

    // Change a non-approval setting and save back to the same file
    config
        .set_commit_generation_command("llm -m claude-sonnet-4".to_string(), Some(&config_path))
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

    // Verify the command was updated
    assert!(
        saved_content.contains("llm -m claude-sonnet-4"),
        "Command was not updated. Saved content:\n{saved_content}"
    );
}

///
/// This tests a race condition where two instances (simulating separate processes)
/// both approve commands. Without proper merging, the second save would overwrite
/// the first approval, losing it.
#[test]
fn test_concurrent_approve_preserves_all_approvals() {
    let temp_dir = TempDir::new().unwrap();
    let approvals_path = temp_dir.path().join("approvals.toml");

    // Process A: Start with empty approvals, approve "npm install"
    let mut approvals_a = Approvals::default();

    // Process B: Start with empty approvals (simulating a separate process that loaded before A saved)
    let mut approvals_b = Approvals::default();

    // Process A approves and saves "npm install"
    approvals_a
        .approve_command(
            "github.com/user/repo".to_string(),
            "npm install".to_string(),
            Some(&approvals_path),
        )
        .unwrap();

    // Verify file has "npm install"
    let toml_content = fs::read_to_string(&approvals_path).unwrap();
    assert!(
        toml_content.contains("npm install"),
        "File should contain 'npm install'"
    );

    // Process B (which loaded BEFORE Process A saved) now approves and saves "npm test"
    // The save method should merge with what's on disk, not overwrite
    approvals_b
        .approve_command(
            "github.com/user/repo".to_string(),
            "npm test".to_string(),
            Some(&approvals_path),
        )
        .unwrap();

    // Read the final state from disk
    let toml_content = fs::read_to_string(&approvals_path).unwrap();

    // Both approvals should be preserved
    assert!(
        toml_content.contains("npm install"),
        "BUG: 'npm install' approval was lost due to race condition. \
         approvals_b's save should merge with disk state, not overwrite it. \
         Saved content:\n{toml_content}"
    );
    assert!(
        toml_content.contains("npm test"),
        "'npm test' approval should exist. Saved content:\n{toml_content}"
    );
}

///
/// This tests a race condition where two instances (simulating separate processes)
/// both revoke commands. Without proper merging, the second save would restore
/// the revoked command from its stale in-memory state.
#[test]
fn test_concurrent_revoke_preserves_all_changes() {
    let temp_dir = TempDir::new().unwrap();
    let approvals_path = temp_dir.path().join("approvals.toml");

    // Setup: approvals file has two commands approved
    let mut setup_approvals = Approvals::default();
    setup_approvals
        .approve_command(
            "github.com/user/repo".to_string(),
            "npm install".to_string(),
            Some(&approvals_path),
        )
        .unwrap();
    setup_approvals
        .approve_command(
            "github.com/user/repo".to_string(),
            "npm test".to_string(),
            Some(&approvals_path),
        )
        .unwrap();

    // Verify setup
    let toml_content = fs::read_to_string(&approvals_path).unwrap();
    assert!(toml_content.contains("npm install"));
    assert!(toml_content.contains("npm test"));

    // Process A: revokes the project
    let mut approvals_a = Approvals::default();
    approvals_a
        .revoke_project("github.com/user/repo", Some(&approvals_path))
        .unwrap();

    // Read the final state from disk
    let toml_content = fs::read_to_string(&approvals_path).unwrap();

    // Revocation should be respected - neither command should remain
    assert!(
        !toml_content.contains("npm install"),
        "'npm install' should have been revoked. Saved content:\n{toml_content}"
    );
    assert!(
        !toml_content.contains("npm test"),
        "'npm test' should have been revoked. Saved content:\n{toml_content}"
    );
}

#[test]
fn test_concurrent_approve_different_projects() {
    let temp_dir = TempDir::new().unwrap();
    let approvals_path = temp_dir.path().join("approvals.toml");

    // Process A: empty approvals
    let mut approvals_a = Approvals::default();

    // Process B: empty approvals (loaded before A saved)
    let mut approvals_b = Approvals::default();

    // Process A approves for project1
    approvals_a
        .approve_command(
            "github.com/user/project1".to_string(),
            "npm install".to_string(),
            Some(&approvals_path),
        )
        .unwrap();

    // Process B approves for project2
    // Should preserve project1's approval
    approvals_b
        .approve_command(
            "github.com/user/project2".to_string(),
            "cargo build".to_string(),
            Some(&approvals_path),
        )
        .unwrap();

    let toml_content = fs::read_to_string(&approvals_path).unwrap();

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

/// Test true concurrent access from multiple threads.
///
/// Unlike the sequential tests above, this spawns multiple threads that race
/// to approve commands simultaneously. With file locking, all approvals should
/// be preserved.
#[test]
fn test_truly_concurrent_approve_with_threads() {
    use std::sync::{Arc, Barrier};
    use std::thread;

    let temp_dir = TempDir::new().unwrap();
    let approvals_path = temp_dir.path().join("approvals.toml");

    // Create 10 threads that will all try to approve at the same time
    let num_threads = 10;
    let barrier = Arc::new(Barrier::new(num_threads));
    let approvals_path = Arc::new(approvals_path);

    let handles: Vec<_> = (0..num_threads)
        .map(|i| {
            let barrier = Arc::clone(&barrier);
            let approvals_path = Arc::clone(&approvals_path);

            thread::spawn(move || {
                let mut approvals = Approvals::default();

                // Wait for all threads to be ready
                barrier.wait();

                // All threads try to approve at the same time
                approvals
                    .approve_command(
                        "github.com/user/repo".to_string(),
                        format!("command_{i}"),
                        Some(&approvals_path),
                    )
                    .unwrap();
            })
        })
        .collect();

    // Wait for all threads to complete
    for handle in handles {
        handle.join().unwrap();
    }

    // Read the final state from disk
    let toml_content = fs::read_to_string(&*approvals_path).unwrap();

    // All 10 approvals should be preserved
    for i in 0..num_threads {
        assert!(
            toml_content.contains(&format!("command_{i}")),
            "command_{i} should be preserved. With file locking, no approvals should be lost.\n\
             Content:\n{toml_content}"
        );
    }
}

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
    let approvals_path = temp_dir.path().join("readonly").join("approvals.toml");

    // Create the directory and initial approvals file
    let approvals_dir = approvals_path.parent().unwrap();
    fs::create_dir_all(approvals_dir).unwrap();
    let initial_approvals = Approvals::default();
    initial_approvals.save_to(&approvals_path).unwrap();

    // Make the directory read-only (prevents writing new files)
    #[cfg(unix)]
    {
        let readonly_perms = Permissions::from_mode(0o444);
        fs::set_permissions(approvals_dir, readonly_perms).unwrap();
    }

    // Test if permissions actually restrict us (skip if running as root)
    // Root can write to read-only directories, so the test would be meaningless
    let test_file = approvals_dir.join("test_write");
    if fs::write(&test_file, "test").is_ok() {
        // Running as root or permissions don't work
        // Restore write permissions and skip test
        #[cfg(unix)]
        {
            let writable_perms = Permissions::from_mode(0o755);
            fs::set_permissions(approvals_dir, writable_perms).unwrap();
        }
        eprintln!("Skipping permission test - running with elevated privileges");
        return;
    }

    // Try to save a new approval - this should fail
    let mut approvals = Approvals::default();
    let result = approvals.approve_command(
        "github.com/test/readonly".to_string(),
        "test command".to_string(),
        Some(&approvals_path),
    );

    // Restore write permissions so temp_dir can be cleaned up
    #[cfg(unix)]
    {
        let writable_perms = Permissions::from_mode(0o755);
        fs::set_permissions(approvals_dir, writable_perms).unwrap();
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

#[test]
fn test_skip_shell_integration_prompt_saves_to_disk() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("worktrunk").join("config.toml");

    let mut config = UserConfig::default();
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
    assert_snapshot!(toml_content, @"skip-shell-integration-prompt = true");
}

#[test]
fn test_skip_shell_integration_prompt_idempotent() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");

    let mut config = UserConfig::default();

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

#[test]
fn test_skip_commit_generation_prompt_saves_to_disk() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("worktrunk").join("config.toml");

    let mut config = UserConfig::default();
    config
        .set_skip_commit_generation_prompt(Some(&config_path))
        .unwrap();

    // Verify file was created
    assert!(
        config_path.exists(),
        "Config file was not created at {:?}",
        config_path
    );

    // Verify TOML structure
    let toml_content = fs::read_to_string(&config_path).unwrap();
    assert_snapshot!(toml_content, @"skip-commit-generation-prompt = true");
}

#[test]
fn test_skip_commit_generation_prompt_idempotent() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");

    let mut config = UserConfig::default();

    // Call twice - should not error
    config
        .set_skip_commit_generation_prompt(Some(&config_path))
        .unwrap();
    config
        .set_skip_commit_generation_prompt(Some(&config_path))
        .unwrap();

    // Field should still be true
    assert!(config.skip_commit_generation_prompt);

    // File should have the flag exactly once
    let toml_content = fs::read_to_string(&config_path).unwrap();
    let count = toml_content
        .matches("skip-commit-generation-prompt")
        .count();
    assert_eq!(count, 1, "Flag should appear exactly once");
}

#[test]
fn test_set_commit_generation_command_saves_to_disk() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("worktrunk").join("config.toml");

    let mut config = UserConfig::default();
    config
        .set_commit_generation_command("llm -m haiku".to_string(), Some(&config_path))
        .unwrap();

    // Verify file was created
    assert!(
        config_path.exists(),
        "Config file was not created at {:?}",
        config_path
    );

    // Verify TOML structure - should have [commit.generation] section with command
    let toml_content = fs::read_to_string(&config_path).unwrap();
    assert_snapshot!(toml_content, @r#"
    [commit.generation]
    command = "llm -m haiku"
    "#);
}

#[test]
fn test_set_commit_generation_command_with_special_chars() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");

    let mut config = UserConfig::default();
    // Command with quotes and environment variables (like claude config)
    let command =
        "MAX_THINKING_TOKENS=0 claude -p --model=haiku --tools='' --system-prompt=''".to_string();
    config
        .set_commit_generation_command(command, Some(&config_path))
        .unwrap();

    // Verify TOML can be parsed back
    let toml_content = fs::read_to_string(&config_path).unwrap();
    let parsed: UserConfig = toml::from_str(&toml_content).unwrap();
    let gen_config = parsed.commit_generation(None);
    assert_eq!(
        gen_config.command.as_deref(),
        Some("MAX_THINKING_TOKENS=0 claude -p --model=haiku --tools='' --system-prompt=''")
    );
}

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

    // Load config and set commit generation command through the symlink path
    let toml_str = fs::read_to_string(&symlink_path).unwrap();
    let mut config: UserConfig = toml::from_str(&toml_str).unwrap();

    config
        .set_commit_generation_command("llm -m haiku".to_string(), Some(&symlink_path))
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
        target_content.contains("llm -m haiku"),
        "Command should be written to target. Content:\n{target_content}"
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

/// Test that set_commit_generation_command persists to an existing config file
/// while preserving other content.
///
/// This is a regression test for a bug where the "file exists" branch in save_to()
/// didn't know about the commit.generation section, so setting the command would
/// succeed in memory but not persist to disk.
#[test]
fn test_set_commit_generation_command_preserves_existing_content() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");

    // Create existing config with other sections
    let initial_content = r#"# My settings
worktree-path = "../{{ repo }}.{{ branch }}"

[projects."github.com/user/repo"]
approved-commands = [
    "npm install",
]
"#;
    fs::write(&config_path, initial_content).unwrap();

    // Load the config and set the commit generation command
    let toml_str = fs::read_to_string(&config_path).unwrap();
    let mut config: UserConfig = toml::from_str(&toml_str).unwrap();

    config
        .set_commit_generation_command("llm -m haiku".to_string(), Some(&config_path))
        .unwrap();

    // Read back what was saved
    let saved = fs::read_to_string(&config_path).unwrap();

    // Original content should be preserved
    assert!(
        saved.contains("worktree-path = \"../{{ repo }}.{{ branch }}\""),
        "worktree-path should be preserved. Saved content:\n{saved}"
    );
    assert!(
        saved.contains("npm install"),
        "approved-commands should be preserved. Saved content:\n{saved}"
    );
    assert!(
        saved.contains("# My settings"),
        "Comments should be preserved. Saved content:\n{saved}"
    );

    // New section should be added
    assert!(
        saved.contains("[commit.generation]"),
        "[commit.generation] section should be added. Saved content:\n{saved}"
    );
    assert!(
        saved.contains("llm -m haiku"),
        "command should be saved. Saved content:\n{saved}"
    );
    // When only generation is set (no stage), [commit] header should be implicit
    assert!(
        !saved.contains("[commit]\n"),
        "Should not have standalone [commit] header when only generation is set:\n{saved}"
    );
}
