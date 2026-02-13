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
        .output()
        .unwrap();
    repo.git_command()
        .args(["commit", "-m", "Feature commit 1"])
        .current_dir(&feature_path)
        .output()
        .unwrap();

    std::fs::write(feature_path.join("feature2.txt"), "more content").unwrap();
    repo.git_command()
        .args(["add", "."])
        .current_dir(&feature_path)
        .output()
        .unwrap();
    repo.git_command()
        .args(["commit", "-m", "Feature commit 2"])
        .current_dir(&feature_path)
        .output()
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
    assert_snapshot!(output, @"[0m main  [36m?[0m[2m^[22m[2m|[22m");
}

#[rstest]
fn test_statusline_commits_ahead(mut repo: TestRepo) {
    add_commits_ahead(&mut repo);
    // Run from the feature worktree to see commits ahead
    let feature_path = repo.worktree_path("feature");
    let output = run_statusline_from_dir(&repo, &[], None, feature_path);
    assert_snapshot!(output, @"[0m feature  [2m↑[22m  [32m↑2[0m  ^[32m+2");
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
    path.display().to_string().replace('\\', "\\\\")
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
        assert_snapshot!(output, @" [PATH]  main  [36m?[39m[90m^[39m[90m|[39m  | Opus");
    });
}

#[rstest]
fn test_statusline_claude_code_minimal(repo: TestRepo) {
    let escaped_path = escape_path_for_json(repo.root_path());
    let json = format!(r#"{{"workspace": {{"current_dir": "{escaped_path}"}}}}"#,);

    let output = run_statusline(&repo, &["--format=claude-code"], Some(&json));
    claude_code_snapshot_settings().bind(|| {
        assert_snapshot!(output, @" [PATH]  main  [90m^[39m[90m|[39m");
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
        assert_snapshot!(output, @" [PATH]  main  [90m^[39m[90m|[39m  | Haiku");
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
        assert_snapshot!(output, @" [PATH]  main  [90m^[39m[90m|[39m  | Opus  🌕 42%");
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
        assert_snapshot!(output, @" [PATH]  main  [90m^[39m[90m|[39m  | Opus  🌕 5%");
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
        assert_snapshot!(output, @" [PATH]  main  [90m^[39m[90m|[39m  | Opus  🌑 98%");
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
        // No gauge symbol (○◔◑◕●) should appear
        assert!(
            !output.contains('○')
                && !output.contains('◔')
                && !output.contains('◑')
                && !output.contains('◕')
                && !output.contains('●'),
            "No gauge should appear when context_window is missing: {output}"
        );
        assert_snapshot!(output, @" [PATH]  main  [90m^[39m[90m|[39m  | Opus");
    });
}

// --- Directive Mode Tests ---
// Note: With the new WORKTRUNK_DIRECTIVE_FILE architecture, data output (like statusline)
// still goes to stdout. The directive file is only used for shell directives like
// `cd '/path'`. So this test is no longer needed - statusline behavior is the same
// regardless of whether WORKTRUNK_DIRECTIVE_FILE is set.

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
    repo.git_command()
        .args(["branch", "other"])
        .output()
        .unwrap();
    let checkout_output = repo
        .git_command()
        .args(["checkout", "other"])
        .current_dir(&feature_path)
        .output()
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
        .output()
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
    assert_snapshot!(output, @"[0m main  [36m?[0m[2m^[22m[2m|[22m  http://main.localhost:3000");
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

    // commit object should exist with sha
    assert!(item["commit"]["sha"].is_string());
    assert!(item["commit"]["short_sha"].is_string());
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
        .output()
        .unwrap();
    repo.git_command()
        .args(["commit", "-m", "Feature commit"])
        .current_dir(&feature_path)
        .output()
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
