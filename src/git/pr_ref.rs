//! GitHub PR reference resolution (`pr:<number>` syntax).
//!
//! This module resolves PR numbers to branches for `wt switch pr:101`.
//! For shared documentation on PR/MR resolution, see the `remote_ref` module.
//!
//! # GitHub-Specific Notes
//!
//! ## Repository Resolution
//!
//! `gh api` needs to know which GitHub repo to query. For fork workflows
//! where the primary remote points to a fork, `gh` needs to know to look at
//! the parent repo for PRs.
//!
//! The `gh` CLI handles this via `gh repo set-default`:
//!
//! ```text
//! gh repo set-default owner/upstream-repo  # Stores: remote.origin.gh-resolved = base
//! gh repo set-default --view               # View current setting
//! ```
//!
//! If `gh-resolved` is not set, `gh` may prompt interactively or use heuristics.
//!
//! ## API Fields
//!
//! We use `gh api repos/{owner}/{repo}/pulls/<number>` which returns:
//! - `head.ref`, `head.repo.owner.login`, `head.repo.name` — PR branch info
//! - `base.repo.owner.login`, `base.repo.name` — target repo (where PR refs live)
//! - `html_url` — PR web URL

use std::io::ErrorKind;
use std::path::Path;

use anyhow::{Context, bail};
use serde::Deserialize;

use super::error::GitError;
use crate::shell_exec::Cmd;

/// Information about a PR retrieved from GitHub.
#[derive(Debug, Clone)]
pub struct PrInfo {
    /// The PR number.
    pub number: u32,
    /// The PR title.
    pub title: String,
    /// The PR author's username.
    pub author: String,
    /// The PR state ("open", "closed").
    pub state: String,
    /// Whether this is a draft PR.
    pub draft: bool,
    /// The branch name in the head repository.
    pub head_ref_name: String,
    /// The owner of the head repository (fork owner for cross-repo PRs).
    pub head_owner: String,
    /// The name of the head repository.
    pub head_repo: String,
    /// The owner of the base repository (where the PR was opened).
    pub base_owner: String,
    /// The name of the base repository.
    pub base_repo: String,
    /// Whether this is a cross-repository (fork) PR.
    pub is_cross_repository: bool,
    /// The GitHub host extracted from `html_url` (e.g., "github.com", "github.enterprise.com").
    pub host: String,
    /// The PR's web URL.
    pub url: String,
}

impl super::RefContext for PrInfo {
    fn ref_type(&self) -> super::RefType {
        super::RefType::Pr
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

#[derive(Debug, Deserialize)]
struct GhUser {
    login: String,
}

#[derive(Debug, Deserialize)]
struct GhPrRef {
    #[serde(rename = "ref")]
    ref_name: String,
    /// The repository for this ref. Can be `null` if the fork was deleted.
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

/// Parse a `pr:<number>` reference, returning the PR number if valid.
///
/// Returns `None` if the input doesn't match the `pr:<number>` pattern.
pub fn parse_pr_ref(input: &str) -> Option<u32> {
    let suffix = input.strip_prefix("pr:")?;
    suffix.parse().ok()
}

/// Fetch PR information from GitHub using the `gh` CLI.
///
/// Uses `gh api` to query the GitHub API directly, which provides
/// both head and base repository information.
///
/// # Errors
///
/// Returns an error if:
/// - `gh` is not installed or not authenticated
/// - The PR doesn't exist
/// - The JSON response is malformed
pub fn fetch_pr_info(pr_number: u32, repo_root: &std::path::Path) -> anyhow::Result<PrInfo> {
    // Use gh api with {owner}/{repo} placeholders - gh resolves these from repo context
    let api_path = format!("repos/{{owner}}/{{repo}}/pulls/{}", pr_number);

    let output = match Cmd::new("gh")
        .args(["api", &api_path])
        .current_dir(repo_root)
        .env("GH_PROMPT_DISABLED", "1")
        .run()
    {
        Ok(output) => output,
        Err(e) => {
            // Check if gh is not installed (OS error for command not found)
            if e.kind() == ErrorKind::NotFound {
                bail!("GitHub CLI (gh) not installed; install from https://cli.github.com/");
            }
            return Err(anyhow::Error::from(e).context("Failed to run gh api"));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr_lower = stderr.to_lowercase();

        // PR not found (HTTP 404)
        if stderr_lower.contains("not found") || stderr_lower.contains("404") {
            bail!("PR #{} not found", pr_number);
        }

        // Authentication errors
        if stderr_lower.contains("authentication")
            || stderr_lower.contains("logged in")
            || stderr_lower.contains("auth login")
            || stderr_lower.contains("not logged")
            || stderr_lower.contains("401")
        {
            bail!("GitHub CLI not authenticated; run gh auth login");
        }

        // Rate limiting
        if stderr_lower.contains("rate limit")
            || stderr_lower.contains("api rate")
            || stderr_lower.contains("403")
        {
            bail!("GitHub API rate limit exceeded; wait a few minutes and retry");
        }

        // Network errors
        if stderr_lower.contains("network")
            || stderr_lower.contains("connection")
            || stderr_lower.contains("timeout")
        {
            bail!("Network error connecting to GitHub; check your internet connection");
        }

        // Unknown error - show full output in gutter for debugging
        return Err(GitError::CliApiError {
            ref_type: super::RefType::Pr,
            message: format!("gh api failed for PR #{}", pr_number),
            stderr: stderr.trim().to_string(),
        }
        .into());
    }

    let response: GhApiPrResponse = serde_json::from_slice(&output.stdout).with_context(|| {
        format!(
            "Failed to parse GitHub API response for PR #{}. \
             This may indicate a GitHub API change.",
            pr_number
        )
    })?;

    // Validate required fields are not empty
    if response.head.ref_name.is_empty() {
        bail!(
            "PR #{} has empty branch name; the PR may be in an invalid state",
            pr_number
        );
    }

    // Extract base repo (should always be present - the PR is opened against an existing repo)
    let base_repo = response.base.repo.context(
        "PR base repository is null; this is unexpected and may indicate a GitHub API issue",
    )?;

    // Extract head repo (can be null if the fork was deleted)
    let head_repo = response.head.repo.ok_or_else(|| {
        anyhow::anyhow!(
            "PR #{} source repository was deleted. \
             The fork that this PR was opened from no longer exists, \
             so the branch cannot be checked out.",
            pr_number
        )
    })?;

    // Compute is_cross_repository by comparing base and head repos (case-insensitive)
    let is_cross_repository = !base_repo
        .owner
        .login
        .eq_ignore_ascii_case(&head_repo.owner.login)
        || !base_repo.name.eq_ignore_ascii_case(&head_repo.name);

    // Extract host from html_url (e.g., "https://github.com/owner/repo/pull/123" → "github.com")
    let host = response
        .html_url
        .strip_prefix("https://")
        .or_else(|| response.html_url.strip_prefix("http://"))
        .and_then(|s| s.split('/').next())
        .filter(|h| !h.is_empty())
        .with_context(|| format!("Failed to parse host from PR URL: {}", response.html_url))?
        .to_string();

    Ok(PrInfo {
        number: pr_number,
        title: response.title,
        author: response.user.login,
        state: response.state,
        draft: response.draft,
        head_ref_name: response.head.ref_name,
        head_owner: head_repo.owner.login,
        head_repo: head_repo.name,
        base_owner: base_repo.owner.login,
        base_repo: base_repo.name,
        is_cross_repository,
        host,
        url: response.html_url,
    })
}

/// Generate the local branch name for a PR.
///
/// Uses `headRefName` directly for both same-repo and fork PRs. This ensures
/// the local branch name matches the remote branch name, which is required for
/// `git push` to work correctly with `push.default = current`.
///
/// See module docs for why we can't use a prefixed name like `<owner>/<branch>`.
pub fn local_branch_name(pr: &PrInfo) -> String {
    pr.head_ref_name.clone()
}

/// Generate a prefixed local branch name for a fork PR when the unprefixed name conflicts.
///
/// Returns `<head_owner>/<head_ref_name>` (e.g., `contributor/main`).
///
/// This is used when the PR's branch name conflicts with an existing local branch.
/// Note: `git push` won't work with this naming because the local and remote
/// branch names don't match. Users must push manually with explicit refspecs.
pub fn prefixed_local_branch_name(pr: &PrInfo) -> String {
    format!("{}/{}", pr.head_owner, pr.head_ref_name)
}

/// Get the git protocol preference from `gh` (GitHub CLI).
///
/// Returns `true` for SSH if `gh config get git_protocol` returns "ssh".
fn use_ssh_protocol() -> bool {
    Cmd::new("gh")
        .args(["config", "get", "git_protocol"])
        .run()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim() == "ssh")
        .unwrap_or(false)
}

/// Construct the remote URL for a fork repository.
///
/// Uses `gh config get git_protocol` to determine SSH vs HTTPS preference.
pub fn fork_remote_url(host: &str, owner: &str, repo: &str) -> String {
    if use_ssh_protocol() {
        fork_remote_url_ssh(host, owner, repo)
    } else {
        fork_remote_url_https(host, owner, repo)
    }
}

/// Construct an SSH-format remote URL.
fn fork_remote_url_ssh(host: &str, owner: &str, repo: &str) -> String {
    format!("git@{}:{}/{}.git", host, owner, repo)
}

/// Construct an HTTPS-format remote URL.
fn fork_remote_url_https(host: &str, owner: &str, repo: &str) -> String {
    format!("https://{}/{}/{}.git", host, owner, repo)
}

/// Check if a branch is tracking a specific PR.
///
/// Returns `Some(true)` if the branch is configured to track `refs/pull/<pr_number>/head`.
/// Returns `Some(false)` if the branch exists but tracks something else.
/// Returns `None` if the branch doesn't exist.
pub fn branch_tracks_pr(repo_root: &Path, branch: &str, pr_number: u32) -> Option<bool> {
    let expected_ref = format!("refs/pull/{}/head", pr_number);
    super::branch_tracks_ref(repo_root, branch, &expected_ref)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_pr_ref() {
        assert_eq!(parse_pr_ref("pr:101"), Some(101));
        assert_eq!(parse_pr_ref("pr:1"), Some(1));
        assert_eq!(parse_pr_ref("pr:99999"), Some(99999));

        // Invalid cases
        assert_eq!(parse_pr_ref("pr:"), None);
        assert_eq!(parse_pr_ref("pr:abc"), None);
        assert_eq!(parse_pr_ref("pr:-1"), None);
        assert_eq!(parse_pr_ref("PR:101"), None); // case-sensitive
        assert_eq!(parse_pr_ref("feature-branch"), None);
        assert_eq!(parse_pr_ref("101"), None);
    }

    #[test]
    fn test_local_branch_name_same_repo() {
        let pr = PrInfo {
            number: 101,
            title: "Fix authentication bug".to_string(),
            author: "alice".to_string(),
            state: "open".to_string(),
            draft: false,
            head_ref_name: "feature-auth".to_string(),
            head_owner: "owner".to_string(),
            head_repo: "repo".to_string(),
            base_owner: "owner".to_string(),
            base_repo: "repo".to_string(),
            is_cross_repository: false,
            host: "github.com".to_string(),
            url: "https://github.com/owner/repo/pull/101".to_string(),
        };
        assert_eq!(local_branch_name(&pr), "feature-auth");
    }

    #[test]
    fn test_local_branch_name_fork() {
        // Fork PRs also use headRefName directly (not owner/branch) because
        // the local branch name must match the fork's branch for git push to work
        let pr = PrInfo {
            number: 101,
            title: "Fix authentication bug".to_string(),
            author: "contributor".to_string(),
            state: "open".to_string(),
            draft: false,
            head_ref_name: "feature-auth".to_string(),
            head_owner: "contributor".to_string(),
            head_repo: "repo".to_string(),
            base_owner: "owner".to_string(),
            base_repo: "repo".to_string(),
            is_cross_repository: true,
            host: "github.com".to_string(),
            url: "https://github.com/owner/repo/pull/101".to_string(),
        };
        assert_eq!(local_branch_name(&pr), "feature-auth");
    }

    #[test]
    fn test_prefixed_local_branch_name() {
        // When the fork's branch name conflicts with a local branch,
        // we use owner/branch format as a fallback (push won't work)
        let pr = PrInfo {
            number: 101,
            head_ref_name: "main".to_string(),
            head_owner: "contributor".to_string(),
            head_repo: "repo".to_string(),
            base_owner: "owner".to_string(),
            base_repo: "repo".to_string(),
            is_cross_repository: true,
            host: "github.com".to_string(),
            url: "https://github.com/owner/repo/pull/101".to_string(),
            title: "Test PR".to_string(),
            author: "contributor".to_string(),
            state: "open".to_string(),
            draft: false,
        };
        assert_eq!(prefixed_local_branch_name(&pr), "contributor/main");
    }

    #[test]
    fn test_fork_remote_url() {
        // Protocol depends on `gh config get git_protocol`
        let url = fork_remote_url("github.com", "contributor", "repo");
        let valid_urls = [
            "git@github.com:contributor/repo.git",
            "https://github.com/contributor/repo.git",
        ];
        assert!(valid_urls.contains(&url.as_str()), "unexpected URL: {url}");

        let url = fork_remote_url("github.example.com", "contributor", "repo");
        let valid_urls = [
            "git@github.example.com:contributor/repo.git",
            "https://github.example.com/contributor/repo.git",
        ];
        assert!(valid_urls.contains(&url.as_str()), "unexpected URL: {url}");
    }

    #[test]
    fn test_fork_remote_url_formats() {
        // Test SSH format explicitly
        assert_eq!(
            fork_remote_url_ssh("github.com", "contributor", "repo"),
            "git@github.com:contributor/repo.git"
        );
        assert_eq!(
            fork_remote_url_ssh("github.example.com", "org", "project"),
            "git@github.example.com:org/project.git"
        );

        // Test HTTPS format explicitly
        assert_eq!(
            fork_remote_url_https("github.com", "contributor", "repo"),
            "https://github.com/contributor/repo.git"
        );
        assert_eq!(
            fork_remote_url_https("github.example.com", "org", "project"),
            "https://github.example.com/org/project.git"
        );
    }
}
