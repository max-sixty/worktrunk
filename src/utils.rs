//! General utilities.

use std::time::{SystemTime, UNIX_EPOCH};

/// Format a Unix timestamp as ISO 8601 string (e.g., "2025-01-01T00:00:00Z").
///
/// Used for human-readable timestamps in diagnostic reports and logs.
pub fn format_timestamp_iso8601(timestamp: u64) -> String {
    let timestamp = timestamp as i64;
    chrono::DateTime::from_timestamp(timestamp, 0)
        .unwrap_or_else(chrono::Utc::now)
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

/// Format the current time as ISO 8601 string.
///
/// Convenience function combining `get_now()` and `format_timestamp_iso8601()`.
pub fn now_iso8601() -> String {
    format_timestamp_iso8601(get_now())
}

/// Get current Unix timestamp in seconds.
///
/// When `WT_TEST_EPOCH` environment variable is set (by tests), returns that
/// value instead of the actual current time. This enables deterministic test
/// snapshots.
///
/// Note: We use `WT_TEST_EPOCH` rather than `SOURCE_DATE_EPOCH` because the
/// latter is a build-time standard for reproducible builds, commonly set by
/// NixOS/direnv in development shells. Using it at runtime causes incorrect
/// age display. See: <https://github.com/max-sixty/worktrunk/issues/763>
///
/// All code that needs timestamps for display or storage should use this
/// function rather than `SystemTime::now()` directly.
pub fn get_now() -> u64 {
    std::env::var("WT_TEST_EPOCH")
        .ok()
        .and_then(|val| val.parse::<u64>().ok())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock before Unix epoch")
                .as_secs()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_now_returns_reasonable_timestamp() {
        let now = get_now();
        // Should be after 2020-01-01
        assert!(now > 1577836800, "get_now() should return current time");
    }

    #[test]
    fn test_get_now_respects_wt_test_epoch() {
        // When WT_TEST_EPOCH is set (by test harness), get_now() returns it
        if let Ok(epoch) = std::env::var("WT_TEST_EPOCH") {
            let expected: u64 = epoch.parse().unwrap();
            assert_eq!(get_now(), expected);
        }
    }
}
