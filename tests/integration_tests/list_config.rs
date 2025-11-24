//! Tests for `wt list` command with user config

use crate::common::{TestRepo, set_temp_home_env, setup_snapshot_settings_with_home, wt_command};
use insta_cmd::assert_cmd_snapshot;
use std::fs;
use tempfile::TempDir;

/// Test `wt list` with config setting full = true
#[test]
fn test_list_config_full_enabled() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");
    let temp_home = TempDir::new().unwrap();

    // Create user config with list.full = true
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."test-repo".list]
full = true
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.arg("list").current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test `wt list` with config setting branches = true
#[test]
fn test_list_config_branches_enabled() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create a branch without a worktree
    let mut cmd = std::process::Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["branch", "feature"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    let temp_home = TempDir::new().unwrap();

    // Create user config with list.branches = true
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."test-repo".list]
branches = true
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.arg("list").current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test that CLI flags override config settings
#[test]
fn test_list_config_cli_override() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create a branch without a worktree
    let mut cmd = std::process::Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["branch", "feature"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    let temp_home = TempDir::new().unwrap();

    // Create user config with list.branches = false (default)
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."test-repo".list]
branches = false
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        // CLI flag --branches should override config
        cmd.arg("list")
            .arg("--branches")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test `wt list` with both full and branches config enabled
#[test]
fn test_list_config_full_and_branches() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create a branch without a worktree
    let mut cmd = std::process::Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["branch", "feature"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    let temp_home = TempDir::new().unwrap();

    // Create user config with both full and branches enabled
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."test-repo".list]
full = true
branches = true
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.arg("list").current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test `wt list` without config (default behavior)
#[test]
fn test_list_no_config() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create a branch without a worktree
    let mut cmd = std::process::Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["branch", "feature"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    let temp_home = TempDir::new().unwrap();

    // Create minimal user config without list settings
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.arg("list").current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}
