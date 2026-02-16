#![cfg(all(unix, feature = "shell-integration-tests"))]
//! TUI snapshot tests for `wt switch` interactive picker
//!
//! These tests use PTY execution combined with vt100 terminal emulation to capture
//! what the user actually sees on screen, enabling meaningful snapshot testing of
//! the skim-based TUI interface.
//!
//! ## Capture-Before-Abort Pattern
//!
//! Abort tests snapshot the screen BEFORE sending Escape, not after. Skim's teardown
//! is asynchronous — sending Escape races with rendering, producing non-deterministic
//! output (variable border painting, incomplete rows). By capturing the stable pre-abort
//! state, we eliminate this entire class of flakiness. After capture, Escape is sent and
//! only the exit code is checked.
//!
//! ## Timing Strategy
//!
//! Instead of fixed delays (which are either too short on slow CI or wastefully
//! long on fast machines), we poll for screen stabilization:
//!
//! - **Long timeouts** (10s) ensure reliability on slow CI
//! - **Fast polling** (10ms) means tests complete quickly when things work
//! - **Content-based readiness** detects when skim has rendered ("> " prompt)
//! - **Stabilization detection** waits for screen to stop changing
//! - **Content expectations** wait for async preview content to load (e.g., "diff --git")

use crate::common::{TestRepo, repo, wt_bin};
use insta::assert_snapshot;
use portable_pty::CommandBuilder;
use rstest::rstest;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Terminal dimensions for TUI tests
const TERM_ROWS: u16 = 30;
const TERM_COLS: u16 = 120;

/// Maximum time to wait for skim to become ready (show "> " prompt).
/// Long timeout ensures reliability on slow CI.
const READY_TIMEOUT: Duration = Duration::from_secs(10);

/// Maximum time to wait for screen to stabilize after input.
/// Long timeout ensures reliability on slow CI.
/// Increased from 5s to 10s to handle macOS CI under heavy load where branch
/// loading can be slow (particularly when waiting for async branch lists to populate).
const STABILIZE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long screen must be unchanged to consider it "stable".
/// Must be long enough for preview content to load (preview commands run async).
/// 500ms balances reliability (allows preview to complete) with speed.
/// Panel switches trigger async git commands that may take time.
const STABLE_DURATION: Duration = Duration::from_millis(500);

/// Polling interval when waiting for output.
/// Fast polling ensures tests complete quickly when ready.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Column where skim renders the │ border between list and preview panels.
/// TERM_COLS=120, preview window spec `right:60` → list gets 60 cols, separator at 60.
const SEPARATOR_COL: u16 = 60;

/// Result of executing a command in a PTY, holding the parsed terminal state.
struct PtyResult {
    parser: vt100::Parser,
    exit_code: i32,
}

impl PtyResult {
    /// Full screen content as rows of text.
    ///
    /// Trailing whitespace is trimmed from each row because `vt100::rows()` pads
    /// rows to the full column width with spaces. This padding is terminal buffer
    /// fill, not meaningful content, and varies across platforms. Trailing empty
    /// lines are also removed (unwritten terminal rows become empty after trim).
    fn screen(&self) -> String {
        self.parser
            .screen()
            .rows(0, TERM_COLS)
            .map(|row| row.trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
            .trim_end()
            .to_string()
    }

    /// List and preview panel content, split at the skim border column.
    /// Avoids the │ border character that causes cross-platform rendering issues.
    fn panels(&self) -> (String, String) {
        let screen = self.parser.screen();
        let list = screen
            .rows(0, SEPARATOR_COL)
            .map(|row| row.trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
            .trim_end()
            .to_string();
        let preview = screen
            .rows(SEPARATOR_COL + 1, TERM_COLS - SEPARATOR_COL - 1)
            .map(|row| row.trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
            .trim_end()
            .to_string();
        (list, preview)
    }
}

/// Assert that exit code is valid for skim abort (0, 1, or 130)
fn assert_valid_abort_exit_code(exit_code: i32) {
    // Skim exits with:
    // - 0: successful selection or no items
    // - 1: normal abort (escape key)
    // - 130: abort via SIGINT (128 + signal 2)
    assert!(
        exit_code == 0 || exit_code == 1 || exit_code == 130,
        "Unexpected exit code: {} (expected 0, 1, or 130 for skim abort)",
        exit_code
    );
}

/// Check if skim is ready (shows "> " prompt indicating it's accepting input)
fn is_skim_ready(screen_content: &str) -> bool {
    // Skim shows "> " at the start when ready, and displays item count like "1/3"
    screen_content.starts_with("> ") || screen_content.contains("\n> ")
}

/// Execute a command in a PTY and return the parsed terminal state.
///
/// Uses polling with stabilization detection instead of fixed delays.
fn exec_in_pty_with_input(
    command: &str,
    args: &[&str],
    working_dir: &Path,
    env_vars: &[(String, String)],
    input: &str,
) -> PtyResult {
    exec_in_pty_with_input_expectations(command, args, working_dir, env_vars, &[(input, None)])
}

/// Execute a command in a PTY with a sequence of inputs and optional content expectations.
///
/// Each input can optionally specify expected content that must appear before considering
/// the screen stable. This is essential for async preview panels where time-based stability
/// alone may capture intermediate placeholder content under system congestion.
///
/// Example: `[("feature", None), ("3", Some("diff --git")), ("\x1b", None)]`
/// - After typing "feature": wait for time-based stability only
/// - After pressing "3" (switch to diff panel): wait until "diff --git" appears
/// - After pressing Escape: wait for time-based stability only
fn exec_in_pty_with_input_expectations(
    command: &str,
    args: &[&str],
    working_dir: &Path,
    env_vars: &[(String, String)],
    inputs: &[(&str, Option<&str>)],
) -> PtyResult {
    let pair = crate::common::open_pty_with_size(TERM_ROWS, TERM_COLS);

    let mut cmd = CommandBuilder::new(command);
    for arg in args {
        cmd.arg(arg);
    }
    cmd.cwd(working_dir);

    // Set up isolated environment with coverage passthrough
    crate::common::configure_pty_command(&mut cmd);
    cmd.env("CLICOLOR_FORCE", "1");
    cmd.env("TERM", "xterm-256color");

    // Add test-specific environment variables
    for (key, value) in env_vars {
        cmd.env(key, value);
    }

    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();

    // Spawn a thread to continuously read PTY output and send chunks via channel
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut temp_buf = [0u8; 4096];
        loop {
            match reader.read(&mut temp_buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(temp_buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let mut parser = vt100::Parser::new(TERM_ROWS, TERM_COLS, 0);

    // Helper to drain available output from the channel (non-blocking)
    let drain_output = |rx: &mpsc::Receiver<Vec<u8>>, parser: &mut vt100::Parser| {
        while let Ok(chunk) = rx.try_recv() {
            parser.process(&chunk);
        }
    };

    // Wait for skim to be ready (show "> " prompt)
    let start = Instant::now();
    loop {
        drain_output(&rx, &mut parser);

        let screen_content = parser.screen().contents();
        if is_skim_ready(&screen_content) {
            break;
        }

        if start.elapsed() > READY_TIMEOUT {
            eprintln!(
                "Warning: Timed out waiting for skim ready state. Screen content:\n{}",
                screen_content
            );
            break;
        }

        std::thread::sleep(POLL_INTERVAL);
    }

    // Wait for initial render to stabilize
    wait_for_stable(&rx, &mut parser);

    // Send each input and wait for screen to stabilize after each
    for (input, expected_content) in inputs {
        writer.write_all(input.as_bytes()).unwrap();
        writer.flush().unwrap();

        // Wait for screen to stabilize after this input, optionally requiring specific content
        wait_for_stable_with_content(&rx, &mut parser, *expected_content);
    }

    // Drop writer to signal EOF on stdin
    drop(writer);

    // Poll for process exit (fast polling, long timeout for CI)
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(5);
    while start.elapsed() < timeout {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill(); // Kill if still running after timeout

    // Drain any remaining output
    drain_output(&rx, &mut parser);

    let exit_status = child.wait().unwrap();
    let exit_code = exit_status.exit_code() as i32;

    PtyResult { parser, exit_code }
}

/// Execute a command in a PTY, capture screen state, then abort with Escape.
///
/// This is the key fix for flaky abort snapshot tests. The problem: snapshotting
/// screen state AFTER sending Escape races with skim's teardown, producing
/// non-deterministic output (variable border painting, incomplete rows, trailing
/// whitespace). The fix: capture the stable screen BEFORE aborting, then only
/// check exit code after abort.
///
/// `pre_abort_inputs` are sent before capturing (e.g., typing a filter or switching
/// preview panels). Each input can optionally specify content that must appear before
/// the screen is considered stable.
fn exec_in_pty_capture_before_abort(
    command: &str,
    args: &[&str],
    working_dir: &Path,
    env_vars: &[(String, String)],
    pre_abort_inputs: &[(&str, Option<&str>)],
) -> PtyResult {
    let pair = crate::common::open_pty_with_size(TERM_ROWS, TERM_COLS);

    let mut cmd = CommandBuilder::new(command);
    for arg in args {
        cmd.arg(arg);
    }
    cmd.cwd(working_dir);

    crate::common::configure_pty_command(&mut cmd);
    cmd.env("CLICOLOR_FORCE", "1");
    cmd.env("TERM", "xterm-256color");

    for (key, value) in env_vars {
        cmd.env(key, value);
    }

    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();

    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut temp_buf = [0u8; 4096];
        loop {
            match reader.read(&mut temp_buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(temp_buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let mut parser = vt100::Parser::new(TERM_ROWS, TERM_COLS, 0);

    let drain_output = |rx: &mpsc::Receiver<Vec<u8>>, parser: &mut vt100::Parser| {
        while let Ok(chunk) = rx.try_recv() {
            parser.process(&chunk);
        }
    };

    // Wait for skim to be ready
    let start = Instant::now();
    loop {
        drain_output(&rx, &mut parser);

        let screen_content = parser.screen().contents();
        if is_skim_ready(&screen_content) {
            break;
        }

        if start.elapsed() > READY_TIMEOUT {
            eprintln!(
                "Warning: Timed out waiting for skim ready state. Screen content:\n{}",
                screen_content
            );
            break;
        }

        std::thread::sleep(POLL_INTERVAL);
    }

    // Wait for initial render to stabilize
    wait_for_stable(&rx, &mut parser);

    // Send pre-abort inputs (filter text, panel switches, etc.)
    for (input, expected_content) in pre_abort_inputs {
        writer.write_all(input.as_bytes()).unwrap();
        writer.flush().unwrap();
        wait_for_stable_with_content(&rx, &mut parser, *expected_content);
    }

    // === CAPTURE: screen state is now stable — snapshot BEFORE aborting ===
    // The parser retains this state because we stop feeding output to it.

    // Send Escape to abort
    writer.write_all(b"\x1b").unwrap();
    writer.flush().unwrap();
    drop(writer);

    // Drain remaining output WITHOUT feeding to parser — preserves pre-abort screen
    let start = Instant::now();
    let timeout = Duration::from_secs(5);
    loop {
        while rx.try_recv().is_ok() {} // discard chunks
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    let exit_status = child.wait().unwrap();
    let exit_code = exit_status.exit_code() as i32;

    PtyResult { parser, exit_code }
}

/// Wait for screen content to stabilize (no changes for STABLE_DURATION)
fn wait_for_stable(rx: &mpsc::Receiver<Vec<u8>>, parser: &mut vt100::Parser) {
    wait_for_stable_with_content(rx, parser, None);
}

/// Wait for screen content to stabilize, optionally requiring specific content.
///
/// If `expected_content` is provided, waits until the screen contains that string
/// AND has stabilized. This is essential for async preview panels where the initial
/// render may show placeholder content before the actual data loads.
///
/// Tip: include the panel border character (`│`) in `expected_content` to ensure
/// the full TUI frame has rendered, not just the preview text content.
fn wait_for_stable_with_content(
    rx: &mpsc::Receiver<Vec<u8>>,
    parser: &mut vt100::Parser,
    expected_content: Option<&str>,
) {
    let start = Instant::now();
    let mut last_change = Instant::now();
    let mut last_content = parser.screen().contents();

    while start.elapsed() < STABILIZE_TIMEOUT {
        // Drain available output
        while let Ok(chunk) = rx.try_recv() {
            parser.process(&chunk);
        }

        let current_content = parser.screen().contents();
        if current_content != last_content {
            last_content = current_content.clone();
            last_change = Instant::now();
        }

        // Check if expected content is present (if required)
        let content_ready = match expected_content {
            Some(expected) => current_content.contains(expected),
            None => true,
        };

        // Screen hasn't changed for STABLE_DURATION and content is ready
        if last_change.elapsed() >= STABLE_DURATION && content_ready {
            return;
        }

        std::thread::sleep(POLL_INTERVAL);
    }

    // Timeout - proceed anyway (test may still pass with partial render)
    eprintln!(
        "Warning: Screen did not fully stabilize within {:?}",
        STABILIZE_TIMEOUT
    );
}

/// Create insta settings with filters for switch picker snapshot stability.
///
/// Replaces the manual `normalize_output()` approach with declarative insta filters.
/// Since `rows()` returns plain text (no ANSI codes, no OSC 8 hyperlinks),
/// `add_pty_filters()` and `strip_osc8_hyperlinks()` are not needed.
fn switch_picker_settings(repo: &TestRepo) -> insta::Settings {
    let mut settings = crate::common::setup_snapshot_settings(repo);

    // Query line has timing variations (shows typed chars at different rates).
    // \A anchors to absolute start of string, matching only the first line.
    settings.add_filter(r"\A> [^\n]*", "> [QUERY]");

    // Skim count indicators (matched/total) at end of lines
    settings.add_filter(r"(?m)\d+/\d+\s*$", "[N/M]");

    // Commit hashes (7-8 hex chars)
    settings.add_filter(r"\b[0-9a-f]{7,8}\b", "[HASH]");

    // Truncated commit hashes (6+ hex chars followed by ..) in narrow columns
    settings.add_filter(r"\b[0-9a-f]{6,8}\.\.", "[HASH]..");

    // Relative timestamps (1d, 16h, etc.)
    settings.add_filter(r"\b\d+[dhms]\b", "[TIME]");

    settings
}

#[rstest]
fn test_switch_picker_abort_with_escape(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    // Remove origin so snapshots don't show origin/main
    repo.run_git(&["remote", "remove", "origin"]);

    let env_vars = repo.test_env_vars();
    let result = exec_in_pty_capture_before_abort(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
        &[], // No inputs before abort
    );

    assert_valid_abort_exit_code(result.exit_code);

    let (list, preview) = result.panels();
    let settings = switch_picker_settings(&repo);
    settings.bind(|| {
        assert_snapshot!("switch_picker_abort_escape_list", list);
        assert_snapshot!("switch_picker_abort_escape_preview", preview);
    });
}

#[rstest]
fn test_switch_picker_with_multiple_worktrees(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    // Remove origin so snapshots don't show origin/main
    repo.run_git(&["remote", "remove", "origin"]);

    repo.add_worktree("feature-one");
    repo.add_worktree("feature-two");

    let env_vars = repo.test_env_vars();
    let result = exec_in_pty_capture_before_abort(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
        // Wait for items to render before capturing (prevents flakiness on slow CI)
        &[("", Some("feature-two"))],
    );

    assert_valid_abort_exit_code(result.exit_code);

    let (list, preview) = result.panels();
    let settings = switch_picker_settings(&repo);
    settings.bind(|| {
        assert_snapshot!("switch_picker_multiple_worktrees_list", list);
        assert_snapshot!("switch_picker_multiple_worktrees_preview", preview);
    });
}

#[rstest]
fn test_switch_picker_with_branches(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    // Remove origin so snapshots don't show origin/main
    repo.run_git(&["remote", "remove", "origin"]);

    repo.add_worktree("active-worktree");
    // Create a branch without a worktree
    let output = repo
        .git_command()
        .args(["branch", "orphan-branch"])
        .output()
        .unwrap();
    assert!(output.status.success(), "Failed to create branch");

    let env_vars = repo.test_env_vars();
    let result = exec_in_pty_capture_before_abort(
        wt_bin().to_str().unwrap(),
        &["switch", "--branches"],
        repo.root_path(),
        &env_vars,
        // Wait for branch items to render before capturing. On macOS CI under
        // heavy load, skim may show the prompt and header before item rows,
        // causing wait_for_stable to capture too early (just the header).
        &[("", Some("orphan-branch"))],
    );

    assert_valid_abort_exit_code(result.exit_code);

    let (list, preview) = result.panels();
    let settings = switch_picker_settings(&repo);
    settings.bind(|| {
        assert_snapshot!("switch_picker_with_branches_list", list);
        assert_snapshot!("switch_picker_with_branches_preview", preview);
    });
}

#[rstest]
fn test_switch_picker_preview_panel_uncommitted(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    // Remove origin so snapshots don't show origin/main
    repo.run_git(&["remote", "remove", "origin"]);

    let feature_path = repo.add_worktree("feature");

    // First, create and commit a file so we have something to modify
    std::fs::write(feature_path.join("tracked.txt"), "Original content\n").unwrap();
    let output = repo
        .git_command()
        .args(["-C", feature_path.to_str().unwrap(), "add", "tracked.txt"])
        .output()
        .unwrap();
    assert!(output.status.success(), "Failed to add file");
    let output = repo
        .git_command()
        .args([
            "-C",
            feature_path.to_str().unwrap(),
            "commit",
            "-m",
            "Add tracked file",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "Failed to commit");

    // Now make uncommitted modifications to the tracked file
    std::fs::write(
        feature_path.join("tracked.txt"),
        "Modified content\nNew line added\nAnother line\n",
    )
    .unwrap();

    let env_vars = repo.test_env_vars();
    // Type "feature" to filter to just the feature worktree, press 1 for HEAD± panel
    // Wait for "diff --git" to appear after pressing 1 - the async preview can be slow under congestion
    let result = exec_in_pty_capture_before_abort(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
        &[
            ("feature", None),
            ("1", Some("│diff --git")), // Wait for diff to load (│ = border drawn)
        ],
    );

    assert_valid_abort_exit_code(result.exit_code);

    let (list, preview) = result.panels();
    let settings = switch_picker_settings(&repo);
    settings.bind(|| {
        assert_snapshot!("switch_picker_preview_uncommitted_list", list);
        assert_snapshot!("switch_picker_preview_uncommitted_preview", preview);
    });
}

#[rstest]
fn test_switch_picker_preview_panel_log(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    // Remove origin so snapshots don't show origin/main
    repo.run_git(&["remote", "remove", "origin"]);

    let feature_path = repo.add_worktree("feature");

    // Make several commits in the feature worktree
    for i in 1..=5 {
        std::fs::write(
            feature_path.join(format!("file{i}.txt")),
            format!("Content for file {i}\n"),
        )
        .unwrap();
        let output = repo
            .git_command()
            .args(["-C", feature_path.to_str().unwrap(), "add", "."])
            .output()
            .unwrap();
        assert!(output.status.success(), "Failed to add files");
        let output = repo
            .git_command()
            .args([
                "-C",
                feature_path.to_str().unwrap(),
                "commit",
                "-m",
                &format!("Add file {i} with content"),
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "Failed to commit");
    }

    let env_vars = repo.test_env_vars();
    // Type "feature" to filter, press 2 for log panel
    // Wait for commit log format "* [hash]" to appear - the async preview can be slow under congestion
    let result = exec_in_pty_capture_before_abort(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
        &[
            ("feature", None),
            ("2", Some("│* ")), // Wait for git log output (│ = border drawn)
        ],
    );

    assert_valid_abort_exit_code(result.exit_code);

    let (list, preview) = result.panels();
    let settings = switch_picker_settings(&repo);
    settings.bind(|| {
        assert_snapshot!("switch_picker_preview_log_list", list);
        assert_snapshot!("switch_picker_preview_log_preview", preview);
    });
}

#[rstest]
fn test_switch_picker_preview_panel_main_diff(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    // Remove origin so snapshots don't show origin/main
    repo.run_git(&["remote", "remove", "origin"]);

    let feature_path = repo.add_worktree("feature");

    // Make commits in the feature worktree that differ from main
    std::fs::write(
        feature_path.join("feature_code.rs"),
        r#"fn new_feature() {
    println!("This is a new feature!");
    let x = 42;
    let y = x * 2;
    println!("Result: {}", y);
}
"#,
    )
    .unwrap();
    let output = repo
        .git_command()
        .args(["-C", feature_path.to_str().unwrap(), "add", "."])
        .output()
        .unwrap();
    assert!(output.status.success(), "Failed to add files");
    let output = repo
        .git_command()
        .args([
            "-C",
            feature_path.to_str().unwrap(),
            "commit",
            "-m",
            "Add new feature implementation",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "Failed to commit");

    // Add another commit
    std::fs::write(
        feature_path.join("tests.rs"),
        r#"#[test]
fn test_new_feature() {
    assert_eq!(42 * 2, 84);
}
"#,
    )
    .unwrap();
    let output = repo
        .git_command()
        .args(["-C", feature_path.to_str().unwrap(), "add", "."])
        .output()
        .unwrap();
    assert!(output.status.success(), "Failed to add files");
    let output = repo
        .git_command()
        .args([
            "-C",
            feature_path.to_str().unwrap(),
            "commit",
            "-m",
            "Add tests for new feature",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "Failed to commit");

    let env_vars = repo.test_env_vars();
    // Type "feature" to filter, press 3 for main…± panel
    // Wait for "diff --git" to appear after pressing 3 - the async preview can be slow under congestion
    let result = exec_in_pty_capture_before_abort(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
        &[
            ("feature", None),
            ("3", Some("│diff --git")), // Wait for diff to load (│ = border drawn)
        ],
    );

    assert_valid_abort_exit_code(result.exit_code);

    let (list, preview) = result.panels();
    let settings = switch_picker_settings(&repo);
    settings.bind(|| {
        assert_snapshot!("switch_picker_preview_main_diff_list", list);
        assert_snapshot!("switch_picker_preview_main_diff_preview", preview);
    });
}

#[rstest]
fn test_switch_picker_respects_list_config(mut repo: TestRepo) {
    repo.add_worktree("active-worktree");
    // Create a branch without a worktree
    let output = repo
        .git_command()
        .args(["branch", "orphan-branch"])
        .output()
        .unwrap();
    assert!(output.status.success(), "Failed to create branch");

    // Write user config with [list] branches = true
    // This should enable branches in the picker without the --branches flag
    repo.write_test_config(
        r#"
[list]
branches = true
"#,
    );

    let env_vars = repo.test_env_vars();
    // Capture screen BEFORE sending Escape. Screen must stabilize with orphan-branch visible.
    let result = exec_in_pty_capture_before_abort(
        wt_bin().to_str().unwrap(),
        &["switch"], // No --branches flag - config should enable it
        repo.root_path(),
        &env_vars,
        &[("", Some("│orphan-branch"))], // Wait for orphan-branch to appear before abort
    );

    assert_valid_abort_exit_code(result.exit_code);

    let screen = result.screen();
    // Verify that orphan-branch appears (enabled by config, not CLI flag)
    assert!(
        screen.contains("orphan-branch"),
        "orphan-branch should appear when [list] branches = true in config.\nScreen:\n{}",
        screen
    );
}

#[rstest]
fn test_switch_picker_create_worktree_with_alt_c(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    // Remove origin so there's no interference from remote branches
    repo.run_git(&["remote", "remove", "origin"]);

    let env_vars = repo.test_env_vars();

    // Type branch name "new-feature", then press Alt-C (escape + c) to create
    let result = exec_in_pty_with_input_expectations(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
        &[
            ("new-feature", None), // Type the branch name
            ("\x1bc", None),       // Alt-C (escape + c) to create worktree
        ],
    );

    // Alt-C triggers accept which should exit normally
    assert_eq!(
        result.exit_code, 0,
        "Expected exit code 0 for successful create"
    );

    let screen = result.screen();

    // Verify the success message shows the new branch
    assert!(
        screen.contains("new-feature") || screen.contains("Switched"),
        "Expected success message showing new-feature branch.\nScreen:\n{}",
        screen
    );

    // Verify the worktree was actually created by checking the branch exists
    let branch_output = repo
        .git_command()
        .args(["branch", "--list", "new-feature"])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&branch_output.stdout).contains("new-feature"),
        "Branch new-feature should have been created"
    );
}

#[rstest]
fn test_switch_picker_create_with_empty_query_fails(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    // Remove origin so there's no interference from remote branches
    repo.run_git(&["remote", "remove", "origin"]);

    let env_vars = repo.test_env_vars();

    // Press Alt-C without typing a query - should error
    let result = exec_in_pty_with_input(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
        "\x1bc", // Alt-C (escape + c) without typing a branch name
    );

    // Should exit with error (non-zero)
    assert_ne!(
        result.exit_code, 0,
        "Expected non-zero exit for empty query"
    );

    let screen = result.screen();

    // Verify the error message
    assert!(
        screen.contains("no branch name entered") || screen.contains("Cannot create"),
        "Expected error message about missing branch name.\nScreen:\n{}",
        screen
    );
}

#[rstest]
fn test_switch_picker_switch_to_existing_worktree(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    // Remove origin so there's no interference from remote branches
    repo.run_git(&["remote", "remove", "origin"]);

    // Create a worktree to switch to
    repo.add_worktree("target-branch");

    let env_vars = repo.test_env_vars();

    // Navigate to target-branch and press Enter to switch
    let result = exec_in_pty_with_input_expectations(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
        &[
            ("target", None), // Filter to "target-branch"
            ("\r", None),     // Enter to switch
        ],
    );

    // Should exit successfully
    assert_eq!(
        result.exit_code, 0,
        "Expected exit code 0 for successful switch"
    );

    let screen = result.screen();

    // Verify the success message or cd directive
    assert!(
        screen.contains("target-branch") || screen.contains("Switched") || screen.contains("cd "),
        "Expected switch output showing target-branch.\nScreen:\n{}",
        screen
    );
}
