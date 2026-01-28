//! PowerShell shell integration tests.
//!
//! These tests verify that PowerShell shell integration works correctly.
//! Requires pwsh (PowerShell Core), which is pre-installed on GitHub Actions runners.

#![cfg(feature = "shell-integration-tests")]

use std::path::Path;
use std::process::Command;

use worktrunk::shell::Shell;

/// Test that the PowerShell config_line() actually works when evaluated.
///
/// This is a regression test for issue #885 where `Invoke-Expression` failed
/// because command output is an array of strings, not a single string.
/// The fix was adding `| Out-String` to the config_line.
#[test]
fn test_powershell_config_line_evaluates_correctly() {
    // Use CARGO_BIN_EXE_wt which Cargo sets to the wt binary path during tests
    let wt_bin = Path::new(env!("CARGO_BIN_EXE_wt"));
    let bin_dir = wt_bin.parent().expect("Failed to get binary directory");

    // Build a script that:
    // 1. Adds the binary directory to PATH so Get-Command wt works
    // 2. Sets WORKTRUNK_BIN so the init script can find the binary
    // 3. Runs the config_line (which uses Invoke-Expression)
    // 4. Checks if the function is defined
    let config_line = Shell::PowerShell.config_line("wt");
    let script = format!(
        r#"
$env:PATH = '{}' + [IO.Path]::PathSeparator + $env:PATH
$env:WORKTRUNK_BIN = '{}'
{}
$cmd = Get-Command wt -ErrorAction SilentlyContinue
if ($cmd -and $cmd.CommandType -eq 'Function') {{
    Write-Output 'FUNCTION_DEFINED'
}} else {{
    Write-Output "FUNCTION_NOT_DEFINED: CommandType=$($cmd.CommandType)"
}}
"#,
        bin_dir.display().to_string().replace('\'', "''"),
        wt_bin.display().to_string().replace('\'', "''"),
        config_line
    );

    let output = Command::new("pwsh")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .expect("Failed to run pwsh");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "pwsh command failed.\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );

    assert!(
        stdout.contains("FUNCTION_DEFINED"),
        "PowerShell config_line failed to define function.\n\
         Config line: {}\n\
         stdout: {}\n\
         stderr: {}",
        config_line,
        stdout,
        stderr
    );
}
