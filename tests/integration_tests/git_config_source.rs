//! Integration tests for the experimental `worktrunk.config.*` git-config
//! project-config source (#3454).
//!
//! Covers: source selection (keys present → git config wins, file ignored),
//! the supersession warning firing iff a file would otherwise load, scope
//! precedence (local over global) resolved by git itself, `include.path`
//! resolution, loud failure on unexpressible values, and approval gating of
//! git-config-sourced hooks.

use crate::common::{
    TestRepo, repo, set_temp_home_env, setup_snapshot_settings_with_home, temp_home, wt_command,
};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::fs;
use tempfile::TempDir;

fn write_user_config(temp_home: &TempDir) {
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ repo }}.{{ branch }}"
"#,
    )
    .unwrap();
}

/// Keys present AND a project config file present: the git-config source
/// wins, the heading names it, and the supersession warning fires.
#[rstest]
fn test_hook_show_git_config_supersedes_project_file(repo: TestRepo, temp_home: TempDir) {
    write_user_config(&temp_home);
    repo.write_project_config(r#"pre-merge = "cargo test""#);
    repo.commit("Add project config");
    repo.run_git(&["config", "worktrunk.config.post-start", "npm install"]);

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("hook").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Keys present, no project config file: same selection, but nothing is
/// superseded so no warning appears.
#[rstest]
fn test_hook_show_git_config_source_without_file(repo: TestRepo, temp_home: TempDir) {
    write_user_config(&temp_home);
    repo.run_git(&["config", "worktrunk.config.post-start", "npm install"]);

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("hook").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// A value the schema cannot accept as a string fails the load loudly; the
/// project config file is NOT silently used instead (all-or-nothing).
#[rstest]
fn test_git_config_source_invalid_value_fails_loudly(repo: TestRepo, temp_home: TempDir) {
    write_user_config(&temp_home);
    repo.write_project_config(r#"pre-merge = "cargo test""#);
    repo.commit("Add project config");
    // `step.copy-ignored.exclude` is an array in the schema; a string leaf
    // cannot satisfy it.
    repo.run_git(&["config", "worktrunk.config.step.copy-ignored.exclude", "target"]);

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("hook").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// `wt config show` names the git-config source, warns about the superseded
/// file, and dumps the mapped TOML.
#[rstest]
fn test_config_show_git_config_source(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();
    write_user_config(&temp_home);
    repo.write_project_config(r#"pre-merge = "cargo test""#);
    repo.commit("Add project config");
    repo.run_git(&["config", "worktrunk.config.post-start", "npm install"]);
    repo.run_git(&["config", "worktrunk.config.list.url", "http://localhost:3000"]);

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Git resolves scope precedence before worktrunk reads the keys: a local
/// key overrides its global twin, and global-only keys still merge in.
#[rstest]
fn test_git_config_scope_precedence_local_over_global(repo: TestRepo, temp_home: TempDir) {
    write_user_config(&temp_home);
    repo.run_git(&["config", "--global", "worktrunk.config.list.url", "http://global:9999"]);
    repo.run_git(&["config", "--global", "worktrunk.config.forge.platform", "gitlab"]);
    repo.run_git(&["config", "worktrunk.config.list.url", "http://local:3000"]);

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.args(["config", "show", "--format=json"])
        .current_dir(repo.root_path());
    set_temp_home_env(&mut cmd, temp_home.path());

    let output = cmd.output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();

    let project = &json["project"];
    assert_eq!(project["source"], "git-config");
    assert_eq!(project["exists"], true);
    assert_eq!(project["path"], serde_json::Value::Null);
    assert_eq!(project["config"]["list"]["url"], "http://local:3000");
    assert_eq!(project["config"]["forge"]["platform"], "gitlab");
}

/// Keys reachable only through `include.path` resolve like any other git
/// config — git processes includes before worktrunk sees the merged list.
#[rstest]
fn test_git_config_source_include_path(repo: TestRepo, temp_home: TempDir) {
    write_user_config(&temp_home);
    let fragment = temp_home.path().join("wt-private.gitconfig");
    fs::write(
        &fragment,
        r#"[worktrunk "config.list"]
	url = http://from-include:1234
"#,
    )
    .unwrap();
    repo.run_git(&["config", "include.path", fragment.to_str().unwrap()]);

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.args(["config", "show", "--format=json"])
        .current_dir(repo.root_path());
    set_temp_home_env(&mut cmd, temp_home.path());

    let output = cmd.output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["project"]["source"], "git-config");
    assert_eq!(
        json["project"]["config"]["list"]["url"],
        "http://from-include:1234"
    );
}

/// Git-config-sourced hooks pass through the same approval gate as
/// file-based project config — nothing about the source is trusted.
#[rstest]
fn test_git_config_source_hooks_require_approval(repo: TestRepo, temp_home: TempDir) {
    write_user_config(&temp_home);
    repo.run_git(&["config", "worktrunk.config.post-start", "npm install"]);

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.args(["hook", "show", "--format=json"])
        .current_dir(repo.root_path());
    set_temp_home_env(&mut cmd, temp_home.path());

    let output = cmd.output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let hook = json
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["source"] == "project")
        .expect("project hook present");
    assert_eq!(hook["template"], "npm install");
    assert_eq!(hook["needs_approval"], true);
}
