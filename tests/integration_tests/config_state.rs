use crate::common::{TEST_EPOCH, TestRepo, repo, repo_with_remote, wt_command};
use insta::assert_snapshot;
use rstest::rstest;
use std::path::{Path, PathBuf};
use std::process::Command;
use worktrunk::path::sanitize_for_filename;

/// Relative path of a hook log file under the wt logs directory.
///
/// Layout: `{branch}/{source}/{hook_type}/{name}.log` — branch and name are
/// sanitized to match `commands::process::HookLog::path`.
fn hook_log_rel_path(branch: &str, source: &str, hook_type: &str, name: &str) -> PathBuf {
    let safe_branch = sanitize_for_filename(branch);
    let safe_name = sanitize_for_filename(name);
    PathBuf::from(safe_branch)
        .join(source)
        .join(hook_type)
        .join(format!("{safe_name}.log"))
}

/// Relative path of an internal-operation log file under the wt logs directory.
///
/// Layout: `{branch}/internal/{op}.log` — branch is sanitized.
fn internal_log_rel_path(branch: &str, op: &str) -> PathBuf {
    let safe_branch = sanitize_for_filename(branch);
    PathBuf::from(safe_branch)
        .join("internal")
        .join(format!("{op}.log"))
}

/// Write a log file at `log_dir / relative`, creating parent directories.
fn write_log_at(log_dir: &Path, relative: &Path, contents: &str) {
    let full = log_dir.join(relative);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(&full, contents).unwrap();
}

/// Display string for a relative log path, forward-slashed for stable snapshots.
fn rel_display(p: &Path) -> String {
    use path_slash::PathExt as _;
    p.to_slash_lossy().into_owned()
}

/// Settings for `wt config state get` snapshots (normalizes log paths)
fn state_get_settings() -> insta::Settings {
    let mut settings = insta::Settings::clone_current();
    // COMMAND LOG / HOOK OUTPUT / DIAGNOSTIC paths vary per test (temp dir), normalize for stable snapshots
    settings.add_filter(r"(COMMAND LOG\x1b\[39m\s+@ )[^\n]+", "${1}<PATH>");
    settings.add_filter(r"(HOOK OUTPUT\x1b\[39m\s+@ )[^\n]+", "${1}<PATH>");
    settings.add_filter(r"(DIAGNOSTIC\x1b\[39m\s+@ )[^\n]+", "${1}<PATH>");
    settings
}

/// Path to the file-based CI cache entry at `.git/wt/cache/ci-status/<branch>.json`.
fn ci_cache_file(repo: &TestRepo, branch: &str) -> PathBuf {
    let safe_branch = sanitize_for_filename(branch);
    repo.root_path()
        .join(".git/wt/cache/ci-status")
        .join(format!("{safe_branch}.json"))
}

/// Write CI status to the file-based cache.
fn write_ci_cache(repo: &TestRepo, branch: &str, json: &str) {
    let cache_file = ci_cache_file(repo, branch);
    std::fs::create_dir_all(cache_file.parent().unwrap()).unwrap();
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
        .run()
        .unwrap();
    repo.git_command().args(["branch", "abc"]).run().unwrap();
    repo.git_command().args(["branch", "def"]).run().unwrap();

    // Now we have: xyz, abc, def - no common names, no init.defaultBranch
    // wt config state default-branch get should fail with an error
    let output = wt_state_cmd(&repo, "default-branch", "get", &[])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[31m✗[39m [31mCannot determine default branch. To configure, run [1mwt config state default-branch set BRANCH[22m[39m");
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
        .run()
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
        .run()
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
// bare subcommand defaults (no action → implicit get)
// ============================================================================

/// `wt config state ci-status` (no subcommand) defaults to `get`.
#[rstest]
fn test_state_bare_ci_status(repo: TestRepo) {
    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.args(["config", "state", "ci-status"]);
    cmd.current_dir(repo.root_path());
    let output = cmd.output().unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "no-ci");
}

/// `wt config state marker` (no subcommand) defaults to `get`.
#[rstest]
fn test_state_bare_marker(repo: TestRepo) {
    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.args(["config", "state", "marker"]);
    cmd.current_dir(repo.root_path());
    let output = cmd.output().unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).trim().is_empty());
}

/// `wt config state logs` (no subcommand) defaults to `get`.
#[rstest]
fn test_state_bare_logs(repo: TestRepo) {
    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.args(["config", "state", "logs"]);
    cmd.current_dir(repo.root_path());
    let output = cmd.output().unwrap();
    assert!(output.status.success());
}

/// `wt config state hints` (no subcommand) defaults to `get`.
#[rstest]
fn test_state_bare_hints(repo: TestRepo) {
    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.args(["config", "state", "hints"]);
    cmd.current_dir(repo.root_path());
    let output = cmd.output().unwrap();
    assert!(output.status.success());
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
        .run()
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
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"
    [31m✗[39m [31mNo branch named [1mnonexistent[22m[39m
    [2m↳[22m [2mTo create a new branch, run [4mwt switch --create nonexistent[24m; to list branches, run [4mwt list --branches --remotes[24m[22m
    ");
}

/// Resolve a branch that exists only as a remote-tracking ref (no local
/// counterpart). Exercises the `remote_branch` arm of the BranchRef match.
#[rstest]
fn test_state_get_ci_status_remote_only_branch(#[from(repo_with_remote)] repo: TestRepo) {
    repo.create_branch("foo");
    repo.run_git(&["checkout", "foo"]);
    std::fs::write(repo.root_path().join("f.txt"), "f").unwrap();
    repo.run_git(&["add", "."]);
    repo.run_git(&["commit", "-m", "foo commit"]);
    repo.push_branch("foo");
    repo.run_git(&["checkout", "main"]);
    repo.run_git(&["branch", "-D", "foo"]);

    let output = wt_state_cmd(&repo, "ci-status", "get", &["--branch", "origin/foo"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "command should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "no-ci");
}

/// A fresh cached CI status returns from `get` without re-fetching. Exercises
/// the cache-hit path where `PrStatus::detect` returns `Some` and the match
/// arm unwraps `ci_status`.
#[rstest]
fn test_state_get_ci_status_returns_cached_status(repo: TestRepo) {
    let head = repo.head_sha();
    write_ci_cache(
        &repo,
        "main",
        &format!(
            r#"{{"status":{{"ci_status":"passed","source":"pr","is_stale":false}},"checked_at":{TEST_EPOCH},"head":"{head}","branch":"main"}}"#
        ),
    );

    let output = wt_state_cmd(&repo, "ci-status", "get", &["--branch", "main"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "command should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "passed");
}

/// `wt config state ci-status --branch origin/foo` must resolve cleanly when a
/// local branch literally named `origin/foo` shadows a remote-tracking ref of
/// the same name. Smoke test: exercises the shadowing code path. The
/// visible-to-user consequences of the underlying bug (is_remote flag out of
/// sync with the HEAD SHA, affecting how `gh`/`glab` get invoked) aren't
/// observable without mocking those tools, but this guards against
/// regressions that would make the command error on ambiguity (e.g., naive
/// use of `rev-parse --symbolic-full-name`, which fails on shadowed refs).
#[rstest]
fn test_state_get_ci_status_shadow_origin_prefixed(#[from(repo_with_remote)] repo: TestRepo) {
    // Remote `foo` pushed to origin.
    repo.create_branch("foo");
    repo.run_git(&["checkout", "foo"]);
    std::fs::write(repo.root_path().join("remote.txt"), "remote").unwrap();
    repo.run_git(&["add", "."]);
    repo.run_git(&["commit", "-m", "Remote foo commit"]);
    repo.push_branch("foo");

    // Drop local `foo` so only `refs/remotes/origin/foo` remains.
    repo.run_git(&["checkout", "main"]);
    repo.run_git(&["branch", "-D", "foo"]);

    // Local branch literally named `origin/foo` with different history.
    repo.run_git(&["checkout", "-b", "origin/foo"]);
    std::fs::write(repo.root_path().join("local.txt"), "local").unwrap();
    repo.run_git(&["add", "."]);
    repo.run_git(&["commit", "-m", "Local origin/foo"]);
    repo.run_git(&["checkout", "main"]);

    let output = wt_state_cmd(&repo, "ci-status", "get", &["--branch", "origin/foo"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "command should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "no-ci");
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
    let head = repo.head_sha();
    write_ci_cache(
        &repo,
        "main",
        &format!(
            r#"{{"status":{{"ci_status":"passed","source":"pr","is_stale":false}},"checked_at":{TEST_EPOCH},"head":"{head}","branch":"main"}}"#
        ),
    );
    let cache_file = ci_cache_file(&repo, "main");
    assert!(cache_file.exists(), "cache file should exist before clear");

    let output = wt_state_cmd(&repo, "ci-status", "clear", &[])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[32m✓[39m [32mCleared CI cache for [1mmain[22m[39m");
    assert!(
        !cache_file.exists(),
        "cache file should be gone after clear"
    );
}

#[rstest]
fn test_state_clear_ci_status_branch_not_cached(repo: TestRepo) {
    let cache_file = ci_cache_file(&repo, "main");
    assert!(!cache_file.exists(), "cache file should not exist");

    let output = wt_state_cmd(&repo, "ci-status", "clear", &[])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[2m○[22m No CI cache for [1mmain[22m");
    assert!(!cache_file.exists(), "cache file should still not exist");
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
        .run()
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
        .run()
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
        .run()
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
        .run()
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
        .run()
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
    state_get_settings().bind(|| {
        assert_snapshot!(String::from_utf8_lossy(&output.stdout), @"
        [36mCOMMAND LOG[39m @ <PATH>
        [107m [0m (none)

        [36mHOOK OUTPUT[39m @ <PATH>
        [107m [0m (none)

        [36mDIAGNOSTIC[39m @ <PATH>
        [107m [0m (none)
        ");
    });
}

#[rstest]
fn test_state_get_logs_with_files(repo: TestRepo) {
    // Create wt/logs directory with hook output and command log files
    let git_dir = repo.root_path().join(".git");
    let log_dir = git_dir.join("wt/logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    write_log_at(
        &log_dir,
        &hook_log_rel_path("feature", "user", "post-start", "npm"),
        "npm output here",
    );
    // >= 1024 bytes to exercise the `{}K` size-formatting branch in
    // render_log_table (the other test files stay under 1KB to exercise `{}B`).
    write_log_at(
        &log_dir,
        &internal_log_rel_path("bugfix", "remove"),
        &"remove output\n".repeat(80),
    );
    std::fs::write(log_dir.join("commands.jsonl"), r#"{"ts":"2026-01-01"}"#).unwrap();

    let output = wt_state_cmd(&repo, "logs", "get", &[]).output().unwrap();
    assert!(output.status.success());
    let mut settings = state_get_settings();
    // File sizes and ages vary across environments
    settings.add_filter(r"(?m)\d+[BK]\s+\S+[ \t]*$", "<SIZE>  <AGE>");
    settings.bind(|| {
        assert_snapshot!(String::from_utf8_lossy(&output.stdout), @"
        [36mCOMMAND LOG[39m @ <PATH>
              File      Size  Age   
         ────────────── ──── ────── 
         commands.jsonl <SIZE>  <AGE>

        [36mHOOK OUTPUT[39m @ <PATH>
                      File               Size  Age   
         ─────────────────────────────── ──── ────── 
         bugfix/internal/remove.log      <SIZE>  <AGE>
         feature/user/post-start/npm.log <SIZE>  <AGE>

        [36mDIAGNOSTIC[39m @ <PATH>
        [107m [0m (none)
        ");
    });
}

#[rstest]
fn test_state_get_logs_dir_exists_no_log_files(repo: TestRepo) {
    // Create wt/logs directory with non-log files (empty of actual log files)
    let git_dir = repo.root_path().join(".git");
    let log_dir = git_dir.join("wt/logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    std::fs::write(log_dir.join("README.txt"), "not a log file").unwrap();
    std::fs::write(log_dir.join(".gitkeep"), "").unwrap();

    let output = wt_state_cmd(&repo, "logs", "get", &[]).output().unwrap();
    assert!(output.status.success());
    state_get_settings().bind(|| {
        assert_snapshot!(String::from_utf8_lossy(&output.stdout), @"
        [36mCOMMAND LOG[39m @ <PATH>
        [107m [0m (none)

        [36mHOOK OUTPUT[39m @ <PATH>
        [107m [0m (none)

        [36mDIAGNOSTIC[39m @ <PATH>
        [107m [0m (none)
        ");
    });
}

#[rstest]
fn test_state_get_logs_diagnostic_files(repo: TestRepo) {
    // Create wt/logs directory with diagnostic files
    let git_dir = repo.root_path().join(".git");
    let log_dir = git_dir.join("wt/logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    std::fs::write(log_dir.join("trace.log"), "debug output").unwrap();
    std::fs::write(log_dir.join("output.log"), "raw subprocess output").unwrap();
    std::fs::write(log_dir.join("diagnostic.md"), "# Diagnostic Report").unwrap();
    // Also add a hook output file to verify separation
    let remove_rel = internal_log_rel_path("feature", "remove");
    write_log_at(&log_dir, &remove_rel, "remove output");

    let output = wt_state_cmd(&repo, "logs", "get", &[]).output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Diagnostic files appear under DIAGNOSTIC, not HOOK OUTPUT
    assert!(stdout.contains("DIAGNOSTIC"), "Expected DIAGNOSTIC heading");
    for name in ["trace.log", "output.log", "diagnostic.md"] {
        assert!(stdout.contains(name), "Expected {name} in output");
    }

    // Hook output should have the remove log but not the diagnostic files
    let hook_section = stdout
        .split("DIAGNOSTIC")
        .next()
        .unwrap()
        .rsplit("HOOK OUTPUT")
        .next()
        .unwrap();
    let remove_display = rel_display(&remove_rel);
    assert!(
        hook_section.contains(&remove_display),
        "Expected {remove_display} in HOOK OUTPUT: {hook_section}"
    );
    for name in ["trace.log", "output.log"] {
        assert!(
            !hook_section.contains(name),
            "{name} should not be in HOOK OUTPUT: {hook_section}"
        );
    }
}

#[rstest]
fn test_state_clear_logs_includes_diagnostic_files(repo: TestRepo) {
    // diagnostic files (.log and .md) should be cleared
    let git_dir = repo.root_path().join(".git");
    let log_dir = git_dir.join("wt/logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    std::fs::write(log_dir.join("trace.log"), "debug output").unwrap();
    std::fs::write(log_dir.join("output.log"), "raw output").unwrap();
    std::fs::write(log_dir.join("diagnostic.md"), "# Report").unwrap();

    let output = wt_state_cmd(&repo, "logs", "clear", &[]).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(
        String::from_utf8_lossy(&output.stderr),
        @"[32m✓[39m [32mCleared [1m3[22m log files[39m"
    );
    assert!(!log_dir.exists());
}

#[rstest]
fn test_state_clear_logs_empty(repo: TestRepo) {
    let output = wt_state_cmd(&repo, "logs", "clear", &[]).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[2m○[22m No logs to clear");
}

#[rstest]
fn test_state_clear_logs_with_files(repo: TestRepo) {
    // Create wt/logs directory with hook output and command log files
    let git_dir = repo.root_path().join(".git");
    let log_dir = git_dir.join("wt/logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    write_log_at(
        &log_dir,
        &hook_log_rel_path("feature", "user", "post-start", "npm"),
        "npm output",
    );
    write_log_at(
        &log_dir,
        &internal_log_rel_path("bugfix", "remove"),
        "remove output",
    );
    std::fs::write(log_dir.join("commands.jsonl"), "jsonl data").unwrap();

    let output = wt_state_cmd(&repo, "logs", "clear", &[]).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[32m✓[39m [32mCleared [1m3[22m log files[39m");

    // Verify logs are gone
    assert!(!log_dir.exists());
}

#[rstest]
fn test_state_clear_logs_sweeps_legacy_flat_files(repo: TestRepo) {
    // Pre-nested layout left orphan `.log` files directly under wt/logs/.
    // `clear_logs` self-heals by sweeping them along with everything else.
    let git_dir = repo.root_path().join(".git");
    let log_dir = git_dir.join("wt/logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    std::fs::write(log_dir.join("feature-post-start-npm.log"), "old layout").unwrap();
    std::fs::write(log_dir.join("bugfix-remove.log"), "old layout").unwrap();

    let output = wt_state_cmd(&repo, "logs", "clear", &[]).output().unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Cleared") && stderr.contains("2") && stderr.contains("log file"));
    assert!(!log_dir.exists(), "log dir should be removed after sweep");
}

#[rstest]
fn test_state_clear_logs_single_file(repo: TestRepo) {
    // Create wt/logs directory with one log file
    let git_dir = repo.root_path().join(".git");
    let log_dir = git_dir.join("wt/logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    write_log_at(
        &log_dir,
        &internal_log_rel_path("feature", "remove"),
        "remove output",
    );

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
        .run()
        .unwrap();

    // Marker (using JSON format)
    repo.set_marker("main", "🚧");

    // CI cache (file-based)
    write_ci_cache(
        &repo,
        "feature",
        r#"{"checked_at":1704067200,"head":"abc123","branch":"feature"}"#,
    );

    // Vars data
    repo.git_command()
        .args(["config", "worktrunk.state.main.vars.env", "staging"])
        .run()
        .unwrap();

    // Git commands cache (SHA-keyed)
    let git_dir = repo.root_path().join(".git");
    let sha_cache_dir = git_dir.join("wt/cache/merge-tree-conflicts");
    std::fs::create_dir_all(&sha_cache_dir).unwrap();
    std::fs::write(sha_cache_dir.join("abc123-def456.json"), "true").unwrap();

    // Logs
    let log_dir = git_dir.join("wt/logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    write_log_at(
        &log_dir,
        &internal_log_rel_path("feature", "remove"),
        "output",
    );

    let output = wt_state_clear_all_cmd(&repo).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"
    [32m✓[39m [32mCleared previous branch[39m
    [32m✓[39m [32mCleared [1m1[22m marker[39m
    [32m✓[39m [32mCleared [1m1[22m CI cache entry[39m
    [32m✓[39m [32mCleared [1m1[22m git commands cache entry[39m
    [32m✓[39m [32mCleared [1m1[22m variable[39m
    [32m✓[39m [32mCleared [1m1[22m log file[39m
    ");

    // Verify everything was cleared
    assert!(
        repo.git_command()
            .args(["config", "--get", "worktrunk.history"])
            .run()
            .unwrap()
            .status
            .code()
            == Some(1)
    ); // Not found
    assert!(
        repo.git_command()
            .args(["config", "--get", "worktrunk.state.main.marker"])
            .run()
            .unwrap()
            .status
            .code()
            == Some(1)
    );
    assert!(
        repo.git_command()
            .args(["config", "--get", "worktrunk.state.main.vars.env"])
            .run()
            .unwrap()
            .status
            .code()
            == Some(1),
        "Vars data should be cleared"
    );
    // CI cache is now file-based, verify the cache file is cleared
    let ci_cache_dir = git_dir.join("wt").join("cache").join("ci-status");
    assert!(
        !ci_cache_dir.join("feature.json").exists(),
        "CI cache file should be cleared"
    );
    assert!(!log_dir.exists());
}

#[rstest]
fn test_state_clear_all_cleans_trash(repo: TestRepo) {
    // Create trash directory with stale entries (simulating failed background rm)
    let git_dir = repo.root_path().join(".git");
    let trash_dir = git_dir.join("wt/trash");
    std::fs::create_dir_all(trash_dir.join("myproject.feature-1234567890/target")).unwrap();
    std::fs::write(
        trash_dir.join("myproject.feature-1234567890/target/.rustc_info.json"),
        "{}",
    )
    .unwrap();
    std::fs::create_dir_all(trash_dir.join("myproject.bugfix-9999999999")).unwrap();
    // Stray file directly in trash (not inside a subdirectory) — exercises the
    // non-directory branch in clear_trash's `if path.is_dir()` guard.
    std::fs::write(trash_dir.join("stray-file.txt"), "stale").unwrap();

    let output = wt_state_clear_all_cmd(&repo).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[32m✓[39m [32mCleared [1m3[22m trash entries[39m");

    // Trash directory itself should be removed (empty after cleanup)
    assert!(!trash_dir.exists(), "Trash directory should be cleaned up");
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
    state_get_settings().bind(|| {
        assert_snapshot!(String::from_utf8_lossy(&output.stdout), @"
        [36mDEFAULT BRANCH[39m
        [107m [0m main

        [36mPREVIOUS BRANCH[39m
        [107m [0m (none)

        [36mBRANCH MARKERS[39m
        [107m [0m (none)

        [36mVARS[39m
        [107m [0m (none)

        [36mCI STATUS CACHE[39m
        [107m [0m (none)

        [36mGIT COMMANDS CACHE[39m
        [107m [0m (none)

        [36mHINTS[39m
        [107m [0m (none)

        [36mCOMMAND LOG[39m @ <PATH>
        [107m [0m (none)

        [36mHOOK OUTPUT[39m @ <PATH>
        [107m [0m (none)

        [36mDIAGNOSTIC[39m @ <PATH>
        [107m [0m (none)

        [36mTRASH[39m @ _REPO_/.git/wt/trash
        [107m [0m (none)
        ");
    });
}

#[rstest]
fn test_state_get_with_ci_entries(repo: TestRepo) {
    // Add CI cache entries - use TEST_EPOCH for deterministic age=0s in snapshots
    write_ci_cache(
        &repo,
        "feature",
        &format!(
            r#"{{"status":{{"ci_status":"passed","source":"pr","is_stale":false}},"checked_at":{TEST_EPOCH},"head":"abc12345def67890","branch":"feature"}}"#
        ),
    );

    write_ci_cache(
        &repo,
        "bugfix",
        &format!(
            r#"{{"status":{{"ci_status":"failed","source":"branch","is_stale":true}},"checked_at":{TEST_EPOCH},"head":"111222333444555","branch":"bugfix"}}"#
        ),
    );

    write_ci_cache(
        &repo,
        "main",
        &format!(
            r#"{{"status":null,"checked_at":{TEST_EPOCH},"head":"deadbeef12345678","branch":"main"}}"#
        ),
    );

    let output = wt_state_get_cmd(&repo).output().unwrap();
    assert!(output.status.success());
    state_get_settings().bind(|| {
        assert_snapshot!(String::from_utf8_lossy(&output.stdout));
    });
}

#[rstest]
fn test_state_get_comprehensive(repo: TestRepo) {
    // Set up previous branch
    repo.git_command()
        .args(["config", "worktrunk.history", "feature"])
        .run()
        .unwrap();

    // Set up branch markers (JSON format with timestamps for deterministic age)
    repo.git_command()
        .args([
            "config",
            "worktrunk.state.feature.marker",
            &format!(r#"{{"marker":"🚧 WIP","set_at":{TEST_EPOCH}}}"#),
        ])
        .run()
        .unwrap();
    repo.git_command()
        .args([
            "config",
            "worktrunk.state.bugfix.marker",
            &format!(r#"{{"marker":"🐛 debugging","set_at":{TEST_EPOCH}}}"#),
        ])
        .run()
        .unwrap();

    // Set up vars data
    repo.git_command()
        .args(["config", "worktrunk.state.main.vars.env", "staging"])
        .run()
        .unwrap();
    repo.git_command()
        .args(["config", "worktrunk.state.feature.vars.port", "3000"])
        .run()
        .unwrap();

    // Set up CI cache (file-based)
    write_ci_cache(
        &repo,
        "feature",
        &format!(
            r#"{{"status":{{"ci_status":"passed","source":"pr","is_stale":false}},"checked_at":{TEST_EPOCH},"head":"abc12345def67890","branch":"feature"}}"#
        ),
    );

    // Create log files
    let git_dir = repo.root_path().join(".git");
    let log_dir = git_dir.join("wt/logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    write_log_at(
        &log_dir,
        &hook_log_rel_path("feature", "user", "post-start", "npm"),
        "npm output",
    );
    write_log_at(
        &log_dir,
        &internal_log_rel_path("bugfix", "remove"),
        "remove output",
    );

    // Create trash entries (staged worktree removals)
    let trash_dir = git_dir.join("wt/trash");
    std::fs::create_dir_all(trash_dir.join("myproject.feature-1234567890")).unwrap();
    std::fs::create_dir_all(trash_dir.join("myproject.bugfix-9999999999")).unwrap();

    // Create git commands cache entries (SHA-keyed caches)
    let sha_cache_dir = git_dir.join("wt/cache/merge-tree-conflicts");
    std::fs::create_dir_all(&sha_cache_dir).unwrap();
    std::fs::write(sha_cache_dir.join("aaaa-bbbb.json"), "{}").unwrap();
    std::fs::write(sha_cache_dir.join("cccc-dddd.json"), "{}").unwrap();

    let output = wt_state_get_cmd(&repo).output().unwrap();
    assert!(output.status.success());
    state_get_settings().bind(|| {
        assert_snapshot!(String::from_utf8_lossy(&output.stdout));
    });
}

#[rstest]
fn test_state_get_json_empty(repo: TestRepo) {
    let output = wt_state_get_json_cmd(&repo).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stdout), @r#"
    {
      "ci_status": [],
      "command_log": [],
      "default_branch": "main",
      "diagnostic": [],
      "git_commands_cache": 0,
      "hints": [],
      "hook_output": [],
      "markers": [],
      "previous_branch": null,
      "trash": [],
      "vars": []
    }
    "#);
}

#[rstest]
fn test_state_get_json_comprehensive(repo: TestRepo) {
    // Set up previous branch
    repo.git_command()
        .args(["config", "worktrunk.history", "feature"])
        .run()
        .unwrap();

    // Set up branch markers (JSON format with timestamps)
    repo.git_command()
        .args([
            "config",
            "worktrunk.state.feature.marker",
            &format!(r#"{{"marker":"🚧 WIP","set_at":{TEST_EPOCH}}}"#),
        ])
        .run()
        .unwrap();

    // Set up vars data
    repo.git_command()
        .args(["config", "worktrunk.state.main.vars.env", "staging"])
        .run()
        .unwrap();

    // Set up CI cache (file-based)
    write_ci_cache(
        &repo,
        "feature",
        &format!(
            r#"{{"status":{{"ci_status":"passed","source":"pr","is_stale":false}},"checked_at":{TEST_EPOCH},"head":"abc12345def67890","branch":"feature"}}"#
        ),
    );

    // Populate trash + git commands cache for parity coverage
    let git_dir = repo.root_path().join(".git");
    std::fs::create_dir_all(git_dir.join("wt/trash/myproject.feature-1234567890")).unwrap();
    let sha_cache_dir = git_dir.join("wt/cache/is-ancestor");
    std::fs::create_dir_all(&sha_cache_dir).unwrap();
    std::fs::write(sha_cache_dir.join("aaaa-bbbb.json"), "{}").unwrap();

    let output = wt_state_get_json_cmd(&repo).output().unwrap();
    assert!(output.status.success());
    let json_str = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    let normalized = serde_json::to_string_pretty(&json).unwrap();
    let mut settings = insta::Settings::clone_current();
    settings.add_filter(r#""modified_at": \d+"#, r#""modified_at": "<MTIME>""#);
    settings.bind(|| {
        assert_snapshot!(normalized, @r#"
        {
          "ci_status": [
            {
              "branch": "feature",
              "checked_at": 1735776000,
              "head": "abc12345def67890",
              "status": "passed"
            }
          ],
          "command_log": [],
          "default_branch": "main",
          "diagnostic": [],
          "git_commands_cache": 1,
          "hints": [],
          "hook_output": [],
          "markers": [
            {
              "branch": "feature",
              "marker": "🚧 WIP",
              "set_at": 1735776000
            }
          ],
          "previous_branch": "feature",
          "trash": [
            {
              "modified_at": "<MTIME>",
              "name": "myproject.feature-1234567890",
              "path": "_REPO_/.git/wt/trash/myproject.feature-1234567890"
            }
          ],
          "vars": [
            {
              "branch": "main",
              "key": "env",
              "value": "staging"
            }
          ]
        }
        "#);
    });
}

#[rstest]
fn test_state_get_json_with_logs(repo: TestRepo) {
    // Create hook output and command log files
    let git_dir = repo.root_path().join(".git");
    let log_dir = git_dir.join("wt/logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    write_log_at(
        &log_dir,
        &hook_log_rel_path("feature", "user", "post-start", "npm"),
        "npm output",
    );
    write_log_at(
        &log_dir,
        &internal_log_rel_path("bugfix", "remove"),
        "remove log output",
    );
    std::fs::write(log_dir.join("commands.jsonl"), r#"{"ts":"2026-01-01"}"#).unwrap();

    let output = wt_state_get_json_cmd(&repo).output().unwrap();
    assert!(output.status.success());
    let json_str = String::from_utf8_lossy(&output.stdout);
    let mut json: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    // Sort log arrays by filename (mtime ties produce platform-dependent order)
    for key in ["command_log", "hook_output"] {
        if let Some(arr) = json.get_mut(key).and_then(|v| v.as_array_mut()) {
            arr.sort_by(|a, b| a["file"].as_str().cmp(&b["file"].as_str()));
        }
    }

    // Normalize dynamic fields before snapshotting
    let normalized = serde_json::to_string_pretty(&json).unwrap();
    let mut settings = insta::Settings::clone_current();
    settings.add_filter(r#""modified_at": \d+"#, r#""modified_at": "<MTIME>""#);
    settings.add_filter(r#""size": \d+"#, r#""size": "<SIZE>""#);
    settings.bind(|| {
        assert_snapshot!(normalized, @r#"
        {
          "ci_status": [],
          "command_log": [
            {
              "file": "commands.jsonl",
              "modified_at": "<MTIME>",
              "path": "_REPO_/.git/wt/logs/commands.jsonl",
              "size": "<SIZE>"
            }
          ],
          "default_branch": "main",
          "diagnostic": [],
          "git_commands_cache": 0,
          "hints": [],
          "hook_output": [
            {
              "branch": "bugfix",
              "file": "bugfix/internal/remove.log",
              "hook_type": null,
              "modified_at": "<MTIME>",
              "name": "remove",
              "path": "_REPO_/.git/wt/logs/bugfix/internal/remove.log",
              "size": "<SIZE>",
              "source": "internal"
            },
            {
              "branch": "feature",
              "file": "feature/user/post-start/npm.log",
              "hook_type": "post-start",
              "modified_at": "<MTIME>",
              "name": "npm",
              "path": "_REPO_/.git/wt/logs/feature/user/post-start/npm.log",
              "size": "<SIZE>",
              "source": "user"
            }
          ],
          "markers": [],
          "previous_branch": null,
          "trash": [],
          "vars": []
        }
        "#);
    });
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
            r#"{{"status":{{"ci_status":"passed","source":"pr","is_stale":false}},"checked_at":{TEST_EPOCH},"head":"abc12345","branch":"feature"}}"#
        ),
    );
    write_ci_cache(
        &repo,
        "bugfix",
        &format!(
            r#"{{"status":{{"ci_status":"failed","source":"branch","is_stale":false}},"checked_at":{TEST_EPOCH},"head":"def67890","branch":"bugfix"}}"#
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
            r#"{{"status":{{"ci_status":"passed","source":"pr","is_stale":false}},"checked_at":{TEST_EPOCH},"head":"abc12345","branch":"feature"}}"#
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
        .run()
        .unwrap();

    let head = repo.head_sha();
    write_ci_cache(
        &repo,
        "feature",
        &format!(
            r#"{{"status":{{"ci_status":"passed","source":"pr","is_stale":false}},"checked_at":{TEST_EPOCH},"head":"{head}","branch":"feature"}}"#
        ),
    );
    let cache_file = ci_cache_file(&repo, "feature");
    assert!(cache_file.exists(), "cache file should exist before clear");

    let output = wt_state_cmd(&repo, "ci-status", "clear", &["--branch", "feature"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[32m✓[39m [32mCleared CI cache for [1mfeature[22m[39m");
    assert!(
        !cache_file.exists(),
        "cache file should be gone after clear"
    );
}

#[rstest]
fn test_state_clear_ci_status_specific_branch_not_cached(repo: TestRepo) {
    // Create a feature branch without any CI cache
    repo.git_command()
        .args(["branch", "feature"])
        .run()
        .unwrap();
    let cache_file = ci_cache_file(&repo, "feature");
    assert!(!cache_file.exists(), "cache file should not exist");

    let output = wt_state_cmd(&repo, "ci-status", "clear", &["--branch", "feature"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[2m○[22m No CI cache for [1mfeature[22m");
    assert!(!cache_file.exists(), "cache file should still not exist");
}

#[rstest]
fn test_state_clear_marker_specific_branch(repo: TestRepo) {
    // Create a feature branch and set marker
    repo.git_command()
        .args(["branch", "feature"])
        .run()
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
        .run()
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
        .run()
        .unwrap();
    repo.git_command()
        .args(["config", "worktrunk.hints.another-hint", "true"])
        .run()
        .unwrap();

    let output = wt_state_cmd(&repo, "hints", "get", &[]).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stdout), @"
    another-hint
    worktree-path
    ");
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
        .run()
        .unwrap();
    repo.git_command()
        .args(["config", "worktrunk.hints.another-hint", "true"])
        .run()
        .unwrap();

    let output = wt_state_cmd(&repo, "hints", "clear", &[]).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[32m✓[39m [32mCleared [1m2[22m hints[39m");

    // Verify hints were cleared
    let output = repo
        .git_command()
        .args(["config", "--get-regexp", r"^worktrunk\.hints\."])
        .run()
        .unwrap();
    assert!(!output.status.success()); // No matches
}

#[rstest]
fn test_state_hints_clear_single(repo: TestRepo) {
    // Set a single hint
    repo.git_command()
        .args(["config", "worktrunk.hints.worktree-path", "true"])
        .run()
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
        .run()
        .unwrap();
    repo.git_command()
        .args(["config", "worktrunk.hints.another-hint", "true"])
        .run()
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
        .run()
        .unwrap();
    assert!(!output.status.success()); // Cleared

    let output = repo
        .git_command()
        .args(["config", "--get", "worktrunk.hints.another-hint"])
        .run()
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

// ============================================================================
// vars
// ============================================================================

#[rstest]
fn test_vars_set_and_get(repo: TestRepo) {
    // Set a value
    let output = wt_state_cmd(&repo, "vars", "set", &["env=staging"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[32m✓[39m [32mSet [1menv[22m for [1mmain[22m[39m");

    // Get the value
    let output = wt_state_cmd(&repo, "vars", "get", &["env"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "staging");
}

#[rstest]
fn test_vars_set_json_value(repo: TestRepo) {
    let json = r#"{"port":3000,"debug":true}"#;
    let output = wt_state_cmd(&repo, "vars", "set", &[&format!("config={json}")])
        .output()
        .unwrap();
    assert!(output.status.success());

    let output = wt_state_cmd(&repo, "vars", "get", &["config"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), json);
}

#[rstest]
fn test_vars_get_missing_key(repo: TestRepo) {
    let output = wt_state_cmd(&repo, "vars", "get", &["nonexistent"])
        .output()
        .unwrap();
    assert!(output.status.success());
    // Empty output for missing keys
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "");
}

#[rstest]
fn test_vars_list(repo: TestRepo) {
    // Set multiple values
    wt_state_cmd(&repo, "vars", "set", &["env=staging"])
        .output()
        .unwrap();
    wt_state_cmd(&repo, "vars", "set", &["port=3000"])
        .output()
        .unwrap();

    let output = wt_state_cmd(&repo, "vars", "list", &[]).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stdout), @"
    env	staging
    port	3000
    ");
}

#[rstest]
fn test_vars_list_empty(repo: TestRepo) {
    let output = wt_state_cmd(&repo, "vars", "list", &[]).output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[2m○[22m No variables for [1mmain[22m");
}

#[rstest]
fn test_vars_clear_single_key(repo: TestRepo) {
    // Set and clear
    wt_state_cmd(&repo, "vars", "set", &["env=staging"])
        .output()
        .unwrap();
    let output = wt_state_cmd(&repo, "vars", "clear", &["env"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[32m✓[39m [32mCleared [1menv[22m for [1mmain[22m[39m");

    // Verify it's gone
    let output = wt_state_cmd(&repo, "vars", "get", &["env"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "");
}

#[rstest]
fn test_vars_clear_all(repo: TestRepo) {
    // Set multiple values
    wt_state_cmd(&repo, "vars", "set", &["env=staging"])
        .output()
        .unwrap();
    wt_state_cmd(&repo, "vars", "set", &["port=3000"])
        .output()
        .unwrap();

    let output = wt_state_cmd(&repo, "vars", "clear", &["--all"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[32m✓[39m [32mCleared [1m2[22m variables for [1mmain[22m[39m");

    // Verify all gone
    let output = wt_state_cmd(&repo, "vars", "list", &[]).output().unwrap();
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[2m○[22m No variables for [1mmain[22m");
}

#[rstest]
fn test_vars_invalid_key(repo: TestRepo) {
    let output = wt_state_cmd(&repo, "vars", "set", &["foo.bar=value"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @r#"[31m✗[39m [31mInvalid key "foo.bar": keys must contain only letters, digits, and hyphens[39m"#);
}

#[rstest]
fn test_vars_branch_flag(repo: TestRepo) {
    // Create a branch
    repo.run_git(&["branch", "feature"]);

    // Set vars on a different branch
    let output = wt_state_cmd(
        &repo,
        "vars",
        "set",
        &["env=production", "--branch=feature"],
    )
    .output()
    .unwrap();
    assert!(output.status.success());

    // Get from that branch
    let output = wt_state_cmd(&repo, "vars", "get", &["env", "--branch=feature"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "production");

    // Current branch should not have the value
    let output = wt_state_cmd(&repo, "vars", "get", &["env"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "");
}

#[rstest]
fn test_vars_value_with_spaces(repo: TestRepo) {
    let output = wt_state_cmd(&repo, "vars", "set", &["note=hello world foo"])
        .output()
        .unwrap();
    assert!(output.status.success());

    let output = wt_state_cmd(&repo, "vars", "get", &["note"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "hello world foo"
    );
}

#[rstest]
fn test_vars_value_containing_equals(repo: TestRepo) {
    let url = "postgres://user:pass@host/db?sslmode=require";
    let output = wt_state_cmd(&repo, "vars", "set", &[&format!("db-url={url}")])
        .output()
        .unwrap();
    assert!(output.status.success());

    let output = wt_state_cmd(&repo, "vars", "get", &["db-url"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), url);
}

#[rstest]
fn test_vars_empty_value(repo: TestRepo) {
    let output = wt_state_cmd(&repo, "vars", "set", &["key="])
        .output()
        .unwrap();
    assert!(output.status.success());

    let output = wt_state_cmd(&repo, "vars", "get", &["key"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "");
}

#[rstest]
fn test_vars_overwrite(repo: TestRepo) {
    wt_state_cmd(&repo, "vars", "set", &["env=staging"])
        .output()
        .unwrap();
    wt_state_cmd(&repo, "vars", "set", &["env=production"])
        .output()
        .unwrap();

    let output = wt_state_cmd(&repo, "vars", "get", &["env"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "production");
}

#[rstest]
fn test_vars_in_json_output(repo: TestRepo) {
    // Set vars data
    repo.git_command()
        .args(["config", "worktrunk.state.main.vars.env", "staging"])
        .run()
        .unwrap();
    repo.git_command()
        .args(["config", "worktrunk.state.main.vars.port", "3000"])
        .run()
        .unwrap();

    let output = repo
        .wt_command()
        .args(["list", "--format=json"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let items = json.as_array().unwrap();
    assert!(!items.is_empty());

    let main_item = &items[0];
    assert_eq!(main_item["vars"]["env"], "staging");
    assert_eq!(main_item["vars"]["port"], "3000");
}

#[rstest]
fn test_vars_absent_in_json_when_empty(repo: TestRepo) {
    // No vars data set — vars field should be absent from JSON
    let output = repo
        .wt_command()
        .args(["list", "--format=json"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let items = json.as_array().unwrap();
    assert!(!items.is_empty());

    // vars should not be present when empty (skip_serializing_if)
    assert!(items[0].get("vars").is_none());
}

#[rstest]
fn test_vars_clear_nonexistent_key(repo: TestRepo) {
    // Clear a key that was never set
    let output = wt_state_cmd(&repo, "vars", "clear", &["nonexistent"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[2m○[22m No variable [1mnonexistent[22m for [1mmain[22m");
}

#[rstest]
fn test_vars_clear_all_empty(repo: TestRepo) {
    // Clear --all when no vars data exists
    let output = wt_state_cmd(&repo, "vars", "clear", &["--all"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[2m○[22m No variables for [1mmain[22m");
}

#[rstest]
fn test_vars_list_with_branch_flag(repo: TestRepo) {
    // Create a branch and set vars data
    repo.run_git(&["branch", "feature"]);
    repo.git_command()
        .args(["config", "worktrunk.state.feature.vars.env", "production"])
        .run()
        .unwrap();

    // List vars for specific branch
    let output = wt_state_cmd(&repo, "vars", "list", &["--branch=feature"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stdout), @"env	production");
}

#[rstest]
fn test_vars_clear_with_branch_flag(repo: TestRepo) {
    // Create a branch and set vars data
    repo.run_git(&["branch", "feature"]);
    repo.git_command()
        .args(["config", "worktrunk.state.feature.vars.env", "production"])
        .run()
        .unwrap();

    // Clear vars for specific branch
    let output = wt_state_cmd(&repo, "vars", "clear", &["env", "--branch=feature"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[32m✓[39m [32mCleared [1menv[22m for [1mfeature[22m[39m");
}

#[rstest]
fn test_vars_branch_with_dots_in_name(repo: TestRepo) {
    // Branch names with dots are common (e.g., "feature.auth") and contain
    // regex metacharacters. Vars must round-trip correctly despite the dots.
    repo.run_git(&["branch", "feature.auth"]);

    let output = wt_state_cmd(
        &repo,
        "vars",
        "set",
        &["port=4000", "--branch=feature.auth"],
    )
    .output()
    .unwrap();
    assert!(output.status.success());

    let output = wt_state_cmd(&repo, "vars", "get", &["port", "--branch=feature.auth"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "4000");

    // Also verify the branch doesn't bleed into a similarly-named branch.
    // "feature.auth" unescaped in regex would match "featurexauth" too.
    repo.run_git(&["branch", "featurexauth"]);
    repo.git_command()
        .args(["config", "worktrunk.state.featurexauth.vars.port", "9999"])
        .run()
        .unwrap();

    // feature.auth should still return 4000, not 9999
    let output = wt_state_cmd(&repo, "vars", "get", &["port", "--branch=feature.auth"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "4000");
}

#[rstest]
fn test_vars_json_branch_with_vars_in_name(repo: TestRepo) {
    // Regression: branch names containing ".vars." must not confuse the
    // all_vars_entries parser (which splits on ".vars." to find the separator).
    let wt_path = repo.root_path().join("..").join("fix-vars-cleanup-wt");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "fix.vars.cleanup",
        wt_path.to_str().unwrap(),
    ]);
    repo.git_command()
        .args([
            "config",
            "worktrunk.state.fix.vars.cleanup.vars.port",
            "5000",
        ])
        .run()
        .unwrap();

    let output = repo
        .wt_command()
        .args(["list", "--format=json"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let items = json.as_array().unwrap();

    let branch_item = items
        .iter()
        .find(|item| item["branch"] == "fix.vars.cleanup")
        .expect("branch fix.vars.cleanup should be in JSON output");

    assert_eq!(
        branch_item["vars"]["port"], "5000",
        "vars key should be 'port', not a mangled key from bad split"
    );
}

// ============================================================================
// --format=json on individual subcommands
// ============================================================================

#[rstest]
fn test_logs_get_json_empty(repo: TestRepo) {
    let output = wt_state_cmd(&repo, "logs", "get", &["--format=json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stdout), @r#"
    {
      "command_log": [],
      "diagnostic": [],
      "hook_output": []
    }
    "#);
}

/// `--format=json` on the bareword subcommand (no `get`) routes to the
/// same list view. `--format` is `global = true` on the parent, so all three
/// call shapes — `logs --format=json`, `logs --format=json get`,
/// `logs get --format=json` — produce JSON.
#[rstest]
fn test_logs_bare_format_json(repo: TestRepo) {
    let mut cmd = repo.wt_command();
    cmd.args(["config", "state", "logs", "--format=json"]);
    cmd.current_dir(repo.root_path());
    let output = cmd.output().unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stdout), @r#"
    {
      "command_log": [],
      "diagnostic": [],
      "hook_output": []
    }
    "#);
}

#[rstest]
fn test_logs_get_json_with_files(repo: TestRepo) {
    let log_dir = repo.root_path().join(".git/wt/logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    std::fs::write(log_dir.join("commands.jsonl"), "{}").unwrap();
    std::fs::write(log_dir.join("diagnostic.md"), "# report").unwrap();
    write_log_at(
        &log_dir,
        &hook_log_rel_path("main", "user", "post-start", "server"),
        "output",
    );

    let output = wt_state_cmd(&repo, "logs", "get", &["--format=json"])
        .output()
        .unwrap();
    assert!(output.status.success());

    // Redact dynamic timestamps and sizes. "main" and "server" are already
    // filename-safe, so sanitize_for_filename passes them through unchanged —
    // no hash suffixes appear and no redaction is needed.
    let mut settings = insta::Settings::clone_current();
    settings.add_filter(r#""modified_at": \d+"#, r#""modified_at": "<TIMESTAMP>""#);
    settings.add_filter(r#""size": \d+"#, r#""size": "<SIZE>""#);
    settings.bind(|| {
        assert_snapshot!(String::from_utf8_lossy(&output.stdout));
    });
}

/// Internal-op entries get `source: "internal"`, `hook_type: null`, and the
/// op goes in `name` — so jq filters like `select(.source == "internal")`
/// work the same as for user/project hooks.
#[rstest]
fn test_logs_get_json_internal_op_structure(repo: TestRepo) {
    let log_dir = repo.root_path().join(".git/wt/logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    write_log_at(
        &log_dir,
        &internal_log_rel_path("feature", "remove"),
        "remove output",
    );

    let output = wt_state_cmd(&repo, "logs", "get", &["--format=json"])
        .output()
        .unwrap();
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let hook = &parsed["hook_output"][0];
    assert_eq!(hook["source"], "internal");
    assert_eq!(hook["hook_type"], serde_json::Value::Null);
    assert_eq!(hook["name"], "remove");
    assert!(hook["branch"].as_str().unwrap().starts_with("feature"));
}

/// Log files that don't match the expected branch subtree layout (`{branch}/{source}/{hook_type}/{name}.log`
/// or `{branch}/internal/{op}.log`) still appear in the JSON listing — just
/// without structured filter fields. Guards the defensive `_ => None` arm in
/// `parse_hook_structure` against future path-layout regressions.
#[rstest]
fn test_logs_get_json_unknown_layout_has_no_structure(repo: TestRepo) {
    let log_dir = repo.root_path().join(".git/wt/logs");
    // 2-segment layout: branch/file.log (missing source & hook_type).
    let relative = PathBuf::from("main").join("stray.log");
    write_log_at(&log_dir, &relative, "stray output");

    let output = wt_state_cmd(&repo, "logs", "get", &["--format=json"])
        .output()
        .unwrap();
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let hook = &parsed["hook_output"][0];
    // Entry appears with `file` and `path`, but structured fields are omitted.
    assert_eq!(hook["file"], "main/stray.log");
    assert!(hook["branch"].is_null());
    assert!(hook["source"].is_null());
    assert!(hook["hook_type"].is_null());
    assert!(hook["name"].is_null());
}

#[rstest]
fn test_ci_status_get_json(repo: TestRepo) {
    let output = wt_state_cmd(&repo, "ci-status", "get", &["--format=json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stdout), @"null");
}

#[rstest]
fn test_ci_status_get_json_with_cached_data(repo: TestRepo) {
    repo.commit("initial");
    let head = repo.head_sha();

    write_ci_cache(
        &repo,
        "main",
        &format!(
            r#"{{"status":{{"ci_status":"passed","source":"pr","is_stale":false}},"checked_at":{TEST_EPOCH},"head":"{head}","branch":"main"}}"#
        ),
    );

    let output = wt_state_cmd(&repo, "ci-status", "get", &["--format=json"])
        .output()
        .unwrap();
    assert!(output.status.success());

    let mut settings = insta::Settings::clone_current();
    settings.add_filter(&head, "<SHA>");
    settings.bind(|| {
        assert_snapshot!(String::from_utf8_lossy(&output.stdout));
    });
}

#[rstest]
fn test_marker_get_json_empty(repo: TestRepo) {
    let output = wt_state_cmd(&repo, "marker", "get", &["--format=json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stdout), @"null");
}

#[rstest]
fn test_marker_get_json_with_value(repo: TestRepo) {
    repo.run_git(&[
        "config",
        "worktrunk.state.main.marker",
        &format!(r#"{{"marker":"🚧 WIP","set_at":{TEST_EPOCH}}}"#),
    ]);

    let output = wt_state_cmd(&repo, "marker", "get", &["--format=json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stdout), @r#"
    {
      "branch": "main",
      "marker": "🚧 WIP",
      "set_at": 1735776000
    }
    "#);
}

#[rstest]
fn test_vars_list_json_empty(repo: TestRepo) {
    let output = wt_state_cmd(&repo, "vars", "list", &["--format=json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stdout), @"{}");
}

#[rstest]
fn test_vars_list_json_with_values(repo: TestRepo) {
    repo.run_git(&["config", "worktrunk.state.main.vars.env", "staging"]);
    repo.run_git(&["config", "worktrunk.state.main.vars.port", "3000"]);

    let output = wt_state_cmd(&repo, "vars", "list", &["--format=json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stdout), @r#"
    {
      "env": "staging",
      "port": "3000"
    }
    "#);
}

#[rstest]
fn test_hints_get_json_empty(repo: TestRepo) {
    let output = wt_state_cmd(&repo, "hints", "get", &["--format=json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stdout), @"[]");
}

#[rstest]
fn test_hints_get_json_with_values(repo: TestRepo) {
    repo.run_git(&["config", "worktrunk.hints.worktree-path", "true"]);

    let output = wt_state_cmd(&repo, "hints", "get", &["--format=json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_snapshot!(String::from_utf8_lossy(&output.stdout), @r#"
    [
      "worktree-path"
    ]
    "#);
}

// ============================================================================
// --format rejected on write actions (set/clear)
// ============================================================================

/// Build `wt config state <key> [args...]` without injecting an action name.
/// Unlike `wt_state_cmd`, this lets tests pass `--format=json` *before* the
/// action to exercise the `global = true` propagation path that silently
/// accepted the flag prior to gating.
fn wt_state_raw_cmd(repo: &TestRepo, key: &str, args: &[&str]) -> Command {
    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.args(["config", "state", key]);
    cmd.args(args);
    cmd.current_dir(repo.root_path());
    cmd
}

#[rstest]
#[case::logs_clear_flag_after("logs", &["clear", "--format=json"], "clear")]
#[case::logs_clear_flag_before("logs", &["--format=json", "clear"], "clear")]
#[case::hints_clear_flag_after("hints", &["clear", "--format=json"], "clear")]
#[case::hints_clear_flag_before("hints", &["--format=json", "clear"], "clear")]
#[case::marker_set_flag_after("marker", &["set", "foo", "--format=json"], "set")]
#[case::marker_set_flag_before("marker", &["--format=json", "set", "foo"], "set")]
#[case::marker_clear_flag_after("marker", &["clear", "--format=json"], "clear")]
#[case::marker_clear_flag_before("marker", &["--format=json", "clear"], "clear")]
#[case::ci_status_clear_flag_after("ci-status", &["clear", "--format=json"], "clear")]
#[case::ci_status_clear_flag_before("ci-status", &["--format=json", "clear"], "clear")]
fn test_format_rejected_on_write_actions(
    repo: TestRepo,
    #[case] key: &str,
    #[case] args: &[&str],
    #[case] action: &str,
) {
    let output = wt_state_raw_cmd(&repo, key, args).output().unwrap();
    assert!(
        !output.status.success(),
        "expected failure for {key} {args:?}"
    );
    assert_eq!(output.status.code(), Some(2));
    // Tolerate the ANSI `invalid` styling clap wraps around `--format <FORMAT>`
    // and the action name by checking the fixed substrings between/around them.
    let stderr = String::from_utf8_lossy(&output.stderr);
    for needle in ["--format <FORMAT>", "cannot be used with", action] {
        assert!(
            stderr.contains(needle),
            "stderr missing {needle:?}: {stderr}"
        );
    }
}
