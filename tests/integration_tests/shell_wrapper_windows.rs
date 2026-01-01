//! Windows-specific shell integration tests
//!
//! These tests verify that the shell wrapper works correctly on Windows (Git Bash/MSYS2/Cygwin).
//!
//! The key issue on Windows is that:
//! 1. Git Bash's `mktemp` creates POSIX-style paths like `/tmp/xxx`
//! 2. The native Windows binary (wt.exe) cannot read/write POSIX paths
//! 3. The shell wrapper must use `cygpath -w` to convert paths before passing to the binary
//!
//! Without this conversion, the directive file mechanism is completely broken on Windows,
//! causing all shell-integrated commands to fail silently (directives not executed).

// Gate entire module: Windows + shell-integration-tests feature
#![cfg(all(windows, feature = "shell-integration-tests"))]

use crate::common::TestRepo;
use insta_cmd::get_cargo_bin;
use rstest::rstest;
use std::path::PathBuf;
use std::process::Command;

/// Find Git Bash executable on Windows.
/// Git Bash is typically installed at C:\Program Files\Git\bin\bash.exe
fn find_git_bash() -> Option<PathBuf> {
    // Try common Git installation paths
    let candidates = [
        r"C:\Program Files\Git\bin\bash.exe",
        r"C:\Program Files (x86)\Git\bin\bash.exe",
        r"C:\Git\bin\bash.exe",
    ];

    for path in &candidates {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }

    // Try to find via PATH - look for git.exe and derive bash location
    if let Ok(output) = Command::new("where").arg("git.exe").output()
        && output.status.success()
    {
        let git_path = String::from_utf8_lossy(&output.stdout);
        if let Some(line) = git_path.lines().next() {
            // git.exe is usually in cmd/, bash.exe is in bin/
            // C:\Program Files\Git\cmd\git.exe -> C:\Program Files\Git\bin\bash.exe
            let git_path = PathBuf::from(line.trim());
            if let Some(git_dir) = git_path.parent().and_then(|p| p.parent()) {
                let bash_path = git_dir.join("bin").join("bash.exe");
                if bash_path.exists() {
                    return Some(bash_path);
                }
            }
        }
    }

    None
}

/// Create a rstest fixture that provides a TestRepo
#[rstest::fixture]
fn repo() -> TestRepo {
    TestRepo::new()
}

/// Test that the shell wrapper correctly handles directive files on Windows.
///
/// This test verifies the core cygpath fix:
/// - Execute a wt command through Git Bash that writes to a directive file
/// - Verify the command succeeds (directive file was written and sourced)
///
/// On Windows without the cygpath fix (main branch), this fails because:
/// - mktemp creates `/tmp/xxx.xxx` (POSIX path)
/// - wt.exe receives WORKTRUNK_DIRECTIVE_FILE=/tmp/xxx.xxx
/// - wt.exe cannot write to this path (not a valid Windows path)
/// - Either the command fails or directives are silently lost
///
/// With the cygpath fix (this branch), this succeeds because:
/// - mktemp creates `/tmp/xxx.xxx` (POSIX path)
/// - Shell wrapper runs: cygpath -w /tmp/xxx.xxx → C:\Users\...\Temp\xxx.xxx
/// - wt.exe receives WORKTRUNK_DIRECTIVE_FILE=C:\Users\...\Temp\xxx.xxx
/// - wt.exe can write to this valid Windows path
/// - Directive file is written, sourced, and shell changes directory
#[rstest]
fn test_directive_file_works_on_windows(repo: TestRepo) {
    // Get paths
    let wt_bin = get_cargo_bin("wt");
    let wt_bin_path = wt_bin.display().to_string();
    let config_path = repo.test_config_path().display().to_string();

    // Build a bash script that:
    // 1. Sets up environment
    // 2. Sources the shell wrapper (which includes the cygpath fix)
    // 3. Runs a command that requires directive file to work
    //
    // We use `wt switch --create test-branch` which:
    // - Creates a worktree
    // - Writes a `cd '/path/to/worktree'` directive
    // - The shell sources this and changes directory
    //
    // If the directive file doesn't work, the cd directive is lost.
    let script = format!(
        r#"
# Debug: Show environment
echo "=== DEBUG INFO ==="
echo "MSYSTEM: $MSYSTEM"
echo "which mktemp: $(which mktemp)"
echo "mktemp output: $(mktemp)"
echo "which cygpath: $(which cygpath 2>/dev/null || echo 'NOT FOUND')"
echo "=================="

# Set up environment
export WORKTRUNK_BIN='{wt_bin}'
export WORKTRUNK_CONFIG_PATH='{config}'
export CLICOLOR_FORCE=1

# Show what shell wrapper we're getting
echo "=== SHELL WRAPPER SNIPPET ==="
"$WORKTRUNK_BIN" config shell init bash | grep -A5 "directive_file"
echo "=================="

# Generate and source the shell wrapper
eval "$("$WORKTRUNK_BIN" config shell init bash)"

# Run a command that uses directive file for cd
echo "=== RUNNING SWITCH ==="
wt switch --create test-windows-directive

# Print current directory to verify we changed
echo "PWD after wt switch: $PWD"
"#,
        wt_bin = wt_bin_path.replace('\\', "/"), // Convert to POSIX path for bash
        config = config_path.replace('\\', "/"),
    );

    // Execute through Git Bash (NOT WSL bash)
    let git_bash = find_git_bash().expect(
        "Git Bash not found. This test requires Git for Windows to be installed.\n\
         Looked in: C:\\Program Files\\Git\\bin\\bash.exe and similar paths.",
    );

    let output = Command::new(&git_bash)
        .args(["-c", &script])
        .current_dir(repo.root_path())
        .env_remove("WORKTRUNK_DIRECTIVE_FILE") // Ensure test isolation
        .output()
        .expect("Failed to execute Git Bash");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("STDOUT:\n{}\n\nSTDERR:\n{}", stdout, stderr);

    // DEBUG: Always show output for diagnosis
    eprintln!("=== TEST OUTPUT ===\n{}\n=== END OUTPUT ===", combined);

    // The command should succeed
    assert!(
        output.status.success(),
        "wt switch --create should succeed on Windows with cygpath fix.\n\
         Exit code: {:?}\n\
         Output:\n{}",
        output.status.code(),
        combined
    );

    // The output should show we created the worktree
    assert!(
        stdout.contains("test-windows-directive") || stderr.contains("test-windows-directive"),
        "Output should mention the branch name.\nOutput:\n{}",
        combined
    );

    // The PWD should have changed (directive file was sourced)
    // This is the key verification - if cygpath fix works, the cd directive executes
    assert!(
        stdout.contains("PWD after wt switch:"),
        "Should print PWD after switch.\nOutput:\n{}",
        combined
    );

    // The PWD should contain the branch name (we changed to the worktree)
    assert!(
        stdout.contains("test-windows-directive"),
        "PWD should contain the worktree path (directive was executed).\n\
         This fails on main branch because the directive file path is a POSIX path\n\
         that the Windows binary cannot write to.\nOutput:\n{}",
        combined
    );

    // DEBUG: Force failure to see full output
    panic!(
        "DEBUG: Forcing test failure to see output above. Remove this line after diagnosis.\n\nOutput:\n{}",
        combined
    );
}

/// Test that the binary works correctly on Windows without shell wrapper.
///
/// This is a simpler test that calls the binary directly (no shell wrapper).
/// It verifies basic command execution works on Windows.
#[rstest]
fn test_binary_works_directly_on_windows(repo: TestRepo) {
    let wt_bin = get_cargo_bin("wt");
    let wt_bin_path = wt_bin.display().to_string();
    let config_path = repo.test_config_path().display().to_string();

    // Call the binary directly (no shell wrapper)
    let script = format!(
        r#"
export WORKTRUNK_CONFIG_PATH='{config}'
'{wt_bin}' list
"#,
        wt_bin = wt_bin_path.replace('\\', "/"),
        config = config_path.replace('\\', "/"),
    );

    // Execute through Git Bash (NOT WSL bash)
    let git_bash = find_git_bash()
        .expect("Git Bash not found. This test requires Git for Windows to be installed.");

    let output = Command::new(&git_bash)
        .args(["-c", &script])
        .current_dir(repo.root_path())
        .env_remove("WORKTRUNK_DIRECTIVE_FILE") // Ensure test isolation
        .output()
        .expect("Failed to execute Git Bash");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "wt list should succeed.\nSTDOUT:\n{}\nSTDERR:\n{}",
        stdout,
        stderr
    );

    // Should show the main worktree
    assert!(
        stdout.contains("main") || stdout.contains("master"),
        "wt list should show the default branch.\nOutput:\n{}",
        stdout
    );
}
