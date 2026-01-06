//! Display utilities for terminal output.
//!
//! This module provides utility functions for:
//! - Relative time formatting
//! - Path manipulation and shortening
//! - Text truncation with word boundaries
//! - Terminal width detection

use std::path::Path;
use worktrunk::path::format_path_for_display;
use worktrunk::utils::get_now;

/// Format timestamp as abbreviated relative time (e.g., "2h")
pub fn format_relative_time_short(timestamp: i64) -> String {
    // Cast to i64 for signed arithmetic (handles future timestamps)
    format_relative_time_impl(timestamp, get_now() as i64)
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
pub fn shorten_path(path: &Path, main_worktree_path: &Path) -> String {
    use std::path::Component;

    // Same path = main worktree
    if path == main_worktree_path {
        return ".".to_string();
    }

    // Try to compute relative path
    if let Some(relative) = pathdiff::diff_paths(path, main_worktree_path) {
        // If relative path starts with "..", it's a sibling/ancestor
        // Otherwise prefix with "./" (or ".\" on Windows) for clarity
        if relative.components().next() == Some(Component::ParentDir) {
            relative.display().to_string()
        } else {
            format!(".{}{}", std::path::MAIN_SEPARATOR, relative.display())
        }
    } else {
        // Can't compute relative path (e.g., different drives on Windows)
        format_path_for_display(path)
    }
}

/// Truncate text with ellipsis at exact width limit.
///
/// Truncates at character boundary (mid-word if needed) to fill the allocated
/// column width exactly. This ensures consistent table output width.
pub fn truncate_to_width(text: &str, max_width: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    use worktrunk::styling::visual_width;

    if visual_width(text) <= max_width {
        return text.to_string();
    }

    // Build up string until we hit the width limit (accounting for "…" = 1 width)
    let target_width = max_width.saturating_sub(1);
    let mut current_width = 0;
    let mut last_idx = 0;

    for (idx, ch) in text.char_indices() {
        let char_width = ch.width().unwrap_or(0);
        if current_width + char_width > target_width {
            break;
        }
        current_width += char_width;
        last_idx = idx + ch.len_utf8();
    }

    // Truncate at exact character boundary (mid-word if needed)
    let truncated = text[..last_idx].trim_end();
    format!("{}…", truncated)
}

// Re-export from styling for convenience
pub use worktrunk::styling::{get_terminal_width, truncate_visible};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_truncate_normal_case() {
        let text = "Fix bug with parsing and more text here";
        let result = truncate_to_width(text, 25);
        println!("Normal truncation:      '{}'", result);
        assert!(result.ends_with('…'), "Should end with ellipsis");
    }

    #[test]
    fn test_truncate_with_existing_ascii_ellipsis() {
        let text = "Fix bug with parsing... more text here";
        let result = truncate_to_width(text, 25);
        // Shows what happens when truncation lands on existing "..."
        println!("ASCII ellipsis:         '{}'", result);
        assert!(result.ends_with('…'), "Should end with ellipsis");
    }

    #[test]
    fn test_truncate_with_existing_unicode_ellipsis() {
        let text = "Fix bug with parsing… more text here";
        let result = truncate_to_width(text, 25);
        // Shows what happens when truncation lands on existing "…"
        println!("Unicode ellipsis:       '{}'", result);
        assert!(result.ends_with('…'), "Should end with ellipsis");
    }

    #[test]
    fn test_truncate_already_has_three_dots() {
        let text = "Short text...";
        let result = truncate_to_width(text, 20);
        // When text fits, should return as-is
        assert_eq!(result, "Short text...");
    }

    #[test]
    fn test_truncate_exact_width() {
        let text = "This is a very long message that needs truncation";
        let result = truncate_to_width(text, 30);
        assert!(result.ends_with('…'), "Should end with ellipsis");
        assert!(
            !result.contains(" …"),
            "Should not have space before ellipsis"
        );
        // Should truncate at exact width (mid-word if needed)
        use unicode_width::UnicodeWidthStr;
        assert_eq!(result.width(), 30, "Should fill exact width");
    }

    #[test]
    fn test_truncate_unicode_width() {
        let text = "Fix bug with café ☕ and more text";
        let result = truncate_to_width(text, 25);
        use unicode_width::UnicodeWidthStr;
        assert!(
            result.width() <= 25,
            "Width {} should be <= 25",
            result.width()
        );
    }

    #[test]
    fn test_truncate_no_truncation_needed() {
        let text = "Short message";
        let result = truncate_to_width(text, 50);
        assert_eq!(result, text);
    }

    #[test]
    fn test_truncate_very_long_word() {
        let text = "Supercalifragilisticexpialidocious extra text";
        let result = truncate_to_width(text, 20);
        use unicode_width::UnicodeWidthStr;
        // Should truncate mid-word if no space found
        assert!(result.width() <= 20, "Width should be <= 20");
        assert!(result.ends_with('…'), "Should end with ellipsis");
    }

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

        // Path is child of main worktree
        let child = PathBuf::from(r"C:\Users\user\project\subdir");
        assert_eq!(shorten_path(&child, &main_worktree), r".\subdir");

        // Path is sibling of main worktree
        let sibling = PathBuf::from(r"C:\Users\user\project.feature");
        assert_eq!(
            shorten_path(&sibling, &main_worktree),
            r"..\project.feature"
        );
    }

    #[test]
    fn test_format_relative_time_short_public() {
        // Test the public function (uses get_now internally)
        let result = format_relative_time_short(0);
        // A timestamp of 0 (Unix epoch) should show years ago
        assert!(
            result.contains('y') || result == "future",
            "Expected years format, got: {}",
            result
        );
    }

    #[test]
    fn test_get_now() {
        // get_now should return a reasonable timestamp
        let now = get_now();
        // Should be after 2020 (1577836800)
        assert!(now > 1577836800, "get_now() should return current time");
    }

    #[test]
    fn test_truncate_edge_cases() {
        // Empty string
        let result = truncate_to_width("", 10);
        assert_eq!(result, "");

        // Single character
        let result = truncate_to_width("X", 10);
        assert_eq!(result, "X");

        // Exact width
        let result = truncate_to_width("12345", 5);
        assert_eq!(result, "12345");

        // Just over width
        let result = truncate_to_width("123456", 5);
        assert!(result.ends_with('…'));
    }
}
