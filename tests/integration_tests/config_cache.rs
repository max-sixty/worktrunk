use crate::common::{TEST_EPOCH, TestRepo, wt_command};
use insta::assert_snapshot;
use std::process::Command;

fn wt_config_cache_cmd(repo: &TestRepo, args: &[&str]) -> Command {
    let mut cmd = wt_command();
    repo.clean_cli_env(&mut cmd);
    cmd.args(["config", "cache"]);
    cmd.args(args);
    cmd.current_dir(repo.root_path());
    cmd
}

// ============================================================================
// cache show
// ============================================================================

#[test]
fn test_config_cache_show_empty() {
    let repo = TestRepo::new();

    let output = wt_config_cache_cmd(&repo, &["show"]).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @r"
    ⚪ Default branch cache:
    [107m [0m  main

    ⚪ CI status cache:
    [107m [0m  (empty)
    ");
}

#[test]
fn test_config_cache_show_with_default_branch() {
    let repo = TestRepo::new();

    // Set default branch cache manually
    repo.git_command(&["config", "worktrunk.defaultBranch", "main"])
        .status()
        .unwrap();

    let output = wt_config_cache_cmd(&repo, &["show"]).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @r"
    ⚪ Default branch cache:
    [107m [0m  main

    ⚪ CI status cache:
    [107m [0m  (empty)
    ");
}

#[test]
fn test_config_cache_show_with_ci_entries() {
    let repo = TestRepo::new();

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

    let output = wt_config_cache_cmd(&repo, &["show"]).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr));
}

// ============================================================================
// cache clear
// ============================================================================

#[test]
fn test_config_cache_clear_all_empty() {
    let repo = TestRepo::new();

    let output = wt_config_cache_cmd(&repo, &["clear"]).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"⚪ No caches to clear");
}

#[test]
fn test_config_cache_clear_all_with_data() {
    let repo = TestRepo::new();

    // Set default branch cache
    repo.git_command(&["config", "worktrunk.defaultBranch", "main"])
        .status()
        .unwrap();

    let output = wt_config_cache_cmd(&repo, &["clear"]).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"✅ [32mCleared all caches[39m");
}

#[test]
fn test_config_cache_clear_default_branch() {
    let repo = TestRepo::new();

    // Set default branch cache
    repo.git_command(&["config", "worktrunk.defaultBranch", "main"])
        .status()
        .unwrap();

    let output = wt_config_cache_cmd(&repo, &["clear", "default-branch"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"✅ [32mCleared default branch cache[39m");
}

#[test]
fn test_config_cache_clear_default_branch_empty() {
    let repo = TestRepo::new();

    let output = wt_config_cache_cmd(&repo, &["clear", "default-branch"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"⚪ No default branch cache to clear");
}

#[test]
fn test_config_cache_clear_ci_empty() {
    let repo = TestRepo::new();

    let output = wt_config_cache_cmd(&repo, &["clear", "ci"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"⚪ No CI cache entries to clear");
}

#[test]
fn test_config_cache_clear_unknown_type() {
    let repo = TestRepo::new();

    let output = wt_config_cache_cmd(&repo, &["clear", "unknown"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @r"
    [1m[31merror:[0m invalid value '[1m[33munknown[0m' for '[1m[36m[CACHE_TYPE][0m'
      [possible values: [1m[32mci[0m, [1m[32mdefault-branch[0m, [1m[32mlogs[0m]

    For more information, try '[1m[36m--help[0m'.
    ");
}

#[test]
fn test_config_cache_clear_logs_empty() {
    let repo = TestRepo::new();

    let output = wt_config_cache_cmd(&repo, &["clear", "logs"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"⚪ No logs to clear");
}

#[test]
fn test_config_cache_clear_logs_with_files() {
    let repo = TestRepo::new();

    // Create wt-logs directory with some log files
    let git_dir = repo.root_path().join(".git");
    let log_dir = git_dir.join("wt-logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    std::fs::write(log_dir.join("feature-post-start-npm.log"), "npm output").unwrap();
    std::fs::write(log_dir.join("bugfix-remove.log"), "remove output").unwrap();

    let output = wt_config_cache_cmd(&repo, &["clear", "logs"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"✅ [32mCleared [1m2[22m log files[39m");

    // Verify logs are gone
    assert!(!log_dir.exists());
}

#[test]
fn test_config_cache_clear_logs_single_file() {
    let repo = TestRepo::new();

    // Create wt-logs directory with one log file
    let git_dir = repo.root_path().join(".git");
    let log_dir = git_dir.join("wt-logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    std::fs::write(log_dir.join("feature-remove.log"), "remove output").unwrap();

    let output = wt_config_cache_cmd(&repo, &["clear", "logs"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"✅ [32mCleared [1m1[22m log file[39m");
}

#[test]
fn test_config_cache_clear_all_with_logs() {
    let repo = TestRepo::new();

    // Create wt-logs directory with a log file
    let git_dir = repo.root_path().join(".git");
    let log_dir = git_dir.join("wt-logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    std::fs::write(log_dir.join("feature-remove.log"), "remove output").unwrap();

    let output = wt_config_cache_cmd(&repo, &["clear"]).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"✅ [32mCleared all caches[39m");

    // Verify logs are gone
    assert!(!log_dir.exists());
}
