//! Windows-specific shell integration tests.
//!
//! These tests verify that shell integration works correctly on Windows.
//! On Windows, binaries have `.exe` extension but shell integration uses the base name
//! (e.g., `wt` not `wt.exe`) because MSYS2/Git Bash handles the resolution automatically.

#![cfg(windows)]

use std::process::Command;

/// Verify shell function uses base name (without .exe) on Windows.
///
/// When the binary is invoked as `wt.exe`, the generated bash script should:
/// 1. Define a function named `wt()` (not `wt.exe()`)
/// 2. Check for `command -v wt` (not `wt.exe`)
/// 3. Set up completions for `wt`
///
/// Users should use `alias wt="wt"` (or just have `wt` in PATH) rather than
/// `alias wt="wt.exe"`. MSYS2/Git Bash automatically resolves `wt` to `wt.exe`.
#[test]
fn test_shell_init_strips_exe_suffix_on_windows() {
    // Run wt.exe config shell init bash
    // Note: This command doesn't need a git repo - it just generates shell init code
    let output = Command::new(env!("CARGO_BIN_EXE_wt"))
        .args(["config", "shell", "init", "bash"])
        .output()
        .expect("Failed to run wt config shell init");

    assert!(output.status.success(), "Command failed: {:?}", output);

    let stdout = String::from_utf8_lossy(&output.stdout);

    // The binary is wt.exe but function should be wt() (base name)
    assert!(
        stdout.contains("wt()"),
        "Expected function definition 'wt()' not found in output.\n\
         Shell integration should use base name without .exe.\n\
         Output:\n{}",
        stdout
    );

    // Verify command -v check uses base name
    assert!(
        stdout.contains("command -v wt"),
        "Expected 'command -v wt' check not found in output.\nOutput:\n{}",
        stdout
    );

    // Should NOT contain .exe in function/command names
    assert!(
        !stdout.contains("wt.exe()"),
        "Function should be 'wt()' not 'wt.exe()'.\nOutput:\n{}",
        stdout
    );
}
