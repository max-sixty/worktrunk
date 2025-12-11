use crate::common::{TestRepo, repo, wt_command};
use insta::assert_snapshot;
use rstest::rstest;
use std::process::Command;

fn wt_var_set_cmd(repo: &TestRepo, args: &[&str]) -> Command {
    let mut cmd = wt_command();
    repo.clean_cli_env(&mut cmd);
    cmd.args(["config", "var", "set"]);
    cmd.args(args);
    cmd.current_dir(repo.root_path());
    cmd
}

fn wt_var_clear_cmd(repo: &TestRepo, args: &[&str]) -> Command {
    let mut cmd = wt_command();
    repo.clean_cli_env(&mut cmd);
    cmd.args(["config", "var", "clear"]);
    cmd.args(args);
    cmd.current_dir(repo.root_path());
    cmd
}

fn wt_var_get_cmd(repo: &TestRepo, args: &[&str]) -> Command {
    let mut cmd = wt_command();
    repo.clean_cli_env(&mut cmd);
    cmd.args(["config", "var", "get"]);
    cmd.args(args);
    cmd.current_dir(repo.root_path());
    cmd
}

#[rstest]
fn test_var_set_marker_branch_default(repo: TestRepo) {
    let output = wt_var_set_cmd(&repo, &["marker", "🚧"]).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"✅ [32mSet marker for [1mmain[22m to [1m🚧[22m[39m");

    // Verify it was set
    let output = repo
        .git_command(&["config", "--get", "worktrunk.marker.main"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "🚧");
}

#[rstest]
fn test_var_set_marker_branch_specific(repo: TestRepo) {
    repo.git_command(&["branch", "feature"]).status().unwrap();

    let output = wt_var_set_cmd(&repo, &["marker", "🔧", "--branch", "feature"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"✅ [32mSet marker for [1mfeature[22m to [1m🔧[22m[39m");

    // Verify it was set
    let output = repo
        .git_command(&["config", "--get", "worktrunk.marker.feature"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "🔧");
}

#[rstest]
fn test_var_clear_marker_branch_default(repo: TestRepo) {
    // Set a marker first
    repo.git_command(&["config", "worktrunk.marker.main", "🚧"])
        .status()
        .unwrap();

    let output = wt_var_clear_cmd(&repo, &["marker"]).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"✅ [32mCleared marker for [1mmain[22m[39m");

    // Verify it was unset
    let output = repo
        .git_command(&["config", "--get", "worktrunk.marker.main"])
        .output()
        .unwrap();
    assert!(!output.status.success());
}

#[rstest]
fn test_var_clear_marker_branch_specific(repo: TestRepo) {
    // Set a marker first
    repo.git_command(&["config", "worktrunk.marker.feature", "🔧"])
        .status()
        .unwrap();

    let output = wt_var_clear_cmd(&repo, &["marker", "--branch", "feature"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"✅ [32mCleared marker for [1mfeature[22m[39m");

    // Verify it was unset
    let output = repo
        .git_command(&["config", "--get", "worktrunk.marker.feature"])
        .output()
        .unwrap();
    assert!(!output.status.success());
}

#[rstest]
fn test_var_clear_marker_all(repo: TestRepo) {
    // Set multiple markers
    repo.git_command(&["config", "worktrunk.marker.main", "🚧"])
        .status()
        .unwrap();
    repo.git_command(&["config", "worktrunk.marker.feature", "🔧"])
        .status()
        .unwrap();
    repo.git_command(&["config", "worktrunk.marker.bugfix", "🐛"])
        .status()
        .unwrap();

    let output = wt_var_clear_cmd(&repo, &["marker", "--all"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"✅ [32mCleared [1m3[22m markers[39m");

    // Verify all were unset
    let output = repo
        .git_command(&["config", "--get-regexp", "^worktrunk\\.marker\\."])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "");
}

#[rstest]
fn test_var_clear_marker_all_empty(repo: TestRepo) {
    let output = wt_var_clear_cmd(&repo, &["marker", "--all"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"⚪ No markers to clear");
}

#[rstest]
fn test_var_get_marker(repo: TestRepo) {
    // Set a marker first
    repo.git_command(&["config", "worktrunk.marker.main", "🚧"])
        .status()
        .unwrap();

    let output = wt_var_get_cmd(&repo, &["marker"]).output().unwrap();
    assert!(output.status.success());
    // data() writes to stdout for piping
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "🚧");
}

#[rstest]
fn test_var_get_marker_empty(repo: TestRepo) {
    let output = wt_var_get_cmd(&repo, &["marker"]).output().unwrap();
    assert!(output.status.success());
    // Empty output when no marker is set
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "");
}

#[rstest]
fn test_var_get_marker_specific_branch(repo: TestRepo) {
    repo.git_command(&["branch", "feature"]).status().unwrap();

    // Set a marker for feature branch
    repo.git_command(&["config", "worktrunk.marker.feature", "🔧"])
        .status()
        .unwrap();

    let output = wt_var_get_cmd(&repo, &["marker", "--branch", "feature"])
        .output()
        .unwrap();
    assert!(output.status.success());
    // data() writes to stdout for piping
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "🔧");
}

#[rstest]
fn test_var_get_default_branch(repo: TestRepo) {
    let output = wt_var_get_cmd(&repo, &["default-branch"]).output().unwrap();
    assert!(output.status.success());
    // data() writes to stdout for piping
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "main");
}

#[rstest]
fn test_var_get_default_branch_no_remote(repo: TestRepo) {
    // Without remote, should infer from local branches
    let output = wt_var_get_cmd(&repo, &["default-branch"]).output().unwrap();
    assert!(output.status.success());
    // Should return the current branch name (main)
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "main");
}

#[rstest]
fn test_var_get_ci_status(repo: TestRepo) {
    // Without any CI configured, should return "noci"
    let output = wt_var_get_cmd(&repo, &["ci-status"]).output().unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "noci");
}

#[rstest]
fn test_var_get_ci_status_specific_branch(repo: TestRepo) {
    repo.git_command(&["branch", "feature"]).status().unwrap();

    // Without any CI configured, should return "noci"
    let output = wt_var_get_cmd(&repo, &["ci-status", "--branch", "feature"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "noci");
}

#[rstest]
fn test_var_get_ci_status_nonexistent_branch(repo: TestRepo) {
    // Should error for nonexistent branch
    let output = wt_var_get_cmd(&repo, &["ci-status", "--branch", "nonexistent"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not found") || stderr.contains("nonexistent"));
}
