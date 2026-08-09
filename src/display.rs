//! Display utilities for terminal output.
//!
//! This module provides utility functions for:
//! - Relative time formatting
//! - Path manipulation and shortening

use std::path::{Component, Path};

use path_slash::PathExt as _;
use worktrunk::path::format_path_for_display;
use worktrunk::utils::epoch_now;

/// Format timestamp as abbreviated relative time (e.g., "2h")
pub(crate) fn format_relative_time_short(timestamp: i64) -> String {
    // Cast to i64 for signed arithmetic (handles future timestamps)
    format_relative_time_impl(timestamp, epoch_now() as i64)
}

fn format_relative_time_impl(timestamp: i64, now: i64) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = MINUTE * 60;
    const DAY: i64 = HOUR * 24;
    const WEEK: i64 = DAY * 7;
    const MONTH: i64 = DAY * 30;
    const YEAR: i64 = DAY * 365;

    let seconds_ago = now - timestamp;

    if seconds_ago < 0 {
        return "future".to_string();
    }

    if seconds_ago < MINUTE {
        return "now".to_string();
    }

    const UNITS: &[(i64, &str)] = &[
        (YEAR, "y"),
        (MONTH, "mo"),
        (WEEK, "w"),
        (DAY, "d"),
        (HOUR, "h"),
        (MINUTE, "m"),
    ];

    for &(unit_seconds, abbrev) in UNITS {
        let value = seconds_ago / unit_seconds;
        if value > 0 {
            return format!("{}{}", value, abbrev);
        }
    }

    "now".to_string()
}

/// Shorten a path relative to the main worktree.
///
/// Returns paths relative to main worktree using `..` components where needed:
/// - Main worktree itself: `.`
/// - Child of main: `./subdir`
/// - Sibling: `../sibling`
/// - Unrelated paths fall back to `~/...` or absolute
pub(crate) fn shorten_path(path: &Path, main_worktree_path: &Path) -> String {
    // Same path = main worktree
    if path == main_worktree_path {
        return ".".to_string();
    }

    // Try to compute relative path
    if let Some(relative) = pathdiff::diff_paths(path, main_worktree_path) {
        // Use forward slashes on all platforms (worktrunk's display convention).
        let rendered = relative.to_slash_lossy();
        // If relative path starts with "..", it's a sibling/ancestor
        // Otherwise prefix with "./" for clarity
        if relative.components().next() == Some(Component::ParentDir) {
            rendered.into_owned()
        } else {
            format!("./{rendered}")
        }
    } else {
        // Can't compute relative path (e.g., different drives on Windows)
        format_path_for_display(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_format_relative_time_short() {
        let now: i64 = 1700000000; // Fixed timestamp for testing

        // Just now (< 1 minute)
        assert_eq!(format_relative_time_impl(now - 30, now), "now");
        assert_eq!(format_relative_time_impl(now - 59, now), "now");

        // Minutes
        assert_eq!(format_relative_time_impl(now - 60, now), "1m");
        assert_eq!(format_relative_time_impl(now - 120, now), "2m");
        assert_eq!(format_relative_time_impl(now - 3599, now), "59m");

        // Hours
        assert_eq!(format_relative_time_impl(now - 3600, now), "1h");
        assert_eq!(format_relative_time_impl(now - 7200, now), "2h");

        // Days
        assert_eq!(format_relative_time_impl(now - 86400, now), "1d");
        assert_eq!(format_relative_time_impl(now - 172800, now), "2d");

        // Weeks
        assert_eq!(format_relative_time_impl(now - 604800, now), "1w");

        // Months
        assert_eq!(format_relative_time_impl(now - 2592000, now), "1mo");

        // Years
        assert_eq!(format_relative_time_impl(now - 31536000, now), "1y");

        // Future timestamp
        assert_eq!(format_relative_time_impl(now + 1000, now), "future");
    }

    #[test]
    #[cfg(unix)] // Uses Unix-style paths
    fn test_shorten_path() {
        let main_worktree = PathBuf::from("/home/user/project");

        // Path is main worktree
        assert_eq!(shorten_path(&main_worktree, &main_worktree), ".");

        // Path is child of main worktree
        let child = PathBuf::from("/home/user/project/subdir");
        assert_eq!(shorten_path(&child, &main_worktree), "./subdir");

        // Path is sibling of main worktree
        let sibling = PathBuf::from("/home/user/project.feature");
        assert_eq!(shorten_path(&sibling, &main_worktree), "../project.feature");

        // Path is parent's sibling
        let cousin = PathBuf::from("/home/user/other-project");
        assert_eq!(shorten_path(&cousin, &main_worktree), "../other-project");

        // Path in completely different location
        let other = PathBuf::from("/var/log/syslog");
        let result = shorten_path(&other, &main_worktree);
        // Should fall back to format_path_for_display or relative with many ../
        // Either way, it shouldn't start with "./" since it's not a child
        assert!(
            result.starts_with("..") || result.starts_with("/"),
            "Expected relative or absolute path for distant location, got: {}",
            result
        );
    }

    #[test]
    #[cfg(windows)]
    fn test_shorten_path_windows() {
        let main_worktree = PathBuf::from(r"C:\Users\user\project");

        // Path is main worktree
        assert_eq!(shorten_path(&main_worktree, &main_worktree), ".");

        // Path is child of main worktree (always forward-slash per display convention)
        let child = PathBuf::from(r"C:\Users\user\project\subdir");
        assert_eq!(shorten_path(&child, &main_worktree), "./subdir");

        // Path is sibling of main worktree
        let sibling = PathBuf::from(r"C:\Users\user\project.feature");
        assert_eq!(shorten_path(&sibling, &main_worktree), "../project.feature");
    }

    #[test]
    fn test_format_relative_time_short_public() {
        // Test the public function (uses epoch_now internally)
        let result = format_relative_time_short(0);
        // A timestamp of 0 (Unix epoch) should show years ago
        assert!(
            result.contains('y') || result == "future",
            "Expected years format, got: {}",
            result
        );
    }

    #[test]
    fn test_epoch_now() {
        // epoch_now should return a reasonable timestamp
        let now = epoch_now();
        // Should be after 2020 (1577836800)
        assert!(now > 1577836800, "epoch_now() should return current time");
    }
}
