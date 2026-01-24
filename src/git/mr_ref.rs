//! GitLab MR reference resolution (`mr:<number>` syntax).
//!
//! This module resolves MR numbers to branches for `wt switch mr:42`.
//! For shared documentation on PR/MR resolution, see the `remote_ref` module.
//!
//! # GitLab-Specific Notes
//!
//! GitLab's permission model differs from GitHub's "maintainer edits" feature.
//! GitLab uses the `allow_collaboration` flag to indicate if fork maintainers
//! can push to the MR branch.
//!
//! ## API Fields
//!
//! We use `glab api projects/:id/merge_requests/<number>` which returns:
//! - `source_branch` — MR branch name
//! - `source_project_id`, `target_project_id` — for fork detection
//! - `web_url` — MR web URL
//!
//! For fork URLs, we make additional API calls to fetch project details.

use std::io::ErrorKind;
use std::path::Path;

use anyhow::{Context, bail};
use serde::Deserialize;

use super::error::GitError;
use crate::shell_exec::Cmd;

/// Information about an MR retrieved from GitLab.
#[derive(Debug, Clone)]
pub struct MrInfo {
    /// The MR number (iid in GitLab terms).
    pub number: u32,
    /// The MR title.
    pub title: String,
    /// The MR author's username.
    pub author: String,
    /// The MR state ("opened", "closed", "merged").
    pub state: String,
    /// Whether this is a draft/WIP MR.
    pub draft: bool,
    /// The branch name in the source project.
    pub source_branch: String,
    /// The source project ID.
    pub source_project_id: u64,
    /// The target project ID.
    pub target_project_id: u64,
    /// The source project's SSH URL (for fork push).
    pub source_project_ssh_url: Option<String>,
    /// The source project's HTTP URL (for fork push).
    pub source_project_http_url: Option<String>,
    /// The target project's SSH URL (for finding the correct remote).
    pub target_project_ssh_url: Option<String>,
    /// The target project's HTTP URL (for finding the correct remote).
    pub target_project_http_url: Option<String>,
    /// Whether this is a cross-project (fork) MR.
    pub is_cross_project: bool,
    /// The MR's web URL.
    pub url: String,
}

impl super::RefContext for MrInfo {
    fn ref_type(&self) -> super::RefType {
        super::RefType::Mr
    }
    fn number(&self) -> u32 {
        self.number
    }
    fn title(&self) -> &str {
        &self.title
    }
    fn author(&self) -> &str {
        &self.author
    }
    fn state(&self) -> &str {
        &self.state
    }
    fn draft(&self) -> bool {
        self.draft
    }
    fn url(&self) -> &str {
        &self.url
    }
    fn source_ref(&self) -> String {
        if self.is_cross_project {
            // Try to extract owner from source project URL
            let owner = self
                .source_project_http_url
                .as_ref()
                .or(self.source_project_ssh_url.as_ref())
                .and_then(|url| extract_owner_from_url(url));
            match owner {
                Some(owner) => format!("{}:{}", owner, self.source_branch),
                None => self.source_branch.clone(),
            }
        } else {
            self.source_branch.clone()
        }
    }
}

/// Extract owner from a git URL.
///
/// Handles both SSH (`git@gitlab.com:owner/repo.git`) and HTTPS
/// (`https://gitlab.com/owner/repo.git`) formats.
fn extract_owner_from_url(url: &str) -> Option<String> {
    // SSH format: git@gitlab.com:owner/repo.git
    if let Some(path) = url.strip_prefix("git@").and_then(|s| s.split(':').nth(1)) {
        return path.split('/').next().map(|s| s.to_string());
    }
    // HTTPS format: https://gitlab.com/owner/repo.git
    // After stripping prefix: "gitlab.com/owner/repo.git"
    // nth(1) skips the host and returns the owner
    if let Some(path) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    {
        return path.split('/').nth(1).map(|s| s.to_string());
    }
    None
}

/// Raw JSON response from `glab api projects/:id/merge_requests/<number>`.
#[derive(Debug, Deserialize)]
struct GlabMrResponse {
    title: String,
    author: GlabAuthor,
    state: String,
    #[serde(default)]
    draft: bool,
    source_branch: String,
    source_project_id: u64,
    target_project_id: u64,
    web_url: String,
}

/// Raw JSON response from `glab api projects/<id>`.
#[derive(Debug, Deserialize)]
struct GlabAuthor {
    username: String,
}

#[derive(Debug, Deserialize)]
struct GlabProject {
    ssh_url_to_repo: Option<String>,
    http_url_to_repo: Option<String>,
}

/// Error response from GitLab API (stdout on failure).
/// Example: `{"message":"404 Not found","error":"404 Not found"}`
#[derive(Debug, Deserialize)]
struct GlabApiErrorResponse {
    #[serde(default)]
    message: String,
    #[serde(default)]
    error: String,
}

/// Parse a `mr:<number>` reference, returning the MR number if valid.
///
/// Returns `None` if the input doesn't match the `mr:<number>` pattern.
pub fn parse_mr_ref(input: &str) -> Option<u32> {
    let suffix = input.strip_prefix("mr:")?;
    suffix.parse().ok()
}

/// Fetch MR information from GitLab using the `glab` CLI.
///
/// Uses `glab api` to get MR metadata. For fork MRs, makes additional
/// API calls to fetch source and target project URLs.
///
/// # Errors
///
/// Returns an error if:
/// - `glab` is not installed or not authenticated
/// - The MR doesn't exist
/// - The JSON response is malformed
pub fn fetch_mr_info(mr_number: u32, repo_root: &std::path::Path) -> anyhow::Result<MrInfo> {
    let api_path = format!("projects/:id/merge_requests/{}", mr_number);

    let output = match Cmd::new("glab")
        .args(["api", &api_path])
        .current_dir(repo_root)
        .env("GLAB_NO_PROMPT", "1")
        .run()
    {
        Ok(output) => output,
        Err(e) => {
            // Check if glab is not installed (OS error for command not found)
            if e.kind() == ErrorKind::NotFound {
                bail!(
                    "GitLab CLI (glab) not installed; install from https://gitlab.com/gitlab-org/cli#installation"
                );
            }
            return Err(anyhow::Error::from(e).context("Failed to run glab api"));
        }
    };

    if !output.status.success() {
        // Parse the JSON error response from stdout for structured error handling.
        // GitLab API returns JSON with "message" or "error" field containing the error.
        if let Ok(error_response) = serde_json::from_slice::<GlabApiErrorResponse>(&output.stdout) {
            let error_text = if !error_response.message.is_empty() {
                &error_response.message
            } else {
                &error_response.error
            };

            // GitLab includes status code in error message: "404 Not found", "401 Unauthorized"
            if error_text.starts_with("404") {
                bail!("MR !{} not found", mr_number);
            }
            if error_text.starts_with("401") {
                bail!("GitLab CLI not authenticated; run glab auth login");
            }
            if error_text.starts_with("403") {
                bail!("GitLab API access forbidden for MR !{}", mr_number);
            }
        }

        // Fallback for non-JSON errors (network issues, glab not configured, etc.)
        // Include stdout if stderr is empty, as some errors are reported there.
        let stderr = String::from_utf8_lossy(&output.stderr);
        let details = if stderr.trim().is_empty() {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        return Err(GitError::CliApiError {
            ref_type: super::RefType::Mr,
            message: format!("glab api failed for MR !{}", mr_number),
            stderr: details,
        }
        .into());
    }

    let response: GlabMrResponse = serde_json::from_slice(&output.stdout).with_context(|| {
        format!(
            "Failed to parse GitLab API response for MR !{}. \
             This may indicate a GitLab API change.",
            mr_number
        )
    })?;

    // Validate required fields are not empty
    if response.source_branch.is_empty() {
        bail!(
            "MR !{} has empty branch name; the MR may be in an invalid state",
            mr_number
        );
    }

    let is_cross_project = response.source_project_id != response.target_project_id;

    // Fetch project URLs for cross-project (fork) MRs.
    // The GitLab MR API only returns project IDs, so we need separate API calls.
    // TODO(perf): Defer URL fetching until after branch_tracks_mr check in switch.rs
    // to avoid unnecessary API calls when branch already exists and tracks this MR.
    let (source_project_ssh_url, source_project_http_url) = if is_cross_project {
        fetch_project_urls(response.source_project_id, repo_root).with_context(|| {
            format!(
                "Failed to fetch source project {} for MR !{}",
                response.source_project_id, mr_number
            )
        })?
    } else {
        (None, None)
    };

    let (target_project_ssh_url, target_project_http_url) = if is_cross_project {
        fetch_project_urls(response.target_project_id, repo_root).with_context(|| {
            format!(
                "Failed to fetch target project {} for MR !{}",
                response.target_project_id, mr_number
            )
        })?
    } else {
        (None, None)
    };

    Ok(MrInfo {
        number: mr_number,
        title: response.title,
        author: response.author.username,
        state: response.state,
        draft: response.draft,
        source_branch: response.source_branch,
        source_project_id: response.source_project_id,
        target_project_id: response.target_project_id,
        source_project_ssh_url,
        source_project_http_url,
        target_project_ssh_url,
        target_project_http_url,
        is_cross_project,
        url: response.web_url,
    })
}

/// Fetch project URLs from GitLab API.
fn fetch_project_urls(
    project_id: u64,
    repo_root: &Path,
) -> anyhow::Result<(Option<String>, Option<String>)> {
    let api_path = format!("projects/{}", project_id);

    let output = Cmd::new("glab")
        .args(["api", &api_path])
        .current_dir(repo_root)
        .env("GLAB_NO_PROMPT", "1")
        .run()?;

    if !output.status.success() {
        bail!("Failed to fetch project {}", project_id);
    }

    let response: GlabProject = serde_json::from_slice(&output.stdout)?;
    Ok((response.ssh_url_to_repo, response.http_url_to_repo))
}

/// Generate the local branch name for an MR.
///
/// Uses `source_branch` directly for both same-repo and fork MRs. This ensures
/// the local branch name matches the remote branch name, which is required for
/// `git push` to work correctly with `push.default = current`.
pub fn local_branch_name(mr: &MrInfo) -> String {
    mr.source_branch.clone()
}

/// Get the git protocol configured in `glab` (GitLab CLI).
///
/// Returns "https" or "ssh" based on `glab config get git_protocol`.
/// Defaults to "https" if the command fails or returns unexpected output.
pub fn get_git_protocol() -> String {
    Cmd::new("glab")
        .args(["config", "get", "git_protocol"])
        .run()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|p| p == "ssh" || p == "https")
        .unwrap_or_else(|| "https".to_string())
}

/// Get the fork remote URL for pushing.
///
/// For fork MRs, we need the source project's URL. GitLab provides both SSH and
/// HTTP URLs; we choose based on `glab config get git_protocol`.
///
/// Falls back to the other protocol if the preferred one is not available.
/// Returns `None` if neither URL is available (shouldn't happen for valid MRs).
pub fn fork_remote_url(mr: &MrInfo) -> Option<String> {
    let use_ssh = get_git_protocol() == "ssh";

    if use_ssh {
        mr.source_project_ssh_url
            .clone()
            .or_else(|| mr.source_project_http_url.clone())
    } else {
        mr.source_project_http_url
            .clone()
            .or_else(|| mr.source_project_ssh_url.clone())
    }
}

/// Get the target project URL (where MR refs live).
///
/// For fork MRs, we need to fetch from the target project's MR refs. GitLab
/// provides both SSH and HTTP URLs; we choose based on `glab config get git_protocol`.
///
/// Returns `None` if glab didn't provide target project URLs (older versions).
pub fn target_remote_url(mr: &MrInfo) -> Option<String> {
    let use_ssh = get_git_protocol() == "ssh";

    if use_ssh {
        mr.target_project_ssh_url
            .clone()
            .or_else(|| mr.target_project_http_url.clone())
    } else {
        mr.target_project_http_url
            .clone()
            .or_else(|| mr.target_project_ssh_url.clone())
    }
}

/// Check if a branch is tracking a specific MR.
///
/// Returns `Some(true)` if the branch is configured to track `refs/merge-requests/<mr_number>/head`.
/// Returns `Some(false)` if the branch exists but tracks something else.
/// Returns `None` if the branch doesn't exist.
pub fn branch_tracks_mr(repo_root: &Path, branch: &str, mr_number: u32) -> Option<bool> {
    let expected_ref = format!("refs/merge-requests/{}/head", mr_number);
    super::branch_tracks_ref(repo_root, branch, &expected_ref)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mr_ref() {
        assert_eq!(parse_mr_ref("mr:101"), Some(101));
        assert_eq!(parse_mr_ref("mr:1"), Some(1));
        assert_eq!(parse_mr_ref("mr:99999"), Some(99999));

        // Invalid cases
        assert_eq!(parse_mr_ref("mr:"), None);
        assert_eq!(parse_mr_ref("mr:abc"), None);
        assert_eq!(parse_mr_ref("mr:-1"), None);
        assert_eq!(parse_mr_ref("MR:101"), None); // case-sensitive
        assert_eq!(parse_mr_ref("feature-branch"), None);
        assert_eq!(parse_mr_ref("101"), None);
        assert_eq!(parse_mr_ref("pr:101"), None); // wrong prefix
    }

    #[test]
    fn test_local_branch_name() {
        let mr = MrInfo {
            number: 101,
            title: "Fix authentication bug".to_string(),
            author: "alice".to_string(),
            state: "opened".to_string(),
            draft: false,
            source_branch: "feature-auth".to_string(),
            source_project_id: 123,
            target_project_id: 123,
            source_project_ssh_url: None,
            source_project_http_url: None,
            target_project_ssh_url: None,
            target_project_http_url: None,
            is_cross_project: false,
            url: "https://gitlab.com/owner/repo/-/merge_requests/101".to_string(),
        };
        assert_eq!(local_branch_name(&mr), "feature-auth");
    }

    #[test]
    fn test_local_branch_name_fork() {
        // Fork MRs also use source_branch directly (not owner/branch) because
        // the local branch name must match the fork's branch for git push to work
        let mr = MrInfo {
            number: 101,
            title: "Fix authentication bug".to_string(),
            author: "contributor".to_string(),
            state: "opened".to_string(),
            draft: false,
            source_branch: "feature-auth".to_string(),
            source_project_id: 456,
            target_project_id: 123,
            source_project_ssh_url: Some("git@gitlab.com:contributor/repo.git".to_string()),
            source_project_http_url: Some("https://gitlab.com/contributor/repo.git".to_string()),
            target_project_ssh_url: Some("git@gitlab.com:owner/repo.git".to_string()),
            target_project_http_url: Some("https://gitlab.com/owner/repo.git".to_string()),
            is_cross_project: true,
            url: "https://gitlab.com/owner/repo/-/merge_requests/101".to_string(),
        };
        assert_eq!(local_branch_name(&mr), "feature-auth");
    }

    #[test]
    fn test_fork_remote_url_with_both_urls() {
        let mr = MrInfo {
            number: 101,
            title: "Feature".to_string(),
            author: "contributor".to_string(),
            state: "opened".to_string(),
            draft: false,
            source_branch: "feature".to_string(),
            source_project_id: 456,
            target_project_id: 123,
            source_project_ssh_url: Some("git@gitlab.com:contributor/repo.git".to_string()),
            source_project_http_url: Some("https://gitlab.com/contributor/repo.git".to_string()),
            target_project_ssh_url: Some("git@gitlab.com:owner/repo.git".to_string()),
            target_project_http_url: Some("https://gitlab.com/owner/repo.git".to_string()),
            is_cross_project: true,
            url: "https://gitlab.com/owner/repo/-/merge_requests/101".to_string(),
        };

        // When both URLs are available, returns one based on glab config
        let url = fork_remote_url(&mr);
        assert!(url.is_some());
        let url = url.unwrap();
        let valid_urls = [
            "git@gitlab.com:contributor/repo.git",
            "https://gitlab.com/contributor/repo.git",
        ];
        assert!(valid_urls.contains(&url.as_str()), "unexpected URL: {url}");
    }

    #[test]
    fn test_fork_remote_url_ssh_only() {
        let mr = MrInfo {
            number: 101,
            title: "Feature".to_string(),
            author: "contributor".to_string(),
            state: "opened".to_string(),
            draft: false,
            source_branch: "feature".to_string(),
            source_project_id: 456,
            target_project_id: 123,
            source_project_ssh_url: Some("git@gitlab.com:contributor/repo.git".to_string()),
            source_project_http_url: None,
            target_project_ssh_url: Some("git@gitlab.com:owner/repo.git".to_string()),
            target_project_http_url: None,
            is_cross_project: true,
            url: "https://gitlab.com/owner/repo/-/merge_requests/101".to_string(),
        };

        // When only SSH is available, returns SSH regardless of config
        let url = fork_remote_url(&mr);
        assert_eq!(url, Some("git@gitlab.com:contributor/repo.git".to_string()));
    }

    #[test]
    fn test_fork_remote_url_https_only() {
        let mr = MrInfo {
            number: 101,
            title: "Feature".to_string(),
            author: "contributor".to_string(),
            state: "opened".to_string(),
            draft: false,
            source_branch: "feature".to_string(),
            source_project_id: 456,
            target_project_id: 123,
            source_project_ssh_url: None,
            source_project_http_url: Some("https://gitlab.com/contributor/repo.git".to_string()),
            target_project_ssh_url: None,
            target_project_http_url: Some("https://gitlab.com/owner/repo.git".to_string()),
            is_cross_project: true,
            url: "https://gitlab.com/owner/repo/-/merge_requests/101".to_string(),
        };

        // When only HTTPS is available, returns HTTPS regardless of config
        let url = fork_remote_url(&mr);
        assert_eq!(
            url,
            Some("https://gitlab.com/contributor/repo.git".to_string())
        );
    }

    #[test]
    fn test_fork_remote_url_none() {
        let mr = MrInfo {
            number: 101,
            title: "Feature".to_string(),
            author: "contributor".to_string(),
            state: "opened".to_string(),
            draft: false,
            source_branch: "feature".to_string(),
            source_project_id: 456,
            target_project_id: 123,
            source_project_ssh_url: None,
            source_project_http_url: None,
            target_project_ssh_url: None,
            target_project_http_url: None,
            is_cross_project: true,
            url: "https://gitlab.com/owner/repo/-/merge_requests/101".to_string(),
        };

        // When no source URLs are available, returns None
        let url = fork_remote_url(&mr);
        assert_eq!(url, None);
    }

    #[test]
    fn test_target_remote_url_with_both_urls() {
        let mr = MrInfo {
            number: 101,
            title: "Feature".to_string(),
            author: "contributor".to_string(),
            state: "opened".to_string(),
            draft: false,
            source_branch: "feature".to_string(),
            source_project_id: 456,
            target_project_id: 123,
            source_project_ssh_url: Some("git@gitlab.com:contributor/repo.git".to_string()),
            source_project_http_url: Some("https://gitlab.com/contributor/repo.git".to_string()),
            target_project_ssh_url: Some("git@gitlab.com:owner/repo.git".to_string()),
            target_project_http_url: Some("https://gitlab.com/owner/repo.git".to_string()),
            is_cross_project: true,
            url: "https://gitlab.com/owner/repo/-/merge_requests/101".to_string(),
        };

        // When both URLs are available, returns one based on glab config
        let url = target_remote_url(&mr);
        assert!(url.is_some());
        let url = url.unwrap();
        let valid_urls = [
            "git@gitlab.com:owner/repo.git",
            "https://gitlab.com/owner/repo.git",
        ];
        assert!(valid_urls.contains(&url.as_str()), "unexpected URL: {url}");
    }

    #[test]
    fn test_target_remote_url_none() {
        let mr = MrInfo {
            number: 101,
            title: "Feature".to_string(),
            author: "contributor".to_string(),
            state: "opened".to_string(),
            draft: false,
            source_branch: "feature".to_string(),
            source_project_id: 456,
            target_project_id: 123,
            source_project_ssh_url: Some("git@gitlab.com:contributor/repo.git".to_string()),
            source_project_http_url: Some("https://gitlab.com/contributor/repo.git".to_string()),
            target_project_ssh_url: None,
            target_project_http_url: None,
            is_cross_project: true,
            url: "https://gitlab.com/owner/repo/-/merge_requests/101".to_string(),
        };

        // When no target URLs are available, returns None
        let url = target_remote_url(&mr);
        assert_eq!(url, None);
    }

    #[test]
    fn test_extract_owner_from_url_ssh() {
        assert_eq!(
            extract_owner_from_url("git@gitlab.com:owner/repo.git"),
            Some("owner".to_string())
        );
        assert_eq!(
            extract_owner_from_url("git@gitlab.example.com:org/repo.git"),
            Some("org".to_string())
        );
    }

    #[test]
    fn test_extract_owner_from_url_https() {
        assert_eq!(
            extract_owner_from_url("https://gitlab.com/owner/repo.git"),
            Some("owner".to_string())
        );
        assert_eq!(
            extract_owner_from_url("http://gitlab.com/owner/repo.git"),
            Some("owner".to_string())
        );
    }

    #[test]
    fn test_extract_owner_from_url_invalid() {
        assert_eq!(extract_owner_from_url("invalid-url"), None);
        assert_eq!(extract_owner_from_url(""), None);
    }

    #[test]
    fn test_source_ref_same_project() {
        let mr = MrInfo {
            number: 101,
            title: "Fix bug".to_string(),
            author: "alice".to_string(),
            state: "opened".to_string(),
            draft: false,
            source_branch: "feature-auth".to_string(),
            source_project_id: 123,
            target_project_id: 123,
            source_project_ssh_url: None,
            source_project_http_url: None,
            target_project_ssh_url: None,
            target_project_http_url: None,
            is_cross_project: false,
            url: "https://gitlab.com/owner/repo/-/merge_requests/101".to_string(),
        };
        use crate::git::RefContext;
        assert_eq!(mr.source_ref(), "feature-auth");
    }

    #[test]
    fn test_source_ref_cross_project() {
        let mr = MrInfo {
            number: 101,
            title: "Fix bug".to_string(),
            author: "contributor".to_string(),
            state: "opened".to_string(),
            draft: false,
            source_branch: "feature-fix".to_string(),
            source_project_id: 456,
            target_project_id: 123,
            source_project_ssh_url: Some("git@gitlab.com:contributor/repo.git".to_string()),
            source_project_http_url: Some("https://gitlab.com/contributor/repo.git".to_string()),
            target_project_ssh_url: None,
            target_project_http_url: None,
            is_cross_project: true,
            url: "https://gitlab.com/owner/repo/-/merge_requests/101".to_string(),
        };
        use crate::git::RefContext;
        assert_eq!(mr.source_ref(), "contributor:feature-fix");
    }

    #[test]
    fn test_source_ref_cross_project_no_url() {
        // When URL parsing fails, falls back to just the branch name
        let mr = MrInfo {
            number: 101,
            title: "Fix bug".to_string(),
            author: "contributor".to_string(),
            state: "opened".to_string(),
            draft: false,
            source_branch: "feature-fix".to_string(),
            source_project_id: 456,
            target_project_id: 123,
            source_project_ssh_url: None,
            source_project_http_url: None,
            target_project_ssh_url: None,
            target_project_http_url: None,
            is_cross_project: true,
            url: "https://gitlab.com/owner/repo/-/merge_requests/101".to_string(),
        };
        use crate::git::RefContext;
        assert_eq!(mr.source_ref(), "feature-fix");
    }
}
