//! Git diff utilities for parsing and formatting diff statistics.

use ansi_str::AnsiStr;
use color_print::cformat;

/// Line-level diff totals (added/deleted counts) used across git operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub struct LineDiff {
    pub added: usize,
    pub deleted: usize,
}

/// Parse a git numstat line and extract insertions/deletions.
///
/// Supports standard `git diff --numstat` output as well as log output with
/// `--graph --color=always` prefixes.
/// Returns `None` for binary entries (`-` counts).
pub fn parse_numstat_line(line: &str) -> Option<(usize, usize)> {
    // Strip ANSI escape sequences (graph coloring contains digits that confuse parsing).
    let stripped = line.ansi_strip();

    // Strip graph prefix (e.g., "| ") and find tab-separated values.
    let trimmed = stripped.trim_start_matches(|c: char| !c.is_ascii_digit() && c != '-');

    let mut parts = trimmed.split('\t');
    let added_str = parts.next()?;
    let deleted_str = parts.next()?;

    // "-" means binary file; line counts are unavailable, so skip.
    if added_str == "-" || deleted_str == "-" {
        return None;
    }

    let added = added_str.parse().ok()?;
    let deleted = deleted_str.parse().ok()?;

    Some((added, deleted))
}

impl LineDiff {
    /// Parse `git diff --numstat` output into aggregated line totals.
    pub fn from_numstat(output: &str) -> anyhow::Result<Self> {
        let mut totals = LineDiff::default();

        for line in output.lines() {
            if let Some((added, deleted)) = parse_numstat_line(line) {
                totals.added += added;
                totals.deleted += deleted;
            }
        }

        Ok(totals)
    }

    pub fn is_empty(&self) -> bool {
        self.added == 0 && self.deleted == 0
    }
}

impl From<LineDiff> for (usize, usize) {
    fn from(diff: LineDiff) -> Self {
        (diff.added, diff.deleted)
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

/// Diff statistics (files changed, insertions, deletions).
#[derive(Debug, Default)]
pub(crate) struct DiffStats {
    pub files: usize,
    pub insertions: usize,
    pub deletions: usize,
}

impl DiffStats {
    /// Construct stats from `git diff --numstat` output.
    pub fn from_numstat(output: &str) -> Self {
        let mut stats = Self::default();
        for line in output.lines() {
            if let Some((added, deleted)) = parse_numstat_line(line) {
                stats.files += 1;
                stats.insertions += added;
                stats.deletions += deleted;
            } else if !line.trim().is_empty() {
                // Binary file (shows as "-\t-\tfilename") - count file but not lines
                stats.files += 1;
            }
        }
        stats
    }

    /// Format stats as a summary string (e.g., "3 files, +45, -12").
    /// Zero values are omitted.
    pub fn format_summary(&self) -> Vec<String> {
        let mut parts = Vec::new();
        if self.files > 0 {
            let s = if self.files == 1 { "" } else { "s" };
            parts.push(format!("{} file{}", self.files, s));
        }
        if self.insertions > 0 {
            parts.push(cformat!("<green>+{}</>", self.insertions));
        }
        if self.deletions > 0 {
            parts.push(cformat!("<red>-{}</>", self.deletions));
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
    // parse_numstat_line Tests
    // ============================================================================

    #[test]
    fn test_parse_numstat_line_basic() {
        // Tab-separated: added<TAB>deleted<TAB>filename
        let result = parse_numstat_line("10\t5\tfile.rs");
        assert_eq!(result, Some((10, 5)));
    }

    #[test]
    fn test_parse_numstat_line_insertions_only() {
        let result = parse_numstat_line("15\t0\tfile.rs");
        assert_eq!(result, Some((15, 0)));
    }

    #[test]
    fn test_parse_numstat_line_deletions_only() {
        let result = parse_numstat_line("0\t8\tfile.rs");
        assert_eq!(result, Some((0, 8)));
    }

    #[test]
    fn test_parse_numstat_line_binary_file() {
        // Binary files show "-" instead of numbers
        let result = parse_numstat_line("-\t-\timage.png");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_numstat_line_with_graph_prefix() {
        // Git graph prefixes the numstat line with graph characters
        let result = parse_numstat_line("| 10\t5\tfile.rs");
        assert_eq!(result, Some((10, 5)));

        // First numstat line after commit has "* | " prefix
        let result = parse_numstat_line("* | 11\t0\tCargo.toml");
        assert_eq!(result, Some((11, 0)));

        // Subsequent numstat lines have "| " prefix
        let result = parse_numstat_line("| 17\t3\tsrc/main.rs");
        assert_eq!(result, Some((17, 3)));

        // With ANSI colors (--color=always adds escape codes to graph)
        // ESC[31m = red, ESC[m = reset
        let esc = '\x1b';
        let ansi_colored = format!("{esc}[31m|{esc}[m 11\t0\tCargo.toml");
        let result = parse_numstat_line(&ansi_colored);
        assert_eq!(result, Some((11, 0)));
    }

    #[test]
    fn test_parse_numstat_line_not_numstat() {
        // Not a numstat line
        assert_eq!(parse_numstat_line("* abc1234 Fix bug"), None);
        assert_eq!(parse_numstat_line(""), None);
        assert_eq!(parse_numstat_line("regular text"), None);
    }

    // ============================================================================
    // DiffStats Tests
    // ============================================================================

    #[test]
    fn test_diff_stats_format_summary_empty() {
        let stats = DiffStats::default();
        assert!(stats.format_summary().is_empty());
    }

    #[test]
    fn test_diff_stats_format_summary_all_parts() {
        let stats = DiffStats {
            files: 3,
            insertions: 45,
            deletions: 12,
        };
        let summary = stats.format_summary();
        assert_eq!(summary.len(), 3);
        assert_eq!(summary[0], "3 files");
        assert!(summary[1].contains("45"));
        assert!(summary[2].contains("12"));
    }

    #[test]
    fn test_diff_stats_format_summary_single_file() {
        let stats = DiffStats {
            files: 1,
            insertions: 10,
            deletions: 0,
        };
        let summary = stats.format_summary();
        assert_eq!(summary.len(), 2);
        assert_eq!(summary[0], "1 file");
        assert!(summary[1].contains("10"));
    }

    // ============================================================================
    // DiffStats::from_numstat Tests
    // ============================================================================

    #[test]
    fn test_diff_stats_from_numstat_empty() {
        let stats = DiffStats::from_numstat("");
        assert_eq!(stats.files, 0);
        assert_eq!(stats.insertions, 0);
        assert_eq!(stats.deletions, 0);
    }

    #[test]
    fn test_diff_stats_from_numstat_single_file() {
        let stats = DiffStats::from_numstat("45\t12\tpath/to/file.rs");
        assert_eq!(stats.files, 1);
        assert_eq!(stats.insertions, 45);
        assert_eq!(stats.deletions, 12);
    }

    #[test]
    fn test_diff_stats_from_numstat_multiple_files() {
        let output = "10\t5\tsrc/main.rs\n20\t3\tsrc/lib.rs\n15\t4\ttests/test.rs";
        let stats = DiffStats::from_numstat(output);
        assert_eq!(stats.files, 3);
        assert_eq!(stats.insertions, 45);
        assert_eq!(stats.deletions, 12);
    }

    #[test]
    fn test_diff_stats_from_numstat_binary_file() {
        // Binary files show as "-" for both counts - file counted, no line stats
        let stats = DiffStats::from_numstat("-\t-\timage.png");
        assert_eq!(stats.files, 1);
        assert_eq!(stats.insertions, 0);
        assert_eq!(stats.deletions, 0);
    }

    #[test]
    fn test_diff_stats_from_numstat_mixed_binary_and_text() {
        let output = "10\t5\tsrc/main.rs\n-\t-\tassets/logo.png\n20\t0\tsrc/lib.rs";
        let stats = DiffStats::from_numstat(output);
        assert_eq!(stats.files, 3);
        assert_eq!(stats.insertions, 30);
        assert_eq!(stats.deletions, 5);
    }
}
