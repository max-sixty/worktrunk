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
/// On Windows ConPTY, the pipe doesn't close properly, so we:
/// 1. Start reading in a background thread
/// 2. Wait for child to exit
/// 3. Drop the master to signal EOF
/// 4. Join the read thread with timeout
fn read_pty_output(
    reader: Box<dyn Read + Send>,
    master: Box<dyn MasterPty + Send>,
    child: &mut portable_pty::Child,
) -> (String, i32) {
    #[cfg(unix)]
    {
        let _ = master; // Not needed on Unix
        let mut reader = reader;
        let mut buf = String::new();
        reader.read_to_string(&mut buf).unwrap();
        let exit_status = child.wait().unwrap();
        (buf, exit_status.exit_code() as i32)
    }

    #[cfg(windows)]
    {
        use std::sync::mpsc;
        use std::thread;
        use std::time::Duration;

        let (tx, rx) = mpsc::channel();
        let read_thread = thread::spawn(move || {
            let mut reader = reader;
            let mut buf = String::new();
            let _ = reader.read_to_string(&mut buf);
            let _ = tx.send(());
            buf
        });

        // Wait for child to exit first
        let exit_status = child.wait().unwrap();
        let exit_code = exit_status.exit_code() as i32;

        // Drop the master to close the PTY and signal EOF
        drop(master);

        // Wait for read to complete (should be quick after master drop)
        let buf = match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(()) => read_thread.join().unwrap(),
            Err(_) => {
                eprintln!("Warning: PTY read timed out after child exit");
                String::new()
            }
        };

        (buf, exit_code)
    }
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
    drop(writer); // Close writer so command sees EOF

    // Read output and wait for exit (platform-specific handling)
    let (buf, exit_code) = read_pty_output(reader, pair.master, &mut child);

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
    drop(writer);

    // Read output and wait for exit (platform-specific handling)
    let (buf, exit_code) = read_pty_output(reader, pair.master, &mut child);

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
    drop(writer);

    // Read output and wait for exit (platform-specific handling)
    let (buf, exit_code) = read_pty_output(reader, pair.master, &mut child);

    // Normalize CRLF to LF
    let normalized = buf.replace("\r\n", "\n");

    (normalized, exit_code)
}
