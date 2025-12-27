//! Integration tests for `wt hook show` command

use crate::common::{
    TestRepo, repo, set_temp_home_env, setup_home_snapshot_settings,
    setup_snapshot_settings_with_home, temp_home, wt_command,
};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::fs;
use tempfile::TempDir;

/// Test `wt hook show` with both user and project hooks
#[rstest]
fn test_hook_show_with_both_configs(repo: TestRepo, temp_home: TempDir) {
    // Create user config with hooks
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[pre-commit]
user-lint = "pre-commit run --all-files"
"#,
    )
    .unwrap();

    // Create project config with hooks
    repo.write_project_config(
        r#"[post-start]
deps = "npm install"

[pre-merge]
build = "cargo build"
test = "cargo test"
"#,
    );
    repo.commit("Add project config");

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("hook").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test `wt hook show` with no hooks configured
#[rstest]
fn test_hook_show_no_hooks(repo: TestRepo, temp_home: TempDir) {
    // Create user config without hooks
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
        cmd.arg("hook").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test `wt hook show` filtering by hook type
#[rstest]
fn test_hook_show_filter_by_type(repo: TestRepo, temp_home: TempDir) {
    // Create user config without hooks
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#,
    )
    .unwrap();

    // Create project config with multiple hook types
    repo.write_project_config(
        r#"[post-start]
deps = "npm install"

[pre-merge]
build = "cargo build"
test = "cargo test"

[post-merge]
deploy = "scripts/deploy.sh"
"#,
    );
    repo.commit("Add project config");

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("hook")
            .arg("show")
            .arg("pre-merge")
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test `wt hook show` shows approval status for project hooks
#[rstest]
fn test_hook_show_approval_status(repo: TestRepo, temp_home: TempDir) {
    // Create user config at XDG path with one approved command
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    let config_path = global_config_dir.join("config.toml");
    fs::write(
        &config_path,
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."repo"]
approved-commands = ["cargo build"]
"#,
    )
    .unwrap();

    // Create project config with approved and unapproved hooks
    repo.write_project_config(
        r#"[pre-merge]
build = "cargo build"
test = "cargo test"
"#,
    );
    repo.commit("Add project config");

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        // Override config path to point to our test config with approval
        cmd.env("WORKTRUNK_CONFIG_PATH", &config_path);
        cmd.arg("hook").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test error formatting with context (error message + gutter for multi-line)
#[rstest]
fn test_error_with_context_formatting(temp_home: TempDir) {
    let temp_dir = tempfile::tempdir().unwrap();

    // Run wt remove outside a git repo - should show "Failed to remove worktree" context
    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        cmd.arg("remove").current_dir(temp_dir.path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test `wt hook show` when project config exists but has no hooks
#[rstest]
fn test_hook_show_project_config_no_hooks(repo: TestRepo, temp_home: TempDir) {
    // Create user config without hooks
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#,
    )
    .unwrap();

    // Create project config without any hook sections
    repo.write_project_config(
        r#"# Project config with no hooks
worktree-path = "../project.{{ branch }}"
"#,
    );
    repo.commit("Add project config without hooks");

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("hook").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test `wt hook show` outside git repo
#[rstest]
fn test_hook_show_outside_git_repo(temp_home: TempDir) {
    let temp_dir = tempfile::tempdir().unwrap();

    // Create user config
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[pre-commit]
lint = "pre-commit run"
"#,
    )
    .unwrap();

    let mut settings = setup_home_snapshot_settings(&temp_home);
    // Replace temp home path with ~ for stable snapshots (override the [TEMP_HOME] filter)
    settings.add_filter(&regex::escape(&temp_home.path().to_string_lossy()), "~");
    settings.bind(|| {
        let mut cmd = wt_command();
        cmd.arg("hook").arg("show").current_dir(temp_dir.path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}
