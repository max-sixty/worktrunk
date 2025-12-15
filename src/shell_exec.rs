//! Cross-platform shell execution
//!
//! Provides a unified interface for executing shell commands across platforms:
//! - Unix: Uses `/bin/sh -c`
//! - Windows: Prefers Git Bash if available, falls back to PowerShell
//!
//! This enables hooks and commands to use the same bash syntax on all platforms,
//! as long as Git for Windows is installed (which is nearly universal among
//! Windows developers).
//!
//! ## Windows Limitations
//!
//! When Git Bash is not available, PowerShell is used as a fallback with limitations:
//! - Hooks using bash syntax won't work
//! - No support for POSIX redirections like `{ cmd; } 1>&2`
//! - Different string escaping rules for JSON piping

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

/// Cached shell configuration for the current platform
static SHELL_CONFIG: OnceLock<ShellConfig> = OnceLock::new();

/// Shell configuration for command execution
#[derive(Debug, Clone)]
pub struct ShellConfig {
    /// Path to the shell executable
    pub executable: PathBuf,
    /// Arguments to pass before the command (e.g., ["-c"] for sh, ["/C"] for cmd)
    pub args: Vec<String>,
    /// Whether this is a POSIX-compatible shell (bash/sh)
    pub is_posix: bool,
    /// Human-readable name for error messages
    pub name: String,
}

impl ShellConfig {
    /// Get the shell configuration for the current platform
    ///
    /// On Unix, this always returns sh.
    /// On Windows, this prefers Git Bash if available, then falls back to PowerShell.
    pub fn get() -> &'static ShellConfig {
        SHELL_CONFIG.get_or_init(detect_shell)
    }

    /// Create a Command configured for shell execution
    ///
    /// The command string will be passed to the shell for interpretation.
    pub fn command(&self, shell_command: &str) -> Command {
        let mut cmd = Command::new(&self.executable);
        for arg in &self.args {
            cmd.arg(arg);
        }
        cmd.arg(shell_command);
        cmd
    }

    /// Check if this shell supports POSIX syntax (bash, sh, zsh, etc.)
    ///
    /// When true, commands can use POSIX features like:
    /// - `{ cmd; } 1>&2` for stdout redirection
    /// - `printf '%s' ... | cmd` for stdin piping
    /// - `nohup ... &` for background execution
    pub fn is_posix(&self) -> bool {
        self.is_posix
    }

    /// Check if running on Windows without Git Bash (using PowerShell fallback)
    ///
    /// Returns true when hooks using bash syntax won't work properly.
    /// Used to show warnings to users about limited functionality.
    #[cfg(windows)]
    pub fn is_windows_without_git_bash(&self) -> bool {
        !self.is_posix
    }

    #[cfg(not(windows))]
    pub fn is_windows_without_git_bash(&self) -> bool {
        false
    }
}

/// Detect the best available shell for the current platform
fn detect_shell() -> ShellConfig {
    #[cfg(unix)]
    {
        ShellConfig {
            executable: PathBuf::from("sh"),
            args: vec!["-c".to_string()],
            is_posix: true,
            name: "sh".to_string(),
        }
    }

    #[cfg(windows)]
    {
        detect_windows_shell()
    }
}

/// Detect the best available shell on Windows
///
/// Priority order:
/// 1. Git Bash (if Git for Windows is installed)
/// 2. PowerShell (fallback, with warnings about syntax differences)
#[cfg(windows)]
fn detect_windows_shell() -> ShellConfig {
    if let Some(bash_path) = find_git_bash() {
        return ShellConfig {
            executable: bash_path,
            args: vec!["-c".to_string()],
            is_posix: true,
            name: "Git Bash".to_string(),
        };
    }

    // Fall back to PowerShell
    ShellConfig {
        executable: PathBuf::from("powershell.exe"),
        args: vec!["-NoProfile".to_string(), "-Command".to_string()],
        is_posix: false,
        name: "PowerShell".to_string(),
    }
}

/// Find Git Bash executable on Windows
///
/// Detection order (designed to always return absolute paths and avoid WSL):
/// 1. `git.exe` in PATH - derive bash.exe location from Git installation
/// 2. Standard Git for Windows and MSYS2 installation paths
///
/// We explicitly avoid `which bash` because on systems with WSL installed,
/// `C:\Windows\System32\bash.exe` (WSL launcher) often comes before Git Bash
/// in PATH, even when MSYSTEM is set.
#[cfg(windows)]
fn find_git_bash() -> Option<PathBuf> {
    // Primary method: Find Git installation via `git.exe` in PATH
    // This is the most reliable method and always returns an absolute path.
    // Works on CI systems like GitHub Actions where Git might be in non-standard locations.
    if let Ok(git_path) = which::which("git") {
        // git.exe is typically at Git/cmd/git.exe or Git/bin/git.exe
        // bash.exe is at Git/bin/bash.exe or Git/usr/bin/bash.exe
        if let Some(git_dir) = git_path.parent().and_then(|p| p.parent()) {
            // Try bin/bash.exe first (most common)
            let bash_path = git_dir.join("bin").join("bash.exe");
            if bash_path.exists() {
                return Some(bash_path);
            }
            // Also try usr/bin/bash.exe (some Git for Windows layouts)
            let bash_path = git_dir.join("usr").join("bin").join("bash.exe");
            if bash_path.exists() {
                return Some(bash_path);
            }
        }
    }

    // Fallback: Check standard installation paths for bash.exe
    // (Git for Windows and MSYS2 both provide POSIX-compatible bash)
    let bash_paths = [
        // Git for Windows
        r"C:\Program Files\Git\bin\bash.exe",
        r"C:\Program Files\Git\usr\bin\bash.exe",
        r"C:\Program Files (x86)\Git\bin\bash.exe",
        r"C:\Git\bin\bash.exe",
        // MSYS2 standalone (popular alternative to Git Bash)
        r"C:\msys64\usr\bin\bash.exe",
        r"C:\msys32\usr\bin\bash.exe",
    ];

    for path in &bash_paths {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_config_is_available() {
        let config = ShellConfig::get();
        assert!(!config.name.is_empty());
        assert!(!config.args.is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn test_unix_shell_is_posix() {
        let config = ShellConfig::get();
        assert!(config.is_posix);
        assert_eq!(config.name, "sh");
    }

    #[test]
    fn test_command_creation() {
        let config = ShellConfig::get();
        let cmd = config.command("echo hello");
        // Just verify it doesn't panic
        let _ = format!("{:?}", cmd);
    }

    #[test]
    fn test_shell_command_execution() {
        let config = ShellConfig::get();
        let output = config
            .command("echo hello")
            .output()
            .expect("Failed to execute shell command");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "echo should succeed. Shell: {} ({:?}), exit: {:?}, stdout: '{}', stderr: '{}'",
            config.name,
            config.executable,
            output.status.code(),
            stdout.trim(),
            stderr.trim()
        );
        assert!(
            stdout.contains("hello"),
            "stdout should contain 'hello', got: '{}'",
            stdout.trim()
        );
    }

    #[test]
    #[cfg(windows)]
    fn test_windows_shell_detection() {
        let config = ShellConfig::get();
        // On Windows CI, Git is installed, so we should have Git Bash
        // If this fails on a system without Git, PowerShell fallback should work
        assert!(
            config.name == "Git Bash" || config.name == "PowerShell",
            "Expected 'Git Bash' or 'PowerShell', got '{}'",
            config.name
        );
    }

    #[test]
    #[cfg(windows)]
    fn test_windows_git_bash_has_posix_syntax() {
        let config = ShellConfig::get();
        if config.name == "Git Bash" {
            assert!(config.is_posix, "Git Bash should support POSIX syntax");
            assert!(
                config.args.contains(&"-c".to_string()),
                "Git Bash should use -c flag"
            );
        }
    }

    #[test]
    #[cfg(windows)]
    fn test_windows_powershell_fallback_not_posix() {
        let config = ShellConfig::get();
        if config.name == "PowerShell" {
            assert!(!config.is_posix, "PowerShell should not be marked as POSIX");
            assert!(
                config.args.contains(&"-Command".to_string()),
                "PowerShell should use -Command flag"
            );
        }
    }

    #[test]
    #[cfg(windows)]
    fn test_windows_echo_command() {
        // Test that echo works regardless of which shell we detected
        let config = ShellConfig::get();
        let output = config
            .command("echo test_output")
            .output()
            .expect("Failed to execute echo");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "echo should succeed. Shell: {} ({:?}), exit: {:?}, stdout: '{}', stderr: '{}'",
            config.name,
            config.executable,
            output.status.code(),
            stdout.trim(),
            stderr.trim()
        );
        assert!(
            stdout.contains("test_output"),
            "stdout should contain 'test_output', got: '{}'",
            stdout.trim()
        );
    }

    #[test]
    #[cfg(windows)]
    fn test_windows_posix_redirection_with_git_bash() {
        let config = ShellConfig::get();
        if config.is_posix() {
            // Test POSIX-style redirection: stdout redirected to stderr
            let output = config
                .command("echo redirected 1>&2")
                .output()
                .expect("Failed to execute redirection test");

            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                output.status.success(),
                "redirection command should succeed. Shell: {} ({:?}), exit: {:?}, stdout: '{}', stderr: '{}'",
                config.name,
                config.executable,
                output.status.code(),
                stdout.trim(),
                stderr.trim()
            );
            assert!(
                stderr.contains("redirected"),
                "stderr should contain 'redirected' (stdout redirected to stderr), got: '{}'",
                stderr.trim()
            );
        }
    }

    #[test]
    fn test_shell_config_debug() {
        let config = ShellConfig::get();
        let debug = format!("{:?}", config);
        assert!(debug.contains("ShellConfig"));
        assert!(debug.contains(&config.name));
    }

    #[test]
    fn test_shell_config_clone() {
        let config = ShellConfig::get();
        let cloned = config.clone();
        assert_eq!(config.name, cloned.name);
        assert_eq!(config.is_posix, cloned.is_posix);
        assert_eq!(config.args, cloned.args);
    }

    #[test]
    fn test_shell_is_posix_method() {
        let config = ShellConfig::get();
        // is_posix method should match the field
        assert_eq!(config.is_posix(), config.is_posix);
    }

    #[test]
    #[cfg(not(windows))]
    fn test_unix_is_not_windows_without_git_bash() {
        let config = ShellConfig::get();
        assert!(!config.is_windows_without_git_bash());
    }
}
