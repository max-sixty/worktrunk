use crate::common::{TEST_EPOCH, TestRepo, repo, wt_command};
use insta::assert_snapshot;
use rstest::rstest;
use std::process::Command;

/// Create a command for `wt config state <key> <action> [args...]`
fn wt_state_cmd(repo: &TestRepo, key: &str, action: &str, args: &[&str]) -> Command {
    let mut cmd = wt_command();
    repo.clean_cli_env(&mut cmd);
    cmd.args(["config", "state", key, action]);
    cmd.args(args);
    cmd.current_dir(repo.root_path());
    cmd
}

fn wt_state_show_cmd(repo: &TestRepo) -> Command {
    let mut cmd = wt_command();
    repo.clean_cli_env(&mut cmd);
    cmd.args(["config", "state", "show"]);
    cmd.current_dir(repo.root_path());
    cmd
}

// ============================================================================
// default-branch
// ============================================================================

#[rstest]
fn test_state_get_default_branch(repo: TestRepo) {
    let output = wt_state_cmd(&repo, "default-branch", "get", &[])
        .output()
        .unwrap();
    assert!(output.status.success());
    // data() writes to stdout for piping
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "main");
}

#[rstest]
fn test_state_get_default_branch_no_remote(repo: TestRepo) {
    // Without remote, should infer from local branches
    let output = wt_state_cmd(&repo, "default-branch", "get", &[])
        .output()
        .unwrap();
    assert!(output.status.success());
    // Should return the current branch name (main)
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "main");
}

#[rstest]
fn test_state_set_default_branch(mut repo: TestRepo) {
    // First set up a remote so set_default_branch works
    repo.setup_remote("main");

    // Create and push a develop branch so we can set it as default
    repo.git_command(&["checkout", "-b", "develop"])
        .status()
        .unwrap();
    repo.git_command(&["push", "origin", "develop"])
        .status()
        .unwrap();
    repo.git_command(&["checkout", "main"]).status().unwrap();

    let output = wt_state_cmd(&repo, "default-branch", "set", &["develop"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"✅ [32mSet default branch to [1mdevelop[22m[39m");

    // Verify it was set by checking origin/HEAD
    let output = repo
        .git_command(&["symbolic-ref", "refs/remotes/origin/HEAD"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "refs/remotes/origin/develop"
    );
}

#[rstest]
fn test_state_clear_default_branch(mut repo: TestRepo) {
    // Set up remote and set default branch first
    repo.setup_remote("main");

    let output = wt_state_cmd(&repo, "default-branch", "clear", &[])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"✅ [32mCleared default branch cache[39m");

    // Verify it was cleared - origin/HEAD should not exist
    let output = repo
        .git_command(&["symbolic-ref", "refs/remotes/origin/HEAD"])
        .output()
        .unwrap();
    assert!(!output.status.success());
}

// ============================================================================
// ci-status
// ============================================================================

#[rstest]
fn test_state_get_ci_status(repo: TestRepo) {
    // Without any CI configured, should return "noci"
    let output = wt_state_cmd(&repo, "ci-status", "get", &[])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "noci");
}

#[rstest]
fn test_state_get_ci_status_specific_branch(repo: TestRepo) {
    repo.git_command(&["branch", "feature"]).status().unwrap();

    // Without any CI configured, should return "noci"
    let output = wt_state_cmd(&repo, "ci-status", "get", &["--branch", "feature"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "noci");
}

#[rstest]
fn test_state_get_ci_status_nonexistent_branch(repo: TestRepo) {
    // Should error for nonexistent branch
    let output = wt_state_cmd(&repo, "ci-status", "get", &["--branch", "nonexistent"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not found") || stderr.contains("nonexistent"));
}

#[rstest]
fn test_state_clear_ci_status_all_empty(repo: TestRepo) {
    let output = wt_state_cmd(&repo, "ci-status", "clear", &["--all"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"⚪ No CI cache entries to clear");
}

#[rstest]
fn test_state_clear_ci_status_branch(repo: TestRepo) {
    // Add CI cache entry
    repo.git_command(&[
        "config",
        "worktrunk.ci.main",
        &format!(r#"{{"status":{{"ci_status":"passed","source":"pullrequest","is_stale":false}},"checked_at":{TEST_EPOCH},"head":"abc12345"}}"#),
    ])
    .status()
    .unwrap();

    let output = wt_state_cmd(&repo, "ci-status", "clear", &[])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"✅ [32mCleared CI cache for [1mmain[22m[39m");
}

#[rstest]
fn test_state_clear_ci_status_branch_not_cached(repo: TestRepo) {
    let output = wt_state_cmd(&repo, "ci-status", "clear", &[])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"⚪ No CI cache for [1mmain[22m");
}

// ============================================================================
// marker
// ============================================================================

#[rstest]
fn test_state_get_marker(repo: TestRepo) {
    // Set a marker first
    repo.git_command(&["config", "worktrunk.marker.main", "🚧"])
        .status()
        .unwrap();

    let output = wt_state_cmd(&repo, "marker", "get", &[]).output().unwrap();
    assert!(output.status.success());
    // data() writes to stdout for piping
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "🚧");
}

#[rstest]
fn test_state_get_marker_empty(repo: TestRepo) {
    let output = wt_state_cmd(&repo, "marker", "get", &[]).output().unwrap();
    assert!(output.status.success());
    // Empty output when no marker is set
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "");
}

#[rstest]
fn test_state_get_marker_specific_branch(repo: TestRepo) {
    repo.git_command(&["branch", "feature"]).status().unwrap();

    // Set a marker for feature branch
    repo.git_command(&["config", "worktrunk.marker.feature", "🔧"])
        .status()
        .unwrap();

    let output = wt_state_cmd(&repo, "marker", "get", &["--branch", "feature"])
        .output()
        .unwrap();
    assert!(output.status.success());
    // data() writes to stdout for piping
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "🔧");
}

#[rstest]
fn test_state_set_marker_branch_default(repo: TestRepo) {
    let output = wt_state_cmd(&repo, "marker", "set", &["🚧"])
        .output()
        .unwrap();
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
fn test_state_set_marker_branch_specific(repo: TestRepo) {
    repo.git_command(&["branch", "feature"]).status().unwrap();

    let output = wt_state_cmd(&repo, "marker", "set", &["🔧", "--branch", "feature"])
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
fn test_state_clear_marker_branch_default(repo: TestRepo) {
    // Set a marker first
    repo.git_command(&["config", "worktrunk.marker.main", "🚧"])
        .status()
        .unwrap();

    let output = wt_state_cmd(&repo, "marker", "clear", &[])
        .output()
        .unwrap();
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
fn test_state_clear_marker_branch_specific(repo: TestRepo) {
    // Set a marker first
    repo.git_command(&["config", "worktrunk.marker.feature", "🔧"])
        .status()
        .unwrap();

    let output = wt_state_cmd(&repo, "marker", "clear", &["--branch", "feature"])
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
fn test_state_clear_marker_all(repo: TestRepo) {
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

    let output = wt_state_cmd(&repo, "marker", "clear", &["--all"])
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
fn test_state_clear_marker_all_empty(repo: TestRepo) {
    let output = wt_state_cmd(&repo, "marker", "clear", &["--all"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"⚪ No markers to clear");
}

// ============================================================================
// logs
// ============================================================================

#[rstest]
fn test_state_get_logs_empty(repo: TestRepo) {
    let output = wt_state_cmd(&repo, "logs", "get", &[]).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"⚪ No logs");
}

#[rstest]
fn test_state_clear_logs_empty(repo: TestRepo) {
    let output = wt_state_cmd(&repo, "logs", "clear", &[]).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"⚪ No logs to clear");
}

#[rstest]
fn test_state_clear_logs_with_files(repo: TestRepo) {
    // Create wt-logs directory with some log files
    let git_dir = repo.root_path().join(".git");
    let log_dir = git_dir.join("wt-logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    std::fs::write(log_dir.join("feature-post-start-npm.log"), "npm output").unwrap();
    std::fs::write(log_dir.join("bugfix-remove.log"), "remove output").unwrap();

    let output = wt_state_cmd(&repo, "logs", "clear", &[]).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"✅ [32mCleared [1m2[22m log files[39m");

    // Verify logs are gone
    assert!(!log_dir.exists());
}

#[rstest]
fn test_state_clear_logs_single_file(repo: TestRepo) {
    // Create wt-logs directory with one log file
    let git_dir = repo.root_path().join(".git");
    let log_dir = git_dir.join("wt-logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    std::fs::write(log_dir.join("feature-remove.log"), "remove output").unwrap();

    let output = wt_state_cmd(&repo, "logs", "clear", &[]).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"✅ [32mCleared [1m1[22m log file[39m");
}

// ============================================================================
// state show
// ============================================================================

#[rstest]
fn test_state_show_empty(repo: TestRepo) {
    let output = wt_state_show_cmd(&repo).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @r"
    ⚪ Default branch:
    [107m [0m  main

    ⚪ CI status cache:
    [107m [0m  (empty)
    ");
}

#[rstest]
fn test_state_show_with_ci_entries(repo: TestRepo) {
    // Add CI cache entries - use TEST_EPOCH for deterministic age=0s in snapshots
    repo.git_command(&[
        "config",
        "worktrunk.ci.feature",
        &format!(r#"{{"status":{{"ci_status":"passed","source":"pullrequest","is_stale":false}},"checked_at":{TEST_EPOCH},"head":"abc12345def67890"}}"#),
    ])
    .status()
    .unwrap();

    repo.git_command(&[
        "config",
        "worktrunk.ci.bugfix",
        &format!(r#"{{"status":{{"ci_status":"failed","source":"branch","is_stale":true}},"checked_at":{TEST_EPOCH},"head":"111222333444555"}}"#),
    ])
    .status()
    .unwrap();

    repo.git_command(&[
        "config",
        "worktrunk.ci.main",
        &format!(r#"{{"status":null,"checked_at":{TEST_EPOCH},"head":"deadbeef12345678"}}"#),
    ])
    .status()
    .unwrap();

    let output = wt_state_show_cmd(&repo).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr));
}
