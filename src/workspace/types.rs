//! Shared types used by both VCS implementations.
//!
//! These types are VCS-agnostic and shared between git and jj workspace
//! implementations. They are re-exported from [`crate::git`] for backward
//! compatibility.

use std::path::Path;

/// Line-level diff totals (added/deleted counts) used across VCS operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub struct LineDiff {
    pub added: usize,
    pub deleted: usize,
}

impl LineDiff {
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

/// Why branch content is considered integrated into the target branch.
///
/// Used by both `wt list` (for status symbols) and `wt remove` (for messages).
/// Each variant corresponds to a specific integration check. In `wt list`,
/// three symbols represent these checks:
/// - `_` for [`SameCommit`](Self::SameCommit) with clean working tree (empty)
/// - `–` for [`SameCommit`](Self::SameCommit) with dirty working tree
/// - `⊂` for all others (content integrated via different history)
///
/// The checks are ordered by cost (cheapest first):
/// 1. [`SameCommit`](Self::SameCommit) - commit SHA comparison (~1ms)
/// 2. [`Ancestor`](Self::Ancestor) - ancestor check (~1ms)
/// 3. [`NoAddedChanges`](Self::NoAddedChanges) - three-dot diff (~50-100ms)
/// 4. [`TreesMatch`](Self::TreesMatch) - tree SHA comparison (~100-300ms)
/// 5. [`MergeAddsNothing`](Self::MergeAddsNothing) - merge simulation (~500ms-2s)
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, strum::IntoStaticStr)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum IntegrationReason {
    /// Branch HEAD is literally the same commit as target.
    ///
    /// Used by `wt remove` to determine if branch is safely deletable.
    /// In `wt list`, same-commit state is shown via `MainState::Empty` (`_`) or
    /// `MainState::SameCommit` (`–`) depending on working tree cleanliness.
    SameCommit,

    /// Branch HEAD is an ancestor of target (target has moved past this branch).
    ///
    /// Symbol in `wt list`: `⊂`
    Ancestor,

    /// Three-dot diff (`main...branch`) shows no files.
    /// The branch has no file changes beyond the merge-base.
    ///
    /// Symbol in `wt list`: `⊂`
    NoAddedChanges,

    /// Branch tree SHA equals target tree SHA.
    /// Commit history differs but file contents are identical.
    ///
    /// Symbol in `wt list`: `⊂`
    TreesMatch,

    /// Simulated merge (`git merge-tree`) produces the same tree as target.
    /// The branch has changes, but they're already in target via a different path.
    ///
    /// Symbol in `wt list`: `⊂`
    MergeAddsNothing,
}

impl IntegrationReason {
    /// Human-readable description for use in messages (e.g., `wt remove` output).
    ///
    /// Returns a phrase that expects the target branch name to follow
    /// (e.g., "same commit as" + "main" → "same commit as main").
    pub fn description(&self) -> &'static str {
        match self {
            Self::SameCommit => "same commit as",
            Self::Ancestor => "ancestor of",
            Self::NoAddedChanges => "no added changes on",
            Self::TreesMatch => "tree matches",
            Self::MergeAddsNothing => "all changes in",
        }
    }

    /// Status symbol used in `wt list` for this integration reason.
    ///
    /// - `SameCommit` → `_` (matches `MainState::Empty`)
    /// - Others → `⊂` (matches `MainState::Integrated`)
    pub fn symbol(&self) -> &'static str {
        match self {
            Self::SameCommit => "_",
            _ => "⊂",
        }
    }
}

/// Display context for the local push progress message.
///
/// Controls the verb and optional notes in the progress line emitted by
/// `local_push`. E.g. merge passes `verb: "Merging"` and notes like
/// "(no commit/squash needed)", while step push uses the default "Pushing".
///
/// "Local push" means advancing a target branch ref to include feature commits —
/// no remote interaction. Git implements this via `git push <local-path>`,
/// jj via `jj bookmark set`.
pub struct LocalPushDisplay<'a> {
    /// Verb in -ing form for the progress line.
    pub verb: &'a str,
    /// Optional parenthetical notes appended after the SHA
    /// (e.g., " (no commit/squash needed)"). Include the leading space.
    /// Empty string = omitted.
    pub notes: &'a str,
}

impl Default for LocalPushDisplay<'_> {
    fn default() -> Self {
        Self {
            verb: "Pushing",
            notes: "",
        }
    }
}

/// Result of a local push operation, with enough data for the command handler
/// to format the final success/info message.
///
/// "Local push" means advancing a target branch ref — no remote interaction.
#[derive(Debug, Clone)]
pub struct LocalPushResult {
    /// Number of commits pushed locally (0 = already up-to-date).
    pub commit_count: usize,
    /// Summary parts for the success message parenthetical.
    /// E.g. `["1 commit", "1 file", "+1"]`. Empty for jj or when count is 0.
    pub stats_summary: Vec<String>,
}

/// Extract the directory name from a path for display purposes.
///
/// Returns the last component of the path as a string, or "(unknown)" if
/// the path has no filename or contains invalid UTF-8.
pub fn path_dir_name(path: &Path) -> &str {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("(unknown)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_line_diff_is_empty() {
        assert!(LineDiff::default().is_empty());
        assert!(
            !LineDiff {
                added: 1,
                deleted: 0
            }
            .is_empty()
        );
        assert!(
            !LineDiff {
                added: 0,
                deleted: 1
            }
            .is_empty()
        );
    }

    #[test]
    fn test_integration_reason_symbol() {
        assert_eq!(IntegrationReason::SameCommit.symbol(), "_");
        assert_eq!(IntegrationReason::Ancestor.symbol(), "⊂");
        assert_eq!(IntegrationReason::NoAddedChanges.symbol(), "⊂");
        assert_eq!(IntegrationReason::TreesMatch.symbol(), "⊂");
        assert_eq!(IntegrationReason::MergeAddsNothing.symbol(), "⊂");
    }

    #[test]
    fn test_path_dir_name() {
        assert_eq!(
            path_dir_name(Path::new("/repos/myrepo.feature")),
            "myrepo.feature"
        );
        assert_eq!(path_dir_name(Path::new("/")), "(unknown)");
    }

    #[test]
    fn test_line_diff_conversions() {
        let diff = LineDiff {
            added: 10,
            deleted: 5,
        };
        let tuple: (usize, usize) = diff.into();
        assert_eq!(tuple, (10, 5));
        let back: LineDiff = (3, 7).into();
        assert_eq!(
            back,
            LineDiff {
                added: 3,
                deleted: 7
            }
        );
    }

    #[test]
    fn test_integration_reason_description() {
        assert_eq!(
            IntegrationReason::SameCommit.description(),
            "same commit as"
        );
        assert_eq!(IntegrationReason::Ancestor.description(), "ancestor of");
        assert_eq!(
            IntegrationReason::NoAddedChanges.description(),
            "no added changes on"
        );
        assert_eq!(IntegrationReason::TreesMatch.description(), "tree matches");
        assert_eq!(
            IntegrationReason::MergeAddsNothing.description(),
            "all changes in"
        );
    }
}
