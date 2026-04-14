//! Tests for diagnostic report generation.
//!
//! These tests verify the markdown structure and content of diagnostic reports
//! to ensure they're suitable for GitHub issue filing.
//!
//! # Test Coverage
//!
//! - `test_diagnostic_report_file_format`: Snapshot of full diagnostic structure
//! - `test_diagnostic_not_created_without_vv`: No file without -vv
//! - `test_diagnostic_hint_without_vv`: Hint tells user to use -vv
//! - `test_diagnostic_contains_required_sections`: All sections present
//! - `test_diagnostic_context_has_no_ansi_codes`: ANSI stripped for GitHub
//! - `test_diagnostic_trace_log_contains_git_commands`: Log has useful data
//! - `test_diagnostic_saved_message_with_vv`: Output shows "Diagnostic saved" with -vv
//! - `test_diagnostic_written_to_correct_location`: File in .git/wt/logs/
//! - `test_diagnostic_gh_hint_with_vv`: Hint shows gist and issue URL when gh installed

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

/// Snapshot the diagnostic report file generated with -vv.
///
/// This test triggers a git error (invalid HEAD) and runs `wt list -vv`
/// to generate a diagnostic report file. We then read and snapshot the file
/// to verify its structure.
///
/// Note: Diagnostic files are only generated when -vv is used.
#[rstest]
fn test_diagnostic_report_file_format(mut repo: TestRepo) {
    repo.add_worktree("feature");
    corrupt_worktree_head(&repo, "feature");

    let output = repo.wt_command().args(["list", "-vv"]).output().unwrap();

    let diagnostic_path = repo
        .root_path()
        .join(".git")
        .join("wt/logs")
        .join("diagnostic.md");
    assert!(
        diagnostic_path.exists(),
        "Diagnostic file should be generated at {:?}",
        diagnostic_path
    );

    let content = fs::read_to_string(&diagnostic_path).unwrap();

    // Verify verbose log section is present (requires -v or higher)
    assert!(
        content.contains("<summary>Trace log</summary>"),
        "Diagnostic should include trace log section when run with -vv"
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

/// Without -vv, no diagnostic file should be created.
///
/// The diagnostic file is only created when -vv is used. Running without
/// any verbose flag or with just -v does not create a diagnostic file.
#[rstest]
fn test_diagnostic_not_created_without_vv(mut repo: TestRepo) {
    repo.add_worktree("feature");
    corrupt_worktree_head(&repo, "feature");

    // Run WITHOUT -vv
    repo.wt_command().args(["list"]).output().unwrap();

    // Diagnostic file should NOT exist
    let diagnostic_path = repo
        .root_path()
        .join(".git")
        .join("wt/logs")
        .join("diagnostic.md");
    assert!(
        !diagnostic_path.exists(),
        "Diagnostic file should NOT be created without -vv"
    );
}

/// Without -vv, the hint should tell users to re-run with -vv.
#[rstest]
fn test_diagnostic_hint_without_vv(mut repo: TestRepo) {
    repo.add_worktree("feature");
    corrupt_worktree_head(&repo, "feature");

    let output = repo.wt_command().args(["list"]).output().unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("To create a diagnostic file, run with"),
        "Should hint to use -vv. stderr: {}",
        stderr
    );
    assert!(
        stderr.contains("-vv"),
        "Hint should mention -vv flag. stderr: {}",
        stderr
    );
}

/// Diagnostic file must contain all required sections for GitHub issues.
#[rstest]
fn test_diagnostic_contains_required_sections(mut repo: TestRepo) {
    repo.add_worktree("feature");
    corrupt_worktree_head(&repo, "feature");

    repo.wt_command().args(["list", "-vv"]).output().unwrap();

    let content = fs::read_to_string(
        repo.root_path()
            .join(".git")
            .join("wt/logs")
            .join("diagnostic.md"),
    )
    .unwrap();

    // Header section
    assert!(
        content.contains("## Diagnostic Report"),
        "Should have header"
    );
    assert!(content.contains("**Generated:**"), "Should have timestamp");
    assert!(content.contains("**Command:**"), "Should have command");
    assert!(content.contains("**Result:**"), "Should have result");

    // Environment section
    assert!(
        content.contains("<summary>Environment</summary>"),
        "Should have environment section"
    );
    assert!(content.contains("wt "), "Should have wt version");
    assert!(content.contains("git "), "Should have git version");
    assert!(
        content.contains("Shell integration:"),
        "Should have shell integration status"
    );

    // Worktrees section
    assert!(
        content.contains("<summary>Worktrees</summary>"),
        "Should have worktrees section"
    );
    assert!(
        content.contains("refs/heads/"),
        "Should have branch refs in worktree list"
    );

    // Config section
    assert!(
        content.contains("<summary>Config</summary>"),
        "Should have config section"
    );

    // Trace log section
    assert!(
        content.contains("<summary>Trace log</summary>"),
        "Should have trace log section"
    );
}

/// The context field should have ANSI codes stripped for clean GitHub display.
#[rstest]
fn test_diagnostic_context_has_no_ansi_codes(mut repo: TestRepo) {
    repo.add_worktree("feature");
    corrupt_worktree_head(&repo, "feature");

    repo.wt_command().args(["list", "-vv"]).output().unwrap();

    let content = fs::read_to_string(
        repo.root_path()
            .join(".git")
            .join("wt/logs")
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

/// Trace log should contain git command traces for debugging.
#[rstest]
fn test_diagnostic_trace_log_contains_git_commands(mut repo: TestRepo) {
    repo.add_worktree("feature");
    corrupt_worktree_head(&repo, "feature");

    repo.wt_command().args(["list", "-vv"]).output().unwrap();

    let content = fs::read_to_string(
        repo.root_path()
            .join(".git")
            .join("wt/logs")
            .join("diagnostic.md"),
    )
    .unwrap();

    // Extract trace log section
    let trace_start = content
        .find("<summary>Trace log</summary>")
        .expect("Should have trace log");
    let trace_section = &content[trace_start..];

    // Should contain git command traces
    assert!(
        trace_section.contains("git worktree list"),
        "Trace log should contain git worktree list command"
    );
    assert!(
        trace_section.contains("[wt-trace]"),
        "Trace log should contain wt-trace entries"
    );
    assert!(
        trace_section.contains("dur_us="),
        "Trace log should contain command durations in microseconds"
    );
    assert!(
        trace_section.contains("ok="),
        "Trace log should contain success/failure indicators"
    );
}

/// Diagnostic is saved with -vv and output mentions it.
#[rstest]
fn test_diagnostic_saved_message_with_vv(mut repo: TestRepo) {
    repo.add_worktree("feature");
    corrupt_worktree_head(&repo, "feature");

    let output = repo.wt_command().args(["list", "-vv"]).output().unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Verify diagnostic was saved
    assert!(
        stderr.contains("Diagnostic saved"),
        "Should mention diagnostic was saved. stderr: {}",
        stderr
    );
}

/// Diagnostic file should be written to .git/wt/logs/diagnostic.md
#[rstest]
fn test_diagnostic_written_to_correct_location(mut repo: TestRepo) {
    repo.add_worktree("feature");
    corrupt_worktree_head(&repo, "feature");

    repo.wt_command().args(["list", "-vv"]).output().unwrap();

    // Should be in .git/wt/logs/ directory
    let wt_logs_dir = repo.root_path().join(".git").join("wt/logs");
    assert!(wt_logs_dir.exists());

    let diagnostic_path = wt_logs_dir.join("diagnostic.md");
    assert!(
        diagnostic_path.exists(),
        "diagnostic.md should be in wt/logs"
    );

    // Should be a markdown file
    let content = fs::read_to_string(&diagnostic_path).unwrap();
    assert!(
        content.starts_with("## "),
        "Should be a markdown file starting with header"
    );
}

/// Both log files (`trace.log` and `output.log`) should be created at `-vv`.
#[rstest]
fn test_log_files_created(mut repo: TestRepo) {
    repo.add_worktree("feature");
    corrupt_worktree_head(&repo, "feature");

    repo.wt_command().args(["list", "-vv"]).output().unwrap();

    let logs_dir = repo.root_path().join(".git").join("wt/logs");
    let trace_log = logs_dir.join("trace.log");
    let output_log = logs_dir.join("output.log");
    assert!(trace_log.exists(), "trace.log should be created with -vv");
    assert!(output_log.exists(), "output.log should be created with -vv");

    let trace = fs::read_to_string(&trace_log).unwrap();
    assert!(!trace.is_empty(), "trace.log should not be empty");
    assert!(
        trace.contains("[wt-trace]"),
        "trace.log should contain trace entries"
    );
}

/// At `-vv`, the full (uncapped) subprocess stdout/stderr should land in
/// `output.log` via `shell_exec::SUBPROCESS_FULL_TARGET` — not in `trace.log`.
/// `trace.log` gets the bounded preview alongside trace records.
#[rstest]
fn test_vv_splits_full_and_bounded_output(repo: TestRepo) {
    repo.wt_command().args(["list", "-vv"]).output().unwrap();

    let logs_dir = repo.root_path().join(".git").join("wt/logs");
    let trace = fs::read_to_string(logs_dir.join("trace.log")).unwrap();
    let output = fs::read_to_string(logs_dir.join("output.log")).unwrap();

    assert!(
        trace.contains("[wt-trace]"),
        "trace.log should contain [wt-trace] records at -vv"
    );
    // Captured stdout is emitted line-by-line with the `  ` / `  ! `
    // continuation prefix used by log_output. Full subprocess output lives
    // in output.log.
    assert!(
        output.lines().any(|l| l.contains("  worktree ")),
        "output.log should contain `git worktree list --porcelain` stdout lines at -vv"
    );
    // Structured trace records stay in trace.log only, not output.log.
    assert!(
        !output.contains("[wt-trace]"),
        "output.log should not contain [wt-trace] records"
    );
}

/// At `-vv`, when a single subprocess produces more than the line cap, stderr
/// and `trace.log` must show the elision marker while `output.log` holds the
/// full body without any elision. This guards the bounded-stderr invariant —
/// a regression that routed full output to stderr would silently fail to
/// elide.
#[rstest]
fn test_vv_bounded_on_stderr_full_in_output_log(repo: TestRepo) {
    // `wt list` runs `git for-each-ref refs/heads/` (captured via `Cmd::run`),
    // so its output flows through `log_output` and exercises the split.
    // Populate packed-refs with 250 fake branches pointing at HEAD — far more
    // than LOG_OUTPUT_MAX_LINES (200), which guarantees elision on stderr.
    let head_out = repo
        .git_command()
        .args(["rev-parse", "HEAD"])
        .run()
        .unwrap();
    let head = String::from_utf8(head_out.stdout)
        .unwrap()
        .trim()
        .to_string();
    let mut content = String::from("# pack-refs with: peeled fully-peeled sorted\n");
    for i in 0..250 {
        use std::fmt::Write as _;
        writeln!(&mut content, "{head} refs/heads/many-branch-{i:03}").unwrap();
    }
    std::fs::write(repo.root_path().join(".git/packed-refs"), content).unwrap();

    let output = repo
        .wt_command()
        .args(["list", "-vv"])
        .env("NO_COLOR", "1")
        .output()
        .expect("wt list");
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Elision marker appears on stderr (bounded preview) and in trace.log.
    let marker = "more lines, ";
    assert!(
        stderr.contains(marker),
        "stderr should contain elision marker for >200-line subprocess output: {stderr}"
    );

    let logs_dir = repo.root_path().join(".git").join("wt/logs");
    let trace = fs::read_to_string(logs_dir.join("trace.log")).unwrap();
    let raw = fs::read_to_string(logs_dir.join("output.log")).unwrap();

    assert!(
        trace.contains(marker),
        "trace.log mirrors stderr and should include the elision marker"
    );
    assert!(
        !raw.contains(marker),
        "output.log holds full output and must not contain an elision marker"
    );
    // The tail refs only appear in the full log.
    let tail_ref = "many-branch-249";
    assert!(
        raw.contains(tail_ref),
        "output.log should contain the full for-each-ref stdout (last ref)"
    );
    assert!(
        !stderr.contains(tail_ref),
        "stderr preview should be bounded before the last ref"
    );
    assert!(
        !trace.contains(tail_ref),
        "trace.log mirrors stderr and should be bounded too"
    );
}

/// `RUST_LOG=debug` at `-v 0` activates Debug logging without creating the
/// on-disk log files. Stderr receives the **bounded** preview
/// (`SUBPROCESS_TERMINAL_TARGET`); the uncapped `SUBPROCESS_FULL_TARGET`
/// records are dropped so raw bodies don't flood the terminal.
#[rstest]
fn test_rust_log_debug_fallback_without_vv(repo: TestRepo) {
    let output = repo
        .wt_command()
        .args(["list"])
        .env("RUST_LOG", "debug")
        .env("NO_COLOR", "1")
        .output()
        .expect("wt list");
    let stderr = String::from_utf8_lossy(&output.stderr);

    // No log files were created — `-vv` is what opens them.
    let logs_dir = repo.root_path().join(".git").join("wt/logs");
    assert!(
        !logs_dir.join("trace.log").exists(),
        "trace.log should NOT be created without -vv"
    );
    assert!(
        !logs_dir.join("output.log").exists(),
        "output.log should NOT be created without -vv"
    );

    // Subprocess stdout still reaches stderr via the bounded preview —
    // look for captured `git worktree list --porcelain` stdout lines.
    assert!(
        stderr.lines().any(|l| l.contains("  worktree ")),
        "stderr should contain subprocess stdout via bounded preview: {stderr}"
    );
    // For small outputs (git worktree list --porcelain is a handful of lines)
    // the bounded preview fits under the cap without an elision marker.
    assert!(
        !stderr.contains("more lines, "),
        "short subprocess output should not trip the elision marker: {stderr}"
    );
}

// =============================================================================
// Tests for -vv verbosity level (always write diagnostic)
// =============================================================================

/// With -vv, diagnostic file should be written even on successful commands.
#[rstest]
fn test_vv_writes_diagnostic_on_success(repo: TestRepo) {
    // Run a successful command with -vv
    let output = repo.wt_command().args(["list", "-vv"]).output().unwrap();

    assert!(output.status.success(), "Command should succeed");

    // Diagnostic file should exist
    let diagnostic_path = repo
        .root_path()
        .join(".git")
        .join("wt/logs")
        .join("diagnostic.md");
    assert!(
        diagnostic_path.exists(),
        "Diagnostic file should be created with -vv even on success"
    );

    // Content should indicate success
    let content = fs::read_to_string(&diagnostic_path).unwrap();
    assert!(
        content.contains("Command completed successfully"),
        "Result should indicate success. Content: {}",
        content
    );

    // stderr should mention diagnostic was saved
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Diagnostic saved"),
        "stderr should mention diagnostic was saved. stderr: {}",
        stderr
    );
}

/// With -vv, diagnostic file should be written on error (same as success case).
#[rstest]
fn test_vv_writes_diagnostic_on_error(mut repo: TestRepo) {
    repo.add_worktree("feature");
    corrupt_worktree_head(&repo, "feature");

    // Run a command that will hit git errors with -vv
    let output = repo.wt_command().args(["list", "-vv"]).output().unwrap();

    // Diagnostic file should exist
    let diagnostic_path = repo
        .root_path()
        .join(".git")
        .join("wt/logs")
        .join("diagnostic.md");
    assert!(
        diagnostic_path.exists(),
        "Diagnostic file should be created with -vv on error"
    );

    // stderr should mention diagnostic was saved
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Diagnostic saved"),
        "stderr should mention diagnostic was saved. stderr: {}",
        stderr
    );
}

/// With just -v, info-level logging goes to stderr but no log files are written.
/// `-vv` is the threshold for `trace.log`, `output.log`, and `diagnostic.md`.
#[rstest]
fn test_v_does_not_write_log_files(repo: TestRepo) {
    // Run a successful command with just -v
    let output = repo.wt_command().args(["list", "-v"]).output().unwrap();

    assert!(output.status.success(), "Command should succeed");

    // None of the -vv diagnostic files should exist with just -v
    let wt_logs = repo.root_path().join(".git").join("wt/logs");
    for name in ["diagnostic.md", "trace.log", "output.log"] {
        assert!(
            !wt_logs.join(name).exists(),
            "{name} should NOT be created with just -v (requires -vv)"
        );
    }
}

/// With -vv outside a git repo, command should still work (no crash).
#[test]
fn test_vv_outside_repo_no_crash() {
    use crate::common::wt_command;

    // Create a temp directory that is NOT a git repo
    let temp_dir = tempfile::tempdir().unwrap();

    let output = wt_command()
        .args(["--version", "-vv"])
        .current_dir(temp_dir.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "Command should succeed");

    // No diagnostic file should be created (not in a git repo)
    let diagnostic_path = temp_dir
        .path()
        .join(".git")
        .join("wt/logs")
        .join("diagnostic.md");
    assert!(
        !diagnostic_path.exists(),
        "Diagnostic file should NOT be created outside a git repo"
    );
}

/// When gh is installed, the hint should show gist creation and issue URL.
#[rstest]
fn test_diagnostic_gh_hint_with_vv(mut repo: TestRepo) {
    // Setup mock gh so it appears installed
    repo.setup_mock_gh();

    let output = repo.wt_command().args(["list", "-vv"]).output().unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Extract the hint line (starts with ↳)
    let hint_line = stderr
        .lines()
        .find(|line| line.contains("report a bug"))
        .expect("Should have hint about reporting a bug");

    // Normalize the path in the hint. The path may be:
    // - Quoted on Windows (drive letter colon requires POSIX escaping): 'D:/a/.../diagnostic.md'
    // - Unquoted on Unix (no special chars): _REPO_/.git/wt/logs/diagnostic.md
    let normalized = regex::Regex::new(r"--web '?[^' \x1b]*diagnostic\.md'?")
        .unwrap()
        .replace(hint_line, "--web [DIAGNOSTIC_PATH]");

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        assert_snapshot!("diagnostic_gh_hint", normalized);
    });
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

    // Normalize command line path (binary path varies between environments)
    // Matches: `target/debug/wt ...` or `/path/to/wt ...`
    result = regex::Regex::new(r"\*\*Command:\*\* `[^`]+`")
        .unwrap()
        .replace_all(
            &result,
            "**Command:** `[PROJECT_ROOT]/target/debug/wt list -vv`",
        )
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

    // Normalize user config path (must come BEFORE generic repo path normalization)
    result = regex::Regex::new(r"User config: [^\n]+")
        .unwrap()
        .replace_all(&result, "User config: [TEST_CONFIG]")
        .to_string();

    // Normalize project config path (must come BEFORE generic repo path normalization)
    // Handle both Unix (/) and Windows (\) path separators
    result = regex::Regex::new(r"Project config: (?:/|[A-Za-z]:)[^\n]+\.config[/\\]wt\.toml")
        .unwrap()
        .replace_all(&result, "Project config: _REPO_/.config/wt.toml")
        .to_string();

    // Normalize worktree temp paths (paths that contain `/repo.<name>` or
    // `\repo.<name>` — the convention used by TestRepo for linked worktrees).
    // Both branches require the `repo.` segment so the main-worktree path
    // (which is `.../repo` without a dot) falls through to insta's prefix
    // filter instead. Stop at whitespace, `)`, or a backtick so paths inlined
    // in markdown code spans don't eat the closing backtick.
    result = regex::Regex::new(r"([A-Z]:[^\s)`]+[\\/]repo\.[^\s)`]+|/[^\s)`]+/repo\.[^\s)`]+)")
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

    // Truncate trace log section - it has parallel git commands that interleave
    // in different orders, making exact snapshot comparison flaky.
    // We verify the section exists separately in the test.
    if let Some(start) = result.find("<summary>Trace log</summary>") {
        // Find the closing </details> after this point
        if let Some(end_offset) = result[start..].find("</details>") {
            let end = start + end_offset + "</details>".len();
            let before = &result[..start];
            let after = &result[end..];
            result = format!(
                "{}<summary>Trace log</summary>\n\n[TRACE_LOG_CONTENT]\n</details>{}",
                before, after
            );
        }
    }

    result
}
