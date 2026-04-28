//! Git diff utilities for parsing and formatting diff statistics.

use ansi_str::AnsiStr;
use color_print::cformat;

/// Line-level diff totals (added/deleted counts) used across git operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
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
    /// Parse `git diff --shortstat` output into line totals.
    ///
    /// Shortstat produces a single line like:
    ///   ` 3 files changed, 45 insertions(+), 12 deletions(-)`
    /// with optional parts omitted when zero. Extracts numbers by position
    /// relative to the `(+)` and `(-)` markers, which are locale-independent.
    pub fn from_shortstat(output: &str) -> Self {
        parse_shortstat(output).map_or(Self::default(), |(_, ins, del)| Self {
            added: ins,
            deleted: del,
        })
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

/// Parse `git diff --shortstat` output into (files, insertions, deletions).
///
/// The format is: ` N file(s) changed, N insertion(s)(+), N deletion(s)(-)`
/// with optional parts omitted when zero. The `(+)` and `(-)` markers are
/// hardcoded in git's C source (`diff.c`) and not subject to localization.
fn parse_shortstat(output: &str) -> Option<(usize, usize, usize)> {
    let line = output.trim();
    if line.is_empty() {
        return None;
    }

    let mut files = 0;
    let mut insertions = 0;
    let mut deletions = 0;

    // Split on commas: "N file(s) changed", "N insertion(s)(+)", "N deletion(s)(-)"
    for (i, part) in line.split(',').enumerate() {
        let num = part
            .split_whitespace()
            .find_map(|w| w.parse::<usize>().ok())
            .unwrap_or(0);

        if i == 0 {
            files = num;
        } else if part.contains("(+)") {
            insertions = num;
        } else if part.contains("(-)") {
            deletions = num;
        }
    }

    Some((files, insertions, deletions))
}

impl DiffStats {
    /// Construct stats from `git diff --shortstat` output.
    pub fn from_shortstat(output: &str) -> Self {
        parse_shortstat(output).map_or(Self::default(), |(files, ins, del)| Self {
            files,
            insertions: ins,
            deletions: del,
        })
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
    use insta::assert_snapshot;

    use super::*;

    // ============================================================================
    // LineDiff Tests
    // ============================================================================

    #[test]
    fn test_line_diff_is_empty() {
        assert!(LineDiff::default().is_empty());
        assert!(
            LineDiff {
                added: 0,
                deleted: 0
            }
            .is_empty()
        );
        assert!(
            !LineDiff {
                added: 5,
                deleted: 0
            }
            .is_empty()
        );
        assert!(
            !LineDiff {
                added: 0,
                deleted: 5
            }
            .is_empty()
        );
    }

    #[test]
    fn test_line_diff_tuple_roundtrip() {
        let diff: LineDiff = (10, 5).into();
        assert_eq!(diff.added, 10);
        assert_eq!(diff.deleted, 5);
        let tuple: (usize, usize) = diff.into();
        assert_eq!(tuple, (10, 5));
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
        assert_snapshot!(stats.format_summary().join(", "), @"3 files, [32m+45[39m, [31m-12[39m");
    }

    #[test]
    fn test_diff_stats_format_summary_single_file() {
        let stats = DiffStats {
            files: 1,
            insertions: 10,
            deletions: 0,
        };
        assert_snapshot!(stats.format_summary().join(", "), @"1 file, [32m+10[39m");
    }

    // ============================================================================
    // parse_shortstat / from_shortstat Tests
    // ============================================================================

    #[test]
    fn test_parse_shortstat_all_parts() {
        let output = " 23 files changed, 624 insertions(+), 160 deletions(-)";
        let (files, ins, del) = parse_shortstat(output).unwrap();
        assert_eq!(files, 23);
        assert_eq!(ins, 624);
        assert_eq!(del, 160);
    }

    #[test]
    fn test_parse_shortstat_insertions_only() {
        let output = " 1 file changed, 6 insertions(+)";
        let (files, ins, del) = parse_shortstat(output).unwrap();
        assert_eq!(files, 1);
        assert_eq!(ins, 6);
        assert_eq!(del, 0);
    }

    #[test]
    fn test_parse_shortstat_deletions_only() {
        let output = " 2 files changed, 10 deletions(-)";
        let (files, ins, del) = parse_shortstat(output).unwrap();
        assert_eq!(files, 2);
        assert_eq!(ins, 0);
        assert_eq!(del, 10);
    }

    #[test]
    fn test_parse_shortstat_empty() {
        assert_eq!(parse_shortstat(""), None);
        assert_eq!(parse_shortstat("  "), None);
        assert_eq!(parse_shortstat("\n"), None);
    }

    #[test]
    fn test_parse_shortstat_single_file_singular() {
        let output = " 1 file changed, 1 insertion(+), 1 deletion(-)";
        let (files, ins, del) = parse_shortstat(output).unwrap();
        assert_eq!(files, 1);
        assert_eq!(ins, 1);
        assert_eq!(del, 1);
    }

    #[test]
    fn test_line_diff_from_shortstat() {
        let output = " 5 files changed, 100 insertions(+), 50 deletions(-)";
        let diff = LineDiff::from_shortstat(output);
        assert_eq!(diff.added, 100);
        assert_eq!(diff.deleted, 50);
    }

    #[test]
    fn test_line_diff_from_shortstat_empty() {
        let diff = LineDiff::from_shortstat("");
        assert!(diff.is_empty());
    }

    #[test]
    fn test_diff_stats_from_shortstat() {
        let output = " 3 files changed, 45 insertions(+), 12 deletions(-)";
        let stats = DiffStats::from_shortstat(output);
        assert_eq!(stats.files, 3);
        assert_eq!(stats.insertions, 45);
        assert_eq!(stats.deletions, 12);
    }

    #[test]
    fn test_diff_stats_from_shortstat_empty() {
        let stats = DiffStats::from_shortstat("");
        assert_eq!(stats.files, 0);
        assert_eq!(stats.insertions, 0);
        assert_eq!(stats.deletions, 0);
    }
}
