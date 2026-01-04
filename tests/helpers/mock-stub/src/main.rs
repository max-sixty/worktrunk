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
use std::process::{Command, exit};

fn main() {
    let exe_path = env::current_exe().expect("failed to get executable path");

    // Strip .exe extension to get companion script path
    // e.g., /tmp/bin/gh.exe -> /tmp/bin/gh
    let script_path = exe_path.with_extension("");

    // Distinguish setup errors from environment errors
    if !script_path.exists() {
        eprintln!(
            "mock-stub: companion script not found: {}",
            script_path.display()
        );
        eprintln!("Check that write_mock_script() created the script file.");
        exit(1);
    }

    // Forward all arguments to bash with the script
    let args: Vec<String> = env::args().skip(1).collect();

    let status = Command::new("bash")
        .arg(&script_path)
        .args(&args)
        .status()
        .unwrap_or_else(|e| {
            eprintln!("mock-stub: failed to execute bash: {e}");
            eprintln!("Is Git Bash installed and in PATH?");
            exit(1);
        });

    exit(status.code().unwrap_or(1));
}
