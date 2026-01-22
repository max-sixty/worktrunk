//! Shell detection and utility functions.
//!
//! This module provides utilities for detecting the current shell, extracting
//! shell names from paths, and probing shell configuration state.

use super::Shell;

/// Extract executable name from a path, stripping `.exe` on Windows.
///
/// Uses `std::path::Path` for platform-native path handling:
/// - Unix: `/usr/bin/bash` -> "bash"
/// - Windows: `C:\Program Files\Git\usr\bin\bash.exe` -> "bash"
///
/// Only strips `.exe` extension (not other extensions like `.9` in `zsh-5.9`).
pub fn extract_filename_from_path(path: &str) -> Option<&str> {
    let filename = std::path::Path::new(path).file_name()?.to_str()?;

    // Strip .exe extension (case-insensitive for Windows)
    // Don't use file_stem() because it would strip version numbers like ".9" from "zsh-5.9"
    // Handle all case variants: .exe, .EXE, .Exe, .eXe, etc.
    if filename.len() > 4 && filename[filename.len() - 4..].eq_ignore_ascii_case(".exe") {
        Some(&filename[..filename.len() - 4])
    } else {
        Some(filename)
    }
}

/// Determine Shell variant from a shell name (without path or extension).
///
/// Handles versioned/prefixed binaries like `zsh-5.9` or `bash5`
/// by checking if the name starts with a known shell.
pub fn shell_from_name(shell_name: &str) -> Option<Shell> {
    // Try exact match first
    if let Ok(shell) = shell_name.parse() {
        return Some(shell);
    }

    // Handle versioned/prefixed binaries (e.g., "zsh-5.9", "bash5")
    // Check if shell name starts with a known shell
    let name_lower = shell_name.to_lowercase();
    if name_lower.starts_with("zsh") {
        Some(Shell::Zsh)
    } else if name_lower.starts_with("bash") {
        Some(Shell::Bash)
    } else if name_lower.starts_with("fish") {
        Some(Shell::Fish)
    } else if name_lower.starts_with("pwsh") || name_lower.starts_with("powershell") {
        Some(Shell::PowerShell)
    } else {
        None
    }
}

/// Get the current shell from `$SHELL` environment variable.
///
/// Returns `None` if `$SHELL` is not set or doesn't match a known shell.
/// Handles versioned/prefixed binaries like `/nix/store/.../zsh-5.9` or `bash5`
/// by checking if the name starts with a known shell.
///
/// Works on both Unix and Windows:
/// - Unix: `/usr/bin/bash` -> Bash
/// - Windows Git Bash: `C:\Program Files\Git\usr\bin\bash.exe` -> Bash
pub fn current_shell() -> Option<Shell> {
    let shell_path = std::env::var("SHELL").ok()?;
    let shell_name = extract_filename_from_path(&shell_path)?;
    shell_from_name(shell_name)
}

/// Detect if user's zsh has compinit enabled by probing for the compdef function.
///
/// Zsh's completion system (compinit) must be explicitly enabled - it's not on by default.
/// When compinit runs, it defines the `compdef` function. We probe for this function
/// by spawning an interactive zsh that sources the user's config, then checking if
/// compdef exists.
///
/// This approach matches what other CLI tools (hugo, podman, dvc) recommend: detect
/// the state and advise users, rather than trying to auto-enable compinit.
///
/// Returns:
/// - `Some(true)` if compinit is enabled (compdef function exists)
/// - `Some(false)` if compinit is NOT enabled
/// - `None` if detection failed (zsh not installed, timeout, error)
pub fn detect_zsh_compinit() -> Option<bool> {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    // Allow tests to bypass this check since zsh subprocess behavior varies across CI envs
    if std::env::var("WORKTRUNK_TEST_COMPINIT_CONFIGURED").is_ok() {
        return Some(true); // Assume compinit is configured
    }

    // Force compinit to be missing (for tests that expect the warning)
    if std::env::var("WORKTRUNK_TEST_COMPINIT_MISSING").is_ok() {
        return Some(false); // Force warning to appear
    }

    // Probe command: check if compdef function exists (proof compinit ran).
    // We use unique markers (__WT_COMPINIT_*) to avoid false matches from any
    // output the user's zshrc might produce during startup.
    let probe_cmd =
        r#"(( $+functions[compdef] )) && echo __WT_COMPINIT_YES__ || echo __WT_COMPINIT_NO__"#;

    log::debug!("$ zsh -ic '{}' (probe)", probe_cmd);

    let mut child = Command::new("zsh")
        .arg("-ic")
        .arg(probe_cmd)
        .stdin(Stdio::null()) // Prevent compinit from prompting interactively
        .stdout(Stdio::piped())
        .stderr(Stdio::null()) // Suppress user's zsh startup messages
        // Suppress zsh's "insecure directories" warning from compinit.
        //
        // When fpath contains directories with insecure permissions, compinit prompts:
        //   "zsh compinit: insecure directories, run compaudit for list."
        //   "Ignore insecure directories and continue [y] or abort compinit [n]?"
        //
        // This prompt goes to /dev/tty (not stderr), bypassing our stderr redirect.
        //
        // Worktrunk does NOT cause this warning - our shell init script doesn't modify
        // fpath or call compinit. It only registers completions with `compdef` if the
        // user has already set up compinit themselves. The warning appears because:
        // 1. This probe runs `zsh -ic` which sources global configs like /etc/zsh/zshrc
        // 2. Some environments (notably Ubuntu CI) have global configs that call compinit
        // 3. Those environments may have insecure fpath directories
        //
        // Safe to suppress because we're only probing shell state, not doing anything
        // security-sensitive, and this only affects our subprocess.
        .env("ZSH_DISABLE_COMPFIX", "true")
        // Prevent subprocesses from writing to the directive file
        .env_remove(crate::shell_exec::DIRECTIVE_FILE_ENV_VAR)
        .spawn()
        .ok()?;

    let start = Instant::now();
    let timeout = Duration::from_secs(2);

    loop {
        match child.try_wait() {
            Ok(Some(_status)) => {
                // Process finished (exit status is always 0 due to || fallback in probe)
                // wait_with_output() collects remaining stdout even after try_wait() succeeds
                let output = child.wait_with_output().ok()?;
                let stdout = String::from_utf8_lossy(&output.stdout);
                return Some(stdout.contains("__WT_COMPINIT_YES__"));
            }
            Ok(None) => {
                // Still running - check timeout
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait(); // Reap zombie process
                    return None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    // ==========================================================================
    // Path extraction tests (Issue #348)
    // ==========================================================================

    #[rstest]
    #[case::just_name("bash", Some("bash"))]
    #[case::just_name_exe("bash.exe", Some("bash"))]
    #[case::mixed_case_exe_title("bash.Exe", Some("bash"))]
    #[case::mixed_case_exe_upper("bash.EXE", Some("bash"))]
    #[case::mixed_case_exe_camel("bash.eXe", Some("bash"))]
    #[case::empty("", None)]
    fn test_extract_filename_from_path_common(#[case] path: &str, #[case] expected: Option<&str>) {
        assert_eq!(extract_filename_from_path(path), expected);
    }

    #[cfg(unix)]
    #[rstest]
    #[case::unix_bash("/usr/bin/bash", Some("bash"))]
    #[case::unix_zsh("/bin/zsh", Some("zsh"))]
    #[case::unix_fish("/usr/local/bin/fish", Some("fish"))]
    #[case::nix_versioned("/nix/store/abc123/zsh-5.9", Some("zsh-5.9"))]
    fn test_extract_filename_from_path_unix(#[case] path: &str, #[case] expected: Option<&str>) {
        assert_eq!(extract_filename_from_path(path), expected);
    }

    #[cfg(windows)]
    #[rstest]
    #[case::windows_git_bash(r"C:\Program Files\Git\usr\bin\bash.exe", Some("bash"))]
    #[case::windows_powershell(
        r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
        Some("powershell")
    )]
    #[case::windows_pwsh(r"C:\Program Files\PowerShell\7\pwsh.exe", Some("pwsh"))]
    #[case::windows_zsh(r"C:\msys64\usr\bin\zsh.exe", Some("zsh"))]
    #[case::uppercase_exe(r"C:\WINDOWS\SYSTEM32\BASH.EXE", Some("BASH"))]
    fn test_extract_filename_from_path_windows(#[case] path: &str, #[case] expected: Option<&str>) {
        assert_eq!(extract_filename_from_path(path), expected);
    }

    /// Issue #348: Windows Git Bash shell detection
    ///
    /// Git Bash sets $SHELL to Windows-style paths like:
    /// `C:\Program Files\Git\usr\bin\bash.exe`
    ///
    /// This test verifies the full path-to-shell detection flow works on Windows.
    #[cfg(windows)]
    #[rstest]
    #[case::git_bash(r"C:\Program Files\Git\usr\bin\bash.exe", Shell::Bash)]
    #[case::msys2_zsh(r"C:\msys64\usr\bin\zsh.exe", Shell::Zsh)]
    #[case::powershell(
        r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
        Shell::PowerShell
    )]
    #[case::pwsh(r"C:\Program Files\PowerShell\7\pwsh.exe", Shell::PowerShell)]
    fn test_issue_348_windows_shell_detection(#[case] shell_path: &str, #[case] expected: Shell) {
        // This is the exact flow that failed before the fix:
        // 1. extract_filename_from_path() extracts "bash" from Windows path
        // 2. shell_from_name() maps "bash" to Shell::Bash
        let shell_name = extract_filename_from_path(shell_path)
            .expect("should extract filename from Windows path");
        let detected =
            shell_from_name(shell_name).expect("should detect shell from extracted name");
        assert_eq!(detected, expected);
    }

    #[rstest]
    #[case::bash("bash", Some(Shell::Bash))]
    #[case::bash_versioned("bash5", Some(Shell::Bash))]
    #[case::zsh("zsh", Some(Shell::Zsh))]
    #[case::zsh_versioned("zsh-5.9", Some(Shell::Zsh))]
    #[case::fish("fish", Some(Shell::Fish))]
    #[case::powershell("powershell", Some(Shell::PowerShell))]
    #[case::pwsh("pwsh", Some(Shell::PowerShell))]
    #[case::pwsh_preview("pwsh-preview", Some(Shell::PowerShell))]
    #[case::unknown("tcsh", None)]
    #[case::unknown_csh("csh", None)]
    fn test_shell_from_name(#[case] name: &str, #[case] expected: Option<Shell>) {
        assert_eq!(shell_from_name(name), expected);
    }
}
