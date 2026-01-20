//! MR reference resolution (`mr:<number>` syntax).
//!
//! This module resolves MR numbers to branches, enabling `wt switch mr:101` to
//! check out the branch associated with a merge request.
//!
//! # Syntax
//!
//! The `mr:<number>` prefix is unambiguous because colons are invalid in git
//! branch names (git rejects them as "not a valid branch name").
//!
//! ```text
//! wt switch mr:101          # Switch to branch for MR !101
//! wt switch mr:101 --yes    # Skip approval prompts
//! ```
//!
//! **Invalid usage:**
//!
//! ```text
//! wt switch --create mr:101   # Error: MR branch already exists
//! ```
//!
//! The `--create` flag is incompatible with `mr:` because the branch must
//! already exist (it's the MR's source branch).
//!
//! # Resolution Flow
//!
//! ```text
//! mr:101
//!   │
//!   ▼
//! ┌─────────────────────────────────────────────────────────┐
//! │ glab mr view 101 --output json                          │
//! │   → source_branch, source_project_id, target_project_id │
//! └─────────────────────────────────────────────────────────┘
//!   │
//!   ├─── source_project_id == target_project_id ───▶ Same-repo MR
//!   │     │
//!   │     └─▶ Branch exists in primary remote, use directly
//!   │
//!   └─── source_project_id != target_project_id ───▶ Fork MR
//!         │
//!         ├─▶ Find remote for target project (where MR refs live)
//!         └─▶ Set up push to fork URL (from source project)
//! ```
//!
//! Push permissions are not checked upfront — if the user lacks permission
//! (doesn't own fork, maintainer edits disabled), push will fail with a clear
//! error. This avoids complex permission detection logic.
//!
//! # Same-Repo MRs
//!
//! When `source_project_id == target_project_id`, the MR's branch exists in
//! the primary remote:
//!
//! 1. Resolve `source_branch` (e.g., `"feature-auth"`)
//! 2. Check if worktree exists for that branch → switch to it
//! 3. Otherwise, create worktree for the branch (DWIM from remote)
//! 4. Pushing works normally: `git push`
//!
//! This is equivalent to `wt switch feature-auth` — the `mr:` syntax is just
//! a convenience for looking up the branch name.
//!
//! # Fork MRs
//!
//! When `source_project_id != target_project_id`, the branch exists in a fork,
//! not the target project.
//!
//! ## The Problem: MR Refs Are Read-Only
//!
//! GitLab's `refs/merge-requests/<N>/head` refs are **read-only** and cannot be
//! pushed to. Similar to GitHub, the only way to update a fork MR is to push
//! directly to the fork's branch.
//!
//! ## Push Strategy (No Remote Required)
//!
//! Git's `branch.<name>.pushRemote` config accepts a URL directly, not just a
//! named remote. This means we can set up push tracking without adding remotes:
//!
//! ```text
//! branch.contributor/feature.remote = origin
//! branch.contributor/feature.merge = refs/merge-requests/101/head
//! branch.contributor/feature.pushRemote = git@gitlab.com:contributor/repo.git
//! ```
//!
//! This configuration gives us:
//! - `git pull` fetches from the target repo's MR ref (stays up to date with MR)
//! - `git push` pushes to the fork URL (updates the MR)
//! - No stray remotes cluttering `git remote -v`
//!
//! ## Checkout Flow (Fork MRs)
//!
//! ```text
//! 1. Get MR metadata from glab mr view
//!      │
//!      ▼
//! 2. Find remote for target project (where MR refs live)
//!    e.g., origin → gitlab.com/group/project
//!      │
//!      ▼
//! 3. Fetch MR head from that remote
//!    git fetch origin merge-requests/101/head
//!      │
//!      ▼
//! 4. Create local branch from FETCH_HEAD
//!    git branch <local-branch> FETCH_HEAD
//!      │
//!      ▼
//! 5. Configure branch tracking
//!    git config branch.<local-branch>.remote origin
//!    git config branch.<local-branch>.merge refs/merge-requests/101/head
//!    git config branch.<local-branch>.pushRemote <fork-url>
//!      │
//!      ▼
//! 6. Create worktree for the branch
//! ```
//!
//! ## Local Branch Naming
//!
//! **The local branch name must match the fork's branch name** for `git push`
//! to work. With `push.default = current` (the common default), git pushes to
//! a same-named branch on the pushRemote. If the names differ, push fails.
//!
//! # Error Handling
//!
//! ## MR Not Found
//!
//! ```text
//! ✗ MR !101 not found
//! ```
//!
//! ## glab Not Authenticated
//!
//! ```text
//! ✗ GitLab CLI not authenticated
//! ↳ Run glab auth login to authenticate
//! ```
//!
//! ## glab Not Installed
//!
//! ```text
//! ✗ GitLab CLI (glab) required for mr: syntax
//! ↳ Install from https://gitlab.com/gitlab-org/cli
//! ```
//!
//! ## --create Conflict
//!
//! ```text
//! ✗ Cannot use --create with mr: syntax
//! ↳ The MR's branch already exists; remove --create
//! ```
//!
//! # Platform Notes
//!
//! This feature is GitLab-specific. For GitHub PRs, use `pr:<number>` syntax
//! (see `pr_ref` module).
//!
//! GitLab's permission model differs from GitHub's "maintainer edits" feature.
//! GitLab uses the `allow_collaboration` flag to indicate if fork maintainers
//! can push to the MR branch.

use std::io::ErrorKind;

use anyhow::{Context, bail};
use serde::Deserialize;

use super::error::GitError;
use crate::shell_exec::Cmd;

/// Information about an MR retrieved from GitLab.
#[derive(Debug, Clone)]
pub struct MrInfo {
    /// The MR number (iid in GitLab terms).
    pub number: u32,
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

/// Raw JSON response from `glab mr view <number> --output json`.
#[derive(Debug, Deserialize)]
struct GlabMrResponse {
    source_branch: String,
    source_project_id: u64,
    target_project_id: u64,
    web_url: String,
    /// Source project info (for getting fork URL)
    #[serde(default)]
    source_project: Option<GlabProject>,
    /// Target project info (for finding the correct remote)
    #[serde(default)]
    target_project: Option<GlabProject>,
}

#[derive(Debug, Deserialize)]
struct GlabProject {
    ssh_url_to_repo: Option<String>,
    http_url_to_repo: Option<String>,
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
/// Uses `glab mr view` to get MR metadata including source and target
/// project information.
///
/// # Errors
///
/// Returns an error if:
/// - `glab` is not installed or not authenticated
/// - The MR doesn't exist
/// - The JSON response is malformed
pub fn fetch_mr_info(mr_number: u32, repo_root: &std::path::Path) -> anyhow::Result<MrInfo> {
    let output = match Cmd::new("glab")
        .args(["mr", "view", &mr_number.to_string(), "--output", "json"])
        .current_dir(repo_root)
        .env("GLAB_NO_PROMPT", "1")
        .run()
    {
        Ok(output) => output,
        Err(e) => {
            // Check if glab is not installed (OS error for command not found)
            if e.kind() == ErrorKind::NotFound {
                bail!(
                    "GitLab CLI (glab) not installed; install from https://gitlab.com/gitlab-org/cli"
                );
            }
            return Err(anyhow::Error::from(e).context("Failed to run glab mr view"));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr_lower = stderr.to_lowercase();

        // TODO: Classifying errors by substring matching is brittle across glab versions
        // and locales. Consider using `glab api` with HTTP status codes for more reliable
        // error detection, or at minimum test against multiple glab versions.

        // MR not found
        if stderr_lower.contains("not found")
            || stderr_lower.contains("404")
            || stderr_lower.contains("could not find")
        {
            bail!("MR !{} not found", mr_number);
        }

        // Authentication errors
        if stderr_lower.contains("authentication")
            || stderr_lower.contains("logged in")
            || stderr_lower.contains("auth login")
            || stderr_lower.contains("not logged")
            || stderr_lower.contains("401")
            || stderr_lower.contains("unauthorized")
        {
            bail!("GitLab CLI not authenticated; run glab auth login");
        }

        // Unknown error - show full output in gutter for debugging
        // (Rate limits, network errors, etc. fall through here with helpful stderr)
        return Err(GitError::CliApiError {
            ref_type: super::RefType::Mr,
            message: format!("glab mr view failed for MR !{}", mr_number),
            stderr: stderr.trim().to_string(),
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

    Ok(MrInfo {
        number: mr_number,
        source_branch: response.source_branch,
        source_project_id: response.source_project_id,
        target_project_id: response.target_project_id,
        source_project_ssh_url: response
            .source_project
            .as_ref()
            .and_then(|p| p.ssh_url_to_repo.clone()),
        source_project_http_url: response
            .source_project
            .as_ref()
            .and_then(|p| p.http_url_to_repo.clone()),
        target_project_ssh_url: response
            .target_project
            .as_ref()
            .and_then(|p| p.ssh_url_to_repo.clone()),
        target_project_http_url: response
            .target_project
            .as_ref()
            .and_then(|p| p.http_url_to_repo.clone()),
        is_cross_project,
        url: response.web_url,
    })
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
pub fn branch_tracks_mr(repo_root: &std::path::Path, branch: &str, mr_number: u32) -> Option<bool> {
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
}
