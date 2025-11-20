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

pub fn format_relative_time(timestamp: i64) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = MINUTE * 60;
    const DAY: i64 = HOUR * 24;
    const WEEK: i64 = DAY * 7;
    const MONTH: i64 = DAY * 30; // Approximate calendar month
    const YEAR: i64 = DAY * 365; // Approximate calendar year

    // Respect SOURCE_DATE_EPOCH for reproducible builds/tests
    let now = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|val| val.parse::<i64>().ok())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64
        });

    let seconds_ago = now - timestamp;

    if seconds_ago < 0 {
        return "in the future".to_string();
    }

    if seconds_ago < MINUTE {
        return "just now".to_string();
    }

    const UNITS: &[(i64, &str)] = &[
        (YEAR, "year"),
        (MONTH, "month"),
        (WEEK, "week"),
        (DAY, "day"),
        (HOUR, "hour"),
        (MINUTE, "minute"),
    ];

    for &(unit_seconds, label) in UNITS {
        let value = seconds_ago / unit_seconds;
        if value > 0 {
            let plural = if value == 1 { "" } else { "s" };
            return format!("{} {}{} ago", value, label, plural);
        }
    }

    "just now".to_string()
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
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

    if text.width() <= max_width {
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

    // Truncate decisively and add ellipsis marker
    // Note: Can produce "...…" if original text has ellipsis at truncation point,
    // but this is rare and acceptable - we're making a clear truncation decision.
    format!("{}…", &text[..truncate_at])
}

/// Get terminal width, defaulting to 80 if detection fails
pub fn get_terminal_width() -> usize {
    // Check COLUMNS environment variable first (for testing and scripts)
    if let Ok(cols) = std::env::var("COLUMNS")
        && let Ok(width) = cols.parse::<usize>()
    {
        return width;
    }

    // Fall back to actual terminal size
    terminal_size::terminal_size()
        .map(|(terminal_size::Width(w), _)| w as usize)
        .unwrap_or(80)
}

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
}
