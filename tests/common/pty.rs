//! PTY execution helpers for integration tests.
//!
//! Three public functions — compose `build_pty_command` with a runner:
//!
//! - **`build_pty_command`** — builds a `CommandBuilder` with env isolation
//! - **`exec_cmd_in_pty`** — pre-buffers input, for non-interactive commands
//! - **`exec_cmd_in_pty_prompted`** — waits for prompt marker before each input
//!
//! ```ignore
//! use crate::common::pty::{build_pty_command, exec_cmd_in_pty_prompted};
//!
//! let cmd = build_pty_command("wt", &["switch", "feature"], dir, &env, None);
//! let (output, exit_code) = exec_cmd_in_pty_prompted(cmd, &["y\n"], "[y/N");
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

/// Build a CommandBuilder with standard PTY isolation and env vars.
///
/// Compose with `exec_cmd_in_pty` or `exec_cmd_in_pty_prompted`:
///
/// ```ignore
/// let cmd = build_pty_command("wt", &["switch", "feature"], dir, &env, None);
/// let (output, exit_code) = exec_cmd_in_pty(cmd, "y\n");
/// ```
pub fn build_pty_command(
    command: &str,
    args: &[&str],
    working_dir: &Path,
    env_vars: &[(String, String)],
    home_dir: Option<&Path>,
) -> CommandBuilder {
    let mut cmd = CommandBuilder::new(command);
    for arg in args {
        cmd.arg(*arg);
    }
    cmd.cwd(working_dir);

    super::configure_pty_command(&mut cmd);

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
        #[cfg(windows)]
        cmd.env("USERPROFILE", home.to_string_lossy().to_string());
        // Suppress nushell auto-detection for deterministic PTY tests
        cmd.env("WORKTRUNK_TEST_NUSHELL_ENV", "0");
    }

    cmd
}

/// Execute a CommandBuilder in a PTY, writing all input immediately.
///
/// Drops the writer before waiting for the child to signal EOF — non-interactive
/// commands may block on stdin until it closes.
///
/// For interactive prompts, use `exec_cmd_in_pty_prompted` instead (it waits
/// for the child before dropping the writer to avoid PTY echo artifacts).
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

    let (buf, exit_code) = read_pty_output(reader, writer, pair.master, &mut child);
    let normalized = buf.replace("\r\n", "\n");

    (normalized, exit_code)
}

/// Execute a CommandBuilder in a PTY, waiting for prompts before sending input.
///
/// For each element of `inputs`, waits until `prompt_marker` appears in the
/// output, then writes that input. This produces output where the echo appears
/// after the prompt — matching real terminal behavior.
pub fn exec_cmd_in_pty_prompted(
    cmd: CommandBuilder,
    inputs: &[&str],
    prompt_marker: &str,
) -> (String, i32) {
    let pair = super::open_pty();

    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);

    let reader = pair.master.try_clone_reader().unwrap();
    let writer = pair.master.take_writer().unwrap();

    prompted_pty_interaction(reader, writer, &mut child, inputs, prompt_marker)
}

/// Core prompt-waiting logic shared by all `_prompted` variants.
///
/// Reads PTY output in a background thread while the main thread waits for
/// `prompt_marker` to appear before sending each input. After all inputs are
/// sent, waits for the child to exit, then drops the writer.
fn prompted_pty_interaction(
    reader: Box<dyn std::io::Read + Send>,
    writer: Box<dyn std::io::Write + Send>,
    child: &mut Box<dyn portable_pty::Child + Send + Sync>,
    inputs: &[&str],
    prompt_marker: &str,
) -> (String, i32) {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let (tx, rx) = mpsc::channel::<Vec<u8>>();

    // Read PTY output in background, sending chunks via channel
    let reader_thread = std::thread::spawn(move || {
        let mut reader = reader;
        let mut buf = [0u8; 4096];
        loop {
            match std::io::Read::read(&mut reader, &mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let mut accumulated = Vec::new();
    let mut writer = writer;
    let timeout = Duration::from_secs(10);
    let poll = Duration::from_millis(10);
    let marker = prompt_marker.as_bytes();

    // For each input, wait for a NEW prompt marker to appear, then send
    let mut markers_seen: usize = 0;
    for input in inputs {
        let target = markers_seen + 1;
        let start = Instant::now();

        loop {
            while let Ok(chunk) = rx.try_recv() {
                accumulated.extend_from_slice(&chunk);
            }

            if count_marker_occurrences(&accumulated, marker) >= target {
                markers_seen = target;
                break;
            }

            if start.elapsed() > timeout {
                panic!(
                    "Timed out waiting for prompt marker {:?} (occurrence {}). Output so far:\n{}",
                    prompt_marker,
                    target,
                    String::from_utf8_lossy(&accumulated)
                );
            }

            std::thread::sleep(poll);
        }

        // Brief drain: give the PTY time to deliver remaining prompt bytes
        // after the marker. Without this, the echo of our input can interleave
        // with trailing prompt bytes (ANSI resets, spaces) that were part of
        // the same write but arrived in a separate read chunk. This race
        // manifests as non-deterministic output ordering on macOS.
        std::thread::sleep(Duration::from_millis(50));
        while let Ok(chunk) = rx.try_recv() {
            accumulated.extend_from_slice(&chunk);
        }

        writer.write_all(input.as_bytes()).unwrap();
        writer.flush().unwrap();
    }

    // Wait for child to exit BEFORE dropping writer.
    //
    // portable_pty's UnixMasterWriter::drop() sends \n + EOT to the PTY.
    // If dropped while the child is still running, the terminal echoes this
    // \n as \r\n, creating a spurious blank line in the captured output.
    // By waiting for the child first, the slave side closes and the echo
    // from the Drop's \n goes to a dead PTY — no artifact.
    //
    // The child won't hang: after read_line() returns for all prompts, it
    // continues executing without reading stdin. EOF isn't needed.
    let exit_status = child.wait().unwrap();
    let exit_code = exit_status.exit_code() as i32;

    // Now safe to drop writer (child already exited, slave side closed)
    drop(writer);

    // Wait for reader thread to finish
    let _ = reader_thread.join();

    // Drain any remaining chunks
    while let Ok(chunk) = rx.try_recv() {
        accumulated.extend_from_slice(&chunk);
    }

    let buf = String::from_utf8_lossy(&accumulated).to_string();
    let normalized = buf.replace("\r\n", "\n");

    (normalized, exit_code)
}

fn count_marker_occurrences(haystack: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() || needle.len() > haystack.len() {
        return 0;
    }
    haystack
        .windows(needle.len())
        .filter(|w| *w == needle)
        .count()
}
