//! Snapshot tests for `wt list statusline` command.
//!
//! Tests the statusline output for shell prompts and Claude Code integration.

use crate::common::{TestRepo, repo, wt_command};
use insta::assert_snapshot;
use rstest::rstest;
use serde_json::Value;
use std::io::Write;
use std::process::Stdio;

/// Run statusline command with optional JSON piped to stdin
fn run_statusline_from_dir(
    repo: &TestRepo,
    args: &[&str],
    stdin_json: Option<&str>,
    cwd: &std::path::Path,
) -> String {
    let mut cmd = wt_command();
    cmd.current_dir(cwd);
    cmd.args(["list", "statusline"]);
    cmd.args(args);

    // Apply repo's git environment
    repo.configure_wt_cmd(&mut cmd);

    if stdin_json.is_some() {
        cmd.stdin(Stdio::piped());
    }
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().expect("failed to spawn command");

    if let Some(json) = stdin_json {
        // Take ownership of stdin so we can drop it after writing
        let mut stdin = child.stdin.take().expect("failed to get stdin");
        stdin
            .write_all(json.as_bytes())
            .expect("failed to write stdin");
        // Explicitly close stdin by dropping it - this signals EOF to the child process.
        // On Windows, not closing stdin can cause the child to hang waiting for more input.
        drop(stdin);
    }

    let output = child.wait_with_output().expect("failed to wait for output");

    // Statusline outputs to stdout in interactive mode
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Return whichever has content (stdout for interactive)
    if !stdout.is_empty() {
        stdout.to_string()
    } else {
        stderr.to_string()
    }
}

fn run_statusline(repo: &TestRepo, args: &[&str], stdin_json: Option<&str>) -> String {
    run_statusline_from_dir(repo, args, stdin_json, repo.root_path())
}

// --- Test Setup Helpers ---

fn add_uncommitted_changes(repo: &TestRepo) {
    // Create uncommitted changes
    std::fs::write(repo.root_path().join("modified.txt"), "modified content").unwrap();
}

fn add_commits_ahead(repo: &mut TestRepo) {
    // Create feature branch with commits ahead
    let feature_path = repo.add_worktree("feature");

    // Add commits in the feature worktree
    std::fs::write(feature_path.join("feature.txt"), "feature content").unwrap();
    repo.git_command()
        .args(["add", "."])
        .current_dir(&feature_path)
        .run()
        .unwrap();
    repo.git_command()
        .args(["commit", "-m", "Feature commit 1"])
        .current_dir(&feature_path)
        .run()
        .unwrap();

    std::fs::write(feature_path.join("feature2.txt"), "more content").unwrap();
    repo.git_command()
        .args(["add", "."])
        .current_dir(&feature_path)
        .run()
        .unwrap();
    repo.git_command()
        .args(["commit", "-m", "Feature commit 2"])
        .current_dir(&feature_path)
        .run()
        .unwrap();
}

// --- Basic Tests ---

#[rstest]
fn test_statusline_basic(repo: TestRepo) {
    let output = run_statusline(&repo, &[], None);
    assert_snapshot!(output, @"[0m main  [2m^[22m[2m|[22m");
}

#[rstest]
fn test_statusline_with_changes(repo: TestRepo) {
    add_uncommitted_changes(&repo);
    let output = run_statusline(&repo, &[], None);
    assert_snapshot!(output, @"[0m main  [36m?[0m[2m^[22m[2m|[22m  @[32m+1");
}

#[rstest]
fn test_statusline_commits_ahead(mut repo: TestRepo) {
    add_commits_ahead(&mut repo);
    // Run from the feature worktree to see commits ahead
    let feature_path = repo.worktree_path("feature");
    let output = run_statusline_from_dir(&repo, &[], None, feature_path);
    assert_snapshot!(output, @"[0m feature  [2m↑[22m  [32m↑2[0m  ^[32m+2");
}

#[rstest]
fn test_statusline_does_not_run_repo_wide_ahead_behind_scan(repo: TestRepo) {
    for i in 0..12 {
        repo.run_git(&["branch", &format!("unused-{i}")]);
    }

    let output = repo
        .wt_command()
        .args(["list", "statusline"])
        .env("RUST_LOG", "debug")
        .output()
        .expect("statusline should run");
    assert!(
        output.status.success(),
        "statusline failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("for-each-ref"),
        "debug trace should include ref scans so this assertion is meaningful:\n{stderr}"
    );
    assert!(
        !stderr.contains("%(ahead-behind:"),
        "single-row statusline must not run repo-wide ahead/behind scans:\n{stderr}"
    );
}

// --- Claude Code Mode Tests ---

/// Create snapshot settings that normalize path output for statusline tests.
///
/// The statusline output varies by platform:
/// - Linux: Raw path is filtered by auto-bound settings to `_REPO_`
/// - macOS: Fish-style abbreviation (e.g., `/p/v/f/.../repo`) bypasses auto-bound filters
///
/// This function normalizes both cases to a consistent `[PATH]` placeholder.
fn claude_code_snapshot_settings() -> insta::Settings {
    let mut settings = insta::Settings::clone_current();
    // Normalize _REPO_ (from auto-bound filters on Linux) to [PATH]
    settings.add_filter(r"_REPO_", "[PATH]");
    // Normalize fish-abbreviated paths (on macOS) to [PATH]
    settings.add_filter(r"/[a-zA-Z0-9/._-]+/repo", "[PATH]");
    // Strip leading ANSI reset code if present (output starts with [0m)
    settings.add_filter(r"^\x1b\[0m ", "");
    settings
}

/// Escape a path for use in JSON strings.
/// On Windows, backslashes must be escaped as double backslashes.
fn escape_path_for_json(path: &std::path::Path) -> String {
    path.display().to_string().replace('\\', r"\\")
}

#[rstest]
fn test_statusline_claude_code_full_context(repo: TestRepo) {
    add_uncommitted_changes(&repo);

    let escaped_path = escape_path_for_json(repo.root_path());
    let json = format!(
        r#"{{
            "hook_event_name": "Status",
            "session_id": "test-session",
            "model": {{
                "id": "claude-opus-4-1",
                "display_name": "Opus"
            }},
            "workspace": {{
                "current_dir": "{escaped_path}",
                "project_dir": "{escaped_path}"
            }},
            "version": "1.0.80"
        }}"#,
    );

    let output = run_statusline(&repo, &["--format=claude-code"], Some(&json));
    claude_code_snapshot_settings().bind(|| {
        assert_snapshot!(output, @"[PATH]  main  [36m?[0m[2m^[22m[2m|[22m  @[32m+1[0m  Opus");
    });
}

#[rstest]
fn test_statusline_claude_code_minimal(repo: TestRepo) {
    let escaped_path = escape_path_for_json(repo.root_path());
    let json = format!(r#"{{"workspace": {{"current_dir": "{escaped_path}"}}}}"#,);

    let output = run_statusline(&repo, &["--format=claude-code"], Some(&json));
    claude_code_snapshot_settings().bind(|| {
        assert_snapshot!(output, @"[PATH]  main  [2m^[22m[2m|[22m");
    });
}

#[rstest]
fn test_statusline_claude_code_with_model(repo: TestRepo) {
    let escaped_path = escape_path_for_json(repo.root_path());
    let json = format!(
        r#"{{
            "workspace": {{"current_dir": "{escaped_path}"}},
            "model": {{"display_name": "Haiku"}}
        }}"#,
    );

    let output = run_statusline(&repo, &["--format=claude-code"], Some(&json));
    claude_code_snapshot_settings().bind(|| {
        assert_snapshot!(output, @"[PATH]  main  [2m^[22m[2m|[22m  Haiku");
    });
}

// --- Context Gauge Tests ---

#[rstest]
fn test_statusline_claude_code_with_context_gauge(repo: TestRepo) {
    let escaped_path = escape_path_for_json(repo.root_path());
    let json = format!(
        r#"{{
            "workspace": {{"current_dir": "{escaped_path}"}},
            "model": {{"display_name": "Opus"}},
            "context_window": {{"used_percentage": 42}}
        }}"#,
    );

    let output = run_statusline(&repo, &["--format=claude-code"], Some(&json));
    claude_code_snapshot_settings().bind(|| {
        assert_snapshot!(output, @"[PATH]  main  [2m^[22m[2m|[22m  Opus  🌕42%");
    });
}

#[rstest]
fn test_statusline_claude_code_context_gauge_low(repo: TestRepo) {
    let escaped_path = escape_path_for_json(repo.root_path());
    let json = format!(
        r#"{{
            "workspace": {{"current_dir": "{escaped_path}"}},
            "model": {{"display_name": "Opus"}},
            "context_window": {{"used_percentage": 5}}
        }}"#,
    );

    let output = run_statusline(&repo, &["--format=claude-code"], Some(&json));
    claude_code_snapshot_settings().bind(|| {
        assert_snapshot!(output, @"[PATH]  main  [2m^[22m[2m|[22m  Opus  🌕5%");
    });
}

#[rstest]
fn test_statusline_claude_code_context_gauge_high(repo: TestRepo) {
    let escaped_path = escape_path_for_json(repo.root_path());
    let json = format!(
        r#"{{
            "workspace": {{"current_dir": "{escaped_path}"}},
            "model": {{"display_name": "Opus"}},
            "context_window": {{"used_percentage": 98}}
        }}"#,
    );

    let output = run_statusline(&repo, &["--format=claude-code"], Some(&json));
    claude_code_snapshot_settings().bind(|| {
        assert_snapshot!(output, @"[PATH]  main  [2m^[22m[2m|[22m  Opus  🌑98%");
    });
}

#[rstest]
fn test_statusline_claude_code_missing_context_window(repo: TestRepo) {
    // When context_window is missing, no gauge should be displayed
    let escaped_path = escape_path_for_json(repo.root_path());
    let json = format!(
        r#"{{
            "workspace": {{"current_dir": "{escaped_path}"}},
            "model": {{"display_name": "Opus"}}
        }}"#,
    );

    let output = run_statusline(&repo, &["--format=claude-code"], Some(&json));
    claude_code_snapshot_settings().bind(|| {
        assert_snapshot!(output, @"[PATH]  main  [2m^[22m[2m|[22m  Opus");
    });
}

// --- Directive Mode Tests ---
// Note: With the split directive file architecture, data output (like statusline)
// still goes to stdout. The directive files are only used for cd paths and exec
// commands. So this test is no longer needed - statusline behavior is the same
// regardless of whether directive env vars are set.

// --- Branch Display Tests ---

///
/// Git updates worktree metadata (`branch` field in `git worktree list`) when
/// you checkout a different branch. This test verifies that statusline correctly
/// shows the new branch name after such a checkout.
#[rstest]
fn test_statusline_reflects_checked_out_branch(mut repo: TestRepo) {
    // Create a feature worktree
    let feature_path = repo.add_worktree("feature");

    // Verify statusline shows "feature" initially
    let output = run_statusline_from_dir(&repo, &[], None, &feature_path);
    assert!(
        output.contains("feature"),
        "statusline should show 'feature' for feature worktree, got: {output}"
    );

    // Create and checkout a different branch "other" in the feature worktree
    repo.git_command().args(["branch", "other"]).run().unwrap();
    let checkout_output = repo
        .git_command()
        .args(["checkout", "other"])
        .current_dir(&feature_path)
        .run()
        .unwrap();
    assert!(
        checkout_output.status.success(),
        "checkout should succeed: {}",
        String::from_utf8_lossy(&checkout_output.stderr)
    );

    // Verify statusline now shows "other"
    let output = run_statusline_from_dir(&repo, &[], None, &feature_path);
    assert!(
        output.contains("other"),
        "statusline should show 'other' after checkout, got: {output}"
    );
    assert!(
        !output.contains("feature"),
        "statusline should not show 'feature' after checkout, got: {output}"
    );
}

#[rstest]
fn test_statusline_detached_head(mut repo: TestRepo) {
    // Create a feature worktree
    let feature_path = repo.add_worktree("feature");

    // Detach HEAD
    repo.git_command()
        .args(["checkout", "--detach"])
        .current_dir(&feature_path)
        .run()
        .unwrap();

    // Verify statusline shows HEAD (not "feature")
    let output = run_statusline_from_dir(&repo, &[], None, &feature_path);
    // In detached state, we show "HEAD" as the branch name
    assert!(
        output.contains("HEAD") || !output.contains("feature"),
        "statusline should not show 'feature' in detached HEAD, got: {output}"
    );
}

// --- URL Display Tests ---

#[rstest]
fn test_statusline_with_url(repo: TestRepo) {
    // Configure URL template with simple branch variable (no hash_port for deterministic output)
    repo.write_project_config(
        r#"[list]
url = "http://{{ branch }}.localhost:3000"
"#,
    );

    let output = run_statusline(&repo, &[], None);
    // Shows `?` because writing project config creates uncommitted file
    assert_snapshot!(output, @"[0m main  [36m?[0m[2m^[22m[2m|[22m  @[32m+2[0m  http://main.localhost:3000");
}

#[rstest]
fn test_statusline_url_in_feature_worktree(mut repo: TestRepo) {
    // Configure URL template with simple branch variable
    repo.write_project_config(
        r#"[list]
url = "http://{{ branch }}.localhost:3000"
"#,
    );

    // Commit the project config so it's visible in worktrees
    repo.run_git(&["add", ".config/wt.toml"]);
    repo.run_git(&["commit", "-m", "Add project config"]);

    // Create feature worktree
    let feature_path = repo.add_worktree("feature");

    // Run statusline from feature worktree
    let output = run_statusline_from_dir(&repo, &[], None, &feature_path);
    assert_snapshot!(output, @"[0m feature  [2m_[22m  http://feature.localhost:3000");
}

// --- JSON Format Tests ---

#[rstest]
fn test_statusline_json_basic(repo: TestRepo) {
    let output = run_statusline(&repo, &["--format=json"], None);
    let parsed: Value = serde_json::from_str(&output).expect("should be valid JSON");

    // Should be an array with one item
    let items = parsed.as_array().expect("should be an array");
    assert_eq!(
        items.len(),
        1,
        "should have exactly one item (current worktree)"
    );

    let item = &items[0];

    // Check essential fields
    assert_eq!(item["branch"], "main");
    assert_eq!(item["kind"], "worktree");
    assert!(item["is_current"].as_bool().unwrap());
    assert!(item["is_main"].as_bool().unwrap());

    // commit object should exist with sha, message, and non-zero timestamp
    assert!(item["commit"]["sha"].is_string());
    assert!(item["commit"]["short_sha"].is_string());
    assert!(
        !item["commit"]["message"].as_str().unwrap().is_empty(),
        "commit.message should be populated from git log"
    );
    assert!(
        item["commit"]["timestamp"].as_i64().unwrap() > 0,
        "commit.timestamp should be populated from git log"
    );
}

#[rstest]
fn test_statusline_json_with_changes(repo: TestRepo) {
    // Create uncommitted changes
    std::fs::write(repo.root_path().join("modified.txt"), "modified content").unwrap();

    let output = run_statusline(&repo, &["--format=json"], None);
    let parsed: Value = serde_json::from_str(&output).expect("should be valid JSON");

    let item = &parsed[0];
    assert_eq!(item["branch"], "main");

    // Should have working_tree status
    let working_tree = &item["working_tree"];
    assert!(
        working_tree["untracked"].as_bool().unwrap(),
        "should show untracked file"
    );
}

#[rstest]
fn test_statusline_json_feature_branch(mut repo: TestRepo) {
    // Create feature worktree with commits
    let feature_path = repo.add_worktree("feature");

    std::fs::write(feature_path.join("feature.txt"), "content").unwrap();
    repo.git_command()
        .args(["add", "."])
        .current_dir(&feature_path)
        .run()
        .unwrap();
    repo.git_command()
        .args(["commit", "-m", "Feature commit"])
        .current_dir(&feature_path)
        .run()
        .unwrap();

    let output = run_statusline_from_dir(&repo, &["--format=json"], None, &feature_path);
    let parsed: Value = serde_json::from_str(&output).expect("should be valid JSON");

    let item = &parsed[0];
    assert_eq!(item["branch"], "feature");
    assert!(item["is_current"].as_bool().unwrap());
    assert!(!item["is_main"].as_bool().unwrap());

    // Should have ahead/behind counts (commits ahead of main)
    assert!(
        item["main"]["ahead"].as_u64().unwrap() >= 1,
        "should be ahead of main"
    );
}

#[rstest]
fn test_statusline_json_ignores_claude_code(repo: TestRepo) {
    // When --format=json is used, --claude-code should be ignored
    let escaped_path = escape_path_for_json(repo.root_path());
    let json = format!(
        r#"{{
            "workspace": {{"current_dir": "{escaped_path}"}},
            "model": {{"display_name": "Opus"}}
        }}"#,
    );

    let output = run_statusline(&repo, &["--format=json", "--claude-code"], Some(&json));
    let parsed: Value = serde_json::from_str(&output).expect("should be valid JSON");

    // Should still produce JSON output (not statusline format)
    assert!(parsed.is_array(), "should produce JSON array output");
    let item = &parsed[0];
    assert_eq!(item["branch"], "main");
}

// --- Deprecated flags ---

#[rstest]
fn test_statusline_claude_code_flag_deprecated(repo: TestRepo) {
    // The hidden `--claude-code` flag still maps to `--format=claude-code`,
    // but emits a deprecation warning per invocation.
    let escaped_path = escape_path_for_json(repo.root_path());
    let json = format!(r#"{{"workspace": {{"current_dir": "{escaped_path}"}}}}"#);

    let mut cmd = wt_command();
    cmd.current_dir(repo.root_path());
    cmd.args(["list", "statusline", "--claude-code"]);
    repo.configure_wt_cmd(&mut cmd);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().expect("spawn");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(json.as_bytes())
        .unwrap();
    let output = child.wait_with_output().expect("wait");

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--claude-code is deprecated"),
        "expected deprecation warning in stderr, got: {stderr}"
    );
    assert!(
        stderr.contains("--format=claude-code"),
        "expected migration hint in stderr, got: {stderr}"
    );
}

/// Tests that statusline correctly identifies nested worktrees.
///
/// When worktrees are placed inside other worktrees (e.g., `.worktrees/` layout),
/// the detection must use git rev-parse --show-toplevel rather than prefix matching,
/// which would incorrectly match the parent worktree.
///
/// Regression test for: prefix matching with starts_with would incorrectly identify
/// the main worktree when running from a nested worktree.
#[rstest]
fn test_statusline_nested_worktree(mut repo: TestRepo) {
    // Create a worktree nested inside the main repo (like .worktrees/ layout)
    let nested_path = repo.root_path().join(".worktrees").join("feature");
    let nested_worktree = repo.add_worktree_at_path("feature", &nested_path);

    // Run statusline from inside the nested worktree - should show "feature", not "main"
    let output = repo
        .wt_command()
        .current_dir(&nested_worktree)
        .args(["list", "statusline"])
        .output()
        .expect("statusline should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("feature"),
        "Nested worktree should show 'feature' branch, got: {stdout}"
    );
    assert!(
        !stdout.contains("main"),
        "Nested worktree should NOT show 'main' branch, got: {stdout}"
    );
}

/// Tests that JSON output correctly identifies nested worktrees.
#[rstest]
fn test_statusline_json_nested_worktree(mut repo: TestRepo) {
    // Create a worktree nested inside the main repo
    let nested_path = repo.root_path().join(".worktrees").join("feature");
    let nested_worktree = repo.add_worktree_at_path("feature", &nested_path);

    // Run statusline --format=json from inside the nested worktree
    let output = repo
        .wt_command()
        .current_dir(&nested_worktree)
        .args(["list", "statusline", "--format=json"])
        .output()
        .expect("statusline should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: Value = serde_json::from_str(&stdout).expect("should be valid JSON");

    assert!(parsed.is_array(), "should produce JSON array");
    let items = parsed.as_array().unwrap();
    assert_eq!(items.len(), 1, "should have exactly one item");
    assert_eq!(
        items[0]["branch"], "feature",
        "Nested worktree should report 'feature' branch, not parent"
    );
}

// --- Rate-limit segment snapshots ---
//
// These tests pin the "now" used by the rate-limit predictor (via
// `WORKTRUNK_TEST_EPOCH`), force `TZ=UTC` so the deadline string is
// deterministic across machines and timezones, and pin `LC_TIME=en_US.UTF-8`
// so the clock segment renders 12h (`3pm`) — matching the docs example and
// the US/macOS default. The 24h path has its own test that overrides
// `LC_TIME` to a non-US locale.
//
// All cases use Thursday 2025-01-02 13:00 UTC as the reference "now", and
// resets_at values relative to it.

use crate::common::TEST_EPOCH;

const TEST_NOW_THU_1PM: u64 = TEST_EPOCH + 13 * 3600;

/// Run statusline with a pinned "now" and timezone; optional COLUMNS override.
///
/// Defaults to `LC_TIME=en_US.UTF-8` (12h clock); pass a different locale
/// string to exercise the 24h path.
fn run_statusline_at_time(
    repo: &TestRepo,
    stdin_json: &str,
    now_epoch: u64,
    columns: Option<u32>,
) -> String {
    run_statusline_with_locale(repo, stdin_json, now_epoch, columns, "en_US.UTF-8")
}

fn run_statusline_with_locale(
    repo: &TestRepo,
    stdin_json: &str,
    now_epoch: u64,
    columns: Option<u32>,
    lc_time: &str,
) -> String {
    let mut cmd = wt_command();
    cmd.current_dir(repo.root_path());
    cmd.args(["list", "statusline", "--format=claude-code"]);
    repo.configure_wt_cmd(&mut cmd);
    cmd.env("WORKTRUNK_TEST_EPOCH", now_epoch.to_string());
    cmd.env("TZ", "UTC");
    // The test harness pins `LC_ALL=C` globally for determinism; clear it
    // so `LC_TIME` actually takes effect (POSIX precedence: LC_ALL beats
    // LC_TIME). Then set `LC_TIME` per-test.
    cmd.env_remove("LC_ALL");
    cmd.env("LC_TIME", lc_time);
    if let Some(c) = columns {
        cmd.env("COLUMNS", c.to_string());
    }
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().expect("failed to spawn command");
    let mut stdin = child.stdin.take().expect("failed to get stdin");
    stdin
        .write_all(stdin_json.as_bytes())
        .expect("failed to write stdin");
    drop(stdin);

    let output = child.wait_with_output().expect("failed to wait for output");
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// Build a minimal Claude Code JSON with the given rate-limit windows.
///
/// `windows`: `[(key, used_percentage, seconds_until_reset_from_now)]`.
fn build_claude_code_json(
    repo_path: &std::path::Path,
    model: Option<&str>,
    context_pct: Option<f64>,
    now_epoch: u64,
    windows: &[(&str, f64, i64)],
) -> String {
    use std::fmt::Write;
    let escaped = escape_path_for_json(repo_path);
    let mut json = format!(r#"{{"workspace":{{"current_dir":"{escaped}"}}"#);
    if let Some(m) = model {
        write!(json, r#","model":{{"display_name":"{m}"}}"#).unwrap();
    }
    if let Some(p) = context_pct {
        write!(json, r#","context_window":{{"used_percentage":{p}}}"#).unwrap();
    }
    if !windows.is_empty() {
        json.push_str(r#","rate_limits":{"#);
        for (i, (key, used, secs_to_reset)) in windows.iter().enumerate() {
            if i > 0 {
                json.push(',');
            }
            let resets_at = now_epoch as i64 + secs_to_reset;
            write!(
                json,
                r#""{key}":{{"used_percentage":{used},"resets_at":{resets_at}}}"#
            )
            .unwrap();
        }
        json.push('}');
    }
    json.push('}');
    json
}

#[rstest]
fn test_statusline_rate_limit_hidden_when_safe(repo: TestRepo) {
    // Both windows well under threshold → segment absent.
    let json = build_claude_code_json(
        repo.root_path(),
        Some("Opus"),
        Some(42.0),
        TEST_NOW_THU_1PM,
        &[
            ("five_hour", 30.0, 2 * 3600 + 1800), // 50% elapsed, 30% used
            ("seven_day", 20.0, 4 * 86400),       // ~43% elapsed, 20% used
        ],
    );
    let out = run_statusline_at_time(&repo, &json, TEST_NOW_THU_1PM, None);
    claude_code_snapshot_settings().bind(|| {
        assert_snapshot!("rate_limit_hidden_when_safe", out);
    });
}

#[rstest]
fn test_statusline_rate_limit_5h_clock_time(repo: TestRepo) {
    // 5h burning mid-window, reset in 2h → 3pm.
    let json = build_claude_code_json(
        repo.root_path(),
        Some("Opus"),
        Some(42.0),
        TEST_NOW_THU_1PM,
        &[("five_hour", 80.0, 2 * 3600)],
    );
    let out = run_statusline_at_time(&repo, &json, TEST_NOW_THU_1PM, None);
    claude_code_snapshot_settings().bind(|| {
        assert_snapshot!("rate_limit_5h_clock_time", out);
    });
}

#[rstest]
fn test_statusline_rate_limit_5h_with_minutes(repo: TestRepo) {
    // Reset 30 minutes from now → 1:30pm — exercises the `:mm` time spec.
    let json = build_claude_code_json(
        repo.root_path(),
        Some("Opus"),
        Some(42.0),
        TEST_NOW_THU_1PM,
        &[("five_hour", 95.0, 1800)],
    );
    let out = run_statusline_at_time(&repo, &json, TEST_NOW_THU_1PM, None);
    claude_code_snapshot_settings().bind(|| {
        assert_snapshot!("rate_limit_5h_with_minutes", out);
    });
}

#[rstest]
fn test_statusline_rate_limit_5h_at_limit(repo: TestRepo) {
    // 100% used — the throttled edge.
    let json = build_claude_code_json(
        repo.root_path(),
        Some("Opus"),
        Some(42.0),
        TEST_NOW_THU_1PM,
        &[("five_hour", 100.0, 2 * 3600)],
    );
    let out = run_statusline_at_time(&repo, &json, TEST_NOW_THU_1PM, None);
    claude_code_snapshot_settings().bind(|| {
        assert_snapshot!("rate_limit_5h_at_limit", out);
    });
}

#[rstest]
fn test_statusline_rate_limit_7d_with_day_and_time(repo: TestRepo) {
    // 7d binding, reset 4d 2h from Thu 1pm → Mon 3pm.
    let json = build_claude_code_json(
        repo.root_path(),
        Some("Opus"),
        Some(42.0),
        TEST_NOW_THU_1PM,
        &[
            ("five_hour", 15.0, 4 * 3600), // safe
            ("seven_day", 80.0, 4 * 86400 + 2 * 3600),
        ],
    );
    let out = run_statusline_at_time(&repo, &json, TEST_NOW_THU_1PM, None);
    claude_code_snapshot_settings().bind(|| {
        assert_snapshot!("rate_limit_7d_with_day_and_time", out);
    });
}

#[rstest]
fn test_statusline_rate_limit_both_picks_worse(repo: TestRepo) {
    // Both windows flagged; 7d projects worse, so 7d is the segment shown.
    let json = build_claude_code_json(
        repo.root_path(),
        Some("Opus"),
        Some(42.0),
        TEST_NOW_THU_1PM,
        &[
            ("five_hour", 85.0, 90 * 60), // 70% elapsed, 85% used → visible
            ("seven_day", 80.0, 4 * 86400 + 2 * 3600), // ~40% elapsed, 80% used → worse
        ],
    );
    let out = run_statusline_at_time(&repo, &json, TEST_NOW_THU_1PM, None);
    claude_code_snapshot_settings().bind(|| {
        assert_snapshot!("rate_limit_both_picks_worse", out);
    });
}

#[rstest]
fn test_statusline_rate_limit_with_dirty_worktree_and_context(repo: TestRepo) {
    // Uncommitted change + 95% context + 5h binding — checks segment
    // composition with other state.
    add_uncommitted_changes(&repo);
    let json = build_claude_code_json(
        repo.root_path(),
        Some("Opus"),
        Some(95.0),
        TEST_NOW_THU_1PM,
        &[("five_hour", 80.0, 2 * 3600)],
    );
    let out = run_statusline_at_time(&repo, &json, TEST_NOW_THU_1PM, None);
    claude_code_snapshot_settings().bind(|| {
        assert_snapshot!("rate_limit_with_dirty_worktree_and_context", out);
    });
}

#[rstest]
fn test_statusline_rate_limit_5h_24h_locale(repo: TestRepo) {
    // Same input as `5h_clock_time` but `LC_TIME=en_GB.UTF-8` → 24h clock.
    // 12h `10am–3pm` becomes 24h `10:00–15:00`. Weekday in `Mon–Mon 15:00`
    // is still English (out of scope for the clock-format follow-up).
    let json = build_claude_code_json(
        repo.root_path(),
        Some("Opus"),
        Some(42.0),
        TEST_NOW_THU_1PM,
        &[("five_hour", 80.0, 2 * 3600)],
    );
    let out = run_statusline_with_locale(&repo, &json, TEST_NOW_THU_1PM, None, "en_GB.UTF-8");
    claude_code_snapshot_settings().bind(|| {
        assert_snapshot!("rate_limit_5h_24h_locale", out);
    });
}

#[rstest]
fn test_statusline_rate_limit_drops_at_narrow_width(repo: TestRepo) {
    // Same input as `5h_clock_time` but COLUMNS=80 — the rate-limit
    // segment is the lowest-priority Claude-Code segment, so it drops first.
    let json = build_claude_code_json(
        repo.root_path(),
        Some("Opus"),
        Some(42.0),
        TEST_NOW_THU_1PM,
        &[("five_hour", 80.0, 2 * 3600)],
    );
    // COLUMNS=40 is narrower than the rendered line with rate-limit, so
    // the lowest-priority segments drop. The full line is ~60 chars wide
    // even on the short test repo path.
    let out = run_statusline_at_time(&repo, &json, TEST_NOW_THU_1PM, Some(40));
    claude_code_snapshot_settings().bind(|| {
        assert_snapshot!("rate_limit_drops_at_narrow_width", out);
    });
}
