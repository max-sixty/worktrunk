//! Git log output formatting.
//!
//! Functions for processing and formatting git log output with diffstats and dimming.

use std::collections::{HashMap, HashSet};

use worktrunk::git::{Repository, parse_numstat_line};
use worktrunk::shell_exec::Cmd;

use super::super::list::layout::{DiffDisplayConfig, DiffVariant};

/// Field delimiter for git log format with timestamps
pub(super) const FIELD_DELIM: char = '\x1f';

/// Start delimiter for full hash (SOH - Start of Heading)
pub(super) const HASH_START: char = '\x01';

/// End delimiter for full hash (NUL)
pub(super) const HASH_END: char = '\x00';

/// Timestamp column width ("12mo" is the longest)
pub(super) const TIMESTAMP_WIDTH: usize = 4;

/// Batch fetch diffstats for multiple commits using git diff-tree --stdin.
/// Returns a map of full_hash -> (insertions, deletions).
///
/// Failures are silent (preview context).
pub(super) fn batch_fetch_stats(
    repo: &Repository,
    hashes: &[String],
) -> HashMap<String, (usize, usize)> {
    if hashes.is_empty() {
        return HashMap::new();
    }

    // --root: include stats for root commits (no parent to diff against)
    // Each hash needs a trailing newline for git to process it
    let stdin_data = hashes.iter().map(|h| format!("{h}\n")).collect::<String>();
    let Ok(output) = Cmd::new("git")
        .args(["diff-tree", "--numstat", "-r", "--root", "--stdin"])
        .current_dir(repo.repo_path().unwrap_or_else(|_| ".".into()))
        .stdin_bytes(stdin_data)
        .run()
    else {
        return HashMap::new();
    };

    // Parse output: hash line followed by numstat lines
    let mut stats: HashMap<String, (usize, usize)> = HashMap::new();
    let mut current_hash: Option<String> = None;
    let mut current_stats = (0usize, 0usize);

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        // Hash line (40 or 64 hex chars)
        if line.chars().all(|c| c.is_ascii_hexdigit()) && (line.len() == 40 || line.len() == 64) {
            // Save previous hash's stats
            if let Some(hash) = current_hash.take() {
                stats.insert(hash, current_stats);
            }
            current_hash = Some(line.to_string());
            current_stats = (0, 0);
        } else if let Some((ins, del)) = parse_numstat_line(line) {
            current_stats.0 += ins;
            current_stats.1 += del;
        }
    }

    // Don't forget the last hash
    if let Some(hash) = current_hash {
        stats.insert(hash, current_stats);
    }

    stats
}

/// Process git log output: strip hash prefix and dim non-unique commits.
///
/// - `unique_commits = None`: show everything bright (default branch)
/// - `unique_commits = Some(set)`: bright if in set, dim otherwise
/// - Graph-only lines pass through unchanged
///
/// Returns (processed_output, list_of_full_hashes) for batch stats lookup.
pub(super) fn process_log_with_dimming(
    log_output: &str,
    unique_commits: Option<&HashSet<String>>,
) -> (String, Vec<String>) {
    use ansi_str::AnsiStr;
    use std::fmt::Write;

    let dim = anstyle::Style::new().dimmed();
    let reset = anstyle::Reset;

    let mut result = String::with_capacity(log_output.len());
    let mut hashes = Vec::new();

    for (i, line) in log_output.lines().enumerate() {
        if i > 0 {
            result.push('\n');
        }

        // Parse commit line: graph_prefix + SOH + full_hash + NUL + display
        if let Some(hash_start) = line.find(HASH_START)
            && let Some(hash_end_offset) = line[hash_start + 1..].find(HASH_END)
        {
            let hash_end = hash_start + 1 + hash_end_offset;
            let graph_prefix = &line[..hash_start];
            let full_hash = &line[hash_start + 1..hash_end];
            let display = &line[hash_end + 1..];

            // Collect hash for stats lookup
            hashes.push(full_hash.to_string());

            // Bright if: no dimming (None) OR commit is in unique set
            let is_bright = match unique_commits {
                None => true,                         // Default branch: all bright
                Some(set) => set.contains(full_hash), // Feature branch: bright if unique
            };

            // Keep SOH hash NUL markers for format_log_output to extract hash for stats lookup
            if is_bright {
                result.push_str(graph_prefix);
                result.push(HASH_START);
                result.push_str(full_hash);
                result.push(HASH_END);
                result.push_str(display);
            } else {
                // Dim: strip colors and wrap in dim style, but keep hash markers
                let _ = write!(
                    result,
                    "{}{HASH_START}{full_hash}{HASH_END}{dim}{}{reset}",
                    graph_prefix,
                    display.ansi_strip()
                );
            }
            continue;
        }
        // Graph-only lines: pass through unchanged
        result.push_str(line);
    }
    (result, hashes)
}

/// Format git log output with timestamps and diffstats.
///
/// Takes pre-processed log output (graph + commits) and a stats map.
/// Each commit line has format: `graph_prefix short_hash \x1f timestamp \x1f decoration message`
///
/// The full hash for stats lookup is embedded as: `SOH full_hash NUL` before the short hash,
/// but this was already stripped by process_log_with_dimming. We need the hash in the line
/// to look up stats - so we keep the full hash in a different delimiter for this function.
pub(super) fn format_log_output(
    log_output: &str,
    stats: &HashMap<String, (usize, usize)>,
) -> String {
    use crate::display::format_relative_time_short;
    format_log_output_with_formatter(log_output, stats, format_relative_time_short)
}

/// Format git log output with a custom time formatter.
///
/// This variant allows dependency injection for testing with deterministic timestamps.
pub(super) fn format_log_output_with_formatter<F>(
    log_output: &str,
    stats: &HashMap<String, (usize, usize)>,
    format_time: F,
) -> String
where
    F: Fn(i64) -> String,
{
    use ansi_str::AnsiStr;
    use unicode_width::UnicodeWidthStr;

    // First pass: find max display width of graph+hash prefix for alignment
    let max_prefix_width = log_output
        .lines()
        .filter(|line| line.contains(FIELD_DELIM))
        .filter_map(|line| {
            let first_delim = line.find(FIELD_DELIM)?;
            let graph_hash_raw = &line[..first_delim];
            let graph_hash = strip_hash_markers(graph_hash_raw);
            // Calculate display width (strip ANSI, measure unicode width)
            Some(graph_hash.ansi_strip().width())
        })
        .max()
        .unwrap_or(0);

    // Second pass: format with alignment
    let mut result = Vec::new();
    for line in log_output.lines() {
        if line.contains(FIELD_DELIM) {
            // Commit line - look up stats by hash extracted from line
            let commit_stats = extract_hash_from_line(line)
                .and_then(|h| stats.get(h))
                .copied()
                .unwrap_or((0, 0));
            result.push(format_commit_line(
                line,
                commit_stats,
                max_prefix_width,
                &format_time,
            ));
        } else {
            // Graph-only line - pass through
            result.push(line.to_string());
        }
    }

    result.join("\n")
}

/// Extract the full hash from a commit line that still has SOH/NUL markers.
/// Returns None if not found (line already processed or malformed).
pub(super) fn extract_hash_from_line(line: &str) -> Option<&str> {
    let hash_start = line.find(HASH_START)?;
    let hash_end_offset = line[hash_start + 1..].find(HASH_END)?;
    Some(&line[hash_start + 1..hash_start + 1 + hash_end_offset])
}

/// Strip SOH...NUL hash markers from output (used when not formatting with timestamps).
pub(super) fn strip_hash_markers(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == HASH_START {
            // Skip until NUL
            while let Some(&next) = chars.peek() {
                chars.next();
                if next == HASH_END {
                    break;
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}

/// Format a single commit line with stats, padding the prefix to target_width for alignment
pub(super) fn format_commit_line<F>(
    commit_line: &str,
    (insertions, deletions): (usize, usize),
    target_width: usize,
    format_time: &F,
) -> String
where
    F: Fn(i64) -> String,
{
    use ansi_str::AnsiStr;
    use unicode_width::UnicodeWidthStr;
    use worktrunk::styling::{ADDITION, DELETION};

    let dim_style = anstyle::Style::new().dimmed();
    let reset = anstyle::Reset;

    if let Some(first_delim) = commit_line.find(FIELD_DELIM)
        && let Some(second_delim) = commit_line[first_delim + 1..].find(FIELD_DELIM)
    {
        let graph_hash_raw = &commit_line[..first_delim];
        // Strip SOH...NUL hash markers from graph_hash portion
        let graph_hash = strip_hash_markers(graph_hash_raw);
        let timestamp_str = &commit_line[first_delim + 1..first_delim + 1 + second_delim];
        let rest = &commit_line[first_delim + 1 + second_delim + 1..];

        let time = timestamp_str
            .parse::<i64>()
            .map(format_time)
            .unwrap_or_default();

        // Use the same diff formatting as wt list (aligned columns)
        let diff_config = DiffDisplayConfig {
            variant: DiffVariant::Signs,
            positive_style: ADDITION,
            negative_style: DELETION,
            always_show_zeros: false,
        };
        let stat_str = format!(" {}", diff_config.format_aligned(insertions, deletions));

        // Pad graph_hash to target_width for column alignment
        let current_width = graph_hash.ansi_strip().width();
        let padding = " ".repeat(target_width.saturating_sub(current_width));

        format!(
            "{}{}{} {dim_style}{:>width$}{reset}{}",
            graph_hash,
            padding,
            stat_str,
            time,
            rest,
            width = TIMESTAMP_WIDTH
        )
    } else {
        commit_line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // format_log_output tests use dependency injection for deterministic time formatting.
    // The format_log_output_with_formatter function accepts a time formatter closure.

    /// Fixed time formatter for deterministic tests
    fn fixed_time_formatter(_timestamp: i64) -> String {
        "1h".to_string() // Return a fixed time for all timestamps
    }

    /// Create a stats map with a single entry
    fn stats_for(
        hash: &str,
        insertions: usize,
        deletions: usize,
    ) -> HashMap<String, (usize, usize)> {
        let mut map = HashMap::new();
        map.insert(hash.to_string(), (insertions, deletions));
        map
    }

    /// Create a stats map with multiple entries
    fn multi_stats(entries: &[(&str, usize, usize)]) -> HashMap<String, (usize, usize)> {
        entries
            .iter()
            .map(|(h, i, d)| (h.to_string(), (*i, *d)))
            .collect()
    }

    #[test]
    fn test_format_log_output_single_commit() {
        // Simulate git log output with SOH/NUL markers: * SOH full_hash NUL short_hash \x1f timestamp \x1f message
        let full_hash = "abc1234567890123456789012345678901234567ab";
        let input = format!("* \x01{}\x00abc1234\x1f1699999000\x1f Fix bug", full_hash);
        let stats = stats_for(full_hash, 5, 2);
        let output = format_log_output_with_formatter(&input, &stats, fixed_time_formatter);

        // Should contain the hash and message
        assert!(output.contains("abc1234"));
        assert!(output.contains("Fix bug"));
        // Should contain formatted time
        assert!(output.contains("1h"));
        // Should contain stats
        assert!(output.contains("+5"), "expected +5 in output: {}", output);
    }

    #[test]
    fn test_format_log_output_with_stats() {
        // Commit line with pre-computed stats
        let full_hash = "abc1234567890123456789012345678901234567ab";
        let input = format!(
            "* \x01{}\x00abc1234\x1f1699999000\x1f Add feature",
            full_hash
        );
        // Stats are pre-computed (accumulated from numstat lines)
        let stats = stats_for(full_hash, 13, 5);
        let output = format_log_output_with_formatter(&input, &stats, fixed_time_formatter);

        // Should contain the hash and message
        assert!(output.contains("abc1234"));
        assert!(output.contains("Add feature"));
        // Verify stats are present (green +13, red -5)
        assert!(output.contains("+13"), "expected +13 in output: {}", output);
        assert!(output.contains("-5"), "expected -5 in output: {}", output);
    }

    #[test]
    fn test_format_log_output_multiple_commits() {
        // Two commits with pre-computed stats
        let hash1 = "abc1234567890123456789012345678901234567ab";
        let hash2 = "def5678901234567890123456789012345678901cd";
        let input = format!(
            "* \x01{}\x00abc1234\x1f1699999000\x1f First commit\n\
             * \x01{}\x00def5678\x1f1699998000\x1f Second commit",
            hash1, hash2
        );
        let stats = multi_stats(&[(hash1, 5, 2), (hash2, 10, 3)]);
        let output = format_log_output_with_formatter(&input, &stats, fixed_time_formatter);

        // Both commits should be in output
        assert!(output.contains("abc1234"));
        assert!(output.contains("def5678"));
        assert!(output.contains("First commit"));
        assert!(output.contains("Second commit"));

        // Output should be two lines (one per commit)
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 2, "Expected 2 lines, got: {:?}", lines);
    }

    #[test]
    fn test_format_log_output_empty_input() {
        let stats = HashMap::new();
        let output = format_log_output_with_formatter("", &stats, fixed_time_formatter);
        assert!(output.is_empty());
    }

    #[test]
    fn test_format_log_output_preserves_graph_lines() {
        // Merge commit with graph continuation line between commits
        let hash1 = "abc1234567890123456789012345678901234567ab";
        let hash2 = "def5678901234567890123456789012345678901cd";
        let input = format!(
            "*   \x01{}\x00abc1234\x1f1699999000\x1f Merge branch\n\
             |\\  \n\
             | * \x01{}\x00def5678\x1f1699998000\x1f Feature commit",
            hash1, hash2
        );
        let stats = multi_stats(&[(hash1, 0, 0), (hash2, 5, 2)]);
        let output = format_log_output_with_formatter(&input, &stats, fixed_time_formatter);

        // Graph line should be preserved between commits
        assert!(output.contains("|\\"), "graph line should be preserved");
        assert!(output.contains("abc1234"), "first commit should be present");
        assert!(
            output.contains("def5678"),
            "second commit should be present"
        );

        // Verify order: merge commit, graph line, feature commit
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 3, "Expected 3 lines: {:?}", lines);
        assert!(lines[0].contains("abc1234"));
        assert!(lines[1].contains("\\"));
        assert!(lines[2].contains("def5678"));
    }

    #[test]
    fn test_format_log_output_no_stats() {
        // Commit without stats (not in stats map)
        let full_hash = "abc1234567890123456789012345678901234567ab";
        let input = format!(
            "* \x01{}\x00abc1234\x1f1699999000\x1f Just a commit",
            full_hash
        );
        let stats = HashMap::new(); // Empty stats - shows no diff
        let output = format_log_output_with_formatter(&input, &stats, fixed_time_formatter);

        assert!(output.contains("abc1234"));
        assert!(output.contains("Just a commit"));
    }

    #[test]
    fn test_format_log_output_with_graph_prefix() {
        // Git graph output includes graph characters
        let full_hash = "abc1234567890123456789012345678901234567ab";
        let input = format!(
            "* \x01{}\x00abc1234\x1f1699999000\x1f Commit with graph",
            full_hash
        );
        let stats = stats_for(full_hash, 5, 2);
        let output = format_log_output_with_formatter(&input, &stats, fixed_time_formatter);

        assert!(output.contains("abc1234"));
        assert!(output.contains("Commit with graph"));
        // Verify stats are present
        assert!(output.contains("+5"), "expected +5 in output: {}", output);
        assert!(output.contains("-2"), "expected -2 in output: {}", output);
    }

    #[test]
    fn test_format_log_output_zero_stats() {
        // Commit with zero stats (e.g., binary-only changes)
        let full_hash = "abc1234567890123456789012345678901234567ab";
        let input = format!("* \x01{}\x00abc1234\x1f1699999000\x1f Add image", full_hash);
        let stats = stats_for(full_hash, 0, 0);
        let output = format_log_output_with_formatter(&input, &stats, fixed_time_formatter);

        assert!(output.contains("abc1234"));
        assert!(output.contains("Add image"));
    }

    #[test]
    fn test_format_log_output_malformed_commit_line() {
        // Line without proper field delimiters passes through
        let input = "abc1234 regular commit line";
        let stats = HashMap::new();
        let output = format_log_output_with_formatter(input, &stats, fixed_time_formatter);

        // Lines without \x1f delimiter pass through unchanged
        assert!(output.contains("abc1234"));
    }

    #[test]
    fn test_format_log_output_commit_line_missing_second_delimiter() {
        // Only one delimiter - malformed
        let input = "abc1234\x1f1699999000 Fix bug";
        let stats = HashMap::new();
        let output = format_log_output_with_formatter(input, &stats, fixed_time_formatter);

        // Should output the line as-is since it's malformed (only one \x1f)
        assert!(output.contains("abc1234"));
    }

    #[test]
    fn test_format_log_output_stats_only_deletions() {
        // Commit with only deletions (no insertions)
        let full_hash = "abc1234567890123456789012345678901234567ab";
        let input = format!(
            "* \x01{}\x00abc1234\x1f1699999000\x1f Remove old code",
            full_hash
        );
        let stats = stats_for(full_hash, 0, 50);
        let output = format_log_output_with_formatter(&input, &stats, fixed_time_formatter);

        assert!(output.contains("abc1234"));
        assert!(output.contains("Remove old code"));
        // Should show deletions
        assert!(output.contains("-50"), "expected -50 in output: {}", output);
    }

    #[test]
    fn test_format_log_output_large_stats() {
        // Commit with large stats (tests K notation)
        let full_hash = "abc1234567890123456789012345678901234567ab";
        let input = format!(
            "* \x01{}\x00abc1234\x1f1699999000\x1f Big refactor",
            full_hash
        );
        let stats = stats_for(full_hash, 1500, 800);
        let output = format_log_output_with_formatter(&input, &stats, fixed_time_formatter);

        assert!(output.contains("abc1234"));
        // Large numbers should use K notation
        assert!(
            output.contains("+1K") || output.contains("+1.5K"),
            "expected K notation in output: {}",
            output
        );
    }

    #[test]
    fn test_format_commit_line_directly() {
        // Test the format_commit_line function directly
        let commit_line = "abc1234\x1f1699999000\x1f Test commit";
        let stats = (10, 5);
        let target_width = 7; // "abc1234" is 7 chars, no padding needed
        let output = format_commit_line(commit_line, stats, target_width, &fixed_time_formatter);

        assert!(output.contains("abc1234"));
        assert!(output.contains("Test commit"));
        assert!(output.contains("+10"), "expected +10 in output: {}", output);
        assert!(output.contains("-5"), "expected -5 in output: {}", output);
        assert!(output.contains("1h"), "expected time in output: {}", output);
    }

    #[test]
    fn test_format_commit_line_with_padding() {
        // Test that padding aligns shorter hashes to target width
        let commit_line = "abc12\x1f1699999000\x1f Short hash";
        let stats = (5, 2);
        let target_width = 9; // Pad "abc12" (5 chars) to 9 chars
        let output = format_commit_line(commit_line, stats, target_width, &fixed_time_formatter);

        // Should have 4 spaces of padding after hash before stats
        assert!(output.contains("abc12    "), "expected padding: {}", output);
    }

    // Tests for process_log_with_dimming
    //
    // Input format: graph_prefix + SOH (\x01) + full_hash + NUL (\x00) + display
    // Example: "* \x01abc123...def456\x00abc1234 (HEAD) message"

    /// Parse output to determine which lines are dimmed vs bright.
    /// Returns (is_dimmed, content) for each line.
    fn parse_dimming_output(output: &str) -> Vec<(bool, String)> {
        use ansi_str::AnsiStr;
        output
            .lines()
            .map(|line| {
                // Check if line contains dim escape sequence (\x1b[2m)
                let is_dimmed = line.contains("\x1b[2m");
                let content = line.ansi_strip().to_string();
                (is_dimmed, content)
            })
            .collect()
    }

    #[test]
    fn test_process_log_with_dimming_parses_commit_line() {
        // Simulates git log output with SOH/NUL delimiters around full hash
        let hash = "abc123456789012345678901234567890123456789";
        let input = format!("* \x01{}\x00abc1234 (HEAD) Fix bug", hash);

        let unique = HashSet::from([hash.to_string()]);
        let (output, hashes) = process_log_with_dimming(&input, Some(&unique));

        // SOH/NUL markers are preserved for format_log_output to extract hashes
        assert!(
            output.contains('\x01'),
            "SOH should be preserved for format_log_output"
        );
        assert!(
            output.contains('\x00'),
            "NUL should be preserved for format_log_output"
        );
        assert!(output.contains("abc1234"), "short hash preserved");
        assert!(output.contains("Fix bug"), "message preserved");

        // Hashes should be collected for batch stats lookup
        assert_eq!(hashes.len(), 1);
        assert_eq!(hashes[0], hash);

        // Should be bright (in unique set)
        let parsed = parse_dimming_output(&output);
        assert_eq!(parsed.len(), 1);
        assert!(!parsed[0].0, "commit in unique set should be bright");
    }

    #[test]
    fn test_process_log_with_dimming_dims_non_unique() {
        let unique_hash = "abc123456789012345678901234567890123456789";
        let other_hash = "def123456789012345678901234567890123456789";

        let input = format!(
            "* \x01{}\x00abc1234 Unique commit\n\
             * \x01{}\x00def1234 Not unique",
            unique_hash, other_hash
        );

        let unique = HashSet::from([unique_hash.to_string()]);
        let (output, hashes) = process_log_with_dimming(&input, Some(&unique));

        // Both hashes should be collected
        assert_eq!(hashes.len(), 2);

        let parsed = parse_dimming_output(&output);
        assert_eq!(parsed.len(), 2);

        // First commit (unique) should be bright
        assert!(!parsed[0].0, "unique commit should be bright");
        assert!(parsed[0].1.contains("Unique commit"));

        // Second commit (not unique) should be dimmed
        assert!(parsed[1].0, "non-unique commit should be dimmed");
        assert!(parsed[1].1.contains("Not unique"));
    }

    #[test]
    fn test_process_log_with_dimming_none_means_all_bright() {
        // None = default branch, show everything bright
        let hash = "abc123456789012345678901234567890123456789";
        let input = format!("* \x01{}\x00abc1234 Some commit", hash);

        let (output, hashes) = process_log_with_dimming(&input, None);

        assert_eq!(hashes.len(), 1);
        let parsed = parse_dimming_output(&output);
        assert_eq!(parsed.len(), 1);
        assert!(!parsed[0].0, "None means default branch, all bright");
    }

    #[test]
    fn test_process_log_with_dimming_empty_set_means_all_dim() {
        // Some(empty) = feature branch with no unique commits, dim everything
        let hash = "abc123456789012345678901234567890123456789";
        let input = format!("* \x01{}\x00abc1234 Some commit", hash);

        let empty: HashSet<String> = HashSet::new();
        let (output, hashes) = process_log_with_dimming(&input, Some(&empty));

        assert_eq!(hashes.len(), 1);
        let parsed = parse_dimming_output(&output);
        assert_eq!(parsed.len(), 1);
        assert!(
            parsed[0].0,
            "Some(empty) means feature branch with no unique commits, all dim"
        );
    }

    #[test]
    fn test_process_log_with_dimming_preserves_graph_lines() {
        let hash = "abc123456789012345678901234567890123456789";
        // Git graph can have continuation lines between commits
        let input = format!(
            "* \x01{}\x00abc1234 First\n\
             |\n\
             * \x01{}\x00def1234 Second",
            hash, "def123456789012345678901234567890123456789"
        );

        let unique = HashSet::from([hash.to_string()]);
        let (output, _hashes) = process_log_with_dimming(&input, Some(&unique));

        // Graph-only line should be preserved unchanged
        assert!(output.contains("\n|\n"), "graph line should be preserved");
    }

    #[test]
    fn test_process_log_with_dimming_sha256_compatible() {
        // SHA-256 hashes are 64 characters (not 40)
        let sha256_hash = "abc1234567890123456789012345678901234567890123456789012345678901";
        assert_eq!(sha256_hash.len(), 64);

        let input = format!("* \x01{}\x00abc1234 SHA-256 repo", sha256_hash);

        let unique = HashSet::from([sha256_hash.to_string()]);
        let (output, hashes) = process_log_with_dimming(&input, Some(&unique));

        assert_eq!(hashes[0], sha256_hash);
        let parsed = parse_dimming_output(&output);
        assert!(!parsed[0].0, "SHA-256 hash should be matched correctly");
        assert!(parsed[0].1.contains("SHA-256 repo"));
    }

    #[test]
    fn test_process_log_with_dimming_strips_ansi_when_dimming() {
        let hash = "abc123456789012345678901234567890123456789";
        // Simulate colored git output
        let input = format!(
            "* \x01{}\x00\x1b[33mabc1234\x1b[m\x1b[33m (HEAD)\x1b[m message",
            hash
        );

        // Use a different hash to trigger dimming
        let other_unique = HashSet::from(["other".to_string()]);
        let (output, _hashes) = process_log_with_dimming(&input, Some(&other_unique));

        // Dimmed output should have colors stripped
        let parsed = parse_dimming_output(&output);
        assert!(parsed[0].0, "should be dimmed");
        // The ansi_strip should have removed the color codes
        assert!(parsed[0].1.contains("abc1234"));
        assert!(parsed[0].1.contains("(HEAD)"));
    }

    // Tests for strip_hash_markers

    #[test]
    fn test_strip_hash_markers_removes_soh_nul_block() {
        let full_hash = "abc1234567890123456789012345678901234567ab";
        let input = format!("* \x01{}\x00abc1234 message", full_hash);
        let output = strip_hash_markers(&input);

        assert!(!output.contains('\x01'));
        assert!(!output.contains('\x00'));
        assert_eq!(output, "* abc1234 message");
    }

    #[test]
    fn test_strip_hash_markers_preserves_other_content() {
        // No markers - content unchanged
        let input = "* abc1234 (HEAD -> main) Initial commit";
        let output = strip_hash_markers(input);
        assert_eq!(output, input);
    }

    #[test]
    fn test_strip_hash_markers_handles_multiple_markers() {
        let input = "line1 \x01hash1\x00 content1\nline2 \x01hash2\x00 content2";
        let output = strip_hash_markers(input);
        assert_eq!(output, "line1  content1\nline2  content2");
    }

    // Tests for extract_hash_from_line

    #[test]
    fn test_extract_hash_from_line_finds_hash() {
        let full_hash = "abc1234567890123456789012345678901234567ab";
        let line = format!("* \x01{}\x00abc1234 message", full_hash);
        let extracted = extract_hash_from_line(&line);
        assert_eq!(extracted, Some(full_hash));
    }

    #[test]
    fn test_extract_hash_from_line_sha256() {
        let sha256_hash = "abc1234567890123456789012345678901234567890123456789012345678901";
        let line = format!("* \x01{}\x00abc1234 message", sha256_hash);
        let extracted = extract_hash_from_line(&line);
        assert_eq!(extracted, Some(sha256_hash));
    }

    #[test]
    fn test_extract_hash_from_line_no_markers() {
        let line = "* abc1234 message";
        let extracted = extract_hash_from_line(line);
        assert_eq!(extracted, None);
    }

    #[test]
    fn test_extract_hash_from_line_incomplete_markers() {
        // Only SOH, no NUL
        let line = "* \x01abc1234 message";
        let extracted = extract_hash_from_line(line);
        assert_eq!(extracted, None);
    }
}
