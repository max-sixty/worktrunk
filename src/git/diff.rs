//! Git diff utilities for parsing and formatting diff statistics.

use super::GitError;
use crate::styling::{ADDITION, DELETION};

/// Line-level diff totals (added/deleted counts) used across git operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub struct LineDiff {
    pub added: usize,
    pub deleted: usize,
}

impl LineDiff {
    /// Parse `git diff --numstat` output into aggregated line totals.
    pub fn from_numstat(output: &str) -> Result<Self, GitError> {
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
            parts.push(format!("{ADDITION}+{insertions}{ADDITION:#}"));
        }
        if let Some(deletions) = self.deletions {
            parts.push(format!("{DELETION}-{deletions}{DELETION:#}"));
        }

        parts
    }
}
