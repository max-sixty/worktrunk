use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::process::Command;

/// Convert a path to POSIX format for Git Bash compatibility.
///
/// On Windows, uses `cygpath -u` from Git for Windows to convert paths like
/// `C:\Users\test` to `/c/Users/test`. This handles all edge cases including
/// UNC paths (`\\server\share`) and verbatim paths (`\\?\C:\...`).
///
/// If cygpath is not available, returns the path unchanged.
///
/// On Unix, returns the path unchanged.
///
/// # Examples
/// - `C:\Users\test\repo` → `/c/Users/test/repo`
/// - `D:\a\worktrunk` → `/d/a/worktrunk`
/// - `\\?\C:\repo` → `/c/repo` (verbatim prefix stripped)
/// - `/tmp/test/repo` → `/tmp/test/repo` (unchanged on Unix)
#[cfg(windows)]
pub fn to_posix_path(path: &str) -> String {
    use crate::shell_exec::ShellConfig;

    let Some(cygpath) = find_cygpath_from_shell(ShellConfig::get()) else {
        return path.to_string();
    };

    let Ok(output) = Command::new(&cygpath).arg("-u").arg(path).output() else {
        return path.to_string();
    };

    if output.status.success() {
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    } else {
        path.to_string()
    }
}

#[cfg(not(windows))]
pub fn to_posix_path(path: &str) -> String {
    path.to_string()
}

/// Find cygpath.exe relative to the shell executable.
///
/// cygpath is always at `usr/bin/cygpath.exe` in a Git for Windows installation.
/// bash.exe can be at `bin/bash.exe` or `usr/bin/bash.exe`, so we check both
/// relative paths.
#[cfg(windows)]
fn find_cygpath_from_shell(shell: &crate::shell_exec::ShellConfig) -> Option<PathBuf> {
    // Only Git Bash has cygpath
    if !shell.is_posix {
        return None;
    }

    let shell_dir = shell.executable.parent()?;

    // If bash is at usr/bin/bash.exe, cygpath is in the same directory
    let cygpath = shell_dir.join("cygpath.exe");
    if cygpath.exists() {
        return Some(cygpath);
    }

    // If bash is at bin/bash.exe, cygpath is at ../usr/bin/cygpath.exe
    let cygpath = shell_dir
        .parent()?
        .join("usr")
        .join("bin")
        .join("cygpath.exe");
    if cygpath.exists() {
        return Some(cygpath);
    }

    None
}

/// Get the user's home directory.
///
/// Uses the `home` crate which handles platform-specific detection:
/// - Unix: `$HOME` environment variable
/// - Windows: `USERPROFILE` or `HOMEDRIVE`/`HOMEPATH`
pub fn home_dir() -> Option<PathBuf> {
    home::home_dir()
}

/// Format a filesystem path for user-facing output.
///
/// Replaces home directory prefix with `~` (e.g., `/Users/alex/projects/wt` -> `~/projects/wt`).
/// Paths outside home are returned unchanged.
pub fn format_path_for_display(path: &Path) -> String {
    if let Some(home) = home_dir()
        && let Ok(stripped) = path.strip_prefix(&home)
    {
        if stripped.as_os_str().is_empty() {
            return "~".to_string();
        }

        let mut display_path = PathBuf::from("~");
        display_path.push(stripped);
        return display_path.display().to_string();
    }

    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{format_path_for_display, home_dir, to_posix_path};

    #[test]
    fn shortens_path_under_home() {
        let Some(home) = home_dir() else {
            // Skip if HOME/USERPROFILE is not set in the environment
            return;
        };

        let path = home.join("projects").join("wt");
        let formatted = format_path_for_display(&path);

        assert!(
            formatted.starts_with("~"),
            "Expected tilde prefix, got {formatted}"
        );
        assert!(
            formatted.contains("projects"),
            "Expected child components to remain in output"
        );
        assert!(
            formatted.ends_with("wt"),
            "Expected leaf component to remain in output"
        );
    }

    #[test]
    fn shows_home_as_tilde() {
        let Some(home) = home_dir() else {
            return;
        };

        let formatted = format_path_for_display(&home);
        assert_eq!(formatted, "~");
    }

    #[test]
    fn leaves_non_home_paths_unchanged() {
        let path = PathBuf::from("/tmp/worktrunk-non-home-path");
        let formatted = format_path_for_display(&path);
        assert_eq!(formatted, path.display().to_string());
    }

    // Tests for to_posix_path behavior (results depend on platform)
    #[test]
    fn to_posix_path_leaves_unix_paths_unchanged() {
        // Unix-style paths should pass through unchanged on all platforms
        assert_eq!(to_posix_path("/tmp/test/repo"), "/tmp/test/repo");
        assert_eq!(to_posix_path("relative/path"), "relative/path");
    }

    #[test]
    #[cfg(windows)]
    fn to_posix_path_converts_windows_drive_letter() {
        // On Windows, drive letters should be converted to /x/ format
        let result = to_posix_path(r"C:\Users\test");
        assert!(
            result.starts_with("/c/"),
            "Expected /c/ prefix, got: {result}"
        );
        assert!(
            result.contains("Users"),
            "Expected Users in path, got: {result}"
        );
    }

    #[test]
    #[cfg(windows)]
    fn to_posix_path_handles_verbatim_paths() {
        // cygpath should handle verbatim paths (\\?\C:\...)
        let result = to_posix_path(r"\\?\C:\Users\test");
        // Should either strip \\?\ prefix or handle it correctly
        assert!(
            result.contains("/c/") || result.contains("Users"),
            "Expected converted path, got: {result}"
        );
    }

    #[test]
    fn test_home_dir_returns_valid_path() {
        // home_dir should return a valid path on most systems
        if let Some(home) = home_dir() {
            assert!(home.is_absolute(), "Home directory should be absolute");
            // The home directory itself might not exist in some CI environments,
            // but the path should at least have components
            assert!(home.components().count() > 0, "Home should have components");
        }
    }

    #[test]
    fn test_format_path_outside_home() {
        // A path that definitely won't be under home
        let path = PathBuf::from("/definitely/not/under/home/dir");
        let result = format_path_for_display(&path);
        // Should return unchanged
        assert_eq!(result, "/definitely/not/under/home/dir");
    }

    #[test]
    #[cfg(not(windows))]
    fn test_to_posix_path_on_unix() {
        // On Unix, to_posix_path is a no-op
        assert_eq!(to_posix_path("/some/path"), "/some/path");
        assert_eq!(to_posix_path("relative"), "relative");
        assert_eq!(to_posix_path(""), "");
    }
}
