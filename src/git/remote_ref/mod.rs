//! Unified PR/MR reference resolution.
//!
//! This module provides a trait-based architecture for resolving GitHub PRs and GitLab MRs
//! to local branches. Both platforms follow the same workflow:
//!
//! 1. Parse `pr:<number>` or `mr:<number>` syntax
//! 2. Fetch metadata from the platform API
//! 3. Check if a local branch already tracks this ref
//! 4. Create/configure the branch if needed
//!
//! # Usage
//!
//! ```ignore
//! use worktrunk::git::remote_ref::{GitHubProvider, RemoteRefProvider};
//!
//! let provider = GitHubProvider;
//! let info = provider.fetch_info(123, repo_root)?;
//! println!("PR #{}: {}", info.number, info.title);
//! ```
//!
//! # Platform-Specific Notes
//!
//! ## GitHub
//!
//! Uses `gh api repos/{owner}/{repo}/pulls/<number>` which returns head/base repo info.
//! For fork workflows, `gh repo set-default` controls which repo is queried.
//!
//! ## GitLab
//!
//! Uses `glab api projects/:id/merge_requests/<number>`. Fork MRs require additional
//! API calls to fetch source/target project URLs.

pub mod github;
pub mod gitlab;
mod info;

pub use github::GitHubProvider;
pub use gitlab::GitLabProvider;
pub use info::{PlatformData, RemoteRefInfo};

use std::path::Path;

use crate::git::RefType;

/// Provider trait for platform-specific PR/MR operations.
///
/// Each platform (GitHub, GitLab) implements this trait to provide
/// unified access to PR/MR metadata and ref paths.
pub trait RemoteRefProvider {
    /// The reference type this provider handles.
    fn ref_type(&self) -> RefType;

    /// Fetch ref information from the platform API.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The CLI tool is not installed or not authenticated
    /// - The ref doesn't exist
    /// - The JSON response is malformed
    fn fetch_info(&self, number: u32, repo_root: &Path) -> anyhow::Result<RemoteRefInfo>;

    /// Get the git ref path for this ref (e.g., "pull/123/head" or "merge-requests/42/head").
    fn ref_path(&self, number: u32) -> String;

    /// Get the full tracking ref (e.g., "refs/pull/123/head").
    fn tracking_ref(&self, number: u32) -> String {
        format!("refs/{}", self.ref_path(number))
    }
}

/// Check if a local branch is tracking a specific remote ref.
///
/// Returns `Some(true)` if the branch is configured to track the given ref.
/// Returns `Some(false)` if the branch exists but tracks something else (or nothing).
/// Returns `None` if the branch doesn't exist.
pub fn branch_tracks_ref(
    repo_root: &Path,
    branch: &str,
    provider: &dyn RemoteRefProvider,
    number: u32,
) -> Option<bool> {
    let expected_ref = provider.tracking_ref(number);
    crate::git::branch_tracks_ref(repo_root, branch, &expected_ref)
}

/// Generate the local branch name for a remote ref.
///
/// Uses the source branch name directly. This ensures the local branch name
/// matches the remote branch name, which is required for `git push` to work
/// correctly with `push.default = current`.
pub fn local_branch_name(info: &RemoteRefInfo) -> String {
    info.source_branch.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ref_paths() {
        let gh = GitHubProvider;
        assert_eq!(gh.ref_path(123), "pull/123/head");
        assert_eq!(gh.tracking_ref(123), "refs/pull/123/head");

        let gl = GitLabProvider;
        assert_eq!(gl.ref_path(42), "merge-requests/42/head");
        assert_eq!(gl.tracking_ref(42), "refs/merge-requests/42/head");
    }
}
