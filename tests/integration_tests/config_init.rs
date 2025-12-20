use crate::common::{
    TestRepo, make_snapshot_cmd, repo, set_temp_home_env, setup_home_snapshot_settings,
    setup_snapshot_settings, temp_home, wt_command,
};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::fs;
use tempfile::TempDir;

/// Test `wt config create` when config already exists (should show info message with emoji)
#[rstest]
fn test_config_init_already_exists(temp_home: TempDir) {
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

        💡 [2mFor format reference, run [90mwt config create --help[39m; to view, run [90mwt config show[39m[22m
        ");
    });
}

/// Test `wt config create` creates new config file
#[rstest]
fn test_config_init_creates_file(temp_home: TempDir) {
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
        ✅ [32mCreated user config: [1m~/.config/worktrunk/config.toml[22m[39m

        💡 [2mEdit this file to customize worktree paths and LLM settings[22m
        ");
    });

    // Verify file was actually created
    let config_path = global_config_dir.join("config.toml");
    assert!(config_path.exists(), "Config file should be created");
}

/// Test `wt config create --project` creates project config file
#[rstest]
fn test_config_create_project_creates_file(repo: TestRepo) {
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "config", &["create", "--project"], None);
        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        ✅ [32mCreated project config: [1m_REPO_/.config/wt.toml[22m[39m

        💡 [2mEdit this file to configure hooks for this repository[22m
        💡 [2mSee https://worktrunk.dev/hooks/ for hook documentation[22m
        ");
    });

    // Verify file was actually created
    let config_path = repo.root_path().join(".config/wt.toml");
    assert!(
        config_path.exists(),
        "Project config file should be created"
    );
}

/// Test `wt config create --project` when project config already exists
#[rstest]
fn test_config_create_project_already_exists(repo: TestRepo) {
    // Create project config file
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("wt.toml"),
        r#"[[project.post-create]]
run = "echo hello"
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "config", &["create", "--project"], None);
        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        ⚪ Project config already exists: [1m_REPO_/.config/wt.toml[22m

        💡 [2mFor format reference, run [90mwt config create --help[39m; to view, run [90mwt config show[39m[22m
        ");
    });
}
