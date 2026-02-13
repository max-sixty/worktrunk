//! GitLab MR provider.
//!
//! Implements `RemoteRefProvider` for GitLab Merge Requests using the `glab` CLI.
//!
//! # API Differences from GitHub
//!
//! GitLab's MR API (`projects/:id/merge_requests/:iid`) only returns project IDs,
//! not full project objects. To get clone URLs for fork MRs, we must make separate
//! calls to the Projects API (`projects/:id`).
//!
//! In contrast, GitHub's PR API returns complete `head.repo` and `base.repo` objects
//! with `clone_url` and `ssh_url` — everything in one call.
//!
//! # Deferred URL Fetching
//!
//! To avoid the ~1 second overhead of 2 extra API calls, we defer URL fetching:
//!
//! 1. `fetch_mr_info()` returns `RemoteRefInfo` with `fork_push_url: None`
//! 2. Caller checks if branch already tracks the MR via `branch_tracks_ref()`
//! 3. Only if a new branch is needed, call `fetch_gitlab_project_urls()`
//!
//! This saves ~1 second for the common case (switching to an existing MR branch).

use std::io::ErrorKind;
use std::path::Path;

use anyhow::{Context, bail};
use serde::Deserialize;

use super::{PlatformData, RemoteRefInfo, RemoteRefProvider};
use crate::git::RefType;
use crate::git::error::GitError;
use crate::shell_exec::Cmd;

/// GitLab Merge Request provider.
#[derive(Debug, Clone, Copy)]
pub struct GitLabProvider;

impl RemoteRefProvider for GitLabProvider {
    fn ref_type(&self) -> RefType {
        RefType::Mr
    }

    fn fetch_info(&self, number: u32, repo_root: &Path) -> anyhow::Result<RemoteRefInfo> {
        fetch_mr_info(number, repo_root)
    }

    fn ref_path(&self, number: u32) -> String {
        format!("merge-requests/{}/head", number)
    }
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

#[derive(Debug, Deserialize)]
struct GlabAuthor {
    username: String,
}

#[derive(Debug, Deserialize)]
struct GlabProject {
    ssh_url_to_repo: Option<String>,
    http_url_to_repo: Option<String>,
}

/// Error response from GitLab API.
#[derive(Debug, Deserialize)]
struct GlabApiErrorResponse {
    #[serde(default)]
    message: String,
    #[serde(default)]
    error: String,
}

/// Fetch MR information from GitLab using the `glab` CLI.
fn fetch_mr_info(mr_number: u32, repo_root: &Path) -> anyhow::Result<RemoteRefInfo> {
    let api_path = format!("projects/:id/merge_requests/{}", mr_number);

    let output = match Cmd::new("glab")
        .args(["api", &api_path])
        .current_dir(repo_root)
        .env("GLAB_NO_PROMPT", "1")
        .run()
    {
        Ok(output) => output,
        Err(e) => {
            if e.kind() == ErrorKind::NotFound {
                bail!(
                    "GitLab CLI (glab) not installed; install from https://gitlab.com/gitlab-org/cli#installation"
                );
            }
            return Err(anyhow::Error::from(e).context("Failed to run glab api"));
        }
    };

    if !output.status.success() {
        if let Ok(error_response) = serde_json::from_slice::<GlabApiErrorResponse>(&output.stdout) {
            let error_text = if !error_response.message.is_empty() {
                &error_response.message
            } else {
                &error_response.error
            };

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

        let stderr = String::from_utf8_lossy(&output.stderr);
        let details = if stderr.trim().is_empty() {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        return Err(GitError::CliApiError {
            ref_type: RefType::Mr,
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

    if response.source_branch.is_empty() {
        bail!(
            "MR !{} has empty branch name; the MR may be in an invalid state",
            mr_number
        );
    }

    let is_cross_repo = response.source_project_id != response.target_project_id;

    // Parse host/owner/repo from the web URL (always available from the MR API).
    // e.g., "https://gitlab.com/owner/repo/-/merge_requests/101" → host/owner/repo
    let (project_url, _) = response
        .web_url
        .split_once("/-/")
        .with_context(|| format!("GitLab MR URL missing /-/ separator: {}", response.web_url))?;
    let parsed_url = crate::git::GitRemoteUrl::parse(project_url).ok_or_else(|| {
        anyhow::anyhow!("Failed to parse GitLab project from MR URL: {project_url}")
    })?;

    // Don't fetch project URLs here - defer until after branch_tracks_ref check
    // in switch.rs, which often short-circuits (branch already configured).
    // Use fetch_gitlab_project_urls() when URLs are actually needed.

    Ok(RemoteRefInfo {
        ref_type: RefType::Mr,
        number: mr_number,
        title: response.title,
        author: response.author.username,
        state: response.state,
        draft: response.draft,
        source_branch: response.source_branch,
        is_cross_repo,
        url: response.web_url,
        fork_push_url: None, // Populated later by fetch_gitlab_project_urls if needed
        platform_data: PlatformData::GitLab {
            host: parsed_url.host().to_string(),
            base_owner: parsed_url.owner().to_string(),
            base_repo: parsed_url.repo().to_string(),
            source_project_id: response.source_project_id,
            target_project_id: response.target_project_id,
        },
    })
}

/// URLs for a GitLab fork MR.
#[derive(Debug)]
pub struct GitLabForkUrls {
    /// URL to push to the fork (source project).
    pub fork_push_url: Option<String>,
    /// Target project URL (where MR refs live) - SSH or HTTPS based on config.
    pub target_url: Option<String>,
}

/// Fetch project URLs for a GitLab fork MR.
///
/// This is deferred from `fetch_mr_info` because GitLab's MR API doesn't include
/// project URLs (unlike GitHub's PR API which returns full repo objects). The 2
/// extra API calls (~500ms each) are only needed when creating a new branch.
///
/// See module-level docs for the full explanation of this optimization.
pub fn fetch_gitlab_project_urls(
    info: &RemoteRefInfo,
    repo_root: &Path,
) -> anyhow::Result<GitLabForkUrls> {
    let PlatformData::GitLab {
        source_project_id,
        target_project_id,
        ..
    } = &info.platform_data
    else {
        bail!("fetch_gitlab_project_urls called on non-GitLab ref");
    };

    // Fetch source project URLs (for fork push)
    let (source_ssh, source_http) = fetch_project_urls(*source_project_id, repo_root)
        .with_context(|| {
            format!(
                "Failed to fetch source project {} for MR !{}",
                source_project_id, info.number
            )
        })?;

    // Fetch target project URLs (where MR refs live)
    let (target_ssh, target_http) = fetch_project_urls(*target_project_id, repo_root)
        .with_context(|| {
            format!(
                "Failed to fetch target project {} for MR !{}",
                target_project_id, info.number
            )
        })?;

    // Compute URLs based on protocol preference
    let use_ssh = get_git_protocol() == "ssh";
    let fork_push_url = if use_ssh {
        source_ssh.or(source_http)
    } else {
        source_http.or(source_ssh)
    };
    let target_url = if use_ssh {
        target_ssh.or(target_http)
    } else {
        target_http.or(target_ssh)
    };

    Ok(GitLabForkUrls {
        fork_push_url,
        target_url,
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

/// Get the git protocol configured in `glab` (GitLab CLI).
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::remote_ref::RemoteRefInfo;

    #[test]
    fn test_ref_path() {
        let provider = GitLabProvider;
        assert_eq!(provider.ref_path(42), "merge-requests/42/head");
        assert_eq!(provider.tracking_ref(42), "refs/merge-requests/42/head");
    }

    #[test]
    fn test_ref_type() {
        let provider = GitLabProvider;
        assert_eq!(provider.ref_type(), RefType::Mr);
    }

    #[test]
    fn test_fetch_gitlab_project_urls_rejects_github_ref() {
        let github_info = RemoteRefInfo {
            ref_type: RefType::Pr,
            number: 123,
            title: "Test PR".to_string(),
            author: "user".to_string(),
            state: "open".to_string(),
            draft: false,
            source_branch: "feature".to_string(),
            is_cross_repo: false,
            url: "https://github.com/owner/repo/pull/123".to_string(),
            fork_push_url: None,
            platform_data: PlatformData::GitHub {
                host: "github.com".to_string(),
                head_owner: "user".to_string(),
                head_repo: "repo".to_string(),
                base_owner: "owner".to_string(),
                base_repo: "repo".to_string(),
            },
        };

        let result = fetch_gitlab_project_urls(&github_info, std::path::Path::new("."));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("non-GitLab ref"));
    }
}
