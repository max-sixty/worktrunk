use crate::common::{set_temp_home_env, setup_home_snapshot_settings, wt_command};
use insta_cmd::assert_cmd_snapshot;
use std::fs;
use tempfile::TempDir;

/// Test `wt config create` when config already exists (should show info message with emoji)
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

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        cmd.arg("config").arg("create");
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        ⚪ User config already exists: [1m~/.config/worktrunk/config.toml[22m

        💡 [2mUse [90mwt config show[39m to view existing configuration[22m
        💡 [2mUse [90mwt config create --help[39m for config format reference[22m
        ");
    });
}

/// Test `wt config create` creates new config file
#[test]
fn test_config_init_creates_file() {
    let temp_home = TempDir::new().unwrap();

    // Don't create config file - let create create it
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        cmd.arg("config").arg("create");
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        ✅ [32mCreated config file: [1m~/.config/worktrunk/config.toml[22m[39m

        💡 [2mEdit this file to customize worktree paths and LLM settings[22m
        ");
    });

    // Verify file was actually created
    let config_path = global_config_dir.join("config.toml");
    assert!(config_path.exists(), "Config file should be created");
}
