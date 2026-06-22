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
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

/// Read output from PTY and wait for child exit.
///
/// On Unix, completion is detected **causally** by polling the child shell for
/// exit, *not* by reading the master to EOF. The naive "read to EOF, then
/// `wait()`" shape deadlocks the run when the session-leader shell fails to
/// terminate — e.g. a job-control SIGTTOU race under parallel load can leave it
/// stopped (`T`). Because that shell owns the slave's controlling terminal, the
/// kernel never revokes the tty, the master never sees EOF, and *both*
/// `read_to_string` and `wait()` block until the test harness's 180s timeout
/// kills the run. (Conversely, once the shell *does* exit, BSD/macOS revokes the
/// tty and the master EOFs even if an orphaned grandchild still holds a slave
/// fd — so a lingering child can't starve the read.)
///
/// So: drain the master on a background thread, treat the shell's exit as the
/// done signal (bounded by a generous fallback that kills a wedged shell so the
/// run ends in seconds, not minutes), then drain any trailing in-flight output.
///
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
        // 60s is a generous safety net: the causal path (child exit) resolves in
        // milliseconds, so this only bounds a genuinely wedged shell, and stays
        // well under the test harness's 180s slow-timeout.
        drain_pty_until_child_exit(reader, child, std::time::Duration::from_secs(60))
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

/// Wait for a PTY child to exit, using its exit as the completion signal and
/// bounding the wait so a wedged shell can't hang the run.
///
/// The normal path resolves in milliseconds. `wedge_timeout` only bounds a shell
/// that never exits on its own — a job-control race under parallel load can
/// leave the session-leader shell stopped (`T`), which `try_wait()` reports as
/// not-yet-exited forever. On overrun the child is killed (`portable_pty`'s
/// `kill` escalates to `SIGKILL`, which reaps even a stopped process) and `124`
/// is returned, mirroring coreutils `timeout`. A read error yields `-1`.
///
/// Shared by both PTY readers ([`read_pty_output`] via [`drain_pty_until_child_exit`]
/// and [`prompted_pty_interaction`]) so neither can block on a non-terminating
/// child.
fn wait_for_child_or_kill(
    child: &mut Box<dyn portable_pty::Child + Send + Sync>,
    wedge_timeout: std::time::Duration,
) -> i32 {
    let deadline = std::time::Instant::now() + wedge_timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.exit_code() as i32,
            Ok(None) => {}
            Err(_) => return -1,
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return 124; // timeout marker (nonzero, like coreutils `timeout`)
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// Drain remaining PTY output from `rx` after the child has exited, bounded so a
/// lingering slave-holder can't block us.
///
/// The reader thread drops its `tx` when it hits EOF, surfacing as
/// `Disconnected` once all output is delivered — the common case on BSD/macOS,
/// where the kernel revokes the tty when the session leader exits (so the master
/// EOFs even if an orphaned grandchild still holds a slave fd). On Linux the tty
/// isn't revoked, so a grandchild holding the slave keeps the reader blocked
/// past EOF; the command's own output is already buffered and collected here in
/// milliseconds, and `ceiling` caps the wait for the EOF that won't come (the
/// reader thread is then left to exit when the process does).
fn drain_remaining(rx: &mpsc::Receiver<Vec<u8>>, out: &mut Vec<u8>, ceiling: std::time::Duration) {
    let deadline = std::time::Instant::now() + ceiling;
    loop {
        match rx.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(data) => out.extend_from_slice(&data),
            // Reader hit EOF and dropped `tx`: all output captured.
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if std::time::Instant::now() >= deadline {
                    break;
                }
            }
        }
    }
}

/// Drain a Unix PTY master, using the child shell's exit — not master EOF — as
/// the completion signal.
///
/// `reader` is the master read end; `child` is the session-leader shell. The
/// reader runs on a background thread accumulating output into a channel;
/// completion is detected causally via [`wait_for_child_or_kill`], then
/// [`drain_remaining`] collects any trailing in-flight output.
///
/// See [`read_pty_output`] for the full rationale. Exposed (rather than inlined)
/// so the wedge-recovery path can be exercised with a short timeout in tests
/// without waiting out the production safety net.
#[cfg(unix)]
pub fn drain_pty_until_child_exit(
    reader: Box<dyn Read + Send>,
    child: &mut Box<dyn portable_pty::Child + Send + Sync>,
    wedge_timeout: std::time::Duration,
) -> (String, i32) {
    // Drain the master on a background thread. Dropping `tx` when the reader
    // hits EOF/error signals that all output has been delivered.
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(chunk[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let exit_code = wait_for_child_or_kill(child, wedge_timeout);

    let mut output = Vec::new();
    drain_remaining(&rx, &mut output, std::time::Duration::from_secs(5));

    let buf = String::from_utf8_lossy(&output).to_string();
    (buf, exit_code)
}

/// Find cursor position request (ESC[6n) in a byte slice.
/// Returns the position if found.
fn find_cursor_request(data: &[u8]) -> Option<usize> {
    // Look for ESC [ 6 n sequence (0x1b 0x5b 0x36 0x6e)
    let pattern = b"\x1b[6n";
    data.windows(pattern.len())
        .position(|window| window == pattern)
}

/// A PTY master writer shared between the reader thread (which answers terminal
/// queries) and the caller (which sends keystrokes). Both need to write to the
/// single master, and `portable_pty` hands out only one writer.
pub type SharedPtyWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// Spawn a thread that drains the PTY master `reader` into the returned channel
/// and answers the cursor-position report query (`ESC[6n`) skim emits while
/// initializing the picker.
///
/// skim 4.x runs the picker in partial-height mode, whose setup calls skim's
/// `cursor_pos_from_tty()`: it writes `ESC[6n` to `/dev/tty` and blocks in
/// `select()` for up to 3s waiting for the `ESC[row;colR` reply. A real terminal
/// answers automatically; `portable_pty` is a bare PTY with no emulation, so
/// without this reply skim fails init with "Cursor position detection timed out"
/// and the picker never renders. The reply (`1;1`) is a safe constant — skim only
/// uses it to place its inline viewport, which the TUI snapshot tests don't assert
/// on. Sharing `writer` with the caller lets keystrokes and query replies both
/// reach the master. The query is matched within a single read chunk; skim emits
/// the 4-byte DSR as one small write, so it never spans chunks.
pub fn spawn_pty_reader_answering_queries(
    reader: Box<dyn Read + Send>,
    writer: SharedPtyWriter,
) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut temp_buf = [0u8; 4096];
        loop {
            match reader.read(&mut temp_buf) {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = &temp_buf[..n];
                    if find_cursor_request(chunk).is_some()
                        && let Ok(mut w) = writer.lock()
                    {
                        let _ = w.write_all(b"\x1b[1;1R");
                        let _ = w.flush();
                    }
                    if tx.send(chunk.to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    rx
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
        // Suppress nushell auto-detection for deterministic PTY tests.
        // Other shell-installed defaults are picked up via STATIC_TEST_ENV_VARS
        // in callers that pass env_vars from TestRepo::test_env_vars.
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
    let _reader_thread = std::thread::spawn(move || {
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
    let timeout = Duration::from_secs(30);
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

        // Quiescence drain: after detecting the marker, wait until the PTY
        // goes quiet before sending input. Without this, trailing prompt bytes
        // (ANSI resets, spaces) that arrive in a separate read chunk interleave
        // with the echo of our input, producing non-deterministic output on macOS.
        let quiescence = Duration::from_millis(20);
        let drain_ceiling = Duration::from_millis(500);
        let drain_start = Instant::now();
        let mut last_data = Instant::now();
        loop {
            while let Ok(chunk) = rx.try_recv() {
                accumulated.extend_from_slice(&chunk);
                last_data = Instant::now();
            }
            if last_data.elapsed() >= quiescence {
                break;
            }
            if drain_start.elapsed() >= drain_ceiling {
                break;
            }
            std::thread::sleep(poll);
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
    // The child won't hang on stdin: after read_line() returns for all prompts,
    // it continues executing without reading. The wait is bounded so a
    // job-control-wedged shell that never exits can't hang the run either (see
    // wait_for_child_or_kill / read_pty_output).
    let exit_code = wait_for_child_or_kill(child, Duration::from_secs(60));

    // Now safe to drop writer (child already exited, slave side closed)
    drop(writer);

    // Collect any remaining output (the reader EOFs now that the child is gone),
    // bounded so we can't block here even if the master never EOFs.
    drain_remaining(&rx, &mut accumulated, Duration::from_secs(5));

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
