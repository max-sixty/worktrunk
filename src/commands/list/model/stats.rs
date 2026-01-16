//! Statistics types for worktree and branch data.
//!
//! These types hold computed statistics like ahead/behind counts,
//! line diff totals, and upstream tracking information.

use worktrunk::git::LineDiff;

/// Commit metadata for a branch or worktree HEAD.
#[derive(serde::Serialize, Clone, Default, Debug)]
pub struct CommitDetails {
    pub timestamp: i64,
    pub commit_message: String,
}

/// Ahead/behind counts relative to a base branch.
#[derive(serde::Serialize, Default, Copy, Clone, Debug)]
pub struct AheadBehind {
    pub ahead: usize,
    pub behind: usize,
}

/// Line diff totals for a branch compared to the integration target.
#[derive(serde::Serialize, Default, Copy, Clone, Debug)]
pub struct BranchDiffTotals {
    #[serde(rename = "branch_diff")]
    pub diff: LineDiff,
}

/// Upstream tracking information for a branch.
#[derive(serde::Serialize, Default, Clone, Debug)]
pub struct UpstreamStatus {
    #[serde(rename = "upstream_remote")]
    pub(crate) remote: Option<String>,
    #[serde(rename = "upstream_ahead")]
    pub(crate) ahead: usize,
    #[serde(rename = "upstream_behind")]
    pub(crate) behind: usize,
}

/// Active upstream tracking information (when a remote is configured).
pub struct ActiveUpstream<'a> {
    pub remote: &'a str,
    pub ahead: usize,
    pub behind: usize,
}

impl UpstreamStatus {
    /// Returns active upstream info if a remote tracking branch is configured.
    pub fn active(&self) -> Option<ActiveUpstream<'_>> {
        self.remote.as_deref().map(|remote| ActiveUpstream {
            remote,
            ahead: self.ahead,
            behind: self.behind,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ahead_behind_default() {
        let ab = AheadBehind::default();
        assert_eq!(ab.ahead, 0);
        assert_eq!(ab.behind, 0);
    }

    #[test]
    fn test_branch_diff_totals_default() {
        let diff = BranchDiffTotals::default();
        assert!(diff.diff.is_empty());
    }

    #[test]
    fn test_commit_details_default() {
        let details = CommitDetails::default();
        assert_eq!(details.timestamp, 0);
        assert_eq!(details.commit_message, "");
    }

    #[test]
    fn test_upstream_status_active_with_remote() {
        let status = UpstreamStatus {
            remote: Some("origin".to_string()),
            ahead: 3,
            behind: 2,
        };
        let active = status.active().unwrap();
        assert_eq!(active.remote, "origin");
        assert_eq!(active.ahead, 3);
        assert_eq!(active.behind, 2);
    }

    #[test]
    fn test_upstream_status_active_no_remote() {
        let status = UpstreamStatus {
            remote: None,
            ahead: 0,
            behind: 0,
        };
        assert!(status.active().is_none());
    }
}
