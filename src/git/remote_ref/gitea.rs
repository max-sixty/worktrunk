//! Gitea PR provider.
//!
//! Implements `RemoteRefProvider` for Gitea Pull Requests using the `tea` CLI.
//!
//! ## API path resolution
//!
//! `tea api <path>` does support `{owner}` and `{repo}` placeholders, but their
//! values come from `tea`'s own repo-context resolver, which depends on the
//! local git remote being a Gitea-accessible URL and on the user having set up
//! `tea login add` first. We resolve owner/repo from the primary remote URL
//! ourselves and pass an already-expanded path so the call works regardless of
//! how `tea` resolves its own context.

use anyhow::{Context, bail};
use serde::Deserialize;

use super::{
    CliApiRequest, PlatformData, RemoteRefInfo, RemoteRefProvider, cli_api_error, run_cli_api,
};
use crate::git::{self, RefType, Repository};

/// Gitea Pull Request provider.
#[derive(Debug, Clone, Copy)]
pub struct GiteaProvider;

impl RemoteRefProvider for GiteaProvider {
    fn ref_type(&self) -> RefType {
        RefType::Pr
    }

    fn fetch_info(&self, number: u32, repo: &Repository) -> anyhow::Result<RemoteRefInfo> {
        fetch_pr_info(number, repo)
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
fn fetch_pr_info(pr_number: u32, repo: &Repository) -> anyhow::Result<RemoteRefInfo> {
    let repo_root = repo.repo_path()?;

    // Resolve owner/repo from the primary remote URL so we pass a fully
    // expanded path to `tea api`. See module docstring for rationale.
    let remote = repo.primary_remote()?;
    let url = repo
        .remote_url(&remote)
        .ok_or_else(|| anyhow::anyhow!("Remote '{}' has no URL", remote))?;
    let parsed = git::GitRemoteUrl::parse(&url)
        .ok_or_else(|| anyhow::anyhow!("Cannot parse remote URL: {}", url))?;

    let api_path = format!(
        "repos/{}/{}/pulls/{}",
        parsed.owner(),
        parsed.repo(),
        pr_number,
    );

    let output = run_cli_api(CliApiRequest {
        tool: "tea",
        args: &["api", &api_path],
        repo_root,
        // tea reads no prompt-disable env var; pass a no-op key/value so the
        // shared helper has something to set without inventing a fake var.
        prompt_env: ("TEA_NO_PROMPT", "1"),
        install_hint: "Gitea CLI (tea) not installed; install from https://gitea.com/gitea/tea",
        run_context: "Failed to run tea api",
    })?;

    if !output.status.success() {
        if let Ok(error_response) = serde_json::from_slice::<TeaApiErrorResponse>(&output.stdout) {
            let message_lower = error_response.message.to_ascii_lowercase();
            if message_lower.contains("404") || message_lower.contains("not found") {
                bail!(
                    "Gitea PR #{} not found on {}/{}",
                    pr_number,
                    parsed.owner(),
                    parsed.repo()
                );
            }
            if message_lower.contains("401") || message_lower.contains("unauthorized") {
                bail!("Gitea CLI not authenticated; run tea login add");
            }
            if message_lower.contains("403") || message_lower.contains("forbidden") {
                bail!("Gitea API access forbidden for PR #{}", pr_number);
            }
        }

        return Err(cli_api_error(
            RefType::Pr,
            format!("tea api failed for PR #{}", pr_number),
            &output,
        ));
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

/// Extract the source branch name from a PR's head ref/label.
///
/// Prefers `label` (Gitea returns `owner:branch` for forks, `branch` otherwise).
/// Falls back to `ref`, which Gitea returns as the bare branch name (e.g.
/// `feature-auth`); the `refs/heads/` strip handles Gitea instances that
/// happen to return a fully-qualified ref. `pulls/<idx>/head` from
/// branch-deleted PRs is excluded because it's a tracking ref, not a branch.
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

    if head.ref_name.is_empty() || head.ref_name.starts_with("pulls/") {
        return None;
    }
    let branch = head
        .ref_name
        .strip_prefix("refs/heads/")
        .unwrap_or(&head.ref_name);
    if branch.is_empty() {
        None
    } else {
        Some(branch.to_string())
    }
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
    fn test_extract_source_branch_fallback_to_bare_ref() {
        // Gitea returns just the branch name in head.ref
        let head = TeaPrRef {
            label: "".to_string(),
            ref_name: "feature-auth".to_string(),
            repo: None,
        };
        assert_eq!(
            extract_source_branch(&head),
            Some("feature-auth".to_string())
        );
    }

    #[test]
    fn test_extract_source_branch_fallback_strips_refs_heads() {
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
    fn test_extract_source_branch_skips_deleted_branch_ref() {
        // Gitea uses "pulls/<idx>/head" when the source branch is deleted —
        // not a usable branch name.
        let head = TeaPrRef {
            label: "".to_string(),
            ref_name: "pulls/42/head".to_string(),
            repo: None,
        };
        assert_eq!(extract_source_branch(&head), None);
    }
}
