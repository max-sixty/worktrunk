//! PowerShell shell integration tests.
//!
//! These tests verify that PowerShell shell integration works correctly.
//! Requires pwsh (PowerShell Core), which is pre-installed on GitHub Actions runners.

#![cfg(feature = "shell-integration-tests")]

use std::process::Command;

use worktrunk::shell::{Shell, ShellInit};

/// Test that the PowerShell config_line() actually works when evaluated.
///
/// This is a regression test for issue #885 where `Invoke-Expression` failed
/// because command output is an array of strings, not a single string.
/// The fix was adding `| Out-String` to the config_line.
#[test]
fn test_powershell_config_line_evaluates_correctly() {
    // Use CARGO_BIN_EXE_wt which Cargo sets to the wt binary path during tests
    let wt_bin = std::path::Path::new(env!("CARGO_BIN_EXE_wt"));
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

/// Regression test: PowerShell wrapper must not consume short flags like -D.
///
/// When the wrapper function uses `[Parameter(ValueFromRemainingArguments)]`, PowerShell
/// promotes it to an "advanced function" which adds common parameters (-Debug, -Verbose,
/// etc.). The `-D` flag is then consumed as `-Debug` instead of being passed to the binary.
/// The fix uses `$args` (simple function automatic variable) for transparent passthrough.
#[test]
fn test_powershell_wrapper_passes_short_flags_through() {
    // Create a .ps1 mock that prints each argument on its own line.
    // Using .ps1 (not a shell script) so this works on Windows too.
    let temp_dir = tempfile::tempdir().unwrap();
    let mock_bin = temp_dir.path().join("mock-wt.ps1");
    std::fs::write(
        &mock_bin,
        "foreach ($a in $args) { Write-Output $a }\n",
    )
    .unwrap();

    let init = ShellInit::with_prefix(Shell::PowerShell, "wt".to_string());
    let wrapper = init.generate().unwrap();

    let mock_bin_escaped = mock_bin.display().to_string().replace('\'', "''");
    let script = format!(
        r#"
$env:WORKTRUNK_BIN = '{mock_bin_escaped}'
{wrapper}
wt remove -D test --force
"#
    );

    let output = Command::new("pwsh")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .expect("Failed to run pwsh");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "pwsh command failed.\nstdout: {stdout}\nstderr: {stderr}",
    );

    // Each argument should appear as a separate line in the mock's output.
    // If -D were consumed as -Debug (advanced function), it would be missing.
    let lines: Vec<&str> = stdout.lines().map(|l| l.trim()).collect();
    for expected in ["remove", "-D", "test", "--force"] {
        assert!(
            lines.contains(&expected),
            "Expected argument {expected:?} to be passed through to binary.\n\
             Got lines: {lines:?}\nstdout: {stdout}\nstderr: {stderr}",
        );
    }
}
