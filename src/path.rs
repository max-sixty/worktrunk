use path_slash::PathExt as _;
use shell_escape::unix::escape;
use std::borrow::Cow;
use std::path::Path;

use sanitize_filename::{Options as SanitizeOptions, sanitize_with_options};

use crate::config::short_hash;
#[cfg(windows)]
use crate::shell_exec::{Cmd, ShellConfig};
#[cfg(windows)]
use std::path::PathBuf;

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
    let Some(cygpath) = find_cygpath_from_shell(ShellConfig::get()) else {
        return path.to_string();
    };

    let Ok(output) = Cmd::new(cygpath.to_string_lossy()).args(["-u", path]).run() else {
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
pub use home::home_dir;

/// Check if a string needs shell escaping (contains characters outside the safe set).
fn needs_shell_escaping(s: &str) -> bool {
    !matches!(escape(Cow::Borrowed(s)), Cow::Borrowed(_))
}

/// Format a filesystem path for user-facing output.
///
/// Replaces home directory prefix with `~` when safe for shell use. Falls back to
/// quoted absolute path when escaping is needed (to avoid tilde-in-quotes issues).
///
/// Uses POSIX shell escaping since all our hints target POSIX-compatible shells
/// (bash, zsh, fish, and Git Bash on Windows).
///
/// # Examples
/// - `/Users/alex/repo` → `~/repo` (no escaping needed)
/// - `/Users/alex/my repo` → `'/Users/alex/my repo'` (needs quoting, use original)
/// - `/tmp/repo` → `/tmp/repo` (no escaping needed)
/// - `/tmp/my repo` → `'/tmp/my repo'` (needs quoting)
pub fn format_path_for_display(path: &Path) -> String {
    // Try to use tilde for home directory paths
    if let Some(home) = home_dir()
        && let Ok(stripped) = path.strip_prefix(&home)
    {
        if stripped.as_os_str().is_empty() {
            return "~".to_string();
        }

        // Build tilde path with forward slash (POSIX style, works everywhere)
        let rest = stripped.to_slash_lossy();

        // Only use tilde form if the rest doesn't need escaping
        // (tilde doesn't expand inside quotes)
        if !needs_shell_escaping(&rest) {
            return format!("~/{rest}");
        }
    }

    // Non-home path or escaping needed - use POSIX quoting
    // Use to_slash_lossy for Windows compatibility (forward slashes in shell hints)
    let original = path.to_slash_lossy();
    match escape(Cow::Borrowed(&original)) {
        Cow::Borrowed(_) => original.into_owned(),
        Cow::Owned(escaped) => escaped,
    }
}

/// Sanitize a string for use as a filename on all platforms.
///
/// Uses `sanitize-filename` crate to handle invalid characters, control characters,
/// Windows reserved names (CON, PRN, etc.), and trailing dots/spaces. Appends a
/// 3-character hash suffix for collision avoidance.
///
/// The hash ensures unique outputs for inputs that would otherwise collide
/// (e.g., `origin/feature` and `origin-feature` both sanitize to `origin-feature`
/// but get different hash suffixes).
pub fn sanitize_for_filename(value: &str) -> String {
    let mut result = sanitize_with_options(
        value,
        SanitizeOptions {
            windows: true,
            truncate: false,
            replacement: "-",
        },
    );

    if result.is_empty() {
        result = "_empty".to_string();
    }

    // Append hash suffix for collision avoidance (computed from original input)
    if !result.ends_with('-') {
        result.push('-');
    }
    result.push_str(&short_hash(value));
    result
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{format_path_for_display, home_dir, sanitize_for_filename, to_posix_path};

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

    #[test]
    fn test_sanitize_for_filename_replaces_invalid_chars() {
        assert!(sanitize_for_filename("foo/bar").starts_with("foo-bar-"));
        assert!(sanitize_for_filename("name:with?chars").starts_with("name-with-chars-"));
    }

    #[test]
    fn test_sanitize_for_filename_trims_trailing_dots_and_spaces() {
        assert!(sanitize_for_filename("file. ").starts_with("file-"));
        assert!(sanitize_for_filename("file...").starts_with("file-"));
    }

    #[test]
    fn test_sanitize_for_filename_handles_reserved_names() {
        // Reserved names are replaced (not preserved) - the hash ensures uniqueness
        let con = sanitize_for_filename("CON");
        let com1 = sanitize_for_filename("com1");
        assert!(
            !con.is_empty() && con.len() > 3,
            "CON should produce valid filename: {con}"
        );
        assert!(
            !com1.is_empty() && com1.len() > 3,
            "com1 should produce valid filename: {com1}"
        );
    }

    #[test]
    fn test_sanitize_for_filename_handles_empty() {
        assert!(sanitize_for_filename("").starts_with("_empty-"));
    }

    #[test]
    fn test_sanitize_for_filename_avoids_collisions() {
        // These would collide without the hash suffix
        let a = sanitize_for_filename("origin/feature");
        let b = sanitize_for_filename("origin-feature");

        assert_ne!(a, b, "collision: {a} == {b}");
        assert!(a.starts_with("origin-feature-"));
        assert!(b.starts_with("origin-feature-"));
    }

    #[test]
    #[cfg(unix)]
    fn format_path_for_display_escaping() {
        use insta::assert_snapshot;

        let Some(home) = home_dir() else {
            return;
        };

        // Build test cases: (input_path, expected_pattern)
        // For home paths, we normalize output by replacing actual result with description
        let mut lines = Vec::new();

        // Non-home paths - predictable across machines
        for path_str in [
            "/tmp/repo",
            "/tmp/my repo",
            "/tmp/file;rm -rf",
            "/tmp/test'quote",
        ] {
            let path = PathBuf::from(path_str);
            lines.push(format!(
                "{} => {}",
                path_str,
                format_path_for_display(&path)
            ));
        }

        // Home-relative paths - normalize by showing ~/... pattern
        let home_cases = [
            "workspace/repo",    // simple -> ~/workspace/repo
            "my workspace/repo", // spaces -> quoted absolute
            "project's/repo",    // quote -> quoted absolute
        ];

        for suffix in home_cases {
            let path = home.join(suffix);
            let result = format_path_for_display(&path);

            let display = if result.starts_with('\'') {
                // Quoted absolute path - normalize for snapshot
                "QUOTED_ABSOLUTE".to_string()
            } else {
                result
            };
            lines.push(format!("$HOME/{} => {}", suffix, display));
        }

        // Home directory itself
        lines.push(format!("$HOME => {}", format_path_for_display(&home)));

        assert_snapshot!(lines.join("\n"), @r"
        /tmp/repo => /tmp/repo
        /tmp/my repo => '/tmp/my repo'
        /tmp/file;rm -rf => '/tmp/file;rm -rf'
        /tmp/test'quote => '/tmp/test'\''quote'
        $HOME/workspace/repo => ~/workspace/repo
        $HOME/my workspace/repo => QUOTED_ABSOLUTE
        $HOME/project's/repo => QUOTED_ABSOLUTE
        $HOME => ~
        ");
    }
}
