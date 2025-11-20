use std::path::{Path, PathBuf};

/// Format a filesystem path for user-facing output.
///
/// When the path lives under the user's home directory, it is shown with a
/// leading `~` (e.g., `/Users/alex/projects/wt` -> `~/projects/wt`). Paths
/// outside home are returned unchanged.
pub fn format_path_for_display(path: &Path) -> String {
    fn home_dir() -> Option<PathBuf> {
        #[cfg(windows)]
        {
            std::env::var_os("USERPROFILE")
                .map(PathBuf::from)
                .or_else(|| {
                    let drive = std::env::var_os("HOMEDRIVE")?;
                    let path = std::env::var_os("HOMEPATH")?;
                    Some(PathBuf::from(drive).join(path))
                })
        }

        #[cfg(not(windows))]
        {
            std::env::var_os("HOME").map(PathBuf::from)
        }
    }

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
    use super::format_path_for_display;
    use std::path::PathBuf;

    fn home_dir() -> Option<PathBuf> {
        #[cfg(windows)]
        {
            std::env::var_os("USERPROFILE")
                .map(PathBuf::from)
                .or_else(|| {
                    let drive = std::env::var_os("HOMEDRIVE")?;
                    let path = std::env::var_os("HOMEPATH")?;
                    Some(PathBuf::from(drive).join(path))
                })
        }

        #[cfg(not(windows))]
        {
            std::env::var_os("HOME").map(PathBuf::from)
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
}
