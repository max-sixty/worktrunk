//! Git diff utilities for parsing and formatting diff statistics.

use color_print::cformat;

/// Line-level diff totals (added/deleted counts) used across git operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub struct LineDiff {
    pub added: usize,
    pub deleted: usize,
}

impl LineDiff {
    /// Parse `git diff --numstat` output into aggregated line totals.
    pub fn from_numstat(output: &str) -> anyhow::Result<Self> {
        let mut totals = LineDiff::default();

        for line in output.lines() {
            if line.trim().is_empty() {
                continue;
            }

            let mut parts = line.split('\t');
            let Some(added_str) = parts.next() else {
                continue;
            };
            let Some(deleted_str) = parts.next() else {
                continue;
            };

            // Binary files show "-" for added/deleted
            if added_str == "-" || deleted_str == "-" {
                continue;
            }

            let Ok(added) = added_str.parse::<usize>() else {
                continue;
            };
            let Ok(deleted) = deleted_str.parse::<usize>() else {
                continue;
            };

            totals.added += added;
            totals.deleted += deleted;
        }

        Ok(totals)
    }

    pub fn is_empty(&self) -> bool {
        self.added == 0 && self.deleted == 0
    }

    pub fn into_tuple(self) -> (usize, usize) {
        (self.added, self.deleted)
    }
}

impl From<LineDiff> for (usize, usize) {
    fn from(diff: LineDiff) -> Self {
        diff.into_tuple()
    }
}

impl From<(usize, usize)> for LineDiff {
    fn from(value: (usize, usize)) -> Self {
        Self {
            added: value.0,
            deleted: value.1,
        }
    }
}

/// Parse git diff --shortstat output
#[derive(Debug)]
pub struct DiffStats {
    pub files: Option<usize>,
    pub insertions: Option<usize>,
    pub deletions: Option<usize>,
}

impl DiffStats {
    /// Construct stats from `git diff --shortstat` output.
    pub fn from_shortstat(output: &str) -> Self {
        let mut stats = DiffStats {
            files: None,
            insertions: None,
            deletions: None,
        };

        // Example: " 3 files changed, 45 insertions(+), 12 deletions(-)"
        let parts: Vec<&str> = output.split(',').collect();

        for part in parts {
            let part = part.trim();

            if part.contains("file") {
                if let Some(num_str) = part.split_whitespace().next() {
                    stats.files = num_str.parse().ok();
                }
            } else if part.contains("insertion") {
                if let Some(num_str) = part.split_whitespace().next() {
                    stats.insertions = num_str.parse().ok();
                }
            } else if part.contains("deletion")
                && let Some(num_str) = part.split_whitespace().next()
            {
                stats.deletions = num_str.parse().ok();
            }
        }

        stats
    }

    /// Format stats as a summary string (e.g., "3 files, +45, -12")
    pub fn format_summary(&self) -> Vec<String> {
        let mut parts = Vec::new();

        if let Some(files) = self.files {
            parts.push(format!(
                "{} file{}",
                files,
                if files == 1 { "" } else { "s" }
            ));
        }
        if let Some(insertions) = self.insertions {
            parts.push(cformat!("<green>+{insertions}</>"));
        }
        if let Some(deletions) = self.deletions {
            parts.push(cformat!("<red>-{deletions}</>"));
        }

        parts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================================
    // LineDiff Tests
    // ============================================================================

    #[test]
    fn test_line_diff_default() {
        let diff = LineDiff::default();
        assert_eq!(diff.added, 0);
        assert_eq!(diff.deleted, 0);
    }

    #[test]
    fn test_line_diff_is_empty_true() {
        let diff = LineDiff {
            added: 0,
            deleted: 0,
        };
        assert!(diff.is_empty());
    }

    #[test]
    fn test_line_diff_is_empty_false_added() {
        let diff = LineDiff {
            added: 5,
            deleted: 0,
        };
        assert!(!diff.is_empty());
    }

    #[test]
    fn test_line_diff_is_empty_false_deleted() {
        let diff = LineDiff {
            added: 0,
            deleted: 5,
        };
        assert!(!diff.is_empty());
    }

    #[test]
    fn test_line_diff_into_tuple() {
        let diff = LineDiff {
            added: 10,
            deleted: 5,
        };
        assert_eq!(diff.into_tuple(), (10, 5));
    }

    #[test]
    fn test_line_diff_from_tuple() {
        let diff: LineDiff = (10, 5).into();
        assert_eq!(diff.added, 10);
        assert_eq!(diff.deleted, 5);
    }

    #[test]
    fn test_tuple_from_line_diff() {
        let diff = LineDiff {
            added: 10,
            deleted: 5,
        };
        let tuple: (usize, usize) = diff.into();
        assert_eq!(tuple, (10, 5));
    }

    #[test]
    fn test_line_diff_from_numstat_empty() {
        let result = LineDiff::from_numstat("").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_line_diff_from_numstat_single_file() {
        let output = "10\t5\tsrc/main.rs";
        let result = LineDiff::from_numstat(output).unwrap();
        assert_eq!(result.added, 10);
        assert_eq!(result.deleted, 5);
    }

    #[test]
    fn test_line_diff_from_numstat_multiple_files() {
        let output = "10\t5\tsrc/main.rs\n20\t3\tsrc/lib.rs\n1\t0\tCargo.toml";
        let result = LineDiff::from_numstat(output).unwrap();
        assert_eq!(result.added, 31); // 10 + 20 + 1
        assert_eq!(result.deleted, 8); // 5 + 3 + 0
    }

    #[test]
    fn test_line_diff_from_numstat_binary_file() {
        // Binary files show "-" for added/deleted
        let output = "-\t-\timage.png";
        let result = LineDiff::from_numstat(output).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_line_diff_from_numstat_mixed_binary_and_text() {
        let output = "10\t5\tsrc/main.rs\n-\t-\timage.png\n3\t2\tREADME.md";
        let result = LineDiff::from_numstat(output).unwrap();
        assert_eq!(result.added, 13); // 10 + 3, skips binary
        assert_eq!(result.deleted, 7); // 5 + 2, skips binary
    }

    #[test]
    fn test_line_diff_from_numstat_empty_lines() {
        let output = "\n10\t5\tsrc/main.rs\n\n";
        let result = LineDiff::from_numstat(output).unwrap();
        assert_eq!(result.added, 10);
        assert_eq!(result.deleted, 5);
    }

    #[test]
    fn test_line_diff_from_numstat_malformed_line_missing_deleted() {
        // Line with only added count (missing tab and deleted)
        let output = "10";
        let result = LineDiff::from_numstat(output).unwrap();
        assert!(result.is_empty()); // Should skip malformed line
    }

    #[test]
    fn test_line_diff_from_numstat_non_numeric_added() {
        let output = "abc\t5\tsrc/main.rs";
        let result = LineDiff::from_numstat(output).unwrap();
        assert!(result.is_empty()); // Should skip non-numeric
    }

    #[test]
    fn test_line_diff_from_numstat_non_numeric_deleted() {
        let output = "10\tabc\tsrc/main.rs";
        let result = LineDiff::from_numstat(output).unwrap();
        assert!(result.is_empty()); // Should skip non-numeric
    }

    // ============================================================================
    // DiffStats Tests
    // ============================================================================

    #[test]
    fn test_diff_stats_from_shortstat_empty() {
        let stats = DiffStats::from_shortstat("");
        assert!(stats.files.is_none());
        assert!(stats.insertions.is_none());
        assert!(stats.deletions.is_none());
    }

    #[test]
    fn test_diff_stats_from_shortstat_full() {
        let output = " 3 files changed, 45 insertions(+), 12 deletions(-)";
        let stats = DiffStats::from_shortstat(output);
        assert_eq!(stats.files, Some(3));
        assert_eq!(stats.insertions, Some(45));
        assert_eq!(stats.deletions, Some(12));
    }

    #[test]
    fn test_diff_stats_from_shortstat_single_file() {
        let output = " 1 file changed, 10 insertions(+)";
        let stats = DiffStats::from_shortstat(output);
        assert_eq!(stats.files, Some(1));
        assert_eq!(stats.insertions, Some(10));
        assert!(stats.deletions.is_none());
    }

    #[test]
    fn test_diff_stats_from_shortstat_only_deletions() {
        let output = " 2 files changed, 5 deletions(-)";
        let stats = DiffStats::from_shortstat(output);
        assert_eq!(stats.files, Some(2));
        assert!(stats.insertions.is_none());
        assert_eq!(stats.deletions, Some(5));
    }

    #[test]
    fn test_diff_stats_from_shortstat_no_changes() {
        // Output when comparing identical trees
        let output = "";
        let stats = DiffStats::from_shortstat(output);
        assert!(stats.files.is_none());
    }

    #[test]
    fn test_diff_stats_format_summary_empty() {
        let stats = DiffStats {
            files: None,
            insertions: None,
            deletions: None,
        };
        let summary = stats.format_summary();
        assert!(summary.is_empty());
    }

    #[test]
    fn test_diff_stats_format_summary_all_parts() {
        let stats = DiffStats {
            files: Some(3),
            insertions: Some(45),
            deletions: Some(12),
        };
        let summary = stats.format_summary();
        assert_eq!(summary.len(), 3);
        assert_eq!(summary[0], "3 files");
        assert!(summary[1].contains("45")); // Has color codes
        assert!(summary[2].contains("12")); // Has color codes
    }

    #[test]
    fn test_diff_stats_format_summary_single_file() {
        let stats = DiffStats {
            files: Some(1),
            insertions: Some(10),
            deletions: None,
        };
        let summary = stats.format_summary();
        assert_eq!(summary.len(), 2);
        assert_eq!(summary[0], "1 file"); // Singular
        assert!(summary[1].contains("10"));
    }

    #[test]
    fn test_diff_stats_format_summary_only_files() {
        let stats = DiffStats {
            files: Some(5),
            insertions: None,
            deletions: None,
        };
        let summary = stats.format_summary();
        assert_eq!(summary.len(), 1);
        assert_eq!(summary[0], "5 files");
    }

    #[test]
    fn test_diff_stats_format_summary_only_insertions() {
        let stats = DiffStats {
            files: None,
            insertions: Some(100),
            deletions: None,
        };
        let summary = stats.format_summary();
        assert_eq!(summary.len(), 1);
        assert!(summary[0].contains("100"));
    }

    #[test]
    fn test_diff_stats_format_summary_only_deletions() {
        let stats = DiffStats {
            files: None,
            insertions: None,
            deletions: Some(50),
        };
        let summary = stats.format_summary();
        assert_eq!(summary.len(), 1);
        assert!(summary[0].contains("50"));
    }
}
