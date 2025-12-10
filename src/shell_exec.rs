//! Cross-platform shell execution
//!
//! Provides a unified interface for executing shell commands across platforms:
//! - Unix: Uses `/bin/sh -c`
//! - Windows: Prefers Git Bash if available, falls back to PowerShell
//!
//! This enables hooks and commands to use the same bash syntax on all platforms,
//! as long as Git for Windows is installed (which is nearly universal among
//! Windows developers).

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
    // Check for Git Bash in standard locations
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
/// Checks:
/// 1. MSYSTEM environment variable (indicates running in Git Bash/MSYS2)
/// 2. Standard Git for Windows installation paths
/// 3. `git.exe` in PATH to find Git installation directory
#[cfg(windows)]
fn find_git_bash() -> Option<PathBuf> {
    // If we're already in Git Bash (MSYSTEM is set), use the system bash
    if std::env::var("MSYSTEM").is_ok() {
        // In Git Bash environment, bash is available directly
        if which_bash().is_some() {
            return Some(PathBuf::from("bash"));
        }
    }

    // Check standard Git for Windows installation paths
    let standard_paths = [
        r"C:\Program Files\Git\bin\bash.exe",
        r"C:\Program Files (x86)\Git\bin\bash.exe",
        r"C:\Git\bin\bash.exe",
    ];

    for path in &standard_paths {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }

    // Try to find Git installation via `git.exe` in PATH
    if let Some(git_path) = which_git() {
        // git.exe is typically at Git/cmd/git.exe or Git/bin/git.exe
        // bash.exe is at Git/bin/bash.exe
        if let Some(git_dir) = git_path.parent().and_then(|p| p.parent()) {
            let bash_path = git_dir.join("bin").join("bash.exe");
            if bash_path.exists() {
                return Some(bash_path);
            }
        }
    }

    None
}

/// Check if bash is available in PATH
#[cfg(windows)]
fn which_bash() -> Option<PathBuf> {
    which_executable("bash")
}

/// Check if git is available in PATH
#[cfg(windows)]
fn which_git() -> Option<PathBuf> {
    which_executable("git")
}

/// Find an executable in PATH (simple cross-platform which)
#[cfg(windows)]
fn which_executable(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    let exe_name = format!("{}.exe", name);

    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(&exe_name);
        if candidate.exists() {
            return Some(candidate);
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
}
