use crate::common::{set_temp_home_env, wt_command};
use insta::Settings;
use insta_cmd::assert_cmd_snapshot;
use std::fs;
use tempfile::TempDir;

/// Test `wt config init` when config already exists (should show info message with emoji)
#[test]
fn test_config_init_already_exists() {
    let temp_home = TempDir::new().unwrap();

    // Create fake global config at XDG path
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#,
    )
    .unwrap();

    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    // Filter out the actual config paths
    settings.add_filter(&temp_home.path().to_string_lossy(), "[TEMP_HOME]");
    settings.add_filter(r"\\", "/");

    settings.bind(|| {
        let mut cmd = wt_command();
        cmd.arg("config").arg("init");
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----
        ⚪ Global config already exists: [1m[TEMP_HOME]/.config/worktrunk/config.toml[0m

        💡 [2mUse 'wt config list' to view existing configuration[0m

        ----- stderr -----
        ");
    });
}

/// Test `wt config init` creates new config file
#[test]
fn test_config_init_creates_file() {
    let temp_home = TempDir::new().unwrap();

    // Don't create config file - let init create it
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();

    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    // Filter out the actual config paths
    settings.add_filter(&temp_home.path().to_string_lossy(), "[TEMP_HOME]");
    settings.add_filter(r"\\", "/");

    settings.bind(|| {
        let mut cmd = wt_command();
        cmd.arg("config").arg("init");
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----
        ✅ [32mCreated config file: [1m[32m[TEMP_HOME]/.config/worktrunk/config.toml[0m

        💡 [2mEdit this file to customize worktree paths and LLM settings[0m

        ----- stderr -----
        ");
    });

    // Verify file was actually created
    let config_path = global_config_dir.join("config.toml");
    assert!(config_path.exists(), "Config file should be created");
}
