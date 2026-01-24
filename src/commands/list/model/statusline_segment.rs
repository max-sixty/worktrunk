//! Statusline segment types for smart truncation.
//!
//! [`StatuslineSegment`] provides a way to format statusline output with
//! priority-based truncation when space is limited.

use ansi_str::AnsiStr;
use unicode_width::UnicodeWidthStr;

use crate::commands::list::columns::ColumnKind;

/// A segment of statusline output with priority for smart truncation.
///
/// Priorities match `wt list` column priorities (lower = more important):
/// - 0: Directory (Claude Code only)
/// - 1: Branch, Model (Claude Code only)
/// - 2: Status symbols
/// - 3-9: Various stats (working diff, commits, upstream, CI, URL)
///
/// Use [`StatuslineSegment::fit_to_width`] to truncate by dropping low-priority
/// segments first.
#[derive(Clone, Debug)]
pub struct StatuslineSegment {
    /// The rendered content (may include ANSI codes)
    pub content: String,
    /// Priority (lower = more important, won't be dropped first)
    pub priority: u8,
    /// Optional column kind for identifying segment type (e.g., for filtering)
    pub kind: Option<ColumnKind>,
}

impl StatuslineSegment {
    /// Create a segment with explicit priority (no associated column kind).
    pub fn new(content: String, priority: u8) -> Self {
        Self {
            content,
            priority,
            kind: None,
        }
    }

    /// Create a segment from a column kind, inheriting its priority.
    pub fn from_column(content: String, kind: ColumnKind) -> Self {
        Self {
            content,
            priority: kind.priority(),
            kind: Some(kind),
        }
    }

    /// Get the visible width of this segment (strips ANSI codes).
    pub fn width(&self) -> usize {
        self.content.ansi_strip().width()
    }

    /// Join segments with 2-space separators.
    pub fn join(segments: &[Self]) -> String {
        segments
            .iter()
            .map(|s| s.content.as_str())
            .collect::<Vec<_>>()
            .join("  ")
    }

    /// Calculate total width of segments when joined with 2-space separators.
    pub fn total_width(segments: &[Self]) -> usize {
        if segments.is_empty() {
            return 0;
        }
        let content_width: usize = segments.iter().map(|s| s.width()).sum();
        let separator_width = (segments.len() - 1) * 2;
        content_width + separator_width
    }

    /// Fit segments to a maximum width by dropping lowest-priority segments.
    ///
    /// Drops segments with the highest priority number (lowest importance) first.
    /// Returns a new Vec with segments that fit within the width budget.
    ///
    /// Algorithm: Start with all segments, repeatedly remove the lowest-priority
    /// segment until either it fits or only one segment remains. This guarantees
    /// that high-priority segments are never dropped in favor of low-priority ones.
    ///
    /// If even the highest-priority segment doesn't fit, returns it anyway
    /// (caller should use `truncate_visible` for final truncation).
    pub fn fit_to_width(segments: Vec<Self>, max_width: usize) -> Vec<Self> {
        if segments.is_empty() {
            return segments;
        }

        if Self::total_width(&segments) <= max_width {
            return segments;
        }

        // Track original indices to restore order after dropping
        let mut indexed: Vec<_> = segments.into_iter().enumerate().collect();

        // Repeatedly drop the lowest-priority (highest priority number) segment
        // until it fits or only one segment remains
        while indexed.len() > 1 && Self::total_width_indexed(&indexed) > max_width {
            // Find the index of the lowest-priority segment (highest priority number)
            // When tied, prefer dropping later segments to preserve order
            let drop_idx = indexed
                .iter()
                .enumerate()
                .max_by(|(i, (_, a)), (j, (_, b))| {
                    // Primary: higher priority number = lower priority = drop first
                    // Secondary: later position = drop first (stable)
                    a.priority.cmp(&b.priority).then_with(|| i.cmp(j))
                })
                .map(|(i, _)| i)
                .unwrap();
            indexed.remove(drop_idx);
        }

        // Restore original order
        indexed.sort_by_key(|(idx, _)| *idx);
        indexed.into_iter().map(|(_, seg)| seg).collect()
    }

    /// Calculate total width of indexed segments when joined with 2-space separators.
    fn total_width_indexed(segments: &[(usize, Self)]) -> usize {
        if segments.is_empty() {
            return 0;
        }
        let content_width: usize = segments.iter().map(|(_, s)| s.width()).sum();
        let separator_width = (segments.len() - 1) * 2;
        content_width + separator_width
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_statusline_segment_width() {
        let seg = StatuslineSegment::new("hello".to_string(), 1);
        assert_eq!(seg.width(), 5);

        // ANSI codes don't count toward width
        use color_print::cformat;
        let styled = StatuslineSegment::new(cformat!("<green>green</>"), 1);
        assert_eq!(styled.width(), 5);
    }

    #[test]
    fn test_statusline_segment_join() {
        let segments = vec![
            StatuslineSegment::new("a".to_string(), 1),
            StatuslineSegment::new("b".to_string(), 2),
            StatuslineSegment::new("c".to_string(), 3),
        ];
        assert_eq!(StatuslineSegment::join(&segments), "a  b  c");
    }

    #[test]
    fn test_statusline_segment_total_width() {
        let segments = vec![
            StatuslineSegment::new("abc".to_string(), 1), // 3 chars
            StatuslineSegment::new("de".to_string(), 2),  // 2 chars
        ];
        // 3 + 2 + 2 (separator) = 7
        assert_eq!(StatuslineSegment::total_width(&segments), 7);

        // Empty segments have 0 total width
        assert_eq!(StatuslineSegment::total_width(&[]), 0);

        // Single segment has no separator
        let single = vec![StatuslineSegment::new("test".to_string(), 1)];
        assert_eq!(StatuslineSegment::total_width(&single), 4);
    }

    #[test]
    fn test_statusline_segment_fit_to_width_no_truncation_needed() {
        let segments = vec![
            StatuslineSegment::new("abc".to_string(), 1),
            StatuslineSegment::new("de".to_string(), 2),
        ];
        // Total width is 7, budget is 10 - no change
        let result = StatuslineSegment::fit_to_width(segments.clone(), 10);
        assert_eq!(result.len(), 2);
        assert_eq!(StatuslineSegment::join(&result), "abc  de");
    }

    #[test]
    fn test_statusline_segment_fit_to_width_drops_low_priority() {
        let segments = vec![
            StatuslineSegment::new("important".to_string(), 1), // 9 chars, priority 1 (high)
            StatuslineSegment::new("optional".to_string(), 10), // 8 chars, priority 10 (low)
        ];
        // Total: 9 + 2 + 8 = 19, budget is 12
        // Should drop "optional" (priority 10) and keep "important" (priority 1)
        let result = StatuslineSegment::fit_to_width(segments, 12);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "important");
    }

    #[test]
    fn test_statusline_segment_fit_to_width_preserves_order() {
        let segments = vec![
            StatuslineSegment::new("A".to_string(), 5), // low priority, but first
            StatuslineSegment::new("B".to_string(), 1), // high priority, second
            StatuslineSegment::new("C".to_string(), 3), // medium priority, third
        ];
        // Budget: A(1) + sep(2) + B(1) + sep(2) + C(1) = 7
        // If we can fit all 3: should be "A  B  C"
        let result = StatuslineSegment::fit_to_width(segments, 10);
        assert_eq!(StatuslineSegment::join(&result), "A  B  C");
    }

    #[test]
    fn test_statusline_segment_fit_to_width_drops_multiple() {
        let segments = vec![
            StatuslineSegment::new("dir".to_string(), 0), // highest priority
            StatuslineSegment::new("branch".to_string(), 1), // high priority
            StatuslineSegment::new("status".to_string(), 2), // medium priority
            StatuslineSegment::new("url".to_string(), 8), // low priority
            StatuslineSegment::new("model".to_string(), 1), // high priority
        ];
        // Total: 3 + 2 + 6 + 2 + 6 + 2 + 3 + 2 + 5 = 31
        // Budget: 15 - need to drop low priority segments
        let result = StatuslineSegment::fit_to_width(segments, 15);

        // Should have dropped url (priority 8) at minimum
        let contents: Vec<_> = result.iter().map(|s| s.content.as_str()).collect();
        assert!(!contents.contains(&"url"), "Should have dropped url");
    }

    #[test]
    fn test_statusline_segment_from_column() {
        let seg = StatuslineSegment::from_column("test".to_string(), ColumnKind::Branch);
        assert_eq!(seg.content, "test");
        assert_eq!(seg.priority, ColumnKind::Branch.priority());
        assert_eq!(seg.kind, Some(ColumnKind::Branch));
    }

    #[test]
    fn test_statusline_segment_fit_to_width_keeps_highest_priority_when_too_wide() {
        // When even the highest-priority segment exceeds the budget,
        // it should still be kept (caller uses truncate_visible for final cut)
        let segments = vec![
            StatuslineSegment::new("very_long_directory_path".to_string(), 0), // 24 chars, highest priority
            StatuslineSegment::new("branch".to_string(), 1),                   // 6 chars
        ];
        // Budget is only 5 - even the smallest segment doesn't fit
        let result = StatuslineSegment::fit_to_width(segments, 5);
        assert_eq!(result.len(), 1, "Should keep at least one segment");
        assert_eq!(
            result[0].content, "very_long_directory_path",
            "Should keep highest-priority segment even if too wide"
        );
    }

    #[test]
    fn test_statusline_segment_fit_to_width_prefers_priority_over_width() {
        // A wide high-priority segment should be kept over narrow low-priority ones
        let segments = vec![
            StatuslineSegment::new("very_important_segment".to_string(), 0), // 22 chars, highest priority
            StatuslineSegment::new("x".to_string(), 10),                     // 1 char, low priority
            StatuslineSegment::new("y".to_string(), 10),                     // 1 char, low priority
        ];
        // Budget is 25 - only fits the important segment (22) or x+y (1+2+1=4), not both
        let result = StatuslineSegment::fit_to_width(segments, 25);
        assert!(
            result.iter().any(|s| s.content == "very_important_segment"),
            "Should keep the high-priority segment"
        );
    }
}
