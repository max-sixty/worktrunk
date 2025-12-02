use crate::common::{TestRepo, wt_command};
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
    repo.commit("Initial commit");

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
    repo.commit("Initial commit");

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

// ============================================================================
// cache clear
// ============================================================================

#[test]
fn test_config_cache_clear_all_empty() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    let output = wt_config_cache_cmd(&repo, &["clear"]).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"⚪ No caches to clear");
}

#[test]
fn test_config_cache_clear_all_with_data() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

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
    repo.commit("Initial commit");

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
    repo.commit("Initial commit");

    let output = wt_config_cache_cmd(&repo, &["clear", "default-branch"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"⚪ No default branch cache to clear");
}

#[test]
fn test_config_cache_clear_ci_empty() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    let output = wt_config_cache_cmd(&repo, &["clear", "ci"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"⚪ No CI cache entries to clear");
}

#[test]
fn test_config_cache_clear_unknown_type() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    let output = wt_config_cache_cmd(&repo, &["clear", "unknown"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @r"
    [1m[31merror:[0m invalid value '[1m[33munknown[0m' for '[1m[36m[CACHE_TYPE][0m'
      [possible values: [1m[32mci[0m, [1m[32mdefault-branch[0m]

    For more information, try '[1m[36m--help[0m'.
    ");
}
