# ConPTY Pipe Closure Issue - Read Blocks Forever When Child Process Exits

## Executive Summary

We are developing a cross-platform CLI tool called "worktrunk" (wt) that manages git worktrees. Our integration tests use pseudo-terminals (PTYs) to test interactive shell integration. On Windows, using the `portable_pty` Rust crate (which uses ConPTY under the hood), we encounter a blocking read issue: when a child process exits, the PTY read pipe does not close, causing `read_to_string()` to block forever waiting for EOF.

## Goals

1. **Primary Goal**: Get Windows PTY-based tests working in CI so we can test PowerShell shell integration
2. **Secondary Goal**: Understand why ConPTY behaves differently from Unix PTYs regarding pipe closure
3. **Tertiary Goal**: Find a reliable workaround or alternative approach for Windows PTY testing

## Technical Context

### The portable_pty Crate

We use the `portable_pty` Rust crate (https://github.com/wez/wezterm/tree/main/pty) which provides cross-platform PTY support:
- On Unix: Uses native PTY (`/dev/ptmx` or equivalent)
- On Windows: Uses ConPTY (Windows Console Pseudo Terminal API, available since Windows 10 1809)

### Our PTY Test Infrastructure

We have a helper module for executing commands in PTYs:

```rust
// tests/common/pty.rs

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
    child: &mut Box<dyn portable_pty::Child + Send + Sync>,
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
pub fn exec_in_pty(
    command: &str,
    args: &[&str],
    working_dir: &Path,
    env_vars: &[(String, String)],
    input: &str,
) -> (String, i32) {
    exec_in_pty_impl(command, args, working_dir, env_vars, input, None)
}

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

    // Override HOME if provided
    if let Some(home) = home_dir {
        cmd.env("HOME", home.to_string_lossy().to_string());
        cmd.env("XDG_CONFIG_HOME", home.join(".config").to_string_lossy().to_string());
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
```

### PTY Opening Code

```rust
// tests/common/mod.rs

/// Open a PTY pair for tests
pub fn open_pty() -> portable_pty::PtyPair {
    use portable_pty::{native_pty_system, PtySize};

    let pty_system = native_pty_system();
    pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("Failed to open PTY")
}

/// Configure a PTY command with isolated environment
pub fn configure_pty_command(cmd: &mut portable_pty::CommandBuilder) {
    cmd.env_clear();

    let home_dir = home::home_dir().unwrap().to_string_lossy().to_string();
    cmd.env("HOME", &home_dir);
    cmd.env("PATH", std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string()));

    #[cfg(windows)]
    {
        // Windows needs these environment variables for DLLs to load
        cmd.env("USERPROFILE", &home_dir);
        if let Ok(val) = std::env::var("SystemRoot") {
            cmd.env("SystemRoot", &val);
            cmd.env("windir", &val);
        }
        if let Ok(val) = std::env::var("SystemDrive") {
            cmd.env("SystemDrive", val);
        }
        if let Ok(val) = std::env::var("TEMP") {
            cmd.env("TEMP", &val);
            cmd.env("TMP", val);
        }
        if let Ok(val) = std::env::var("COMSPEC") {
            cmd.env("COMSPEC", val);
        }
        if let Ok(val) = std::env::var("PSModulePath") {
            cmd.env("PSModulePath", val);
        }
    }

    // Pass through coverage environment variables
    pass_coverage_env_to_pty_cmd(cmd);
}
```

## What We Have Tried

### Diagnostic Test 1: Basic PowerShell Spawn

```rust
#[test]
fn test_diag_01_pwsh_spawn_basic() {
    use portable_pty::CommandBuilder;
    use std::io::Read;
    use std::time::{Duration, Instant};

    eprintln!("DIAG01: Starting basic pwsh spawn test");

    let pair = crate::common::open_pty();
    eprintln!("DIAG01: PTY opened successfully");

    let mut cmd = CommandBuilder::new("pwsh");
    cmd.arg("-NoProfile");
    cmd.arg("-NonInteractive");
    cmd.arg("-Command");
    cmd.arg("Write-Host 'HELLO_FROM_PWSH'; exit 0");

    eprintln!("DIAG01: Spawning pwsh...");
    let start = Instant::now();
    let mut child = pair.slave.spawn_command(cmd).expect("Failed to spawn pwsh");
    eprintln!("DIAG01: pwsh spawned in {:?}", start.elapsed());

    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().unwrap();
    eprintln!("DIAG01: Reading output (with 30s timeout)...");

    // Use a thread with timeout for reading
    let (tx, rx) = std::sync::mpsc::channel();
    let read_thread = std::thread::spawn(move || {
        let mut buf = String::new();
        let result = reader.read_to_string(&mut buf);
        let _ = tx.send(());
        (buf, result)
    });

    // Wait up to 30 seconds for the read to complete
    let read_result = rx.recv_timeout(Duration::from_secs(30));
    let output = match read_result {
        Ok(()) => {
            let (buf, _) = read_thread.join().unwrap();
            buf
        }
        Err(_) => {
            eprintln!("DIAG01: TIMEOUT waiting for output after 30s");
            if read_thread.is_finished() {
                let (buf, _) = read_thread.join().unwrap();
                buf
            } else {
                eprintln!("DIAG01: Read thread still blocked");
                String::from("<timeout - no output>")
            }
        }
    };

    eprintln!("DIAG01: Output received: {:?}", output);

    let status = child.wait().expect("Failed to wait for child");
    eprintln!("DIAG01: Exit code: {:?}", status.exit_code());

    assert!(
        output.contains("HELLO_FROM_PWSH"),
        "Should see pwsh output. Got: {}",
        output
    );
}
```

**Result in CI:**
```
DIAG01: Starting basic pwsh spawn test
DIAG01: PTY opened successfully
DIAG01: Spawning pwsh...
DIAG01: pwsh spawned in 10.6308ms
DIAG01: Reading output (with 30s timeout)...
DIAG01: TIMEOUT waiting for output after 30s
DIAG01: Read thread still blocked
DIAG01: Output received: "<timeout - no output>"

(test timed out)
```

### Diagnostic Test 2: cmd.exe (Even Simpler)

```rust
#[test]
fn test_diag_08_cmd_exe_basic() {
    use portable_pty::CommandBuilder;
    use std::io::Read;

    eprintln!("DIAG08: Testing cmd.exe via PTY");

    let pair = crate::common::open_pty();

    let mut cmd = CommandBuilder::new("cmd.exe");
    cmd.arg("/C");
    cmd.arg("echo CMD_EXE_WORKS");

    eprintln!("DIAG08: Spawning cmd.exe...");
    let mut child = pair.slave.spawn_command(cmd).expect("Failed to spawn");
    drop(pair.slave);

    // Try dropping writer
    let writer = pair.master.take_writer().unwrap();
    drop(writer);

    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut buf = String::new();
    eprintln!("DIAG08: Reading output...");
    reader.read_to_string(&mut buf).ok();

    eprintln!("DIAG08: Output: {:?}", buf);

    let status = child.wait().expect("Failed to wait");
    eprintln!("DIAG08: Exit code: {:?}", status.exit_code());

    assert!(
        buf.contains("CMD_EXE_WORKS"),
        "cmd.exe should work. Got: {}",
        buf
    );
}
```

**Result:** Also timed out - even `cmd.exe /C "echo hello"` blocks forever on read.

### Diagnostic Test 3: Wait-then-Read Pattern

```rust
#[test]
fn test_diag_11_wait_then_read() {
    use portable_pty::CommandBuilder;
    use std::io::Read;

    eprintln!("DIAG11: Testing wait-then-read pattern");

    let pair = crate::common::open_pty();

    let mut cmd = CommandBuilder::new("cmd.exe");
    cmd.arg("/C");
    cmd.arg("echo WAIT_THEN_READ");

    eprintln!("DIAG11: Spawning cmd.exe...");
    let mut child = pair.slave.spawn_command(cmd).expect("Failed to spawn");
    drop(pair.slave);

    // Drop writer
    let writer = pair.master.take_writer().unwrap();
    drop(writer);

    // Wait for child to exit FIRST
    eprintln!("DIAG11: Waiting for child to exit...");
    let status = child.wait().expect("Failed to wait");
    eprintln!("DIAG11: Child exited with: {:?}", status.exit_code());

    // THEN read output
    eprintln!("DIAG11: Now reading output...");
    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut buf = String::new();
    reader.read_to_string(&mut buf).ok();

    eprintln!("DIAG11: Output: {:?}", buf);

    assert!(
        buf.contains("WAIT_THEN_READ"),
        "Wait-then-read should work. Got: {}",
        buf
    );
}
```

**Result:**
```
DIAG11: Testing wait-then-read pattern
DIAG11: Spawning cmd.exe...
DIAG11: Waiting for child to exit...
DIAG11: Child exited with: 3221225786
DIAG11: Now reading output...

(test timed out)
```

**Note:** Exit code 3221225786 is `0xC0000138` = `STATUS_DLL_NOT_FOUND`. This was before we added the Windows environment variables fix.

### Diagnostic Test 4: Using exec_in_pty Helper

```rust
#[test]
fn test_diag_14_exec_in_pty_helper() {
    use crate::common::pty::exec_in_pty;

    eprintln!("DIAG14: Testing with exec_in_pty() helper");

    let wt_bin = get_cargo_bin("wt");
    eprintln!("DIAG14: wt binary: {:?}", wt_bin);

    let tmp = tempfile::tempdir().unwrap();
    let (output, exit_code) = exec_in_pty(
        wt_bin.to_str().unwrap(),
        &["--version"],
        tmp.path(),
        &[],
        "",
    );

    eprintln!("DIAG14: Output: {:?}", output);
    eprintln!("DIAG14: Exit code: {}", exit_code);

    assert_eq!(
        exit_code, 0,
        "wt --version should succeed. Output: {}",
        output
    );
    assert!(
        output.contains("wt") || output.contains("worktrunk"),
        "Should contain version. Output: {}",
        output
    );
}
```

**Result:**
```
DIAG14: Testing with exec_in_pty() helper
DIAG14: wt binary: "D:\\a\\worktrunk\\worktrunk\\target\\debug\\wt.exe"
DIAG14: Output: "\u{1b}[6n\u{1b}[?9001h\u{1b}[?1004h\u{1b}[?25l\u{1b}[?9001l\u{1b}[?1004l\u{1b}[2J\u{1b}[m\u{1b}[H\u{1b}]0;D:\\a\\worktrunk\\worktrunk\\target\\debug\\wt.exe\u{7}\u{1b}[?25h"
DIAG14: Exit code: -1073741510

assertion `left == right` failed: wt --version should succeed. Output:
  left: -1073741510
  right: 0
```

**Note:** Exit code -1073741510 is `0xC000013A` = `STATUS_CONTROL_C_EXIT`. The process was killed with CTRL+C!

The output shows ANSI escape sequences:
- `\u{1b}[6n` - Cursor Position Report request
- `\u{1b}[?9001h` - Enable mouse tracking
- `\u{1b}[?1004h` - Enable focus reporting
- `\u{1b}[?25l` - Hide cursor
- `\u{1b}[2J` - Clear screen
- `\u{1b}]0;...` - Set window title

These are ConPTY initialization sequences, but no actual command output.

### Diagnostic Test 5: std::process::Command (No PTY)

```rust
#[test]
fn test_diag_10_no_pty_cmd_works() {
    use std::process::Command;

    eprintln!("DIAG10: Testing std::process::Command (no PTY)");

    let output = Command::new("cmd.exe")
        .args(["/C", "echo NO_PTY_WORKS"])
        .output()
        .expect("Failed to run cmd.exe");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!("DIAG10: stdout: {:?}", stdout);
    eprintln!("DIAG10: stderr: {:?}", stderr);
    eprintln!("DIAG10: exit code: {:?}", output.status.code());

    assert!(
        stdout.contains("NO_PTY_WORKS"),
        "std::process::Command should work. Got: {}",
        stdout
    );
}
```

**Result:** PASSED! This confirms the issue is specific to ConPTY, not general Windows process spawning.

## Key Findings

### 1. ConPTY Does Not Close Read Pipe on Child Exit

On Unix PTYs, when the child process exits, the slave side of the PTY is closed, which causes reads on the master side to return EOF. This allows blocking `read_to_string()` to complete.

On Windows ConPTY, when the child process exits, the read pipe apparently remains open, causing `read_to_string()` to block forever waiting for more data.

### 2. Dropping the Master PTY Sends CTRL+C

When we try to work around the blocking read by dropping the master PTY after the child exits, the child process receives a CTRL+C signal and exits with `STATUS_CONTROL_C_EXIT` (0xC000013A).

This suggests that:
- Either the child hasn't actually exited yet when `child.wait()` returns
- Or dropping the ConPTY master sends a signal to the attached console

### 3. DLL Loading Issues with env_clear()

When we call `cmd.env_clear()` to isolate the test environment, Windows processes fail to load with `STATUS_DLL_NOT_FOUND` (0xC0000138) because they need certain environment variables:
- `SystemRoot` / `windir` - Windows system directory
- `SystemDrive` - Usually C:
- `USERPROFILE` - User's home directory
- `TEMP` / `TMP` - Temporary directory
- `COMSPEC` - Path to cmd.exe
- `PSModulePath` - PowerShell module paths

We fixed this by adding these variables back after `env_clear()`, but the pipe closure issue persists.

### 4. ANSI Sequences Are Received

DIAG14 showed that we DO receive some output - specifically ConPTY initialization escape sequences. This means the pipe is working for initial output, but:
- The actual command output is not received, OR
- The command is killed before it can produce output

## Our Assumptions

1. **Assumption**: `child.wait()` blocks until the child process actually exits
   - **Status**: Uncertain - exit codes suggest process may be killed during our operations

2. **Assumption**: Dropping the master PTY is a safe way to signal EOF
   - **Status**: DISPROVEN - it sends CTRL+C to the child

3. **Assumption**: ConPTY should behave similarly to Unix PTYs for basic operations
   - **Status**: DISPROVEN - pipe closure semantics are different

4. **Assumption**: The portable_pty crate correctly wraps ConPTY
   - **Status**: Unknown - may be a crate bug or expected ConPTY behavior

## Open Questions

### Q1: How does ConPTY signal EOF to readers?

On Unix, closing the slave FD signals EOF to readers of the master. How does this work on ConPTY? Is there a specific API call needed?

### Q2: Why does dropping the master send CTRL+C?

Is this expected ConPTY behavior? Is there a flag to prevent this? Should we be using a different shutdown sequence?

### Q3: What's the correct shutdown sequence for ConPTY?

The portable_pty documentation doesn't seem to cover this. What's the proper way to:
1. Wait for child to finish
2. Read all output
3. Clean up resources

### Q4: Are there portable_pty issues reported for this?

Has anyone else encountered this? Are there open issues on the wezterm/portable_pty repository?

### Q5: How does WezTerm itself handle this?

WezTerm is a terminal emulator that uses the same portable_pty crate. How does it handle reading output from ConPTY without blocking forever?

### Q6: Are there alternative approaches?

Could we use:
- Named pipes directly instead of ConPTY?
- Windows-specific PTY APIs with different flags?
- A completely different testing strategy for Windows (e.g., process spawning without PTY)?

## Environment

- **CI Environment**: GitHub Actions Windows runner (windows-latest)
- **Rust Version**: Latest stable (1.75+)
- **portable_pty Version**: From wezterm crate ecosystem
- **Windows Version**: Windows Server 2022 (GitHub Actions)

## Desired Outcome

We want one of:

1. **A working ConPTY solution**: Properly read output and detect child exit
2. **A documented workaround**: Some sequence of operations that works reliably
3. **An alternative approach**: Different way to test PowerShell integration on Windows

## References

- portable_pty crate: https://github.com/wez/wezterm/tree/main/pty
- ConPTY documentation: https://docs.microsoft.com/en-us/windows/console/creating-a-pseudoconsole-session
- WezTerm (uses portable_pty): https://github.com/wez/wezterm

## Code Repository

The code is at https://github.com/max-sixty/worktrunk on the `windows-users` branch. The relevant files are:
- `tests/common/pty.rs` - PTY execution helpers
- `tests/common/mod.rs` - PTY opening and configuration
- `tests/integration_tests/shell_wrapper.rs` - The actual tests (PowerShell tests are `#[ignore]`)
