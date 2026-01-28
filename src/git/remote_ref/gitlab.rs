//! GitLab MR provider.
//!
//! Implements `RemoteRefProvider` for GitLab Merge Requests using the `glab` CLI.

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

    // Fetch project URLs for cross-project (fork) MRs.
    // TODO(perf): These 2 API calls add noticeable latency (~500ms each). Defer URL
    // fetching until after the branch_tracks_ref check in switch.rs, which often
    // short-circuits (branch already configured) and doesn't need the URLs.
    let (source_ssh_url, source_http_url) = if is_cross_repo {
        fetch_project_urls(response.source_project_id, repo_root).with_context(|| {
            format!(
                "Failed to fetch source project {} for MR !{}",
                response.source_project_id, mr_number
            )
        })?
    } else {
        (None, None)
    };

    let (target_ssh_url, target_http_url) = if is_cross_repo {
        fetch_project_urls(response.target_project_id, repo_root).with_context(|| {
            format!(
                "Failed to fetch target project {} for MR !{}",
                response.target_project_id, mr_number
            )
        })?
    } else {
        (None, None)
    };

    // Compute fork push URL based on protocol preference
    let fork_push_url = if is_cross_repo {
        let use_ssh = get_git_protocol() == "ssh";
        if use_ssh {
            source_ssh_url.or(source_http_url)
        } else {
            source_http_url.or(source_ssh_url)
        }
    } else {
        None
    };

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
        fork_push_url,
        platform_data: PlatformData::GitLab {
            source_project_id: response.source_project_id,
            target_project_id: response.target_project_id,
            target_ssh_url,
            target_http_url,
        },
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
}
