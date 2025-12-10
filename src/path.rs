use std::path::{Path, PathBuf};

/// Canonicalize a path without Windows verbatim prefix (`\\?\`).
///
/// On Windows, `std::fs::canonicalize()` returns verbatim paths like `\\?\C:\...`
/// which external tools like git cannot handle. The `dunce` crate strips this
/// prefix when safe. On Unix, this is equivalent to `std::fs::canonicalize()`.
pub fn canonicalize(path: &Path) -> std::io::Result<PathBuf> {
    dunce::canonicalize(path)
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

    use super::{format_path_for_display, home_dir};

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
}
