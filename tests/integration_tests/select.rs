#![cfg(unix)]
//! TUI snapshot tests for `wt select`
//!
//! These tests use PTY execution combined with vt100 terminal emulation to capture
//! what the user actually sees on screen, enabling meaningful snapshot testing of
//! the skim-based TUI interface.
//!
//! The tests normalize timing-sensitive parts of the output (query line, count
//! indicators) to ensure stable snapshots despite TUI rendering variations.
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

use crate::common::TestRepo;
use insta::assert_snapshot;
use insta_cmd::get_cargo_bin;
use portable_pty::{CommandBuilder, PtySize};
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
const STABILIZE_TIMEOUT: Duration = Duration::from_secs(5);

/// How long screen must be unchanged to consider it "stable".
/// Must be long enough for preview content to load (preview commands run async).
/// 500ms balances reliability (allows preview to complete) with speed.
/// Panel switches trigger async git commands that may take time.
const STABLE_DURATION: Duration = Duration::from_millis(500);

/// Polling interval when waiting for output.
/// Fast polling ensures tests complete quickly when ready.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

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

/// Execute a command in a PTY and return raw output bytes
///
/// Uses polling with stabilization detection instead of fixed delays.
fn exec_in_pty_with_input(
    command: &str,
    args: &[&str],
    working_dir: &Path,
    env_vars: &[(String, String)],
    input: &str,
) -> (Vec<u8>, i32) {
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
) -> (Vec<u8>, i32) {
    let pty_system = crate::common::native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: TERM_ROWS,
            cols: TERM_COLS,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();

    let mut cmd = CommandBuilder::new(command);
    for arg in args {
        cmd.arg(arg);
    }
    cmd.cwd(working_dir);

    // Set minimal environment
    cmd.env_clear();
    cmd.env(
        "HOME",
        home::home_dir().unwrap().to_string_lossy().to_string(),
    );
    cmd.env(
        "PATH",
        std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string()),
    );
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
    let mut raw_output = Vec::new();

    // Helper to drain available output from the channel (non-blocking)
    let drain_output =
        |rx: &mpsc::Receiver<Vec<u8>>, parser: &mut vt100::Parser, raw_output: &mut Vec<u8>| {
            while let Ok(chunk) = rx.try_recv() {
                raw_output.extend_from_slice(&chunk);
                parser.process(&chunk);
            }
        };

    // Wait for skim to be ready (show "> " prompt)
    let start = Instant::now();
    loop {
        drain_output(&rx, &mut parser, &mut raw_output);

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
    wait_for_stable(&rx, &mut parser, &mut raw_output);

    // Send each input and wait for screen to stabilize after each
    for (input, expected_content) in inputs {
        writer.write_all(input.as_bytes()).unwrap();
        writer.flush().unwrap();

        // Wait for screen to stabilize after this input, optionally requiring specific content
        wait_for_stable_with_content(&rx, &mut parser, &mut raw_output, *expected_content);
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
    drain_output(&rx, &mut parser, &mut raw_output);

    let exit_status = child.wait().unwrap();
    let exit_code = exit_status.exit_code() as i32;

    (raw_output, exit_code)
}

/// Wait for screen content to stabilize (no changes for STABLE_DURATION)
fn wait_for_stable(
    rx: &mpsc::Receiver<Vec<u8>>,
    parser: &mut vt100::Parser,
    raw_output: &mut Vec<u8>,
) {
    wait_for_stable_with_content(rx, parser, raw_output, None);
}

/// Wait for screen content to stabilize, optionally requiring specific content.
///
/// If `expected_content` is provided, waits until the screen contains that string
/// AND has stabilized. This is essential for async preview panels where the initial
/// render may show placeholder content before the actual data loads.
fn wait_for_stable_with_content(
    rx: &mpsc::Receiver<Vec<u8>>,
    parser: &mut vt100::Parser,
    raw_output: &mut Vec<u8>,
    expected_content: Option<&str>,
) {
    let start = Instant::now();
    let mut last_change = Instant::now();
    let mut last_content = parser.screen().contents();

    while start.elapsed() < STABILIZE_TIMEOUT {
        // Drain available output
        while let Ok(chunk) = rx.try_recv() {
            raw_output.extend_from_slice(&chunk);
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

/// Render raw PTY output through vt100 terminal emulator to get clean screen text
fn render_terminal_screen(raw_output: &[u8]) -> String {
    let mut parser = vt100::Parser::new(TERM_ROWS, TERM_COLS, 0);
    parser.process(raw_output);

    let screen = parser.screen();
    let mut result = String::new();

    for row in 0..TERM_ROWS {
        let mut line = String::new();
        for col in 0..TERM_COLS {
            if let Some(cell) = screen.cell(row, col) {
                line.push_str(cell.contents());
            }
        }
        // Trim trailing whitespace but preserve the line
        result.push_str(line.trim_end());
        result.push('\n');
    }

    // Trim trailing empty lines
    while result.ends_with("\n\n") {
        result.pop();
    }

    result
}

/// Normalize output for snapshot stability
fn normalize_output(output: &str) -> String {
    let mut lines: Vec<&str> = output.lines().collect();

    // Normalize line 1 (query line) - replace with fixed marker
    // This line shows typed query which has timing variations
    if !lines.is_empty() {
        lines[0] =
            "> [QUERY]                                                     │[PREVIEW_HEADER]";
    }

    let output = lines.join("\n");

    // Replace temp paths like /var/folders/.../repo.XXX with [REPO]
    let re = regex::Regex::new(r"/[^\s]+\.tmp[^\s/]*").unwrap();
    let output = re.replace_all(&output, "[REPO]");

    // Replace count indicators like "1/4", "3/4" etc at end of lines
    let count_re = regex::Regex::new(r"\d+/\d+$").unwrap();
    let output = count_re.replace_all(&output, "[N/M]");

    // Replace home directory paths
    if let Some(home) = home::home_dir() {
        let home_str = home.to_string_lossy();
        output.replace(&*home_str, "~")
    } else {
        output.to_string()
    }
}

#[test]

fn test_select_abort_with_escape() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    let env_vars = repo.test_env_vars();
    let (raw_output, exit_code) = exec_in_pty_with_input(
        get_cargo_bin("wt").to_str().unwrap(),
        &["select"],
        repo.root_path(),
        &env_vars,
        "\x1b", // Escape key to abort
    );

    assert_valid_abort_exit_code(exit_code);

    let screen = render_terminal_screen(&raw_output);
    let normalized = normalize_output(&screen);
    assert_snapshot!("select_abort_escape", normalized);
}

#[test]

fn test_select_with_multiple_worktrees() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.add_worktree("feature-one");
    repo.add_worktree("feature-two");

    let env_vars = repo.test_env_vars();
    let (raw_output, exit_code) = exec_in_pty_with_input(
        get_cargo_bin("wt").to_str().unwrap(),
        &["select"],
        repo.root_path(),
        &env_vars,
        "\x1b", // Escape to abort after viewing
    );

    assert_valid_abort_exit_code(exit_code);

    let screen = render_terminal_screen(&raw_output);
    let normalized = normalize_output(&screen);
    assert_snapshot!("select_multiple_worktrees", normalized);
}

#[test]

fn test_select_with_branches() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.add_worktree("active-worktree");
    // Create a branch without a worktree
    let output = repo
        .git_command(&["branch", "orphan-branch"])
        .output()
        .unwrap();
    assert!(output.status.success(), "Failed to create branch");

    let env_vars = repo.test_env_vars();
    let (raw_output, exit_code) = exec_in_pty_with_input(
        get_cargo_bin("wt").to_str().unwrap(),
        &["select"],
        repo.root_path(),
        &env_vars,
        "\x1b", // Escape to abort
    );

    assert_valid_abort_exit_code(exit_code);

    let screen = render_terminal_screen(&raw_output);
    let normalized = normalize_output(&screen);
    assert_snapshot!("select_with_branches", normalized);
}

/// Test preview panel 1: HEAD± shows uncommitted changes
#[test]

fn test_select_preview_panel_uncommitted() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    let feature_path = repo.add_worktree("feature");

    // First, create and commit a file so we have something to modify
    std::fs::write(feature_path.join("tracked.txt"), "Original content\n").unwrap();
    let output = repo
        .git_command(&["-C", feature_path.to_str().unwrap(), "add", "tracked.txt"])
        .output()
        .unwrap();
    assert!(output.status.success(), "Failed to add file");
    let output = repo
        .git_command(&[
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
    let (raw_output, exit_code) = exec_in_pty_with_input_expectations(
        get_cargo_bin("wt").to_str().unwrap(),
        &["select"],
        repo.root_path(),
        &env_vars,
        &[
            ("feature", None),
            ("1", Some("diff --git")), // Wait for diff to load
            ("\x1b", None),
        ],
    );

    assert_valid_abort_exit_code(exit_code);

    let screen = render_terminal_screen(&raw_output);
    let normalized = normalize_output(&screen);
    assert_snapshot!("select_preview_uncommitted", normalized);
}

/// Test preview panel 2: log shows recent commits
#[test]

fn test_select_preview_panel_log() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    let feature_path = repo.add_worktree("feature");

    // Make several commits in the feature worktree
    for i in 1..=5 {
        std::fs::write(
            feature_path.join(format!("file{i}.txt")),
            format!("Content for file {i}\n"),
        )
        .unwrap();
        let output = repo
            .git_command(&["-C", feature_path.to_str().unwrap(), "add", "."])
            .output()
            .unwrap();
        assert!(output.status.success(), "Failed to add files");
        let output = repo
            .git_command(&[
                "-C",
                feature_path.to_str().unwrap(),
                "commit",
                "-m",
                &format!("Add file {i} with important changes"),
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "Failed to commit");
    }

    let env_vars = repo.test_env_vars();
    // Type "feature" to filter, press 2 for log panel
    // Wait for commit log format "* [hash]" to appear - the async preview can be slow under congestion
    let (raw_output, exit_code) = exec_in_pty_with_input_expectations(
        get_cargo_bin("wt").to_str().unwrap(),
        &["select"],
        repo.root_path(),
        &env_vars,
        &[
            ("feature", None),
            ("2", Some("* ")), // Wait for git log output (starts with "* [hash]")
            ("\x1b", None),
        ],
    );

    assert_valid_abort_exit_code(exit_code);

    let screen = render_terminal_screen(&raw_output);
    let normalized = normalize_output(&screen);
    assert_snapshot!("select_preview_log", normalized);
}

/// Test preview panel 3: main…± shows diff vs main branch
#[test]

fn test_select_preview_panel_main_diff() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
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
        .git_command(&["-C", feature_path.to_str().unwrap(), "add", "."])
        .output()
        .unwrap();
    assert!(output.status.success(), "Failed to add files");
    let output = repo
        .git_command(&[
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
        .git_command(&["-C", feature_path.to_str().unwrap(), "add", "."])
        .output()
        .unwrap();
    assert!(output.status.success(), "Failed to add files");
    let output = repo
        .git_command(&[
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
    let (raw_output, exit_code) = exec_in_pty_with_input_expectations(
        get_cargo_bin("wt").to_str().unwrap(),
        &["select"],
        repo.root_path(),
        &env_vars,
        &[
            ("feature", None),
            ("3", Some("diff --git")), // Wait for diff to load
            ("\x1b", None),
        ],
    );

    assert_valid_abort_exit_code(exit_code);

    let screen = render_terminal_screen(&raw_output);
    let normalized = normalize_output(&screen);
    assert_snapshot!("select_preview_main_diff", normalized);
}
