//! Display utilities for terminal output.
//!
//! This module provides utility functions for:
//! - Relative time formatting
//! - Path manipulation and shortening
//! - Text truncation with word boundaries
//! - Terminal width detection

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use worktrunk::path::format_path_for_display;

/// Format timestamp as abbreviated relative time (e.g., "2h")
pub fn format_relative_time_short(timestamp: i64) -> String {
    format_relative_time_impl(timestamp, get_now(), true)
}

/// Get current time, respecting SOURCE_DATE_EPOCH for reproducible builds/tests
fn get_now() -> i64 {
    std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|val| val.parse::<i64>().ok())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64
        })
}

fn format_relative_time_impl(timestamp: i64, now: i64, short: bool) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = MINUTE * 60;
    const DAY: i64 = HOUR * 24;
    const WEEK: i64 = DAY * 7;
    const MONTH: i64 = DAY * 30;
    const YEAR: i64 = DAY * 365;

    let seconds_ago = now - timestamp;

    if seconds_ago < 0 {
        return if short { "future" } else { "in the future" }.to_string();
    }

    if seconds_ago < MINUTE {
        return if short { "now" } else { "just now" }.to_string();
    }

    const UNITS: &[(i64, &str, &str)] = &[
        (YEAR, "year", "y"),
        (MONTH, "month", "mo"),
        (WEEK, "week", "w"),
        (DAY, "day", "d"),
        (HOUR, "hour", "h"),
        (MINUTE, "minute", "m"),
    ];

    for &(unit_seconds, label, abbrev) in UNITS {
        let value = seconds_ago / unit_seconds;
        if value > 0 {
            return if short {
                format!("{}{}", value, abbrev)
            } else {
                let plural = if value == 1 { "" } else { "s" };
                format!("{} {}{} ago", value, label, plural)
            };
        }
    }

    if short { "now" } else { "just now" }.to_string()
}

/// Find the common prefix among all paths
pub fn find_common_prefix<P: AsRef<Path>>(paths: &[P]) -> PathBuf {
    if paths.is_empty() {
        return PathBuf::new();
    }

    let first = paths[0].as_ref();
    let mut prefix = PathBuf::new();

    for component in first.components() {
        let candidate = prefix.join(component);
        if paths.iter().all(|p| p.as_ref().starts_with(&candidate)) {
            prefix = candidate;
        } else {
            break;
        }
    }

    prefix
}

/// Shorten a path relative to a common prefix
pub fn shorten_path(path: &Path, prefix: &Path) -> String {
    match path.strip_prefix(prefix) {
        Ok(rel) if rel.as_os_str().is_empty() => ".".to_string(),
        Ok(rel) => format!("./{}", rel.display()),
        Err(_) => format_path_for_display(path),
    }
}

/// Truncate text at word boundary with ellipsis, respecting terminal width
pub fn truncate_at_word_boundary(text: &str, max_width: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    use worktrunk::styling::visual_width;

    if visual_width(text) <= max_width {
        return text.to_string();
    }

    // Build up string until we hit the width limit (accounting for "…" = 1 width)
    let target_width = max_width.saturating_sub(1);
    let mut current_width = 0;
    let mut last_space_idx = None;
    let mut last_idx = 0;

    for (idx, ch) in text.char_indices() {
        let char_width = ch.width().unwrap_or(0);
        if current_width + char_width > target_width {
            break;
        }
        if ch.is_whitespace() {
            last_space_idx = Some(idx);
        }
        current_width += char_width;
        last_idx = idx + ch.len_utf8();
    }

    // Use last space if found, otherwise truncate at last character that fits
    let truncate_at = last_space_idx.unwrap_or(last_idx);

    // Truncate and trim trailing whitespace before adding ellipsis
    // This prevents "text …" with space before ellipsis
    let truncated = text[..truncate_at].trim_end();
    format!("{}…", truncated)
}

// Re-export from styling for convenience
pub use worktrunk::styling::{get_terminal_width, truncate_visible};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_normal_case() {
        let text = "Fix bug with parsing and more text here";
        let result = truncate_at_word_boundary(text, 25);
        println!("Normal truncation:      '{}'", result);
        assert!(result.ends_with('…'), "Should end with ellipsis");
    }

    #[test]
    fn test_truncate_with_existing_ascii_ellipsis() {
        let text = "Fix bug with parsing... more text here";
        let result = truncate_at_word_boundary(text, 25);
        // Shows what happens when truncation lands on existing "..."
        println!("ASCII ellipsis:         '{}'", result);
        assert!(result.ends_with('…'), "Should end with ellipsis");
    }

    #[test]
    fn test_truncate_with_existing_unicode_ellipsis() {
        let text = "Fix bug with parsing… more text here";
        let result = truncate_at_word_boundary(text, 25);
        // Shows what happens when truncation lands on existing "…"
        println!("Unicode ellipsis:       '{}'", result);
        assert!(result.ends_with('…'), "Should end with ellipsis");
    }

    #[test]
    fn test_truncate_already_has_three_dots() {
        let text = "Short text...";
        let result = truncate_at_word_boundary(text, 20);
        // When text fits, should return as-is
        assert_eq!(result, "Short text...");
    }

    #[test]
    fn test_truncate_word_boundary() {
        let text = "This is a very long message that needs truncation";
        let result = truncate_at_word_boundary(text, 30);
        assert!(result.ends_with('…'), "Should end with ellipsis");
        assert!(
            !result.contains(" …"),
            "Should not have space before ellipsis"
        );
        // Should truncate at word boundary
        assert!(
            !result.contains("truncate"),
            "Should not break word 'truncation'"
        );
    }

    #[test]
    fn test_truncate_unicode_width() {
        let text = "Fix bug with café ☕ and more text";
        let result = truncate_at_word_boundary(text, 25);
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
        let result = truncate_at_word_boundary(text, 50);
        assert_eq!(result, text);
    }

    #[test]
    fn test_truncate_very_long_word() {
        let text = "Supercalifragilisticexpialidocious extra text";
        let result = truncate_at_word_boundary(text, 20);
        use unicode_width::UnicodeWidthStr;
        // Should truncate mid-word if no space found
        assert!(result.width() <= 20, "Width should be <= 20");
        assert!(result.ends_with('…'), "Should end with ellipsis");
    }

    #[test]
    fn test_format_relative_time_short() {
        let now: i64 = 1700000000; // Fixed timestamp for testing

        // Just now (< 1 minute)
        assert_eq!(format_relative_time_impl(now - 30, now, true), "now");
        assert_eq!(format_relative_time_impl(now - 59, now, true), "now");

        // Minutes
        assert_eq!(format_relative_time_impl(now - 60, now, true), "1m");
        assert_eq!(format_relative_time_impl(now - 120, now, true), "2m");
        assert_eq!(format_relative_time_impl(now - 3599, now, true), "59m");

        // Hours
        assert_eq!(format_relative_time_impl(now - 3600, now, true), "1h");
        assert_eq!(format_relative_time_impl(now - 7200, now, true), "2h");

        // Days
        assert_eq!(format_relative_time_impl(now - 86400, now, true), "1d");
        assert_eq!(format_relative_time_impl(now - 172800, now, true), "2d");

        // Weeks
        assert_eq!(format_relative_time_impl(now - 604800, now, true), "1w");

        // Months
        assert_eq!(format_relative_time_impl(now - 2592000, now, true), "1mo");

        // Years
        assert_eq!(format_relative_time_impl(now - 31536000, now, true), "1y");

        // Future timestamp
        assert_eq!(format_relative_time_impl(now + 1000, now, true), "future");
    }

    #[test]
    fn test_format_relative_time_long() {
        let now: i64 = 1700000000;

        assert_eq!(format_relative_time_impl(now - 30, now, false), "just now");
        assert_eq!(
            format_relative_time_impl(now - 60, now, false),
            "1 minute ago"
        );
        assert_eq!(
            format_relative_time_impl(now - 120, now, false),
            "2 minutes ago"
        );
        assert_eq!(
            format_relative_time_impl(now - 3600, now, false),
            "1 hour ago"
        );
        assert_eq!(
            format_relative_time_impl(now - 86400, now, false),
            "1 day ago"
        );
        assert_eq!(
            format_relative_time_impl(now + 1000, now, false),
            "in the future"
        );
    }

    #[test]
    fn test_find_common_prefix() {
        // Empty input
        let empty: Vec<PathBuf> = vec![];
        assert_eq!(find_common_prefix(&empty), PathBuf::new());

        // Single path
        let single = vec![PathBuf::from("/home/user/projects")];
        assert_eq!(
            find_common_prefix(&single),
            PathBuf::from("/home/user/projects")
        );

        // Common prefix exists
        let paths = vec![
            PathBuf::from("/home/user/projects/foo"),
            PathBuf::from("/home/user/projects/bar"),
            PathBuf::from("/home/user/projects/baz"),
        ];
        assert_eq!(
            find_common_prefix(&paths),
            PathBuf::from("/home/user/projects")
        );

        // No common prefix beyond root
        let paths = vec![
            PathBuf::from("/home/user/projects"),
            PathBuf::from("/var/log"),
        ];
        assert_eq!(find_common_prefix(&paths), PathBuf::from("/"));
    }

    #[test]
    fn test_shorten_path() {
        let prefix = PathBuf::from("/home/user/projects");

        // Path within prefix
        let path = PathBuf::from("/home/user/projects/foo/bar");
        assert_eq!(shorten_path(&path, &prefix), "./foo/bar");

        // Path equals prefix
        assert_eq!(shorten_path(&prefix, &prefix), ".");

        // Path outside prefix (falls back to full path)
        let other = PathBuf::from("/var/log/syslog");
        // The result includes tilde expansion, so just check it doesn't start with "./"
        let result = shorten_path(&other, &prefix);
        assert!(!result.starts_with("./"));
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
        let result = truncate_at_word_boundary("", 10);
        assert_eq!(result, "");

        // Single character
        let result = truncate_at_word_boundary("X", 10);
        assert_eq!(result, "X");

        // Exact width
        let result = truncate_at_word_boundary("12345", 5);
        assert_eq!(result, "12345");

        // Just over width
        let result = truncate_at_word_boundary("123456", 5);
        assert!(result.ends_with('…'));
    }

    #[test]
    fn test_find_common_prefix_relative_paths() {
        let paths = vec![PathBuf::from("src/main.rs"), PathBuf::from("src/lib.rs")];
        assert_eq!(find_common_prefix(&paths), PathBuf::from("src"));
    }
}
