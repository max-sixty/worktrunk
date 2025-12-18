//! General utilities.

use std::time::{SystemTime, UNIX_EPOCH};

/// Get current Unix timestamp in seconds, respecting `SOURCE_DATE_EPOCH`.
///
/// When `SOURCE_DATE_EPOCH` environment variable is set, returns that value
/// instead of the actual current time. This enables reproducible builds and
/// deterministic test snapshots.
///
/// All code that needs timestamps for display or storage should use this
/// function rather than `SystemTime::now()` directly.
pub fn get_now() -> u64 {
    std::env::var("SOURCE_DATE_EPOCH")
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
    fn test_get_now_respects_source_date_epoch() {
        // When SOURCE_DATE_EPOCH is set (by test harness), get_now() returns it
        if let Ok(epoch) = std::env::var("SOURCE_DATE_EPOCH") {
            let expected: u64 = epoch.parse().unwrap();
            assert_eq!(get_now(), expected);
        }
    }
}
