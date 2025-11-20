use crate::common::{TestRepo, set_temp_home_env, wt_command};
use insta::Settings;
use insta_cmd::assert_cmd_snapshot;
use std::fs;
use tempfile::TempDir;

/// Test `wt config list` with both global and project configs present
#[test]
fn test_config_list_with_project_config() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    // Create fake global config at XDG path (used on all platforms with etcetera)
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[[approved-commands]]
project = "test-project"
command = "npm install"
"#,
    )
    .unwrap();

    // Create project config
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("wt.toml"),
        r#"post-create-command = "npm install"

[post-start-command]
server = "npm run dev"
"#,
    )
    .unwrap();

    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    // Filter out the actual config paths to use placeholders
    settings.add_filter(repo.root_path().to_str().unwrap(), "[REPO]");
    settings.add_filter(&temp_home.path().to_string_lossy(), "[TEMP_HOME]");
    // Normalize Windows paths to Unix style
    settings.add_filter(r"\\", "/");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("config").arg("list").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd, @r#"
        success: true
        exit_code: 0
        ----- stdout -----
        ⚪ Global Config: [1m~/.config/worktrunk/config.toml[0m
        [40m [0m  worktree-path = [32m"../{{ main_worktree }}.{{ branch }}"[0m
        [40m [0m  
        [40m [0m  [1m[36m[[approved-commands]][0m
        [40m [0m  project = [32m"test-project"[0m
        [40m [0m  command = [32m"npm install"[0m

        ⚪ Project Config: [1m[REPO]/.config/wt.toml[0m
        [40m [0m  post-create-command = [32m"npm install"[0m
        [40m [0m  
        [40m [0m  [1m[36m[post-start-command][0m
        [40m [0m  server = [32m"npm run dev"[0m

        ----- stderr -----
        "#);
    });
}

/// Test `wt config list` when there is no project config
#[test]
fn test_config_list_no_project_config() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    // Create fake global config (but no project config) at XDG path
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

    // Filter out the actual config paths to use placeholders
    settings.add_filter(repo.root_path().to_str().unwrap(), "[REPO]");
    settings.add_filter(&temp_home.path().to_string_lossy(), "[TEMP_HOME]");
    settings.add_filter(r"\\", "/");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("config").arg("list").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd, @r#"
        success: true
        exit_code: 0
        ----- stdout -----
        ⚪ Global Config: [1m~/.config/worktrunk/config.toml[0m
        [40m [0m  worktree-path = [32m"../{{ main_worktree }}.{{ branch }}"[0m

        ⚪ Project Config: [1m[REPO]/.config/wt.toml[0m
        💡 [2mNot found[0m

        ----- stderr -----
        "#);
    });
}

/// Test `wt config list` outside a git repository
#[test]
fn test_config_list_outside_git_repo() {
    let temp_dir = tempfile::tempdir().unwrap();
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
        cmd.arg("config").arg("list").current_dir(temp_dir.path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd, @r#"
        success: true
        exit_code: 0
        ----- stdout -----
        ⚪ Global Config: [1m~/.config/worktrunk/config.toml[0m
        [40m [0m  worktree-path = [32m"../{{ main_worktree }}.{{ branch }}"[0m

        ⚪ [2mProject Config: Not in a git repository[0m

        ----- stderr -----
        "#);
    });
}
