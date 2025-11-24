//! PTY-based tests for `wt list` command
//!
//! These tests verify the list command output in a real PTY environment where stdout is a TTY.
//! This allows testing the progressive rendering mode that uses indicatif progress bars.

use crate::common::TestRepo;
use insta::assert_snapshot;
use insta_cmd::get_cargo_bin;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::Read;
use std::path::Path;
use std::time::Duration;

/// Execute wt list in a PTY
///
/// Returns (combined_output, exit_code)
fn exec_wt_list_in_pty(
    working_dir: &Path,
    env_vars: &[(String, String)],
    args: &[&str],
) -> (String, i32) {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 48,
            cols: 150, // Match COLUMNS in standard tests
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();

    // Spawn wt list inside the PTY
    let mut cmd = CommandBuilder::new(get_cargo_bin("wt"));
    cmd.arg("list");
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

    // Deterministic test environment
    cmd.env("CLICOLOR_FORCE", "1");
    cmd.env("LANG", "C");
    cmd.env("LC_ALL", "C");
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null");
    cmd.env("GIT_CONFIG_SYSTEM", "/dev/null");
    cmd.env("GIT_AUTHOR_DATE", "2025-01-01T00:00:00Z");
    cmd.env("GIT_COMMITTER_DATE", "2025-01-01T00:00:00Z");
    cmd.env("SOURCE_DATE_EPOCH", "1761609600");

    // Add test-specific environment variables
    for (key, value) in env_vars {
        cmd.env(key, value);
    }

    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave); // Close slave in parent

    // Get reader for the PTY master
    let mut reader = pair.master.try_clone_reader().unwrap();

    // Read output with timeout to handle progressive rendering
    let mut buf = Vec::new();
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(10);

    loop {
        let mut temp_buf = [0u8; 4096];
        match reader.read(&mut temp_buf) {
            Ok(0) => break, // EOF
            Ok(n) => {
                buf.extend_from_slice(&temp_buf[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // Check if child has exited
                if let Ok(Some(_)) = child.try_wait() {
                    // Child exited, read remaining data
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
                if start.elapsed() > timeout {
                    panic!("Timeout waiting for command output");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => panic!("Failed to read PTY output: {}", e),
        }
    }

    // Wait for child to exit
    let exit_status = child.wait().unwrap();
    let exit_code = exit_status.exit_code() as i32;

    let output = String::from_utf8_lossy(&buf).to_string();
    (output, exit_code)
}

/// Normalize PTY output for snapshot testing
fn normalize_pty_output(output: &str) -> String {
    // Strip ANSI codes for easier snapshot reading
    let output = worktrunk::styling::strip_ansi_codes(output);

    // Normalize line endings
    output.replace("\r\n", "\n").replace('\r', "\n")
}

// Flaky: PTY progressive rendering has timing-dependent output that changes between runs.
// The non-PTY tests already verify status column rendering with emoji user status.
#[test]
#[ignore]
fn test_list_pty_status_column_padding_with_emoji() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create wli-sequence worktree with large diff
    let wli_seq = repo.add_worktree("wli-sequence", "wli-sequence");

    // Create initial content: 200 lines
    let initial_content = (1..=200)
        .map(|i| format!("original line {}", i))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(wli_seq.join("main.txt"), &initial_content).unwrap();

    let mut cmd = std::process::Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "main.txt"])
        .current_dir(&wli_seq)
        .output()
        .unwrap();

    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Initial content"])
        .current_dir(&wli_seq)
        .output()
        .unwrap();

    // Modify to create large diff: +164, -111 (roughly)
    let modified_content = (1..=253)
        .map(|i| {
            if i % 2 == 0 {
                format!("modified line {}", i)
            } else {
                format!("original line {}", i)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(wli_seq.join("main.txt"), &modified_content).unwrap();

    // Add untracked and modified files for Status symbols
    std::fs::write(wli_seq.join("untracked.txt"), "new file").unwrap();

    // Set user status emoji for wli-sequence
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["config", "worktrunk.status.wli-sequence", "🤖"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    // Create pr-link worktree with emoji
    let pr_link = repo.add_worktree("pr-link", "pr-link");
    std::fs::write(pr_link.join("pr.txt"), "PR commit").unwrap();
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "pr.txt"])
        .current_dir(&pr_link)
        .output()
        .unwrap();
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "PR commit"])
        .current_dir(&pr_link)
        .output()
        .unwrap();

    repo.configure_git_cmd(&mut cmd);
    cmd.args(["config", "worktrunk.status.pr-link", "🤖"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    // Create main-symbol worktree with emoji
    let main_symbol = repo.add_worktree("main-symbol", "main-symbol");
    std::fs::write(main_symbol.join("symbol.txt"), "Symbol commit").unwrap();
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "symbol.txt"])
        .current_dir(&main_symbol)
        .output()
        .unwrap();
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Symbol commit"])
        .current_dir(&main_symbol)
        .output()
        .unwrap();

    repo.configure_git_cmd(&mut cmd);
    cmd.args(["config", "worktrunk.status.main-symbol", "💬"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    // Run wt list in PTY
    let env_vars = vec![(
        "WORKTRUNK_CONFIG_PATH".to_string(),
        repo.test_config_path().to_string_lossy().to_string(),
    )];
    let (output, exit_code) = exec_wt_list_in_pty(repo.root_path(), &env_vars, &[]);

    assert_eq!(exit_code, 0, "wt list should succeed");

    let normalized = normalize_pty_output(&output);
    assert_snapshot!("list_pty_status_column_padding_emoji", normalized);
}
