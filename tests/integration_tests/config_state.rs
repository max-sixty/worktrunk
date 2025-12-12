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

fn wt_state_show_json_cmd(repo: &TestRepo) -> Command {
    let mut cmd = wt_command();
    repo.clean_cli_env(&mut cmd);
    cmd.args(["config", "state", "show", "--format=json"]);
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

    // Verify it was set (use wt command to parse JSON storage)
    let output = wt_state_cmd(&repo, "marker", "get", &[]).output().unwrap();
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

    // Verify it was set (use wt command to parse JSON storage)
    let output = wt_state_cmd(&repo, "marker", "get", &["--branch", "feature"])
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
    [36mDEFAULT BRANCH[39m
    [107m [0m  main

    [36mSWITCH HISTORY[39m
    [107m [0m  (none)

    [36mBRANCH MARKERS[39m
    [107m [0m  (none)

    [36mCI STATUS CACHE[39m
    [107m [0m  (none)

    [36mLOG FILES[39m  @ .git/wt-logs
    [107m [0m  (none)
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

#[rstest]
fn test_state_show_comprehensive(repo: TestRepo) {
    // Set up switch history
    repo.git_command(&["config", "worktrunk.history", "feature"])
        .status()
        .unwrap();

    // Set up branch markers (JSON format with timestamps for deterministic age)
    repo.git_command(&[
        "config",
        "worktrunk.marker.feature",
        &format!(r#"{{"marker":"🚧 WIP","set_at":{TEST_EPOCH}}}"#),
    ])
    .status()
    .unwrap();
    repo.git_command(&[
        "config",
        "worktrunk.marker.bugfix",
        &format!(r#"{{"marker":"🐛 debugging","set_at":{TEST_EPOCH}}}"#),
    ])
    .status()
    .unwrap();

    // Set up CI cache
    repo.git_command(&[
        "config",
        "worktrunk.ci.feature",
        &format!(r#"{{"status":{{"ci_status":"passed","source":"pullrequest","is_stale":false}},"checked_at":{TEST_EPOCH},"head":"abc12345def67890"}}"#),
    ])
    .status()
    .unwrap();

    // Create log files
    let git_dir = repo.root_path().join(".git");
    let log_dir = git_dir.join("wt-logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    std::fs::write(log_dir.join("feature-post-start-npm.log"), "npm output").unwrap();
    std::fs::write(log_dir.join("bugfix-remove.log"), "remove output").unwrap();

    let output = wt_state_show_cmd(&repo).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr));
}

#[rstest]
fn test_state_show_json_empty(repo: TestRepo) {
    let output = wt_state_show_json_cmd(&repo).output().unwrap();
    assert!(output.status.success());
    // JSON output goes to stdout
    let json_str = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    assert_eq!(json["default_branch"], "main");
    assert_eq!(json["switch_previous"], serde_json::Value::Null);
    assert_eq!(json["markers"], serde_json::json!([]));
    assert_eq!(json["ci_status"], serde_json::json!([]));
    assert_eq!(json["logs"], serde_json::json!([]));
}

#[rstest]
fn test_state_show_json_comprehensive(repo: TestRepo) {
    // Set up switch history
    repo.git_command(&["config", "worktrunk.history", "feature"])
        .status()
        .unwrap();

    // Set up branch markers (JSON format with timestamps)
    repo.git_command(&[
        "config",
        "worktrunk.marker.feature",
        &format!(r#"{{"marker":"🚧 WIP","set_at":{TEST_EPOCH}}}"#),
    ])
    .status()
    .unwrap();

    // Set up CI cache
    repo.git_command(&[
        "config",
        "worktrunk.ci.feature",
        &format!(r#"{{"status":{{"ci_status":"passed","source":"pullrequest","is_stale":false}},"checked_at":{TEST_EPOCH},"head":"abc12345def67890"}}"#),
    ])
    .status()
    .unwrap();

    let output = wt_state_show_json_cmd(&repo).output().unwrap();
    assert!(output.status.success());
    // JSON output goes to stdout
    let json_str = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    assert_eq!(json["default_branch"], "main");
    assert_eq!(json["switch_previous"], "feature");

    // Check markers
    let markers = json["markers"].as_array().unwrap();
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0]["branch"], "feature");
    assert_eq!(markers[0]["marker"], "🚧 WIP");
    assert_eq!(markers[0]["set_at"], TEST_EPOCH);

    // Check CI status
    let ci_status = json["ci_status"].as_array().unwrap();
    assert_eq!(ci_status.len(), 1);
    assert_eq!(ci_status[0]["branch"], "feature");
    assert_eq!(ci_status[0]["status"], "passed");
    assert_eq!(ci_status[0]["checked_at"], TEST_EPOCH);
    assert_eq!(ci_status[0]["head"], "abc12345def67890");
}
