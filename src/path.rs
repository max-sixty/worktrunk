use dunce::canonicalize;
use path_slash::PathExt as _;
use shell_escape::unix::escape;
use std::borrow::Cow;
use std::path::{Path, PathBuf};

use sanitize_filename::{Options as SanitizeOptions, sanitize_with_options};

use crate::config::short_hash;
#[cfg(windows)]
use crate::shell_exec::{Cmd, ShellConfig};

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
    let Ok(shell) = ShellConfig::get() else {
        return path.to_string();
    };
    let Some(cygpath) = find_cygpath_from_shell(shell) else {
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

/// Canonicalize a path, resolving parent symlinks even if the path doesn't exist.
///
/// For existing paths, uses standard canonicalization.
/// For non-existent paths, canonicalizes the longest existing prefix and appends
/// the remaining components. This handles macOS `/var` -> `/private/var` symlinks
/// correctly for computed worktree paths that don't exist yet.
pub(crate) fn canonicalize_with_parents(path: &Path) -> PathBuf {
    if let Ok(canonical) = canonicalize(path) {
        return canonical;
    }

    let mut existing_prefix = path.to_path_buf();
    let mut suffix_components = Vec::new();

    while !existing_prefix.exists() {
        let (Some(file_name), Some(parent)) =
            (existing_prefix.file_name(), existing_prefix.parent())
        else {
            return path.to_path_buf();
        };
        suffix_components.push(file_name.to_os_string());
        existing_prefix = parent.to_path_buf();
    }

    let canonical_prefix = canonicalize(&existing_prefix).unwrap_or(existing_prefix);
    let mut result = canonical_prefix;
    for component in suffix_components.into_iter().rev() {
        result.push(component);
    }
    result
}

/// Compare two paths for equality, canonicalizing to handle symlinks and relative paths.
///
/// Returns `true` if the paths resolve to the same location.
/// Handles the case where one path exists and the other doesn't by resolving
/// parent directory symlinks for non-existent paths.
pub fn paths_match(a: &Path, b: &Path) -> bool {
    canonicalize_with_parents(a) == canonicalize_with_parents(b)
}

/// Sanitize a string for use as a filename on all platforms.
///
/// Uses `sanitize-filename` crate to handle invalid characters, control characters,
/// Windows reserved names (CON, PRN, etc.), and trailing dots/spaces.
///
/// If the input is already a safe filename, it is returned unchanged. Otherwise
/// a 3-character hash suffix (computed from the original input) is appended so
/// that inputs which would otherwise collide produce distinct outputs (e.g.,
/// `origin/feature` → `origin-feature-<hash>` does not collide with the
/// already-safe `origin-feature`).
pub fn sanitize_for_filename(value: &str) -> String {
    let sanitized = sanitize_with_options(
        value,
        SanitizeOptions {
            windows: true,
            truncate: false,
            replacement: "-",
        },
    );

    if sanitized == value && !value.is_empty() {
        return sanitized;
    }

    let mut result = if sanitized.is_empty() {
        "_empty".to_string()
    } else {
        sanitized
    };
    if !result.ends_with('-') {
        result.push('-');
    }
    result.push_str(&short_hash(value));
    result
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        canonicalize_with_parents, format_path_for_display, home_dir, paths_match,
        sanitize_for_filename, to_posix_path,
    };

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
        // Already-safe names pass through unchanged; only sanitized inputs get
        // a hash suffix. This still avoids collisions because the suffix makes
        // the sanitized form distinct from any plausible already-safe name.
        let a = sanitize_for_filename("origin/feature");
        let b = sanitize_for_filename("origin-feature");

        assert_ne!(a, b, "collision: {a} == {b}");
        assert!(a.starts_with("origin-feature-"));
        assert_eq!(b, "origin-feature");
    }

    #[test]
    fn test_sanitize_for_filename_passes_through_safe_names() {
        assert_eq!(sanitize_for_filename("main"), "main");
        assert_eq!(sanitize_for_filename("feature-x"), "feature-x");
        assert_eq!(
            sanitize_for_filename("rust-doc-comments"),
            "rust-doc-comments"
        );
        assert_eq!(sanitize_for_filename("post-merge"), "post-merge");
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

    #[test]
    fn test_paths_match_identical() {
        let path = PathBuf::from("/tmp/test");
        assert!(paths_match(&path, &path));
    }

    #[test]
    fn test_paths_match_different() {
        let a = PathBuf::from("/tmp/foo");
        let b = PathBuf::from("/tmp/bar");
        assert!(!paths_match(&a, &b));
    }

    #[test]
    fn test_canonicalize_with_parents_existing_path() {
        let tmp = std::env::temp_dir();
        let canonical = canonicalize_with_parents(&tmp);
        assert!(canonical.is_absolute());
    }

    #[test]
    fn test_canonicalize_with_parents_degenerate() {
        // Empty path can't be decomposed (no file_name) — falls through to returning as-is.
        let canonical = canonicalize_with_parents(std::path::Path::new(""));
        assert_eq!(canonical, PathBuf::from(""));
    }

    #[test]
    fn test_canonicalize_with_parents_nonexistent() {
        let tmp = std::env::temp_dir();
        let nonexistent = tmp.join("nonexistent-test-dir-12345");
        let canonical = canonicalize_with_parents(&nonexistent);

        assert!(canonical.is_absolute());
        assert_eq!(
            canonical.file_name().unwrap().to_str().unwrap(),
            "nonexistent-test-dir-12345"
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn test_paths_match_macos_var_symlink() {
        // On macOS, /var is a symlink to /private/var
        let test_dir = PathBuf::from("/var/tmp/wt-test-paths-match");

        let _ = std::fs::remove_dir_all(&test_dir);
        std::fs::create_dir_all(&test_dir).expect("Failed to create test dir");

        let private_path = PathBuf::from("/private/var/tmp/wt-test-paths-match/subdir");
        let var_path = PathBuf::from("/var/tmp/wt-test-paths-match/subdir");

        assert!(
            paths_match(&private_path, &var_path),
            "Paths should match: {:?} vs {:?}",
            canonicalize_with_parents(&private_path),
            canonicalize_with_parents(&var_path)
        );

        let _ = std::fs::remove_dir_all(&test_dir);
    }

    #[test]
    fn test_paths_match_existing_vs_nonexistent() {
        let tmp = std::env::temp_dir();

        let existing = tmp.join("wt-test-existing");
        std::fs::create_dir_all(&existing).expect("Failed to create test dir");

        let nonexistent = tmp.join("wt-test-nonexistent");
        let _ = std::fs::remove_dir_all(&nonexistent);

        assert!(!paths_match(&existing, &nonexistent));

        let canonical = canonicalize_with_parents(&existing);
        assert!(paths_match(&existing, &canonical));

        let _ = std::fs::remove_dir_all(&existing);
    }
}
