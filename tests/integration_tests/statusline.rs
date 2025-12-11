//! Snapshot tests for `wt list statusline` command.
//!
//! Tests the statusline output for shell prompts and Claude Code integration.

use crate::common::{TestRepo, wt_command};
use insta::assert_snapshot;
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
    repo.clean_cli_env(&mut cmd);

    if stdin_json.is_some() {
        cmd.stdin(Stdio::piped());
    }
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().expect("failed to spawn command");

    if let Some(json) = stdin_json {
        let stdin = child.stdin.as_mut().expect("failed to get stdin");
        stdin
            .write_all(json.as_bytes())
            .expect("failed to write stdin");
    }

    let output = child.wait_with_output().expect("failed to wait for output");

    // Statusline outputs to stdout in interactive mode, stderr in directive mode
    // For tests without --internal, we capture stdout
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Return whichever has content (stdout for interactive, stderr for --internal)
    if !stdout.is_empty() {
        stdout.to_string()
    } else {
        stderr.to_string()
    }
}

fn run_statusline(repo: &TestRepo, args: &[&str], stdin_json: Option<&str>) -> String {
    run_statusline_from_dir(repo, args, stdin_json, repo.root_path())
}

// --- Test Fixtures ---

fn setup_basic_repo() -> TestRepo {
    TestRepo::new()
}

fn setup_repo_with_changes() -> TestRepo {
    let repo = TestRepo::new();

    // Create uncommitted changes
    std::fs::write(repo.root_path().join("modified.txt"), "modified content").unwrap();

    repo
}

fn setup_repo_with_commits_ahead() -> TestRepo {
    let mut repo = TestRepo::new();

    // Create feature branch with commits ahead
    let feature_path = repo.add_worktree("feature");

    // Add commits in the feature worktree
    std::fs::write(feature_path.join("feature.txt"), "feature content").unwrap();
    repo.git_command(&["add", "."])
        .current_dir(&feature_path)
        .output()
        .unwrap();
    repo.git_command(&["commit", "-m", "Feature commit 1"])
        .current_dir(&feature_path)
        .output()
        .unwrap();

    std::fs::write(feature_path.join("feature2.txt"), "more content").unwrap();
    repo.git_command(&["add", "."])
        .current_dir(&feature_path)
        .output()
        .unwrap();
    repo.git_command(&["commit", "-m", "Feature commit 2"])
        .current_dir(&feature_path)
        .output()
        .unwrap();

    repo
}

// --- Basic Tests ---

#[test]
fn test_statusline_basic() {
    let repo = setup_basic_repo();
    let output = run_statusline(&repo, &[], None);
    assert_snapshot!(output, @"main  [2m^[22m");
}

#[test]
fn test_statusline_with_changes() {
    let repo = setup_repo_with_changes();
    let output = run_statusline(&repo, &[], None);
    assert_snapshot!(output, @"main  [36m?[39m[2m^[22m");
}

#[test]
fn test_statusline_commits_ahead() {
    let repo = setup_repo_with_commits_ahead();
    // Run from the feature worktree to see commits ahead
    let feature_path = repo.worktree_path("feature");
    let output = run_statusline_from_dir(&repo, &[], None, feature_path);
    assert_snapshot!(output, @"feature  [2m↑[22m  [32m↑2[0m  ^[32m+2[0m");
}

// --- Claude Code Mode Tests ---

/// Create snapshot settings that filter the dynamic temp path
fn claude_code_snapshot_settings(repo: &TestRepo) -> insta::Settings {
    let mut settings = insta::Settings::clone_current();
    // The path gets fish-style abbreviated, so filter the abbreviated form
    // e.g., /private/var/folders/.../repo -> /p/v/f/.../repo
    // We replace everything up to "repo" with [PATH]
    settings.add_filter(r"(?m)^.*repo", "[PATH]");
    // Also filter the raw path in case it appears
    settings.add_filter(
        &regex::escape(&repo.root_path().display().to_string()),
        "[PATH]",
    );
    settings
}

/// Skipped on Windows: JSON contains paths that need escaping on Windows.
#[test]
#[cfg_attr(windows, ignore)]
fn test_statusline_claude_code_full_context() {
    let repo = setup_repo_with_changes();

    let json = format!(
        r#"{{
            "hook_event_name": "Status",
            "session_id": "test-session",
            "model": {{
                "id": "claude-opus-4-1",
                "display_name": "Opus"
            }},
            "workspace": {{
                "current_dir": "{}",
                "project_dir": "{}"
            }},
            "version": "1.0.80"
        }}"#,
        repo.root_path().display(),
        repo.root_path().display()
    );

    let output = run_statusline(&repo, &["--claude-code"], Some(&json));
    claude_code_snapshot_settings(&repo).bind(|| {
        assert_snapshot!(output, @"[PATH]  main  [36m?[0m[2m^[22m  | Opus");
    });
}

/// Skipped on Windows: JSON contains paths that need escaping on Windows.
#[test]
#[cfg_attr(windows, ignore)]
fn test_statusline_claude_code_minimal() {
    let repo = setup_basic_repo();

    let json = format!(
        r#"{{"workspace": {{"current_dir": "{}"}}}}"#,
        repo.root_path().display()
    );

    let output = run_statusline(&repo, &["--claude-code"], Some(&json));
    claude_code_snapshot_settings(&repo).bind(|| {
        assert_snapshot!(output, @"[PATH]  main  [2m^[22m");
    });
}

/// Skipped on Windows: JSON contains paths that need escaping on Windows.
#[test]
#[cfg_attr(windows, ignore)]
fn test_statusline_claude_code_with_model() {
    let repo = setup_basic_repo();

    let json = format!(
        r#"{{
            "workspace": {{"current_dir": "{}"}},
            "model": {{"display_name": "Haiku"}}
        }}"#,
        repo.root_path().display()
    );

    let output = run_statusline(&repo, &["--claude-code"], Some(&json));
    claude_code_snapshot_settings(&repo).bind(|| {
        assert_snapshot!(output, @"[PATH]  main  [2m^[22m  | Haiku");
    });
}

// --- Directive Mode Tests ---

#[test]
fn test_statusline_directive_mode() {
    // When called with --internal, output goes to stderr (stdout empty for shell eval)
    let repo = setup_repo_with_changes();

    let mut cmd = wt_command();
    cmd.current_dir(repo.root_path());
    cmd.args(["--internal", "list", "statusline"]);
    repo.clean_cli_env(&mut cmd);

    let output = cmd.output().expect("failed to run command");

    // stdout should be empty in directive mode
    assert!(
        output.stdout.is_empty(),
        "stdout should be empty in directive mode, got: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );

    // stderr should have the statusline
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.is_empty(),
        "stderr should have statusline output in directive mode"
    );
}
