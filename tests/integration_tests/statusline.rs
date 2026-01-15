//! Snapshot tests for `wt list statusline` command.
//!
//! Tests the statusline output for shell prompts and Claude Code integration.

use crate::common::{TestRepo, repo, wt_command};
use insta::assert_snapshot;
use rstest::rstest;
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

    let output = run_statusline(&repo, &["--claude-code"], Some(&json));
    claude_code_snapshot_settings().bind(|| {
        assert_snapshot!(output, @"[PATH]  main  [36m?[0m[2m^[22m[2m|[22m  | Opus");
    });
}

#[rstest]
fn test_statusline_claude_code_minimal(repo: TestRepo) {
    let escaped_path = escape_path_for_json(repo.root_path());
    let json = format!(r#"{{"workspace": {{"current_dir": "{escaped_path}"}}}}"#,);

    let output = run_statusline(&repo, &["--claude-code"], Some(&json));
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

    let output = run_statusline(&repo, &["--claude-code"], Some(&json));
    claude_code_snapshot_settings().bind(|| {
        assert_snapshot!(output, @"[PATH]  main  [2m^[22m[2m|[22m  | Haiku");
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
