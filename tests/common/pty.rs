//! PTY execution helpers for integration tests.
//!
//! Provides unified PTY execution with consistent:
//! - Environment isolation via `configure_pty_command()`
//! - CRLF normalization (PTYs use CRLF on some platforms)
//! - Coverage passthrough for subprocess coverage collection
//!
//! # Usage
//!
//! ```ignore
//! use crate::common::pty::exec_in_pty;
//!
//! // Simple execution with single input
//! let (output, exit_code) = exec_in_pty(
//!     "wt",
//!     &["switch", "--create", "feature"],
//!     repo.root_path(),
//!     &repo.test_env_vars(),
//!     "y\n",
//! );
//!
//! // With HOME override for shell config tests
//! let (output, exit_code) = exec_in_pty_with_home(
//!     "wt",
//!     &["config", "shell", "install"],
//!     repo.root_path(),
//!     &repo.test_env_vars(),
//!     "y\n",
//!     temp_home.path(),
//! );
//! ```

use portable_pty::{CommandBuilder, MasterPty};
use std::io::{Read, Write};
use std::path::Path;

/// Read output from PTY and wait for child exit.
///
/// On Unix, this simply reads to EOF then waits for child.
/// On Windows ConPTY, special handling is required because:
/// - The output pipe doesn't close when child exits (owned by pseudoconsole)
/// - ConPTY may send cursor position requests (ESC[6n) that must be answered
/// - ClosePseudoConsole must be called on a separate thread while draining output
///
/// See: https://learn.microsoft.com/en-us/windows/console/closepseudoconsole
pub fn read_pty_output(
    reader: Box<dyn Read + Send>,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: &mut Box<dyn portable_pty::Child + Send + Sync>,
) -> (String, i32) {
    #[cfg(unix)]
    {
        let _ = master; // Not needed on Unix
        // Drop writer to signal EOF to child's stdin (important for Unix PTYs)
        drop(writer);
        let mut reader = reader;
        let mut buf = String::new();
        reader.read_to_string(&mut buf).unwrap();
        let exit_status = child.wait().unwrap();
        (buf, exit_status.exit_code() as i32)
    }

    #[cfg(windows)]
    {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Arc, mpsc};
        use std::thread;
        use std::time::Duration;

        // Flag to signal the reader to stop
        let should_stop = Arc::new(AtomicBool::new(false));
        let should_stop_reader = should_stop.clone();

        // Channel for the reader to send back the output
        let (tx, rx) = mpsc::channel();

        // Spawn reader thread that drains output in chunks and responds to cursor queries
        let read_thread = thread::spawn(move || {
            let mut reader = reader;
            let mut writer = writer;
            let mut output = Vec::new();
            let mut temp_buf = [0u8; 4096];

            loop {
                // Check if we should stop
                if should_stop_reader.load(Ordering::Relaxed) {
                    // Do one final read attempt with short timeout
                    // (output might still be in the pipe)
                    break;
                }

                // Read with a short timeout by using non-blocking behavior
                // Unfortunately, portable_pty doesn't expose non-blocking reads,
                // so we do blocking reads but with a timeout signal from the main thread
                match reader.read(&mut temp_buf) {
                    Ok(0) => {
                        // EOF - pipe closed
                        break;
                    }
                    Ok(n) => {
                        let chunk = &temp_buf[..n];
                        output.extend_from_slice(chunk);

                        // Check for cursor position request (ESC[6n) and respond
                        // This is required when PSEUDOCONSOLE_INHERIT_CURSOR is set
                        if let Some(pos) = find_cursor_request(chunk) {
                            // Respond with cursor at position 1,1
                            // Format: ESC [ row ; col R
                            let response = b"\x1b[1;1R";
                            let _ = writer.write_all(response);
                            let _ = writer.flush();
                            // Log for debugging
                            eprintln!(
                                "ConPTY: Responded to cursor position request at byte {}",
                                pos
                            );
                        }
                    }
                    Err(e) => {
                        // Check if it's a "would block" or pipe closed error
                        if e.kind() == std::io::ErrorKind::WouldBlock {
                            thread::sleep(Duration::from_millis(10));
                            continue;
                        }
                        // Other errors - likely pipe closed
                        eprintln!("ConPTY: Read error: {}", e);
                        break;
                    }
                }
            }

            let _ = tx.send(output);
        });

        // Wait for child to exit
        let exit_status = child.wait().unwrap();
        let exit_code = exit_status.exit_code() as i32;

        // Signal the reader to stop
        should_stop.store(true, Ordering::Relaxed);

        // Close the master on a separate thread to avoid deadlock.
        // This triggers ClosePseudoConsole which sends CTRL_CLOSE_EVENT
        // and eventually closes the output pipe.
        //
        // We spawn this in parallel with recv_timeout because:
        // 1. ClosePseudoConsole might block waiting for output to drain
        // 2. We need to be checking for reader output while close happens
        // 3. Without parallelism, we could deadlock
        let close_thread = thread::spawn(move || {
            drop(master);
        });

        // Wait for the reader to finish (with timeout).
        // The close_thread runs in parallel, triggering pipe closure.
        let output = match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(data) => data,
            Err(_) => {
                eprintln!("ConPTY: Read thread timed out after child exit");
                Vec::new()
            }
        };

        // Don't join either thread - they may be stuck in blocking operations:
        // - read_thread may be stuck in read() waiting for data
        // - close_thread may be stuck in ClosePseudoConsole waiting for reader to drain
        //
        // These form a potential deadlock: ClosePseudoConsole waits for reader,
        // reader waits for ClosePseudoConsole to close the pipe.
        //
        // "Leaking" these threads is acceptable for test code - they'll be cleaned
        // up when the test process exits. We already have the output (or timed out).
        drop(close_thread);
        drop(read_thread);

        // Convert to string (lossy for any invalid UTF-8)
        let buf = String::from_utf8_lossy(&output).to_string();

        (buf, exit_code)
    }
}

/// Find cursor position request (ESC[6n) in a byte slice.
/// Returns the position if found.
#[cfg(windows)]
fn find_cursor_request(data: &[u8]) -> Option<usize> {
    // Look for ESC [ 6 n sequence (0x1b 0x5b 0x36 0x6e)
    let pattern = b"\x1b[6n";
    data.windows(pattern.len())
        .position(|window| window == pattern)
}

/// Execute a command in a PTY with optional interactive input.
///
/// Returns (combined_output, exit_code).
///
/// Output is normalized:
/// - CRLF → LF (PTYs use CRLF on some platforms)
///
/// Environment is isolated via `configure_pty_command()`:
/// - Cleared and rebuilt with minimal required vars
/// - Coverage env vars passed through
pub fn exec_in_pty(
    command: &str,
    args: &[&str],
    working_dir: &Path,
    env_vars: &[(String, String)],
    input: &str,
) -> (String, i32) {
    exec_in_pty_impl(command, args, working_dir, env_vars, input, None)
}

/// Execute a command in a PTY with HOME directory override.
///
/// Same as `exec_in_pty` but overrides HOME and XDG_CONFIG_HOME to the
/// specified directory. Use this for tests that need isolated shell config.
pub fn exec_in_pty_with_home(
    command: &str,
    args: &[&str],
    working_dir: &Path,
    env_vars: &[(String, String)],
    input: &str,
    home_dir: &Path,
) -> (String, i32) {
    exec_in_pty_impl(command, args, working_dir, env_vars, input, Some(home_dir))
}

/// Internal implementation with optional home override.
fn exec_in_pty_impl(
    command: &str,
    args: &[&str],
    working_dir: &Path,
    env_vars: &[(String, String)],
    input: &str,
    home_dir: Option<&Path>,
) -> (String, i32) {
    let pair = super::open_pty();

    let mut cmd = CommandBuilder::new(command);
    for arg in args {
        cmd.arg(*arg);
    }
    cmd.cwd(working_dir);

    // Set up isolated environment with coverage passthrough
    super::configure_pty_command(&mut cmd);

    // Add test-specific environment variables
    for (key, value) in env_vars {
        cmd.env(key, value);
    }

    // Override HOME if provided (must be after configure_pty_command which sets HOME)
    if let Some(home) = home_dir {
        cmd.env("HOME", home.to_string_lossy().to_string());
        cmd.env(
            "XDG_CONFIG_HOME",
            home.join(".config").to_string_lossy().to_string(),
        );
        // Windows: the `home` crate uses USERPROFILE for home_dir()
        #[cfg(windows)]
        cmd.env("USERPROFILE", home.to_string_lossy().to_string());
    }

    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave); // Close slave in parent

    // Get reader and writer for the PTY master
    let reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();

    // Write input to the PTY (simulating user typing)
    if !input.is_empty() {
        writer.write_all(input.as_bytes()).unwrap();
        writer.flush().unwrap();
    }

    // Read output and wait for exit (platform-specific handling)
    // Note: writer is passed to read_pty_output for ConPTY cursor response handling
    let (buf, exit_code) = read_pty_output(reader, writer, pair.master, &mut child);

    // Normalize CRLF to LF (PTYs use CRLF on some platforms)
    let normalized = buf.replace("\r\n", "\n");

    (normalized, exit_code)
}

/// Execute a pre-configured CommandBuilder in a PTY.
///
/// Use this when you need custom command configuration beyond what `exec_in_pty`
/// and `exec_in_pty_with_home` provide. You're responsible for:
/// - Setting up the command (binary, args, cwd)
/// - Calling `configure_pty_command()` or equivalent for env isolation
/// - Any additional env vars
///
/// Returns (combined_output, exit_code).
pub fn exec_cmd_in_pty(cmd: CommandBuilder, input: &str) -> (String, i32) {
    let pair = super::open_pty();

    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);

    let reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();

    if !input.is_empty() {
        writer.write_all(input.as_bytes()).unwrap();
        writer.flush().unwrap();
    }

    // Read output and wait for exit (platform-specific handling)
    let (buf, exit_code) = read_pty_output(reader, writer, pair.master, &mut child);

    // Normalize CRLF to LF
    let normalized = buf.replace("\r\n", "\n");

    (normalized, exit_code)
}

/// Execute a command in a PTY with multiple sequential inputs.
///
/// Each input is written and flushed before moving to the next.
/// Use this when multiple distinct user inputs are needed (e.g., multi-step prompts).
///
/// Returns (combined_output, exit_code).
pub fn exec_in_pty_multi_input(
    command: &str,
    args: &[&str],
    working_dir: &Path,
    env_vars: &[(String, String)],
    inputs: &[&str],
) -> (String, i32) {
    let pair = super::open_pty();

    let mut cmd = CommandBuilder::new(command);
    for arg in args {
        cmd.arg(*arg);
    }
    cmd.cwd(working_dir);

    // Set up isolated environment with coverage passthrough
    super::configure_pty_command(&mut cmd);

    // Add test-specific environment variables
    for (key, value) in env_vars {
        cmd.env(key, value);
    }

    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);

    let reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();

    // Write all inputs sequentially
    for input in inputs {
        writer.write_all(input.as_bytes()).unwrap();
        writer.flush().unwrap();
    }

    // Read output and wait for exit (platform-specific handling)
    let (buf, exit_code) = read_pty_output(reader, writer, pair.master, &mut child);

    // Normalize CRLF to LF
    let normalized = buf.replace("\r\n", "\n");

    (normalized, exit_code)
}
