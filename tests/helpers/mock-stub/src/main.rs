//! Windows-compatible mock executable stub.
//!
//! When invoked as `foo.exe`, runs `bash foo <args>` where `foo` is a companion
//! shell script in the same directory. This allows Rust's Command::new() to find
//! mock commands on Windows (CreateProcessW only searches for .exe files).
//!
//! Usage in tests:
//! 1. Copy mock-stub.exe to bin_dir/gh.exe
//! 2. Write shell script to bin_dir/gh
//! 3. Command::new("gh") now works on Windows

use std::env;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio, exit};

/// Convert Windows path to MSYS2/Git Bash style path.
///
/// Git Bash expects Unix-style paths. When we pass a Windows path like
/// `D:\temp\bin\gh` to bash, it doesn't understand it. Convert to `/d/temp/bin/gh`.
fn to_msys_path(path: &Path) -> String {
    let path_str = path.to_string_lossy();

    // Check for Windows drive letter (e.g., "D:\...")
    let chars: Vec<char> = path_str.chars().collect();
    if chars.len() >= 2 && chars[1] == ':' {
        // D:\foo\bar -> /d/foo/bar
        let drive = chars[0].to_ascii_lowercase();
        format!("/{}{}", drive, path_str[2..].replace('\\', "/"))
    } else {
        // Already Unix-style or relative path
        path_str.replace('\\', "/")
    }
}

fn main() {
    // Always write a debug marker for CI debugging
    // This tells us if mock-stub.exe even starts running
    use std::fs::OpenOptions;
    use std::io::Write;
    // Use TEMP on Windows, /tmp on Unix
    let debug_log_path =
        env::var("TEMP").unwrap_or_else(|_| "/tmp".to_string()) + "/mock-stub-debug.log";
    // Open debug log file once and reuse
    let mut debug_log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&debug_log_path)
        .ok();
    macro_rules! debug_log {
        ($($arg:tt)*) => {
            if let Some(ref mut f) = debug_log {
                let _ = writeln!(f, $($arg)*);
            }
        };
    }
    debug_log!("=== mock-stub invocation ===");
    debug_log!("args: {:?}", std::env::args().collect::<Vec<_>>());
    debug_log!("exe_path: {:?}", std::env::current_exe().ok());

    let exe_path = env::current_exe().expect("failed to get executable path");

    // Strip .exe extension to get companion script path
    // e.g., /tmp/bin/gh.exe -> /tmp/bin/gh
    let script_path = exe_path.with_extension("");
    let script_dir = script_path
        .parent()
        .expect("mock-stub: script path has no parent directory");

    debug_log!("script_path: {}", script_path.display());
    debug_log!("script_path exists: {}", script_path.exists());

    // Distinguish setup errors from environment errors
    if !script_path.exists() {
        debug_log!("ERROR: companion script not found!");
        eprintln!(
            "mock-stub: companion script not found: {}",
            script_path.display()
        );
        eprintln!("Check that write_mock_script() created the script file.");
        exit(1);
    }

    // Convert to MSYS2-style path for bash on Windows
    let script_path_str = to_msys_path(&script_path);
    let script_dir_str = to_msys_path(script_dir);

    debug_log!("script_path_str (MSYS): {}", script_path_str);
    debug_log!("script_dir_str (MSYS): {}", script_dir_str);

    // Forward all arguments to bash with the script
    let args: Vec<String> = env::args().skip(1).collect();
    debug_log!("calling: bash {} {:?}", script_path_str, args);

    // Debug: Show what we're about to execute (only when MOCK_DEBUG is set)
    if env::var("MOCK_DEBUG").is_ok() {
        eprintln!("mock-stub: exe_path={}", exe_path.display());
        eprintln!("mock-stub: script_path={}", script_path.display());
        eprintln!("mock-stub: script_path_str={}", script_path_str);
        eprintln!("mock-stub: script_dir={}", script_dir.display());
        eprintln!("mock-stub: script_dir_str={}", script_dir_str);
        eprintln!("mock-stub: args={:?}", args);
    }

    // Use .output() to capture stdout/stderr, then forward them.
    // This is more reliable than relying on handle inheritance on Windows,
    // which can fail silently when pipes are involved.
    //
    // Pass MOCK_SCRIPT_DIR so scripts can reliably find sibling files.
    // This avoids $0/dirname/pwd issues in Git Bash on Windows CI.
    let output = Command::new("bash")
        .arg(&script_path_str)
        .args(&args)
        .env("MOCK_SCRIPT_DIR", &script_dir_str)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap_or_else(|e| {
            debug_log!("ERROR: failed to execute bash: {}", e);
            eprintln!("mock-stub: failed to execute bash: {e}");
            eprintln!("Is Git Bash installed and in PATH?");
            exit(1);
        });

    debug_log!(
        "bash result: exit={} stdout_len={} stderr_len={}",
        output.status.code().unwrap_or(-1),
        output.stdout.len(),
        output.stderr.len()
    );
    if !output.stdout.is_empty() {
        debug_log!(
            "stdout (first 200): {}",
            String::from_utf8_lossy(&output.stdout[..output.stdout.len().min(200)])
        );
    }
    if !output.stderr.is_empty() {
        debug_log!(
            "stderr (first 500): {}",
            String::from_utf8_lossy(&output.stderr[..output.stderr.len().min(500)])
        );
    }

    // Debug: Show exit code and output lengths
    if env::var("MOCK_DEBUG").is_ok() {
        eprintln!(
            "mock-stub: exit={} stdout_len={} stderr_len={}",
            output.status.code().unwrap_or(-1),
            output.stdout.len(),
            output.stderr.len()
        );
        if !output.stderr.is_empty() {
            eprintln!(
                "mock-stub: stderr={}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    // Forward captured output to our stdout/stderr.
    // Must flush before exit() since exit() doesn't run destructors.
    io::stdout().write_all(&output.stdout).unwrap();
    io::stdout().flush().unwrap();
    io::stderr().write_all(&output.stderr).unwrap();
    io::stderr().flush().unwrap();

    exit(output.status.code().unwrap_or(1));
}
