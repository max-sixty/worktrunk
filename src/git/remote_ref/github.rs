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

/// Result of a single GitHub API attempt for a PR.
enum PrApiResult {
    /// Successfully fetched the PR response.
    Success(std::process::Output),
    /// PR not found on this owner/repo (404).
    NotFound,
    /// A non-retryable error occurred.
    Error(anyhow::Error),
}

/// Try fetching a PR from a specific owner/repo via the GitHub API.
fn try_fetch_pr(
    owner: &str,
    repo_name: &str,
    pr_number: u32,
    repo_root: &std::path::Path,
    hostname: Option<&str>,
) -> PrApiResult {
    let api_path = format!("repos/{}/{}/pulls/{}", owner, repo_name, pr_number);

    let mut args = vec!["api", api_path.as_str()];
    if let Some(h) = hostname {
        args.extend(["--hostname", h]);
    }
    let output = match run_cli_api(CliApiRequest {
        tool: "gh",
        args: &args,
        repo_root,
        prompt_env: ("GH_PROMPT_DISABLED", "1"),
        install_hint: "GitHub CLI (gh) not installed; install from https://cli.github.com/",
        run_context: "Failed to run gh api",
    }) {
        Ok(output) => output,
        Err(e) => return PrApiResult::Error(e),
    };

    if !output.status.success() {
        if let Ok(error_response) = serde_json::from_slice::<GhApiErrorResponse>(&output.stdout) {
            match error_response.status.as_str() {
                "404" => return PrApiResult::NotFound,
                "401" => {
                    return PrApiResult::Error(anyhow::anyhow!(
                        "GitHub CLI not authenticated; run gh auth login"
                    ));
                }
                "403" => {
                    let message_lower = error_response.message.to_lowercase();
                    if message_lower.contains("rate limit") {
                        return PrApiResult::Error(anyhow::anyhow!(
                            "GitHub API rate limit exceeded; wait a few minutes and retry"
                        ));
                    }
                    return PrApiResult::Error(anyhow::anyhow!(
                        "GitHub API access forbidden: {}",
                        error_response.message
                    ));
                }
                _ => {}
            }
        }

        return PrApiResult::Error(cli_api_error(
            RefType::Pr,
            format!("gh api failed for PR #{}", pr_number),
            &output,
        ));
    }

    PrApiResult::Success(output)
}

/// Fetch PR information from GitHub using the `gh` CLI.
///
/// Tries the primary remote first. If the PR is not found (404), retries with
/// other configured remotes. This handles fork setups where origin points to the
/// user's fork but the PR lives on the upstream repository.
fn fetch_pr_info(pr_number: u32, repo: &Repository) -> anyhow::Result<RemoteRefInfo> {
    let repo_root = repo.repo_path()?;

    // Only pass --hostname when explicitly configured (for GHE / self-hosted).
    let hostname = repo
        .load_project_config()
        .ok()
        .flatten()
        .and_then(|c| c.forge_hostname().map(String::from));

    // Collect unique owner/repo pairs from all remotes, primary remote first.
    // Uses raw URLs (not effective) because insteadOf may rewrite to a
    // non-parseable path. SSH aliases only affect the host, not the path —
    // owner/repo is always real.
    let primary_remote = repo.primary_remote()?;
    let all_remotes = repo.all_remote_urls();
    let mut tried = Vec::new();

    // Build ordered list: primary remote first, then others
    let mut remote_entries: Vec<&(String, String)> = Vec::new();
    for entry in &all_remotes {
        if entry.0 == primary_remote {
            remote_entries.insert(0, entry);
        } else {
            remote_entries.push(entry);
        }
    }

    for (_remote_name, url) in &remote_entries {
        let Some(parsed) = git::GitRemoteUrl::parse(url) else {
            continue;
        };
        let owner = parsed.owner().to_string();
        let repo_name = parsed.repo().to_string();

        // Skip if we already tried this owner/repo pair
        if tried.iter().any(|(o, r): &(String, String)| {
            o.eq_ignore_ascii_case(&owner) && r.eq_ignore_ascii_case(&repo_name)
        }) {
            continue;
        }
        tried.push((owner.clone(), repo_name.clone()));

        match try_fetch_pr(
            &owner,
            &repo_name,
            pr_number,
            repo_root,
            hostname.as_deref(),
        ) {
            PrApiResult::Success(output) => {
                return parse_pr_response(pr_number, &output);
            }
            PrApiResult::NotFound => continue,
            PrApiResult::Error(e) => return Err(e),
        }
    }

    bail!("PR #{} not found", pr_number)
}

/// Parse a successful GitHub API response into `RemoteRefInfo`.
fn parse_pr_response(
    pr_number: u32,
    output: &std::process::Output,
) -> anyhow::Result<RemoteRefInfo> {
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
