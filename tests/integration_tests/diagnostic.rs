//! Tests for diagnostic report generation.
//!
//! These tests verify the markdown structure and content of diagnostic reports
//! to ensure they're suitable for GitHub issue filing.
//!
//! # Test Coverage
//!
//! - `test_diagnostic_report_file_format`: Snapshot of full diagnostic structure
//! - `test_diagnostic_not_created_without_verbose`: No file without --verbose
//! - `test_diagnostic_hint_without_verbose`: Hint tells user to use --verbose
//! - `test_diagnostic_contains_required_sections`: All sections present
//! - `test_diagnostic_context_has_no_ansi_codes`: ANSI stripped for GitHub
//! - `test_diagnostic_verbose_log_contains_git_commands`: Log has useful data
//! - `test_diagnostic_shows_gh_command_hint`: gh CLI suggestion when available
//! - `test_diagnostic_written_to_correct_location`: File in .git/wt-logs/

use std::fs;
use std::path::PathBuf;

use insta::assert_snapshot;
use rstest::rstest;

use crate::common::{TestRepo, repo, setup_snapshot_settings};

/// Helper to corrupt a worktree's HEAD file to trigger git errors.
fn corrupt_worktree_head(repo: &TestRepo, worktree_name: &str) -> PathBuf {
    let feature_path = repo.worktrees.get(worktree_name).unwrap();
    let git_dir = feature_path.join(".git");
    let git_content = fs::read_to_string(&git_dir).unwrap();
    let actual_git_dir = git_content
        .strip_prefix("gitdir: ")
        .unwrap()
        .trim()
        .to_string();
    let head_path = PathBuf::from(&actual_git_dir).join("HEAD");
    fs::write(&head_path, "invalid").unwrap();
    head_path
}

/// Snapshot the diagnostic report file generated on git errors with --verbose.
///
/// This test triggers a git error (invalid HEAD) which causes `wt list --verbose`
/// to generate a diagnostic report file. We then read and snapshot the file
/// to verify its structure.
///
/// Note: Diagnostic files are only generated when --verbose is used.
#[rstest]
fn test_diagnostic_report_file_format(mut repo: TestRepo) {
    repo.add_worktree("feature");
    corrupt_worktree_head(&repo, "feature");

    let output = repo
        .wt_command()
        .args(["list", "--verbose"])
        .output()
        .unwrap();

    let diagnostic_path = repo
        .root_path()
        .join(".git")
        .join("wt-logs")
        .join("diagnostic.md");
    assert!(
        diagnostic_path.exists(),
        "Diagnostic file should be generated at {:?}",
        diagnostic_path
    );

    let content = fs::read_to_string(&diagnostic_path).unwrap();

    // Verify verbose log section is present (requires --verbose)
    assert!(
        content.contains("<summary>Verbose log</summary>"),
        "Diagnostic should include verbose log section when run with --verbose"
    );

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        assert_snapshot!("diagnostic_file_format", normalize_report(&content));
    });

    // Verify the stderr mentions the diagnostic was saved
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Diagnostic saved"),
        "Output should mention diagnostic was saved. stderr: {}",
        stderr
    );
}

/// Without --verbose, no diagnostic file should be created.
///
/// The diagnostic file is only useful with verbose logs, so we don't create
/// an empty/minimal diagnostic when --verbose is not used.
#[rstest]
fn test_diagnostic_not_created_without_verbose(mut repo: TestRepo) {
    repo.add_worktree("feature");
    corrupt_worktree_head(&repo, "feature");

    // Run WITHOUT --verbose
    repo.wt_command().args(["list"]).output().unwrap();

    // Diagnostic file should NOT exist
    let diagnostic_path = repo
        .root_path()
        .join(".git")
        .join("wt-logs")
        .join("diagnostic.md");
    assert!(
        !diagnostic_path.exists(),
        "Diagnostic file should NOT be created without --verbose"
    );
}

/// Without --verbose, the hint should tell users to re-run with --verbose.
#[rstest]
fn test_diagnostic_hint_without_verbose(mut repo: TestRepo) {
    repo.add_worktree("feature");
    corrupt_worktree_head(&repo, "feature");

    let output = repo.wt_command().args(["list"]).output().unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("To create a diagnostic file, re-run with"),
        "Should hint to use --verbose. stderr: {}",
        stderr
    );
    assert!(
        stderr.contains("--verbose"),
        "Hint should mention --verbose flag. stderr: {}",
        stderr
    );
}

/// Diagnostic file must contain all required sections for GitHub issues.
#[rstest]
fn test_diagnostic_contains_required_sections(mut repo: TestRepo) {
    repo.add_worktree("feature");
    corrupt_worktree_head(&repo, "feature");

    repo.wt_command()
        .args(["list", "--verbose"])
        .output()
        .unwrap();

    let content = fs::read_to_string(
        repo.root_path()
            .join(".git")
            .join("wt-logs")
            .join("diagnostic.md"),
    )
    .unwrap();

    // Header section
    assert!(
        content.contains("## Diagnostic Report"),
        "Should have header"
    );
    assert!(content.contains("**Generated:**"), "Should have timestamp");
    assert!(content.contains("**Context:**"), "Should have context");

    // Privacy notice
    assert!(
        content.contains("### What's included"),
        "Should have privacy section"
    );
    assert!(
        content.contains("Commit messages"),
        "Should mention commit messages are included"
    );
    assert!(
        content.contains("Does NOT contain:"),
        "Should clarify what's excluded"
    );
    assert!(
        content.contains("file contents"),
        "Should clarify file contents are excluded"
    );

    // Diagnostic data section
    assert!(
        content.contains("<summary>Diagnostic data</summary>"),
        "Should have diagnostic data section"
    );
    assert!(content.contains("wt v"), "Should have wt version");
    assert!(content.contains("git "), "Should have git version");
    assert!(
        content.contains("Shell integration:"),
        "Should have shell integration status"
    );
    assert!(
        content.contains("git worktree list --porcelain"),
        "Should have worktree list output"
    );

    // Verbose log section
    assert!(
        content.contains("<summary>Verbose log</summary>"),
        "Should have verbose log section"
    );
}

/// The context field should have ANSI codes stripped for clean GitHub display.
#[rstest]
fn test_diagnostic_context_has_no_ansi_codes(mut repo: TestRepo) {
    repo.add_worktree("feature");
    corrupt_worktree_head(&repo, "feature");

    repo.wt_command()
        .args(["list", "--verbose"])
        .output()
        .unwrap();

    let content = fs::read_to_string(
        repo.root_path()
            .join(".git")
            .join("wt-logs")
            .join("diagnostic.md"),
    )
    .unwrap();

    // ANSI escape codes start with \x1b[ or \033[
    assert!(
        !content.contains("\x1b["),
        "Diagnostic file should not contain ANSI escape codes"
    );
    assert!(
        !content.contains("\u{001b}"),
        "Diagnostic file should not contain ANSI escape codes (unicode)"
    );
}

/// Verbose log should contain git command traces for debugging.
#[rstest]
fn test_diagnostic_verbose_log_contains_git_commands(mut repo: TestRepo) {
    repo.add_worktree("feature");
    corrupt_worktree_head(&repo, "feature");

    repo.wt_command()
        .args(["list", "--verbose"])
        .output()
        .unwrap();

    let content = fs::read_to_string(
        repo.root_path()
            .join(".git")
            .join("wt-logs")
            .join("diagnostic.md"),
    )
    .unwrap();

    // Extract verbose log section
    let verbose_start = content
        .find("<summary>Verbose log</summary>")
        .expect("Should have verbose log");
    let verbose_section = &content[verbose_start..];

    // Should contain git command traces
    assert!(
        verbose_section.contains("git worktree list"),
        "Verbose log should contain git worktree list command"
    );
    assert!(
        verbose_section.contains("[wt-trace]"),
        "Verbose log should contain wt-trace entries"
    );
    assert!(
        verbose_section.contains("dur="),
        "Verbose log should contain command durations"
    );
    assert!(
        verbose_section.contains("ok="),
        "Verbose log should contain success/failure indicators"
    );
}

/// When gh CLI is available, should show command to create issue.
#[rstest]
fn test_diagnostic_shows_gh_command_hint(mut repo: TestRepo) {
    repo.add_worktree("feature");
    corrupt_worktree_head(&repo, "feature");

    let output = repo
        .wt_command()
        .args(["list", "--verbose"])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Check if gh is installed (the hint varies based on this)
    let gh_available = std::process::Command::new("gh")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if gh_available {
        assert!(
            stderr.contains("gh issue create"),
            "Should show gh command when gh CLI is available. stderr: {}",
            stderr
        );
        assert!(
            stderr.contains("--body-file"),
            "gh command should use --body-file. stderr: {}",
            stderr
        );
    }
    // If gh is not available, just verify diagnostic was saved
    assert!(
        stderr.contains("Diagnostic saved"),
        "Should always mention diagnostic was saved. stderr: {}",
        stderr
    );
}

/// Diagnostic file should be written to .git/wt-logs/diagnostic.md
#[rstest]
fn test_diagnostic_written_to_correct_location(mut repo: TestRepo) {
    repo.add_worktree("feature");
    corrupt_worktree_head(&repo, "feature");

    repo.wt_command()
        .args(["list", "--verbose"])
        .output()
        .unwrap();

    // Should be in .git/wt-logs/ directory
    let wt_logs_dir = repo.root_path().join(".git").join("wt-logs");
    assert!(wt_logs_dir.exists(), "wt-logs directory should exist");

    let diagnostic_path = wt_logs_dir.join("diagnostic.md");
    assert!(
        diagnostic_path.exists(),
        "diagnostic.md should be in wt-logs"
    );

    // Should be a markdown file
    let content = fs::read_to_string(&diagnostic_path).unwrap();
    assert!(
        content.starts_with("## "),
        "Should be a markdown file starting with header"
    );
}

/// Verbose log file should also be created alongside diagnostic.
#[rstest]
fn test_verbose_log_file_created(mut repo: TestRepo) {
    repo.add_worktree("feature");
    corrupt_worktree_head(&repo, "feature");

    repo.wt_command()
        .args(["list", "--verbose"])
        .output()
        .unwrap();

    let verbose_log_path = repo
        .root_path()
        .join(".git")
        .join("wt-logs")
        .join("verbose.log");
    assert!(
        verbose_log_path.exists(),
        "verbose.log should be created with --verbose"
    );

    let content = fs::read_to_string(&verbose_log_path).unwrap();
    assert!(!content.is_empty(), "verbose.log should not be empty");
    assert!(
        content.contains("[wt-trace]"),
        "verbose.log should contain trace entries"
    );
}

/// Normalize the report for snapshot comparison.
///
/// Replaces variable content (versions, paths, timestamps) with placeholders.
fn normalize_report(content: &str) -> String {
    let mut result = content.to_string();

    // Normalize timestamp (e.g., "2025-01-01T00:00:00Z" -> "[TIMESTAMP]")
    result = regex::Regex::new(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z")
        .unwrap()
        .replace_all(&result, "[TIMESTAMP]")
        .to_string();

    // Normalize wt version line (e.g., "wt v0.9.3-dirty (macos aarch64)" or "wt be89089 (linux x86_64)")
    // CI builds use commit hashes instead of version numbers
    result = regex::Regex::new(r"wt [^ ]+ \([^)]+\)")
        .unwrap()
        .replace_all(&result, "wt [VERSION] ([OS] [ARCH])")
        .to_string();

    // Normalize git version line
    result = regex::Regex::new(r"git \d+\.\d+[^\n]*")
        .unwrap()
        .replace_all(&result, "git [GIT_VERSION]")
        .to_string();

    // Normalize worktree paths in porcelain output (Unix and Windows absolute paths)
    result = regex::Regex::new(r"worktree (?:/|[A-Za-z]:)[^\n]+")
        .unwrap()
        .replace_all(&result, "worktree [PATH]")
        .to_string();

    // Normalize commit hashes (40 hex chars) - in "HEAD xxx" format
    result = regex::Regex::new(r"HEAD [a-f0-9]{40}")
        .unwrap()
        .replace_all(&result, "HEAD [COMMIT]")
        .to_string();

    // Normalize all other commit hashes (40 hex chars standalone)
    result = regex::Regex::new(r"\b[a-f0-9]{40}\b")
        .unwrap()
        .replace_all(&result, "[HASH]")
        .to_string();

    // Normalize temp paths in context (repo paths) - handles both Unix and Windows paths
    // Unix: /var/folders/.../repo.xxx or /tmp/.../repo.xxx
    // Windows: D:\a\worktrunk\worktrunk\... or C:\Users\...\repo.xxx
    // Match Windows paths first (drive letter + colon + any path chars)
    result = regex::Regex::new(r"([A-Z]:[^\s)]+|/[^\s)]+/repo\.[^\s)]+)")
        .unwrap()
        .replace_all(&result, "[REPO_PATH]")
        .to_string();

    // Normalize line breaks in git error messages (cross-platform consistency)
    // Some platforms wrap "fatal: not a git repository:\n  /path" on two lines,
    // others keep it on one line. Normalize to single-line format.
    result = regex::Regex::new(r"(fatal: not a git repository:)\s*\n\s*(\[REPO_PATH\])")
        .unwrap()
        .replace_all(&result, "$1 $2")
        .to_string();

    // Truncate verbose log section - it has parallel git commands that interleave
    // in different orders, making exact snapshot comparison flaky.
    // We verify the section exists separately in the test.
    if let Some(start) = result.find("<summary>Verbose log</summary>") {
        // Find the closing </details> after this point
        if let Some(end_offset) = result[start..].find("</details>") {
            let end = start + end_offset + "</details>".len();
            let before = &result[..start];
            let after = &result[end..];
            result = format!(
                "{}<summary>Verbose log</summary>\n\n[VERBOSE_LOG_CONTENT]\n</details>{}",
                before, after
            );
        }
    }

    result
}
