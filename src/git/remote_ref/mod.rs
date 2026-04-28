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
//! let info = provider.fetch_info(123, &repo)?;
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

use std::io::ErrorKind;
use std::path::Path;
use std::process::Output;

use anyhow::bail;

use crate::git::error::GitError;
use crate::git::{RefType, Repository};
use crate::shell_exec::Cmd;

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
    fn fetch_info(&self, number: u32, repo: &Repository) -> anyhow::Result<RemoteRefInfo>;

    /// Get the git ref path for this ref (e.g., "pull/123/head" or "merge-requests/42/head").
    fn ref_path(&self, number: u32) -> String;

    /// Get the full tracking ref (e.g., "refs/pull/123/head").
    fn tracking_ref(&self, number: u32) -> String {
        format!("refs/{}", self.ref_path(number))
    }
}

pub(super) struct CliApiRequest<'a> {
    pub tool: &'a str,
    pub args: &'a [&'a str],
    pub repo_root: &'a Path,
    pub prompt_env: (&'a str, &'a str),
    pub install_hint: &'a str,
    pub run_context: &'a str,
}

pub(super) fn run_cli_api(request: CliApiRequest<'_>) -> anyhow::Result<Output> {
    match Cmd::new(request.tool)
        .args(request.args.iter().copied())
        .current_dir(request.repo_root)
        .env(request.prompt_env.0, request.prompt_env.1)
        .run()
    {
        Ok(output) => Ok(output),
        Err(error) => {
            if error.kind() == ErrorKind::NotFound {
                bail!("{}", request.install_hint);
            }
            Err(anyhow::Error::from(error).context(request.run_context.to_string()))
        }
    }
}

pub(super) fn cli_api_error_details(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.trim().is_empty() {
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    } else {
        stderr.trim().to_string()
    }
}

pub(super) fn cli_api_error(ref_type: RefType, message: String, output: &Output) -> anyhow::Error {
    GitError::CliApiError {
        ref_type,
        message,
        stderr: cli_api_error_details(output),
    }
    .into()
}

pub(super) fn cli_config_value(tool: &str, key: &str) -> Option<String> {
    Cmd::new(tool)
        .args(["config", "get", key])
        .run()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Find the local remote that points to the base (target) project for a PR/MR.
///
/// Matches by owner/repo only (host is not required to match). This handles
/// SSH host aliases where the local hostname differs from the API hostname.
/// The suggested URL in the error respects each platform's configured git
/// protocol (SSH vs HTTPS).
pub fn find_remote(repo: &Repository, info: &RemoteRefInfo) -> Result<String, GitError> {
    let (owner, repo_name) = match &info.platform_data {
        PlatformData::GitHub {
            base_owner,
            base_repo,
            ..
        }
        | PlatformData::GitLab {
            base_owner,
            base_repo,
            ..
        } => (base_owner.as_str(), base_repo.as_str()),
    };

    repo.find_remote_for_repo(None, owner, repo_name)
        .ok_or_else(|| {
            let suggested_url = match &info.platform_data {
                PlatformData::GitHub {
                    host,
                    base_owner,
                    base_repo,
                    ..
                } => github::fork_remote_url(host, base_owner, base_repo),
                PlatformData::GitLab {
                    host,
                    base_owner,
                    base_repo,
                    ..
                } => gitlab::fork_remote_url(host, base_owner, base_repo),
            };
            GitError::NoRemoteForRepo {
                owner: owner.to_string(),
                repo: repo_name.to_string(),
                suggested_url,
            }
        })
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
    expected_remote: Option<&str>,
) -> Option<bool> {
    let expected_ref = provider.tracking_ref(number);
    crate::git::branch_tracks_ref(repo_root, branch, &expected_ref, expected_remote)
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
