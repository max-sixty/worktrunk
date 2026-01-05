//! Windows-specific shell integration tests.
//!
//! These tests verify that shell integration works correctly on Windows,
//! particularly around the `.exe` suffix handling (Issue #348).
//!
//! On Windows, binaries have `.exe` extension. When a user invokes `wt.exe config shell init bash`,
//! the generated shell function must be named `wt.exe()` so that aliases like `alias wt="wt.exe"`
//! correctly invoke the shell function instead of the binary directly.

#![cfg(windows)]

use crate::common::{TestRepo, repo};
use std::process::Command;

/// Issue #348: Shell function name must include .exe suffix on Windows.
///
/// When the binary is invoked as `wt.exe`, the generated bash script must:
/// 1. Define a function named `wt.exe()` (not `wt()`)
/// 2. Check for `command -v wt.exe` (not `wt`)
/// 3. Set up completions for `wt.exe`
///
/// This ensures that when a user has `alias wt="wt.exe"` in their bashrc,
/// the alias expands to `wt.exe` which matches the shell function name.
#[test]
fn test_shell_init_preserves_exe_suffix_on_windows(repo: TestRepo) {
    // Run wt.exe config shell init bash
    // On Windows, CARGO_BIN_EXE_wt points to wt.exe
    let output = Command::new(env!("CARGO_BIN_EXE_wt"))
        .args(["config", "shell", "init", "bash"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to run wt config shell init");

    assert!(output.status.success(), "Command failed: {:?}", output);

    let stdout = String::from_utf8_lossy(&output.stdout);

    // The binary is invoked as wt.exe on Windows, so the function should be wt.exe()
    // Note: The actual binary name from argv[0] determines the function name
    let binary_name = std::path::Path::new(env!("CARGO_BIN_EXE_wt"))
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();

    // Verify the function definition uses the binary name (with .exe if present)
    let expected_function = format!("{}()", binary_name);
    assert!(
        stdout.contains(&expected_function),
        "Expected function definition '{}' not found in output.\n\
         This is critical for Windows Git Bash: if the function is named 'wt()' \
         but the user's alias points to 'wt.exe', the shell function won't be invoked.\n\
         Output:\n{}",
        expected_function,
        stdout
    );

    // Verify command -v check uses the same name
    let expected_command_check = format!("command -v {}", binary_name);
    assert!(
        stdout.contains(&expected_command_check),
        "Expected '{}' check not found in output.\nOutput:\n{}",
        expected_command_check,
        stdout
    );

    // Verify completion setup uses the same name
    let expected_complete = format!(
        "complete -o nospace -o bashdefault -F _{}_lazy_complete {}",
        binary_name, binary_name
    );
    assert!(
        stdout.contains(&expected_complete),
        "Expected completion setup for '{}' not found.\nOutput:\n{}",
        binary_name,
        stdout
    );
}

/// Verify git-wt.exe also preserves the .exe suffix.
///
/// Users installing via WinGet get `git-wt.exe`. The same principle applies:
/// the shell function must match the invocation name.
#[test]
fn test_shell_init_git_wt_preserves_exe_suffix_on_windows(repo: TestRepo) {
    // Run git-wt.exe config shell init bash
    let output = Command::new(env!("CARGO_BIN_EXE_git-wt"))
        .args(["config", "shell", "init", "bash"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to run git-wt config shell init");

    assert!(output.status.success(), "Command failed: {:?}", output);

    let stdout = String::from_utf8_lossy(&output.stdout);

    let binary_name = std::path::Path::new(env!("CARGO_BIN_EXE_git-wt"))
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();

    // Verify function name matches binary name
    let expected_function = format!("{}()", binary_name);
    assert!(
        stdout.contains(&expected_function),
        "Expected function definition '{}' not found.\n\
         Binary: {}\nOutput:\n{}",
        expected_function,
        binary_name,
        stdout
    );
}
