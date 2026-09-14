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

/// Expand a leading `~` to the user's home directory.
///
/// The inverse of [`format_path_for_display`], which renders home-relative paths
/// in tilde form. wt prints those paths in its own status lines and hints, so a
/// user pasting one back — quoted, where the shell leaves `~` alone — names the
/// same directory wt named. Paths wt derives itself are already absolute and
/// pass through untouched.
///
/// Only a bare `~` or a leading `~/` expands. `~user` is a shell feature that
/// resolves another account's home directory, which wt does not reimplement, so
/// it stays a literal relative path — as does a `~` anywhere but the front.
pub fn expand_tilde(path: &Path) -> Cow<'_, Path> {
    let Ok(rest) = path.strip_prefix("~") else {
        return Cow::Borrowed(path);
    };
    let Some(home) = home_dir() else {
        return Cow::Borrowed(path);
    };
    Cow::Owned(home.join(rest))
}

/// Canonicalize a path, resolving parent symlinks even if the path doesn't exist.
///
/// For existing paths, uses standard canonicalization.
/// For non-existent paths, canonicalizes the longest existing prefix and appends
/// the remaining components. This handles macOS `/var` -> `/private/var` symlinks
/// correctly for computed worktree paths that don't exist yet.
///
/// A `..` is resolved by the filesystem rather than collapsed lexically, which is
/// load-bearing where [`paths_match`] decides whether two paths name one
/// directory: crossing a symlink, the two readings name different directories,
/// and `WorkingTree::ensure_holds_this_worktree` compares a worktree
/// registration's recorded path against the directory sitting at it to decide
/// whether removal may delete that directory.
///
/// On Windows a drive path's result never carries a `\\?\` prefix — a
/// `VerbatimUNC` share keeps its own — because every caller compares it against
/// another result of this function. `dunce` strips that
/// prefix only from paths of 260 characters or fewer, so a deep path and the
/// short root containing it come back spelled `\\?\C:\…` and `C:\…` — one
/// `Prefix::VerbatimDisk` component and one `Prefix::Disk`, which
/// [`Path::starts_with`] reads as unrelated drives. That split made
/// `wt step copy-ignored` refuse to copy a deep `node_modules` tree into a
/// worktree it was squarely inside (#3898).
pub fn canonicalize_with_parents(path: &Path) -> PathBuf {
    strip_verbatim_disk_prefix(resolve_with_parents(path))
}

fn resolve_with_parents(path: &Path) -> PathBuf {
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

/// Drop the `\\?\` prefix from a canonicalized drive path. A no-op off Windows.
///
/// Only a `VerbatimDisk` prefix is stripped: `\\?\UNC\server\share` names a
/// share, and cutting the same four characters off it would leave the relative
/// `UNC\server\share`. Unlike `dunce`, nothing about the path's contents keeps
/// the prefix — not its length, and not the reserved DOS names (`con`, `aux`,
/// `com1`) or trailing dots and spaces that `dunce`'s `is_safe_to_strip_unc`
/// also declines to strip. [`canonicalize_with_parents`] feeds comparisons and
/// messages, not the legacy APIs those rules protect, and `std::fs` re-applies
/// a verbatim prefix itself for any path it opens. What that trades is confined
/// to the message case: `registration_worktree_path` shows its result in
/// `GitError::WorktreePathNotOurs`, and for a path a reserved component makes
/// unreachable under the legacy spelling, the reported spelling is one the user
/// can't `cd` into.
#[cfg(windows)]
fn strip_verbatim_disk_prefix(path: PathBuf) -> PathBuf {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::{Component, Prefix};

    let verbatim_disk = matches!(
        path.components().next(),
        Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::VerbatimDisk(_))
    );
    if !verbatim_disk {
        return path;
    }
    // Cut the prefix in UTF-16 rather than slicing a `&str`, so a path `to_str`
    // can't render is spelled like every other result instead of keeping the
    // split this function exists to close. `\\?\` is four ASCII characters, so
    // four UTF-16 units.
    let stripped: Vec<u16> = path.as_os_str().encode_wide().skip(4).collect();
    PathBuf::from(OsString::from_wide(&stripped))
}

#[cfg(not(windows))]
fn strip_verbatim_disk_prefix(path: PathBuf) -> PathBuf {
    path
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

/// The command name a path to an executable names: its file name with the
/// platform's executable suffix stripped.
///
/// The one place wt turns `argv[0]` into a command name.
/// `invocation::binary_name` names wt in the shell integration it generates and
/// in its own hints; [`testing::mock_stub`](crate::testing::mock_stub) selects a
/// mock's config by it. A name that resolved differently in the two would be a
/// silent disagreement about what this process is called. The shell-integration
/// warning is not a third caller: it reports the name as typed, `.exe` and all,
/// so it can tell a Windows user to drop the suffix.
///
/// Stripping [`std::env::consts::EXE_SUFFIX`] rather than taking
/// [`Path::file_stem`] keeps a dotted name (`wt.old`, `python3.11`) whole.
/// `file_stem` cuts at the last dot wherever that dot is, so the same name
/// resolves differently per platform: Unix's `python3.11` becomes `python3`
/// while the Windows link `python3.11.exe` keeps `python3.11`. The suffix
/// matches case-insensitively, because Windows resolves `WT.EXE` and `wt.exe`
/// to the same file.
///
/// Non-UTF8 bytes convert lossily rather than failing. The callers that embed
/// the name in shell syntax validate it first
/// ([`shell::validate_shell_command_name`](crate::shell::validate_shell_command_name)),
/// and a name that can't be spelled is better rejected there than silently
/// replaced by wt's own.
///
/// `None` when the path has no file name, or when the file name is nothing but
/// the suffix — a degenerate `argv[0]`, which a caller treats as "no name"
/// rather than panicking inside `main()`.
///
/// Not [`shell::extract_filename_from_path`](crate::shell::extract_filename_from_path),
/// which strips `.exe` on every platform: that one names shells from `$SHELL`
/// and the process tree, where a Unix-form path can still carry a Windows
/// `.exe` (Git Bash), while `argv[0]` always spells its name the way the
/// running platform does.
pub fn executable_name(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy();
    let name = strip_suffix_ignoring_case(&name, std::env::consts::EXE_SUFFIX);
    (!name.is_empty()).then(|| name.to_string())
}

/// Strip `suffix` from the end of `name`, matching it case-insensitively.
///
/// The suffix is a parameter because the two callers strip different ones:
/// [`executable_name`] strips the running platform's
/// [`std::env::consts::EXE_SUFFIX`], while
/// [`shell::extract_filename_from_path`](crate::shell::extract_filename_from_path)
/// strips `.exe` on every platform. It also lets a test on any platform exercise
/// the Windows answer, which `EXE_SUFFIX` — empty on Unix — cannot.
///
/// `str::get` rather than a byte slice, so a name whose trailing bytes fall
/// mid-character yields the name unchanged instead of panicking. The macOS `ps`
/// snapshot feeds every process name on the machine through here, so a name
/// like `日本語` is ordinary input rather than a hypothetical.
pub(crate) fn strip_suffix_ignoring_case<'a>(name: &'a str, suffix: &str) -> &'a str {
    let stem_len = name.len().saturating_sub(suffix.len());
    match name.get(stem_len..) {
        Some(tail) if tail.eq_ignore_ascii_case(suffix) => &name[..stem_len],
        _ => name,
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{
        canonicalize_with_parents, executable_name, expand_tilde, format_path_for_display,
        home_dir, paths_match, sanitize_for_filename, strip_suffix_ignoring_case, to_posix_path,
    };

    /// The tilde form `format_path_for_display` prints is a form wt reads back,
    /// so a path from wt's own output can be pasted into a wt command.
    #[test]
    fn expand_tilde_round_trips_displayed_paths() {
        // No skip guard: every platform the suite runs on sets HOME or
        // USERPROFILE, and skipping would leave the assertion silently unrun.
        let home = home_dir().expect("HOME or USERPROFILE is set");

        let path = home.join("workspace").join("repo.feature");
        let displayed = format_path_for_display(&path);
        assert_eq!(expand_tilde(&PathBuf::from(displayed)), path);

        assert_eq!(expand_tilde(&PathBuf::from("~")), home);
    }

    /// Only a leading `~` component expands: `~user` is a shell feature wt does
    /// not reimplement, and a tilde mid-path is an ordinary directory name.
    #[test]
    fn expand_tilde_leaves_other_tildes_alone() {
        for literal in ["~user/repo", "sub/~/repo", "repo~", "../repo.feature"] {
            let path = PathBuf::from(literal);
            assert_eq!(expand_tilde(&path), path, "{literal} should not expand");
        }
    }

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

    /// A path long enough that `dunce` keeps its `\\?\` prefix must still
    /// compare as a descendant of the short root it sits under — the split that
    /// made `wt step copy-ignored` refuse a deep `node_modules` tree (#3898).
    #[test]
    #[cfg(windows)]
    fn canonicalize_with_parents_agrees_across_the_verbatim_threshold() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let root = tmp.path();

        // 260 characters is `dunce`'s cutoff for stripping the prefix; overshoot
        // it so the deep end is unambiguously on the other side.
        let mut deep = root.to_path_buf();
        while deep.as_os_str().len() < 320 {
            deep.push("nested-dependency-directory");
        }
        std::fs::create_dir_all(&deep).expect("create deep tree");

        // Pin the mechanism, so a `dunce` that stopped splitting the two
        // spellings shows up here rather than leaving the assertion below
        // passing for a reason it was not written for.
        let verbatim = |p: &Path| {
            dunce::canonicalize(p)
                .expect("canonicalize")
                .to_string_lossy()
                .starts_with(r"\\?\")
        };
        assert!(verbatim(&deep), "expected dunce to keep the prefix here");
        assert!(!verbatim(root), "expected dunce to strip the prefix here");

        // The path `ensure_path_within_root` actually checks is a destination
        // directory that has not been created yet, so ask about a child.
        let canonical_root = canonicalize_with_parents(root);
        let canonical_deep = canonicalize_with_parents(&deep.join("not-yet-created"));
        assert!(
            !canonical_deep.to_string_lossy().starts_with(r"\\?\"),
            "expected the deep result to drop the verbatim prefix, got {}",
            canonical_deep.display()
        );
        assert!(
            canonical_deep.starts_with(&canonical_root),
            "{} should sit under {}",
            canonical_deep.display(),
            canonical_root.display()
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

    /// A dot in `argv[0]` is part of the name, not an extension to drop:
    /// `file_stem` would cut `wt.old` down to `wt` while leaving the Windows
    /// `wt.old.exe` whole, and both callers need one answer everywhere.
    #[test]
    fn executable_name_strips_only_the_executable_suffix() {
        let cases = [
            ("wt", Some("wt")),
            ("./wt", Some("wt")),
            ("target/debug/wt", Some("wt")),
            ("/usr/local/bin/git-wt", Some("git-wt")),
            ("wt.old", Some("wt.old")),
            ("python3.11", Some("python3.11")),
            ("", None),
            ("/", None),
        ];
        for (arg0, expected) in cases {
            assert_eq!(
                executable_name(Path::new(arg0)).as_deref(),
                expected,
                "{arg0}"
            );
        }

        // The suffix stripped is the running platform's own: a Unix file
        // really named `wt.exe` keeps the name it has.
        let (exe, suffix_only) = if cfg!(windows) {
            (Some("wt"), None)
        } else {
            (Some("wt.exe"), Some(".exe"))
        };
        assert_eq!(executable_name(Path::new("wt.exe")).as_deref(), exe);
        assert_eq!(executable_name(Path::new(".exe")).as_deref(), suffix_only);
    }

    /// The `.exe` half of that, on whichever platform runs the test: `EXE_SUFFIX`
    /// is empty on Unix, so `executable_name` alone can never exercise the
    /// answer Windows gets.
    #[test]
    fn strip_suffix_ignoring_case_matches_every_exe_spelling() {
        let cases = [
            ("wt.exe", "wt"),
            // Windows resolves these to one file, so all three name one command.
            ("WT.EXE", "WT"),
            ("wt.Exe", "wt"),
            ("git-wt.exe", "git-wt"),
            // Only a suffix: `wt.exe.old` is named for its dot, not its middle.
            ("wt.exe.old", "wt.exe.old"),
            ("wt", "wt"),
            ("exe", "exe"),
            (".exe", ""),
            // Trailing bytes falling mid-character are not a suffix match.
            ("€ab", "€ab"),
        ];
        for (name, expected) in cases {
            assert_eq!(strip_suffix_ignoring_case(name, ".exe"), expected, "{name}");
        }

        // The empty suffix Unix carries strips nothing.
        assert_eq!(strip_suffix_ignoring_case("wt.exe", ""), "wt.exe");
    }
}
