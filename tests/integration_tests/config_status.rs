use crate::common::{TestRepo, wt_command};
use insta::assert_snapshot;
use std::process::Command;

fn wt_config_status_cmd(repo: &TestRepo, args: &[&str]) -> Command {
    let mut cmd = wt_command();
    repo.clean_cli_env(&mut cmd);
    cmd.args(["config", "status"]);
    cmd.args(args);
    cmd.current_dir(repo.root_path());
    cmd
}

#[test]
fn test_config_status_set_branch_default() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    let output = wt_config_status_cmd(&repo, &["set", "🚧"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"✅ [32mSet status for [1mmain[22m to [1m🚧[22m[39m");

    // Verify it was set
    let output = repo
        .git_command(&["config", "--get", "worktrunk.status.main"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "🚧");
}

#[test]
fn test_config_status_set_branch_specific() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.git_command(&["branch", "feature"]).status().unwrap();

    let output = wt_config_status_cmd(&repo, &["set", "--branch", "feature", "🔧"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"✅ [32mSet status for [1mfeature[22m to [1m🔧[22m[39m");

    // Verify it was set
    let output = repo
        .git_command(&["config", "--get", "worktrunk.status.feature"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "🔧");
}

#[test]
fn test_config_status_unset_branch_default() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Set a status first
    repo.git_command(&["config", "worktrunk.status.main", "🚧"])
        .status()
        .unwrap();

    let output = wt_config_status_cmd(&repo, &["unset"]).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"✅ [32mCleared status for [1mmain[22m[39m");

    // Verify it was unset
    let output = repo
        .git_command(&["config", "--get", "worktrunk.status.main"])
        .output()
        .unwrap();
    assert!(!output.status.success());
}

#[test]
fn test_config_status_unset_branch_specific() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Set a status first
    repo.git_command(&["config", "worktrunk.status.feature", "🔧"])
        .status()
        .unwrap();

    let output = wt_config_status_cmd(&repo, &["unset", "feature"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"✅ [32mCleared status for [1mfeature[22m[39m");

    // Verify it was unset
    let output = repo
        .git_command(&["config", "--get", "worktrunk.status.feature"])
        .output()
        .unwrap();
    assert!(!output.status.success());
}

#[test]
fn test_config_status_unset_all() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Set multiple statuses
    repo.git_command(&["config", "worktrunk.status.main", "🚧"])
        .status()
        .unwrap();
    repo.git_command(&["config", "worktrunk.status.feature", "🔧"])
        .status()
        .unwrap();
    repo.git_command(&["config", "worktrunk.status.bugfix", "🐛"])
        .status()
        .unwrap();

    let output = wt_config_status_cmd(&repo, &["unset", "*"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"✅ [32mCleared [1m3[22m statuses[39m");

    // Verify all were unset
    let output = repo
        .git_command(&["config", "--get-regexp", "^worktrunk\\.status\\."])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "");
}

#[test]
fn test_config_status_unset_all_empty() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    let output = wt_config_status_cmd(&repo, &["unset", "*"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"⚪ No statuses to clear");
}
