//! GitHub PR provider.
//!
//! Implements `RemoteRefProvider` for GitHub Pull Requests using the `gh` CLI.

use anyhow::{Context, bail};
use serde::Deserialize;

use super::{
    CliApiRequest, PlatformData, RemoteRefInfo, RemoteRefProvider, cli_api_error, cli_config_value,
    run_cli_api,
};
use crate::git::{self, RefType, Repository};

/// GitHub Pull Request provider.
#[derive(Debug, Clone, Copy)]
pub struct GitHubProvider;

impl RemoteRefProvider for GitHubProvider {
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

/// Raw JSON response from `gh api repos/{owner}/{repo}/pulls/{number}`.
#[derive(Debug, Deserialize)]
struct GhApiPrResponse {
    title: String,
    user: GhUser,
    state: String,
    #[serde(default)]
    draft: bool,
    head: GhPrRef,
    base: GhPrRef,
    html_url: String,
}

/// Error response from GitHub API.
#[derive(Debug, Deserialize)]
struct GhApiErrorResponse {
    #[serde(default)]
    message: String,
    #[serde(default)]
    status: String,
}

#[derive(Debug, Deserialize)]
struct GhUser {
    login: String,
}

#[derive(Debug, Deserialize)]
struct GhPrRef {
    #[serde(rename = "ref")]
    ref_name: String,
    repo: Option<GhPrRepo>,
}

#[derive(Debug, Deserialize)]
struct GhPrRepo {
    name: String,
    owner: GhOwner,
}

#[derive(Debug, Deserialize)]
struct GhOwner {
    login: String,
}

/// Fetch PR information from GitHub using the `gh` CLI.
fn fetch_pr_info(pr_number: u32, repo: &Repository) -> anyhow::Result<RemoteRefInfo> {
    let repo_root = repo.repo_path()?;

    // Extract owner/repo from primary remote URL. Uses raw URL (not
    // effective) because insteadOf may rewrite to a non-parseable path.
    // SSH aliases only affect the host, not the path — owner/repo is always real.
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
        pr_number
    );

    // Only pass --hostname when explicitly configured (for GHE / self-hosted).
    let hostname = repo
        .load_project_config()
        .ok()
        .flatten()
        .and_then(|c| c.forge_hostname().map(String::from));

    let mut args = vec!["api", api_path.as_str()];
    if let Some(h) = &hostname {
        args.extend(["--hostname", h.as_str()]);
    }
    let output = run_cli_api(CliApiRequest {
        tool: "gh",
        args: &args,
        repo_root,
        prompt_env: ("GH_PROMPT_DISABLED", "1"),
        install_hint: "GitHub CLI (gh) not installed; install from https://cli.github.com/",
        run_context: "Failed to run gh api",
    })?;

    if !output.status.success() {
        if let Ok(error_response) = serde_json::from_slice::<GhApiErrorResponse>(&output.stdout) {
            match error_response.status.as_str() {
                "404" => {
                    bail!(
                        "PR #{pr_number} not found on {}/{} ({remote}). \
                         If the PR is on a different repository, \
                         run `gh repo set-default` to set the default \
                         or configure a different primary remote.",
                        parsed.owner(),
                        parsed.repo(),
                    );
                }
                "401" => bail!("GitHub CLI not authenticated; run gh auth login"),
                "403" => {
                    let message_lower = error_response.message.to_lowercase();
                    if message_lower.contains("rate limit") {
                        bail!("GitHub API rate limit exceeded; wait a few minutes and retry");
                    }
                    bail!("GitHub API access forbidden: {}", error_response.message);
                }
                _ => {}
            }
        }

        return Err(cli_api_error(
            RefType::Pr,
            format!("gh api failed for PR #{}", pr_number),
            &output,
        ));
    }

    let response: GhApiPrResponse = serde_json::from_slice(&output.stdout).with_context(|| {
        format!(
            "Failed to parse GitHub API response for PR #{}. \
             This may indicate a GitHub API change.",
            pr_number
        )
    })?;

    if response.head.ref_name.is_empty() {
        bail!(
            "PR #{} has empty branch name; the PR may be in an invalid state",
            pr_number
        );
    }

    let base_repo = response.base.repo.context(
        "PR base repository is null; this is unexpected and may indicate a GitHub API issue",
    )?;

    let head_repo = response.head.repo.ok_or_else(|| {
        anyhow::anyhow!(
            "PR #{} source repository was deleted. \
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

    // Compute fork push URL only for cross-repo PRs
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
        source_branch: response.head.ref_name,
        is_cross_repo,
        url: response.html_url,
        fork_push_url,
        platform_data: PlatformData::GitHub {
            host,
            head_owner: head_repo.owner.login,
            head_repo: head_repo.name,
            base_owner: base_repo.owner.login,
            base_repo: base_repo.name,
        },
    })
}

/// Get the git protocol preference from `gh` (GitHub CLI).
fn use_ssh_protocol() -> bool {
    cli_config_value("gh", "git_protocol").as_deref() == Some("ssh")
}

/// Construct the remote URL for a fork repository.
pub fn fork_remote_url(host: &str, owner: &str, repo: &str) -> String {
    if use_ssh_protocol() {
        format!("git@{}:{}/{}.git", host, owner, repo)
    } else {
        format!("https://{}/{}/{}.git", host, owner, repo)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ref_path() {
        let provider = GitHubProvider;
        assert_eq!(provider.ref_path(123), "pull/123/head");
        assert_eq!(provider.tracking_ref(123), "refs/pull/123/head");
    }

    #[test]
    fn test_ref_type() {
        let provider = GitHubProvider;
        assert_eq!(provider.ref_type(), RefType::Pr);
    }

    #[test]
    fn test_fork_remote_url_formats() {
        // Protocol depends on `gh config get git_protocol`, so just check format
        let url = fork_remote_url("github.com", "contributor", "repo");
        let valid_urls = [
            "git@github.com:contributor/repo.git",
            "https://github.com/contributor/repo.git",
        ];
        assert!(valid_urls.contains(&url.as_str()), "unexpected URL: {url}");

        let url = fork_remote_url("github.example.com", "org", "project");
        let valid_urls = [
            "git@github.example.com:org/project.git",
            "https://github.example.com/org/project.git",
        ];
        assert!(valid_urls.contains(&url.as_str()), "unexpected URL: {url}");
    }
}
