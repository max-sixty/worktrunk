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

/// Extract the directory name from a path for display purposes.
///
/// Returns the last component of the path as a string, or "(unknown)" if
/// the path has no filename or contains invalid UTF-8.
pub fn path_dir_name(path: &Path) -> &str {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("(unknown)")
}
