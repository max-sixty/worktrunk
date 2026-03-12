//! Gitea PR provider.
//!
//! Implements `RemoteRefProvider` for Gitea Pull Requests using the `tea` CLI.

use std::io::ErrorKind;
use std::path::Path;

use anyhow::{Context, bail};
use serde::Deserialize;

use super::{PlatformData, RemoteRefInfo, RemoteRefProvider};
use crate::git::RefType;
use crate::git::error::GitError;
use crate::shell_exec::Cmd;

/// Gitea Pull Request provider.
#[derive(Debug, Clone, Copy)]
pub struct GiteaProvider;

impl RemoteRefProvider for GiteaProvider {
    fn ref_type(&self) -> RefType {
        RefType::Pr
    }

    fn fetch_info(&self, number: u32, repo_root: &Path) -> anyhow::Result<RemoteRefInfo> {
        fetch_pr_info(number, repo_root)
    }

    fn ref_path(&self, number: u32) -> String {
        format!("pull/{}/head", number)
    }
}

/// Raw JSON response from `tea api repos/{owner}/{repo}/pulls/{number}`.
#[derive(Debug, Deserialize)]
struct TeaApiPrResponse {
    title: String,
    user: TeaUser,
    state: String,
    #[serde(default)]
    draft: bool,
    head: TeaPrRef,
    base: TeaPrRef,
    html_url: String,
}

/// Error response from Gitea API.
#[derive(Debug, Deserialize)]
struct TeaApiErrorResponse {
    #[serde(default)]
    message: String,
}

#[derive(Debug, Deserialize)]
struct TeaUser {
    login: String,
}

#[derive(Debug, Deserialize)]
struct TeaPrRef {
    #[serde(default)]
    label: String,
    #[serde(rename = "ref")]
    #[serde(default)]
    ref_name: String,
    repo: Option<TeaPrRepo>,
}

#[derive(Debug, Deserialize)]
struct TeaPrRepo {
    name: String,
    owner: TeaOwner,
}

#[derive(Debug, Deserialize)]
struct TeaOwner {
    login: String,
}

/// Fetch PR information from Gitea using the `tea` CLI.
fn fetch_pr_info(pr_number: u32, repo_root: &Path) -> anyhow::Result<RemoteRefInfo> {
    let api_path = format!("repos/{{owner}}/{{repo}}/pulls/{}", pr_number);

    let output = match Cmd::new("tea")
        .args(["api", &api_path])
        .current_dir(repo_root)
        .run()
    {
        Ok(output) => output,
        Err(e) => {
            if e.kind() == ErrorKind::NotFound {
                bail!("Gitea CLI (tea) not installed; install from https://gitea.com/gitea/tea");
            }
            return Err(anyhow::Error::from(e).context("Failed to run tea api"));
        }
    };

    if !output.status.success() {
        if let Ok(error_response) = serde_json::from_slice::<TeaApiErrorResponse>(&output.stdout) {
            let message_lower = error_response.message.to_ascii_lowercase();
            if message_lower.contains("404") || message_lower.contains("not found") {
                bail!("Gitea PR #{} not found", pr_number);
            }
            if message_lower.contains("401") || message_lower.contains("unauthorized") {
                bail!("Gitea CLI not authenticated; run tea login add");
            }
            if message_lower.contains("403") || message_lower.contains("forbidden") {
                bail!("Gitea API access forbidden for PR #{}", pr_number);
            }
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        let details = if stderr.trim().is_empty() {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        return Err(GitError::CliApiError {
            ref_type: RefType::Pr,
            message: format!("tea api failed for PR #{}", pr_number),
            stderr: details,
        }
        .into());
    }

    let response: TeaApiPrResponse = serde_json::from_slice(&output.stdout).with_context(|| {
        format!(
            "Failed to parse Gitea API response for PR #{}. \
             This may indicate a Gitea API change.",
            pr_number
        )
    })?;

    let source_branch = extract_source_branch(&response.head).ok_or_else(|| {
        anyhow::anyhow!(
            "Gitea PR #{} has no source branch in head.label/head.ref; \
             the PR may be in an invalid state",
            pr_number
        )
    })?;

    let base_repo = response.base.repo.context(
        "Gitea PR base repository is null; this is unexpected and may indicate a Gitea API issue",
    )?;

    let head_repo = response.head.repo.ok_or_else(|| {
        anyhow::anyhow!(
            "Gitea PR #{} source repository was deleted. \
             The fork that this PR was opened from no longer exists, \
             so the branch cannot be checked out.",
            pr_number
        )
    })?;

    let is_cross_repo = !base_repo
        .owner
        .login
        .eq_ignore_ascii_case(&head_repo.owner.login)
        || !base_repo.name.eq_ignore_ascii_case(&head_repo.name);

    let host = response
        .html_url
        .strip_prefix("https://")
        .or_else(|| response.html_url.strip_prefix("http://"))
        .and_then(|s| s.split('/').next())
        .filter(|h| !h.is_empty())
        .with_context(|| format!("Failed to parse host from PR URL: {}", response.html_url))?
        .to_string();

    let fork_push_url = if is_cross_repo {
        Some(fork_remote_url(
            &host,
            &head_repo.owner.login,
            &head_repo.name,
        ))
    } else {
        None
    };

    Ok(RemoteRefInfo {
        ref_type: RefType::Pr,
        number: pr_number,
        title: response.title,
        author: response.user.login,
        state: response.state,
        draft: response.draft,
        source_branch,
        is_cross_repo,
        url: response.html_url,
        fork_push_url,
        platform_data: PlatformData::Gitea {
            host,
            head_owner: head_repo.owner.login,
            head_repo: head_repo.name,
            base_owner: base_repo.owner.login,
            base_repo: base_repo.name,
        },
    })
}

fn extract_source_branch(head: &TeaPrRef) -> Option<String> {
    if !head.label.is_empty() {
        let branch = head
            .label
            .split_once(':')
            .map(|(_, b)| b)
            .unwrap_or(&head.label)
            .trim();
        if !branch.is_empty() {
            return Some(branch.to_string());
        }
    }

    if !head.ref_name.is_empty()
        && let Some(branch) = head.ref_name.strip_prefix("refs/heads/")
        && !branch.is_empty()
    {
        return Some(branch.to_string());
    }

    None
}

/// Construct the remote URL for a Gitea repository.
pub fn fork_remote_url(host: &str, owner: &str, repo: &str) -> String {
    format!("https://{}/{}/{}.git", host, owner, repo)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ref_path() {
        let provider = GiteaProvider;
        assert_eq!(provider.ref_path(7), "pull/7/head");
        assert_eq!(provider.tracking_ref(7), "refs/pull/7/head");
    }

    #[test]
    fn test_ref_type() {
        let provider = GiteaProvider;
        assert_eq!(provider.ref_type(), RefType::Pr);
    }

    #[test]
    fn test_extract_source_branch_prefers_label() {
        let head = TeaPrRef {
            label: "alice:feature-auth".to_string(),
            ref_name: "refs/pull/42/head".to_string(),
            repo: None,
        };
        assert_eq!(
            extract_source_branch(&head),
            Some("feature-auth".to_string())
        );
    }

    #[test]
    fn test_extract_source_branch_from_plain_label() {
        let head = TeaPrRef {
            label: "feature-auth".to_string(),
            ref_name: "refs/pull/42/head".to_string(),
            repo: None,
        };
        assert_eq!(
            extract_source_branch(&head),
            Some("feature-auth".to_string())
        );
    }

    #[test]
    fn test_extract_source_branch_fallback_to_ref() {
        let head = TeaPrRef {
            label: "".to_string(),
            ref_name: "refs/heads/feature-auth".to_string(),
            repo: None,
        };
        assert_eq!(
            extract_source_branch(&head),
            Some("feature-auth".to_string())
        );
    }

    #[test]
    fn test_extract_source_branch_invalid() {
        let head = TeaPrRef {
            label: "".to_string(),
            ref_name: "refs/pull/42/head".to_string(),
            repo: None,
        };
        assert_eq!(extract_source_branch(&head), None);
    }
}
