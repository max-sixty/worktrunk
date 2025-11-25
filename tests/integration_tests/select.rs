//! TUI snapshot tests for `wt beta select`
//!
//! These tests use PTY execution combined with vt100 terminal emulation to capture
//! what the user actually sees on screen, enabling meaningful snapshot testing of
//! the skim-based TUI interface.
//!
//! **Note:** These tests are marked `#[ignore]` because TUI timing variations cause
//! flakiness (preview tabs appearing/not appearing, query text partially visible).
//! Run manually with: `cargo test --test integration select:: -- --ignored`

use crate::common::TestRepo;
use insta::assert_snapshot;
use insta_cmd::get_cargo_bin;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

/// Terminal dimensions for TUI tests
const TERM_ROWS: u16 = 30;
const TERM_COLS: u16 = 120;

/// Initial wait for TUI to fully render before sending input
const TUI_RENDER_DELAY: Duration = Duration::from_millis(800);

/// Delay between keystrokes to allow TUI to process input
const KEYSTROKE_DELAY: Duration = Duration::from_millis(150);

/// Final delay after last input before capturing screen
const FINAL_CAPTURE_DELAY: Duration = Duration::from_millis(200);

/// Timeout for waiting for process output before killing
const OUTPUT_TIMEOUT: Duration = Duration::from_secs(3);

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

/// Execute a command in a PTY and return raw output bytes
///
/// Uses a thread to read PTY output with a timeout, then sends input and
/// kills the process to capture the TUI state.
fn exec_in_pty_with_input(
    command: &str,
    args: &[&str],
    working_dir: &Path,
    env_vars: &[(String, String)],
    input: &str,
) -> (Vec<u8>, i32) {
    exec_in_pty_with_input_sequence(command, args, working_dir, env_vars, &[input])
}

/// Execute a command in a PTY with a sequence of inputs, each sent with a delay
///
/// This allows testing interactive TUIs where inputs need time to be processed.
fn exec_in_pty_with_input_sequence(
    command: &str,
    args: &[&str],
    working_dir: &Path,
    env_vars: &[(String, String)],
    inputs: &[&str],
) -> (Vec<u8>, i32) {
    use std::sync::mpsc;

    let pty_system = native_pty_system();
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

    // Spawn a thread to read output
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut temp_buf = [0u8; 4096];
        loop {
            match reader.read(&mut temp_buf) {
                Ok(0) => break,
                Ok(n) => buf.extend_from_slice(&temp_buf[..n]),
                Err(e) => {
                    // Log error to help debug flaky tests
                    eprintln!("PTY read error (may be expected on process exit): {e}");
                    break;
                }
            }
        }
        let _ = tx.send(buf);
    });

    // Wait for TUI to render
    std::thread::sleep(TUI_RENDER_DELAY);

    // Send each input with a delay between them
    for input in inputs {
        writer.write_all(input.as_bytes()).unwrap();
        writer.flush().unwrap();
        std::thread::sleep(KEYSTROKE_DELAY);
    }

    // Wait for final input to be processed
    std::thread::sleep(FINAL_CAPTURE_DELAY);

    // Drop writer to signal EOF on stdin
    drop(writer);

    // Wait for output with timeout, then kill if necessary
    let buf = match rx.recv_timeout(OUTPUT_TIMEOUT) {
        Ok(buf) => buf,
        Err(_) => {
            // Timeout - kill the process
            let _ = child.kill();
            rx.recv().unwrap_or_default()
        }
    };

    let exit_status = child.wait().unwrap();
    let exit_code = exit_status.exit_code() as i32;

    (buf, exit_code)
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

/// Normalize paths in output for snapshot stability
fn normalize_paths(output: &str) -> String {
    // Replace temp paths like /var/folders/.../test-repo.XXX with [REPO]
    let re = regex::Regex::new(r"/[^\s]+\.tmp[^\s/]*").unwrap();
    let output = re.replace_all(output, "[REPO]");

    // Replace home directory paths
    if let Some(home) = home::home_dir() {
        let home_str = home.to_string_lossy();
        output.replace(&*home_str, "~")
    } else {
        output.to_string()
    }
}

#[test]
#[ignore]
fn test_select_abort_with_escape() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    let env_vars = repo.test_env_vars();
    let (raw_output, exit_code) = exec_in_pty_with_input(
        get_cargo_bin("wt").to_str().unwrap(),
        &["beta", "select"],
        repo.root_path(),
        &env_vars,
        "\x1b", // Escape key to abort
    );

    assert_valid_abort_exit_code(exit_code);

    let screen = render_terminal_screen(&raw_output);
    let normalized = normalize_paths(&screen);
    assert_snapshot!("select_abort_escape", normalized);
}

#[test]
#[ignore]
fn test_select_with_multiple_worktrees() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.add_worktree("feature-one", "feature-one");
    repo.add_worktree("feature-two", "feature-two");

    let env_vars = repo.test_env_vars();
    let (raw_output, exit_code) = exec_in_pty_with_input(
        get_cargo_bin("wt").to_str().unwrap(),
        &["beta", "select"],
        repo.root_path(),
        &env_vars,
        "\x1b", // Escape to abort after viewing
    );

    assert_valid_abort_exit_code(exit_code);

    let screen = render_terminal_screen(&raw_output);
    let normalized = normalize_paths(&screen);
    assert_snapshot!("select_multiple_worktrees", normalized);
}

#[test]
#[ignore]
fn test_select_with_branches() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.add_worktree("active-worktree", "active-worktree");
    // Create a branch without a worktree
    let output = repo
        .git_command(&["branch", "orphan-branch"])
        .output()
        .unwrap();
    assert!(output.status.success(), "Failed to create branch");

    let env_vars = repo.test_env_vars();
    let (raw_output, exit_code) = exec_in_pty_with_input(
        get_cargo_bin("wt").to_str().unwrap(),
        &["beta", "select"],
        repo.root_path(),
        &env_vars,
        "\x1b", // Escape to abort
    );

    assert_valid_abort_exit_code(exit_code);

    let screen = render_terminal_screen(&raw_output);
    let normalized = normalize_paths(&screen);
    assert_snapshot!("select_with_branches", normalized);
}

/// Test preview panel 1: HEAD± shows uncommitted changes
#[test]
#[ignore]
fn test_select_preview_panel_uncommitted() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    let feature_path = repo.add_worktree("feature", "feature");

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
    let (raw_output, exit_code) = exec_in_pty_with_input_sequence(
        get_cargo_bin("wt").to_str().unwrap(),
        &["beta", "select"],
        repo.root_path(),
        &env_vars,
        &["feature", "1", "\x1b"], // Type "feature" to filter, 1, Escape
    );

    assert_valid_abort_exit_code(exit_code);

    let screen = render_terminal_screen(&raw_output);
    let normalized = normalize_paths(&screen);
    assert_snapshot!("select_preview_uncommitted", normalized);
}

/// Test preview panel 2: history shows recent commits
#[test]
#[ignore]
fn test_select_preview_panel_history() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    let feature_path = repo.add_worktree("feature", "feature");

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
    // Type "feature" to filter, press 2 for history panel
    let (raw_output, exit_code) = exec_in_pty_with_input_sequence(
        get_cargo_bin("wt").to_str().unwrap(),
        &["beta", "select"],
        repo.root_path(),
        &env_vars,
        &["feature", "2", "\x1b"], // Type "feature", 2, Escape
    );

    assert_valid_abort_exit_code(exit_code);

    let screen = render_terminal_screen(&raw_output);
    let normalized = normalize_paths(&screen);
    assert_snapshot!("select_preview_history", normalized);
}

/// Test preview panel 3: main…± shows diff vs main branch
#[test]
#[ignore]
fn test_select_preview_panel_main_diff() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    let feature_path = repo.add_worktree("feature", "feature");

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
    let (raw_output, exit_code) = exec_in_pty_with_input_sequence(
        get_cargo_bin("wt").to_str().unwrap(),
        &["beta", "select"],
        repo.root_path(),
        &env_vars,
        &["feature", "3", "\x1b"], // Type "feature", 3, Escape
    );

    assert_valid_abort_exit_code(exit_code);

    let screen = render_terminal_screen(&raw_output);
    let normalized = normalize_paths(&screen);
    assert_snapshot!("select_preview_main_diff", normalized);
}
