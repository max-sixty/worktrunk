use crate::common::{TEST_EPOCH, TestRepo, repo, wt_command};
use insta::assert_snapshot;
use regex::Regex;
use rstest::rstest;
use std::process::Command;

/// Normalize LOG FILES path in output for stable snapshots (temp dir varies per test)
fn normalize_log_path(output: &str) -> String {
    // Match "LOG FILES" followed by ANSI codes, spaces, and "@ " then the path
    // Pattern: LOG FILES + optional ANSI reset + spaces + @ + path until newline
    let re = Regex::new(r"(LOG FILES\x1b\[39m\s+@ )[^\n]+").unwrap();
    re.replace_all(output, "${1}<PATH>").to_string()
}

/// Write CI status to the file-based cache at .git/wt-cache/ci-status/<branch>.json
fn write_ci_cache(repo: &TestRepo, branch: &str, json: &str) {
    let git_dir = repo.root_path().join(".git");
    let cache_dir = git_dir.join("wt-cache").join("ci-status");
    std::fs::create_dir_all(&cache_dir).unwrap();

    // Sanitize branch name for filename
    let safe_branch: String = branch
        .chars()
        .map(|c| if c == '/' || c == '\\' { '-' } else { c })
        .collect();
    let cache_file = cache_dir.join(format!("{safe_branch}.json"));
    std::fs::write(&cache_file, json).unwrap();
}

/// Create a command for `wt config state <key> <action> [args...]`
fn wt_state_cmd(repo: &TestRepo, key: &str, action: &str, args: &[&str]) -> Command {
    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.args(["config", "state", key, action]);
    cmd.args(args);
    cmd.current_dir(repo.root_path());
    cmd
}

fn wt_state_get_cmd(repo: &TestRepo) -> Command {
    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.args(["config", "state", "get"]);
    cmd.current_dir(repo.root_path());
    cmd
}

fn wt_state_get_json_cmd(repo: &TestRepo) -> Command {
    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.args(["config", "state", "get", "--format=json"]);
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
fn test_state_get_default_branch_fails_when_undetermined(repo: TestRepo) {
    // Remove origin (fixture has it) - otherwise remote can determine default branch
    repo.run_git(&["remote", "remove", "origin"]);

    // Rename main to something non-standard so default branch can't be determined
    repo.git_command()
        .args(["branch", "-m", "main", "xyz"])
        .status()
        .unwrap();
    repo.git_command().args(["branch", "abc"]).status().unwrap();
    repo.git_command().args(["branch", "def"]).status().unwrap();

    // Now we have: xyz, abc, def - no common names, no init.defaultBranch
    // wt config state default-branch get should fail with an error
    let output = wt_state_cmd(&repo, "default-branch", "get", &[])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Cannot determine default branch"),
        "Expected error message about cannot determine default branch, got: {}",
        stderr
    );
}

#[rstest]
fn test_state_set_default_branch(repo: TestRepo) {
    let output = wt_state_cmd(&repo, "default-branch", "set", &["develop"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"
    [33m▲[39m [33mBranch [1mdevelop[22m does not exist locally[39m
    [32m✓[39m [32mSet default branch to [1mdevelop[22m[39m
    ");

    // Verify it was set in worktrunk's cache
    let output = repo
        .git_command()
        .args(["config", "--get", "worktrunk.default-branch"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "develop");
}

#[rstest]
fn test_state_clear_default_branch(mut repo: TestRepo) {
    // Set up remote and populate worktrunk's cache
    repo.setup_remote("main");
    // Trigger cache population by reading default branch
    let _ = wt_state_cmd(&repo, "default-branch", "get", &[])
        .output()
        .unwrap();

    // Now clear the cache
    let output = wt_state_cmd(&repo, "default-branch", "clear", &[])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[32m✓[39m [32mCleared default branch cache[39m");

    // Verify worktrunk's cache was cleared
    let output = repo
        .git_command()
        .args(["config", "--get", "worktrunk.default-branch"])
        .output()
        .unwrap();
    assert!(!output.status.success());
}

#[rstest]
fn test_state_clear_default_branch_empty(repo: TestRepo) {
    // Fixture already has origin remote, no default branch cache set
    let output = wt_state_cmd(&repo, "default-branch", "clear", &[])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[2m○[22m No default branch cache to clear");
}

// ============================================================================
// previous-branch
// ============================================================================

#[rstest]
fn test_state_get_previous_branch(repo: TestRepo) {
    // Without any previous branch set, should return empty
    let output = wt_state_cmd(&repo, "previous-branch", "get", &[])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "");
}

#[rstest]
fn test_state_set_previous_branch(repo: TestRepo) {
    let output = wt_state_cmd(&repo, "previous-branch", "set", &["feature"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[32m✓[39m [32mSet previous branch to [1mfeature[22m[39m");

    // Verify it was set
    let output = wt_state_cmd(&repo, "previous-branch", "get", &[])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "feature");
}

#[rstest]
fn test_state_clear_previous_branch(repo: TestRepo) {
    // Set a previous branch first
    wt_state_cmd(&repo, "previous-branch", "set", &["feature"])
        .output()
        .unwrap();

    let output = wt_state_cmd(&repo, "previous-branch", "clear", &[])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[32m✓[39m [32mCleared previous branch[39m");

    // Verify it was cleared
    let output = wt_state_cmd(&repo, "previous-branch", "get", &[])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "");
}

#[rstest]
fn test_state_clear_previous_branch_empty(repo: TestRepo) {
    // Clear without any previous branch set
    let output = wt_state_cmd(&repo, "previous-branch", "clear", &[])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[2m○[22m No previous branch to clear");
}

// ============================================================================
// ci-status
// ============================================================================

#[rstest]
fn test_state_get_ci_status(repo: TestRepo) {
    // Without any CI configured, should return "no-ci"
    let output = wt_state_cmd(&repo, "ci-status", "get", &[])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "no-ci");
}

#[rstest]
fn test_state_get_ci_status_specific_branch(repo: TestRepo) {
    repo.git_command()
        .args(["branch", "feature"])
        .status()
        .unwrap();

    // Without any CI configured, should return "no-ci"
    let output = wt_state_cmd(&repo, "ci-status", "get", &["--branch", "feature"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "no-ci");
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
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[2m○[22m No CI cache entries to clear");
}

#[rstest]
fn test_state_clear_ci_status_branch(repo: TestRepo) {
    // Add CI cache entry
    repo.git_command().args([
        "config",
        "worktrunk.state.main.ci-status",
        &format!(r#"{{"status":{{"ci_status":"passed","source":"pull-request","is_stale":false}},"checked_at":{TEST_EPOCH},"head":"abc12345"}}"#),
    ])
    .status()
    .unwrap();

    let output = wt_state_cmd(&repo, "ci-status", "clear", &[])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[32m✓[39m [32mCleared CI cache for [1mmain[22m[39m");
}

#[rstest]
fn test_state_clear_ci_status_branch_not_cached(repo: TestRepo) {
    let output = wt_state_cmd(&repo, "ci-status", "clear", &[])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[2m○[22m No CI cache for [1mmain[22m");
}

// ============================================================================
// marker
// ============================================================================

#[rstest]
fn test_state_get_marker(repo: TestRepo) {
    // Set a marker first (using JSON format)
    repo.set_marker("main", "🚧");

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
    repo.git_command()
        .args(["branch", "feature"])
        .status()
        .unwrap();

    // Set a marker for feature branch (using JSON format)
    repo.set_marker("feature", "🔧");

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
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[32m✓[39m [32mSet marker for [1mmain[22m to [1m🚧[22m[39m");

    // Verify it was set (use wt command to parse JSON storage)
    let output = wt_state_cmd(&repo, "marker", "get", &[]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "🚧");
}

#[rstest]
fn test_state_set_marker_branch_specific(repo: TestRepo) {
    repo.git_command()
        .args(["branch", "feature"])
        .status()
        .unwrap();

    let output = wt_state_cmd(&repo, "marker", "set", &["🔧", "--branch", "feature"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[32m✓[39m [32mSet marker for [1mfeature[22m to [1m🔧[22m[39m");

    // Verify it was set (use wt command to parse JSON storage)
    let output = wt_state_cmd(&repo, "marker", "get", &["--branch", "feature"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "🔧");
}

#[rstest]
fn test_state_clear_marker_branch_default(repo: TestRepo) {
    // Set a marker first (using JSON format)
    repo.set_marker("main", "🚧");

    let output = wt_state_cmd(&repo, "marker", "clear", &[])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[32m✓[39m [32mCleared marker for [1mmain[22m[39m");

    // Verify it was unset
    let output = repo
        .git_command()
        .args(["config", "--get", "worktrunk.state.main.marker"])
        .output()
        .unwrap();
    assert!(!output.status.success());
}

#[rstest]
fn test_state_clear_marker_branch_specific(repo: TestRepo) {
    // Set a marker first (using JSON format)
    repo.set_marker("feature", "🔧");

    let output = wt_state_cmd(&repo, "marker", "clear", &["--branch", "feature"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[32m✓[39m [32mCleared marker for [1mfeature[22m[39m");

    // Verify it was unset
    let output = repo
        .git_command()
        .args(["config", "--get", "worktrunk.state.feature.marker"])
        .output()
        .unwrap();
    assert!(!output.status.success());
}

#[rstest]
fn test_state_clear_marker_all(repo: TestRepo) {
    // Set multiple markers (using JSON format)
    repo.set_marker("main", "🚧");
    repo.set_marker("feature", "🔧");
    repo.set_marker("bugfix", "🐛");

    let output = wt_state_cmd(&repo, "marker", "clear", &["--all"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[32m✓[39m [32mCleared [1m3[22m markers[39m");

    // Verify all were unset
    let output = repo
        .git_command()
        .args(["config", "--get-regexp", r"^worktrunk\.state\..+\.marker$"])
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
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[2m○[22m No markers to clear");
}

// ============================================================================
// logs
// ============================================================================

#[rstest]
fn test_state_get_logs_empty(repo: TestRepo) {
    let output = wt_state_cmd(&repo, "logs", "get", &[]).output().unwrap();
    assert!(output.status.success());
    // Path is dynamic (temp dir), so check pattern instead of exact snapshot
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("LOG FILES"),
        "Expected LOG FILES heading: {stderr}"
    );
    // Path separator varies by platform, so check for either / or \
    assert!(
        stderr.contains(".git/wt-logs") || stderr.contains(".git\\wt-logs"),
        "Expected .git/wt-logs or .git\\wt-logs in output: {stderr}"
    );
}

#[rstest]
fn test_state_get_logs_with_files(repo: TestRepo) {
    // Create wt-logs directory with some log files
    let git_dir = repo.root_path().join(".git");
    let log_dir = git_dir.join("wt-logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    std::fs::write(
        log_dir.join("feature-post-start-npm.log"),
        "npm output here",
    )
    .unwrap();
    std::fs::write(log_dir.join("bugfix-remove.log"), "remove output").unwrap();

    let output = wt_state_cmd(&repo, "logs", "get", &[]).output().unwrap();
    assert!(output.status.success());
    // Verify we get a table with file info
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("LOG FILES"),
        "Expected LOG FILES heading: {stderr}"
    );
    // Path separator varies by platform, so check for either / or \
    assert!(
        stderr.contains(".git/wt-logs") || stderr.contains(".git\\wt-logs"),
        "Expected .git/wt-logs or .git\\wt-logs in output: {stderr}"
    );
    assert!(stderr.contains("File"));
    assert!(stderr.contains("Size"));
    assert!(stderr.contains("Age"));
    assert!(stderr.contains("feature-post-start-npm.log"));
    assert!(stderr.contains("bugfix-remove.log"));
}

#[rstest]
fn test_state_get_logs_dir_exists_no_log_files(repo: TestRepo) {
    // Create wt-logs directory with non-.log files (empty of actual log files)
    let git_dir = repo.root_path().join(".git");
    let log_dir = git_dir.join("wt-logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    std::fs::write(log_dir.join("README.txt"), "not a log file").unwrap();
    std::fs::write(log_dir.join(".gitkeep"), "").unwrap();

    let output = wt_state_cmd(&repo, "logs", "get", &[]).output().unwrap();
    assert!(output.status.success());
    // Should show "(none)" since no .log files exist
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("LOG FILES"),
        "Expected LOG FILES heading: {stderr}"
    );
    assert!(
        stderr.contains("(none)"),
        "Expected (none) when log dir exists but has no .log files: {stderr}"
    );
}

#[rstest]
fn test_state_clear_logs_empty(repo: TestRepo) {
    let output = wt_state_cmd(&repo, "logs", "clear", &[]).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[2m○[22m No logs to clear");
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
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[32m✓[39m [32mCleared [1m2[22m log files[39m");

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
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[32m✓[39m [32mCleared [1m1[22m log file[39m");
}

// ============================================================================
// state clear (all)
// ============================================================================

fn wt_state_clear_all_cmd(repo: &TestRepo) -> std::process::Command {
    let mut cmd = wt_command();
    cmd.current_dir(repo.root_path());
    cmd.env("CLICOLOR_FORCE", "1");
    cmd.args(["config", "state", "clear"]);
    cmd
}

#[rstest]
fn test_state_clear_all_empty(repo: TestRepo) {
    // Clear when no state exists
    let output = wt_state_clear_all_cmd(&repo).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[2m○[22m No stored state to clear");
}

#[rstest]
fn test_state_clear_all_comprehensive(repo: TestRepo) {
    // Set up various state
    // Previous branch
    repo.git_command()
        .args(["config", "worktrunk.history", "feature"])
        .status()
        .unwrap();

    // Marker (using JSON format)
    repo.set_marker("main", "🚧");

    // CI cache (file-based)
    write_ci_cache(
        &repo,
        "feature",
        r#"{"checked_at":1704067200,"head":"abc123"}"#,
    );

    // Logs
    let git_dir = repo.root_path().join(".git");
    let log_dir = git_dir.join("wt-logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    std::fs::write(log_dir.join("feature-remove.log"), "output").unwrap();

    let output = wt_state_clear_all_cmd(&repo).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[32m✓[39m [32mCleared all stored state[39m");

    // Verify everything was cleared
    assert!(
        repo.git_command()
            .args(["config", "--get", "worktrunk.history"])
            .output()
            .unwrap()
            .status
            .code()
            == Some(1)
    ); // Not found
    assert!(
        repo.git_command()
            .args(["config", "--get", "worktrunk.state.main.marker"])
            .output()
            .unwrap()
            .status
            .code()
            == Some(1)
    );
    // CI cache is now file-based, verify the cache file is cleared
    let ci_cache_dir = git_dir.join("wt-cache").join("ci-status");
    assert!(
        !ci_cache_dir.join("feature.json").exists(),
        "CI cache file should be cleared"
    );
    assert!(!log_dir.exists());
}

#[rstest]
fn test_state_clear_all_nothing_to_clear(repo: TestRepo) {
    // First clear to ensure nothing exists
    wt_state_clear_all_cmd(&repo).output().unwrap();

    // Clear again when nothing exists
    let output = wt_state_clear_all_cmd(&repo).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[2m○[22m No stored state to clear");
}

// ============================================================================
// state get
// ============================================================================

#[rstest]
fn test_state_get_empty(repo: TestRepo) {
    let output = wt_state_get_cmd(&repo).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(normalize_log_path(&String::from_utf8_lossy(&output.stderr)), @"
    [36mDEFAULT BRANCH[39m
    [107m [0m main

    [36mPREVIOUS BRANCH[39m
    [107m [0m (none)

    [36mBRANCH MARKERS[39m
    [107m [0m (none)

    [36mCI STATUS CACHE[39m
    [107m [0m (none)

    [36mHINTS[39m
    [107m [0m (none)

    [36mLOG FILES[39m  @ <PATH>
    [107m [0m (none)
    ");
}

#[rstest]
fn test_state_get_with_ci_entries(repo: TestRepo) {
    // Add CI cache entries - use TEST_EPOCH for deterministic age=0s in snapshots
    write_ci_cache(
        &repo,
        "feature",
        &format!(
            r#"{{"status":{{"ci_status":"passed","source":"pull-request","is_stale":false}},"checked_at":{TEST_EPOCH},"head":"abc12345def67890"}}"#
        ),
    );

    write_ci_cache(
        &repo,
        "bugfix",
        &format!(
            r#"{{"status":{{"ci_status":"failed","source":"branch","is_stale":true}},"checked_at":{TEST_EPOCH},"head":"111222333444555"}}"#
        ),
    );

    write_ci_cache(
        &repo,
        "main",
        &format!(r#"{{"status":null,"checked_at":{TEST_EPOCH},"head":"deadbeef12345678"}}"#),
    );

    let output = wt_state_get_cmd(&repo).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(normalize_log_path(&String::from_utf8_lossy(&output.stderr)));
}

#[rstest]
fn test_state_get_comprehensive(repo: TestRepo) {
    // Set up previous branch
    repo.git_command()
        .args(["config", "worktrunk.history", "feature"])
        .status()
        .unwrap();

    // Set up branch markers (JSON format with timestamps for deterministic age)
    repo.git_command()
        .args([
            "config",
            "worktrunk.state.feature.marker",
            &format!(r#"{{"marker":"🚧 WIP","set_at":{TEST_EPOCH}}}"#),
        ])
        .status()
        .unwrap();
    repo.git_command()
        .args([
            "config",
            "worktrunk.state.bugfix.marker",
            &format!(r#"{{"marker":"🐛 debugging","set_at":{TEST_EPOCH}}}"#),
        ])
        .status()
        .unwrap();

    // Set up CI cache (file-based)
    write_ci_cache(
        &repo,
        "feature",
        &format!(
            r#"{{"status":{{"ci_status":"passed","source":"pull-request","is_stale":false}},"checked_at":{TEST_EPOCH},"head":"abc12345def67890"}}"#
        ),
    );

    // Create log files
    let git_dir = repo.root_path().join(".git");
    let log_dir = git_dir.join("wt-logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    std::fs::write(log_dir.join("feature-post-start-npm.log"), "npm output").unwrap();
    std::fs::write(log_dir.join("bugfix-remove.log"), "remove output").unwrap();

    let output = wt_state_get_cmd(&repo).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(normalize_log_path(&String::from_utf8_lossy(&output.stderr)));
}

#[rstest]
fn test_state_get_json_empty(repo: TestRepo) {
    let output = wt_state_get_json_cmd(&repo).output().unwrap();
    assert!(output.status.success());
    // JSON output goes to stdout
    let json_str = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    assert_eq!(json["default_branch"], "main");
    assert_eq!(json["previous_branch"], serde_json::Value::Null);
    assert_eq!(json["markers"], serde_json::json!([]));
    assert_eq!(json["ci_status"], serde_json::json!([]));
    assert_eq!(json["hints"], serde_json::json!([]));
    assert_eq!(json["logs"], serde_json::json!([]));
}

#[rstest]
fn test_state_get_json_comprehensive(repo: TestRepo) {
    // Set up previous branch
    repo.git_command()
        .args(["config", "worktrunk.history", "feature"])
        .status()
        .unwrap();

    // Set up branch markers (JSON format with timestamps)
    repo.git_command()
        .args([
            "config",
            "worktrunk.state.feature.marker",
            &format!(r#"{{"marker":"🚧 WIP","set_at":{TEST_EPOCH}}}"#),
        ])
        .status()
        .unwrap();

    // Set up CI cache (file-based)
    write_ci_cache(
        &repo,
        "feature",
        &format!(
            r#"{{"status":{{"ci_status":"passed","source":"pull-request","is_stale":false}},"checked_at":{TEST_EPOCH},"head":"abc12345def67890"}}"#
        ),
    );

    let output = wt_state_get_json_cmd(&repo).output().unwrap();
    assert!(output.status.success());
    // JSON output goes to stdout
    let json_str = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    assert_eq!(json["default_branch"], "main");
    assert_eq!(json["previous_branch"], "feature");

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

#[rstest]
fn test_state_get_json_with_logs(repo: TestRepo) {
    // Create log files
    let git_dir = repo.root_path().join(".git");
    let log_dir = git_dir.join("wt-logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    std::fs::write(log_dir.join("feature-post-start-npm.log"), "npm output").unwrap();
    std::fs::write(log_dir.join("bugfix-remove.log"), "remove log output").unwrap();

    let output = wt_state_get_json_cmd(&repo).output().unwrap();
    assert!(output.status.success());
    // JSON output goes to stdout
    let json_str = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    // Check logs - they should be present and have expected fields
    let logs = json["logs"].as_array().unwrap();
    assert_eq!(logs.len(), 2);

    // Logs are sorted by modified time (newest first), but in test both are same time
    // Just check that both log files are present with expected fields
    let log_files: Vec<&str> = logs.iter().map(|l| l["file"].as_str().unwrap()).collect();
    assert!(log_files.contains(&"feature-post-start-npm.log"));
    assert!(log_files.contains(&"bugfix-remove.log"));

    // Each log entry should have file, size, and modified_at
    for log in logs {
        assert!(log.get("file").is_some());
        assert!(log.get("size").is_some());
        assert!(log.get("modified_at").is_some());
    }
}

// ============================================================================
// Additional coverage tests for uncovered user messages
// ============================================================================

#[rstest]
fn test_state_clear_ci_status_all_with_entries(repo: TestRepo) {
    // Add file-based CI cache entries (the format used by CachedCiStatus::clear_all)
    write_ci_cache(
        &repo,
        "feature",
        &format!(
            r#"{{"status":{{"ci_status":"passed","source":"pull-request","is_stale":false}},"checked_at":{TEST_EPOCH},"head":"abc12345"}}"#
        ),
    );
    write_ci_cache(
        &repo,
        "bugfix",
        &format!(
            r#"{{"status":{{"ci_status":"failed","source":"branch","is_stale":false}},"checked_at":{TEST_EPOCH},"head":"def67890"}}"#
        ),
    );

    let output = wt_state_cmd(&repo, "ci-status", "clear", &["--all"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[32m✓[39m [32mCleared [1m2[22m CI cache entries[39m");
}

#[rstest]
fn test_state_clear_ci_status_all_single_entry(repo: TestRepo) {
    // Test singular form "entry" vs "entries"
    write_ci_cache(
        &repo,
        "feature",
        &format!(
            r#"{{"status":{{"ci_status":"passed","source":"pull-request","is_stale":false}},"checked_at":{TEST_EPOCH},"head":"abc12345"}}"#
        ),
    );

    let output = wt_state_cmd(&repo, "ci-status", "clear", &["--all"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[32m✓[39m [32mCleared [1m1[22m CI cache entry[39m");
}

#[rstest]
fn test_state_clear_ci_status_specific_branch(repo: TestRepo) {
    // Create a feature branch
    repo.git_command()
        .args(["branch", "feature"])
        .status()
        .unwrap();

    // Add CI cache via git config for the specific branch
    repo.git_command().args([
        "config",
        "worktrunk.state.feature.ci-status",
        &format!(r#"{{"status":{{"ci_status":"passed","source":"pull-request","is_stale":false}},"checked_at":{TEST_EPOCH},"head":"abc12345"}}"#),
    ])
    .status()
    .unwrap();

    let output = wt_state_cmd(&repo, "ci-status", "clear", &["--branch", "feature"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[32m✓[39m [32mCleared CI cache for [1mfeature[22m[39m");
}

#[rstest]
fn test_state_clear_ci_status_specific_branch_not_cached(repo: TestRepo) {
    // Create a feature branch without any CI cache
    repo.git_command()
        .args(["branch", "feature"])
        .status()
        .unwrap();

    let output = wt_state_cmd(&repo, "ci-status", "clear", &["--branch", "feature"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[2m○[22m No CI cache for [1mfeature[22m");
}

#[rstest]
fn test_state_clear_marker_specific_branch(repo: TestRepo) {
    // Create a feature branch and set marker
    repo.git_command()
        .args(["branch", "feature"])
        .status()
        .unwrap();
    repo.set_marker("feature", "🔧");

    let output = wt_state_cmd(&repo, "marker", "clear", &["--branch", "feature"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[32m✓[39m [32mCleared marker for [1mfeature[22m[39m");
}

#[rstest]
fn test_state_clear_marker_specific_branch_not_set(repo: TestRepo) {
    // Create a feature branch without any marker
    repo.git_command()
        .args(["branch", "feature"])
        .status()
        .unwrap();

    let output = wt_state_cmd(&repo, "marker", "clear", &["--branch", "feature"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[2m○[22m No marker set for [1mfeature[22m");
}

#[rstest]
fn test_state_clear_marker_current_branch_not_set(repo: TestRepo) {
    // Clear marker on current branch (main) when none is set
    let output = wt_state_cmd(&repo, "marker", "clear", &[])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[2m○[22m No marker set for [1mmain[22m");
}

#[rstest]
fn test_state_clear_marker_all_single(repo: TestRepo) {
    // Test singular form "marker" vs "markers"
    repo.set_marker("main", "🚧");

    let output = wt_state_cmd(&repo, "marker", "clear", &["--all"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[32m✓[39m [32mCleared [1m1[22m marker[39m");
}

// ============================================================================
// hints
// ============================================================================

#[rstest]
fn test_state_hints_get_empty(repo: TestRepo) {
    let output = wt_state_cmd(&repo, "hints", "get", &[]).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[2m○[22m No hints have been shown");
}

#[rstest]
fn test_state_hints_get_with_hints(repo: TestRepo) {
    // Set hints via git config (as the code stores them)
    repo.git_command()
        .args(["config", "worktrunk.hints.worktree-path", "true"])
        .status()
        .unwrap();
    repo.git_command()
        .args(["config", "worktrunk.hints.another-hint", "true"])
        .status()
        .unwrap();

    let output = wt_state_cmd(&repo, "hints", "get", &[]).output().unwrap();
    assert!(output.status.success());
    // Output goes to stdout
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("worktree-path"));
    assert!(stdout.contains("another-hint"));
}

#[rstest]
fn test_state_hints_clear_empty(repo: TestRepo) {
    let output = wt_state_cmd(&repo, "hints", "clear", &[]).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[2m○[22m No hints to clear");
}

#[rstest]
fn test_state_hints_clear_all(repo: TestRepo) {
    // Set hints
    repo.git_command()
        .args(["config", "worktrunk.hints.worktree-path", "true"])
        .status()
        .unwrap();
    repo.git_command()
        .args(["config", "worktrunk.hints.another-hint", "true"])
        .status()
        .unwrap();

    let output = wt_state_cmd(&repo, "hints", "clear", &[]).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[32m✓[39m [32mCleared [1m2[22m hints[39m");

    // Verify hints were cleared
    let output = repo
        .git_command()
        .args(["config", "--get-regexp", r"^worktrunk\.hints\."])
        .output()
        .unwrap();
    assert!(!output.status.success()); // No matches
}

#[rstest]
fn test_state_hints_clear_single(repo: TestRepo) {
    // Set a single hint
    repo.git_command()
        .args(["config", "worktrunk.hints.worktree-path", "true"])
        .status()
        .unwrap();

    let output = wt_state_cmd(&repo, "hints", "clear", &[]).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[32m✓[39m [32mCleared [1m1[22m hint[39m");
}

#[rstest]
fn test_state_hints_clear_specific(repo: TestRepo) {
    // Set hints
    repo.git_command()
        .args(["config", "worktrunk.hints.worktree-path", "true"])
        .status()
        .unwrap();
    repo.git_command()
        .args(["config", "worktrunk.hints.another-hint", "true"])
        .status()
        .unwrap();

    let output = wt_state_cmd(&repo, "hints", "clear", &["worktree-path"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[32m✓[39m [32mCleared hint [1mworktree-path[22m[39m");

    // Verify only that hint was cleared
    let output = repo
        .git_command()
        .args(["config", "--get", "worktrunk.hints.worktree-path"])
        .output()
        .unwrap();
    assert!(!output.status.success()); // Cleared

    let output = repo
        .git_command()
        .args(["config", "--get", "worktrunk.hints.another-hint"])
        .output()
        .unwrap();
    assert!(output.status.success()); // Still there
}

#[rstest]
fn test_state_hints_clear_specific_not_set(repo: TestRepo) {
    let output = wt_state_cmd(&repo, "hints", "clear", &["nonexistent"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[2m○[22m Hint [1mnonexistent[22m was not set");
}
