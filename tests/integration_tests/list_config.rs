//! Tests for `wt list` command with user config

use crate::common::{
    TestRepo, repo, set_temp_home_env, setup_snapshot_settings_with_home, temp_home, wt_command,
};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::fs;
use tempfile::TempDir;

/// Test `wt list` with config setting full = true
#[rstest]
fn test_list_config_full_enabled(repo: TestRepo, temp_home: TempDir) {
    // Create user config with list.full = true
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."repo".list]
full = true
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.arg("list").current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test `wt list` with config setting branches = true
#[rstest]
fn test_list_config_branches_enabled(repo: TestRepo, temp_home: TempDir) {
    // Create a branch without a worktree
    repo.run_git(&["branch", "feature"]);

    // Create user config with list.branches = true
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."repo".list]
branches = true
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.arg("list").current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test that CLI flags override config settings
#[rstest]
fn test_list_config_cli_override(repo: TestRepo, temp_home: TempDir) {
    // Create a branch without a worktree
    repo.run_git(&["branch", "feature"]);

    // Create user config with list.branches = false (default)
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."repo".list]
branches = false
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        // CLI flag --branches should override config
        cmd.arg("list")
            .arg("--branches")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test `wt list` with both full and branches config enabled
#[rstest]
fn test_list_config_full_and_branches(repo: TestRepo, temp_home: TempDir) {
    // Create a branch without a worktree
    repo.run_git(&["branch", "feature"]);

    // Create user config with both full and branches enabled
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."repo".list]
full = true
branches = true
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.arg("list").current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test `wt list` without config (default behavior)
#[rstest]
fn test_list_no_config(repo: TestRepo, temp_home: TempDir) {
    // Create a branch without a worktree
    repo.run_git(&["branch", "feature"]);

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
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.arg("list").current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test `wt list` with project config URL template
#[rstest]
fn test_list_project_url_column(repo: TestRepo, temp_home: TempDir) {
    // Create project config with URL template
    repo.write_project_config(
        r#"[list]
url = "http://localhost:{{ branch | hash_port }}"
"#,
    );

    // Create user config
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
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.arg("list").current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test `wt list --format=json` includes URL fields when template configured
#[rstest]
fn test_list_json_url_fields(repo: TestRepo, temp_home: TempDir) {
    // Create project config with URL template
    repo.write_project_config(
        r#"[list]
url = "http://localhost:{{ branch | hash_port }}"
"#,
    );

    // Create user config
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#,
    )
    .unwrap();

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    set_temp_home_env(&mut cmd, temp_home.path());
    cmd.args(["list", "--format=json"])
        .current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse JSON and verify URL fields
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let items = json.as_array().unwrap();
    assert!(!items.is_empty());

    let first = &items[0];
    // URL should be present and contain the hash_port result
    assert!(first.get("url").is_some());
    let url = first["url"].as_str().unwrap();
    assert!(url.starts_with("http://localhost:"));
    // Port should be 5 digits (10000-19999)
    let port: u16 = url.split(':').next_back().unwrap().parse().unwrap();
    assert!((10000..=19999).contains(&port));

    // url_active should be present and be a boolean
    // Note: We can't assert the specific value since it depends on whether
    // something happens to be listening on the hashed port
    assert!(first.get("url_active").is_some());
    assert!(first["url_active"].as_bool().is_some());
}

/// Test `wt list --format=json` has null URL fields when no template configured
#[rstest]
fn test_list_json_no_url_without_template(repo: TestRepo, temp_home: TempDir) {
    // Create user config WITHOUT URL template
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#,
    )
    .unwrap();

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    set_temp_home_env(&mut cmd, temp_home.path());
    cmd.args(["list", "--format=json"])
        .current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse JSON and verify URL fields are null
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let items = json.as_array().unwrap();
    assert!(!items.is_empty());

    let first = &items[0];
    // URL should be null when no template configured
    assert!(first["url"].is_null());
    assert!(first["url_active"].is_null());
}

/// Test URL column with --branches flag
#[rstest]
fn test_list_url_with_branches_flag(repo: TestRepo, temp_home: TempDir) {
    // Create a branch without a worktree
    repo.run_git(&["branch", "feature"]);

    // Create project config with URL template
    repo.write_project_config(
        r#"[list]
url = "http://localhost:{{ branch | hash_port }}"
"#,
    );

    // Create user config
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#,
    )
    .unwrap();

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    set_temp_home_env(&mut cmd, temp_home.path());
    cmd.args(["list", "--branches", "--format=json"])
        .current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse JSON and verify both worktree and branch have URLs
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let items = json.as_array().unwrap();
    assert_eq!(items.len(), 2); // main worktree + feature branch

    // Both items should have URLs
    for item in items {
        let url = item["url"].as_str().unwrap();
        assert!(url.starts_with("http://localhost:"));
    }
}

/// Test URL with {{ branch }} variable (not hash_port)
#[rstest]
fn test_list_url_with_branch_variable(repo: TestRepo, temp_home: TempDir) {
    // Create project config with {{ branch }} in URL
    repo.write_project_config(
        r#"[list]
url = "http://localhost:8080/{{ branch }}"
"#,
    );

    // Create user config
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#,
    )
    .unwrap();

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    set_temp_home_env(&mut cmd, temp_home.path());
    cmd.args(["list", "--format=json"])
        .current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse JSON and verify URL contains branch name
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let items = json.as_array().unwrap();
    let first = &items[0];

    let url = first["url"].as_str().unwrap();
    assert_eq!(url, "http://localhost:8080/main");
}
