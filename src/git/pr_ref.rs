//! PR reference resolution (`pr:<number>` syntax).
//!
//! This module resolves PR numbers to branches, enabling `wt switch pr:101` to
//! check out the branch associated with a pull request.
//!
//! # Syntax
//!
//! The `pr:<number>` prefix is unambiguous because colons are invalid in git
//! branch names (git rejects them as "not a valid branch name").
//!
//! ```text
//! wt switch pr:101          # Switch to branch for PR #101
//! wt switch pr:101 --yes    # Skip approval prompts
//! ```
//!
//! **Invalid usage:**
//!
//! ```text
//! wt switch --create pr:101   # Error: PR branch already exists
//! ```
//!
//! The `--create` flag is incompatible with `pr:` because the branch must
//! already exist (it's the PR's head branch).
//!
//! # Resolution Flow
//!
//! ```text
//! pr:101
//!   │
//!   ▼
//! ┌─────────────────────────────────────────────────────────┐
//! │ gh api repos/{owner}/{repo}/pulls/101                   │
//! │   → head.ref, head.repo, base.repo, html_url            │
//! └─────────────────────────────────────────────────────────┘
//!   │
//!   ├─── base.repo == head.repo ───▶ Same-repo PR
//!   │     │
//!   │     └─▶ Branch exists in origin, use directly
//!   │
//!   └─── base.repo != head.repo ───▶ Fork PR
//!         │
//!         ├─▶ Find remote for base.repo (where PR refs live)
//!         └─▶ Set up push to fork URL
//! ```
//!
//! Push permissions are not checked upfront — if the user lacks permission
//! (doesn't own fork, maintainer edits disabled), push will fail with a clear
//! error. This avoids complex permission detection logic.
//!
//! # Same-Repo PRs
//!
//! When `isCrossRepository` is `false`, the PR's branch exists in `origin`:
//!
//! 1. Resolve `headRefName` (e.g., `"feature-auth"`)
//! 2. Check if worktree exists for that branch → switch to it
//! 3. Otherwise, create worktree for the branch (DWIM from remote)
//! 4. Pushing works normally: `git push origin feature-auth`
//!
//! This is equivalent to `wt switch feature-auth` — the `pr:` syntax is just
//! a convenience for looking up the branch name.
//!
//! # Fork PRs
//!
//! When `isCrossRepository` is `true`, the branch exists in a fork, not `origin`.
//!
//! ## The Problem
//!
//! GitHub's `refs/pull/<N>/head` refs are **read-only**. You cannot push to them:
//!
//! ```text
//! $ git push origin HEAD:refs/pull/101/head
//! error: cannot lock ref 'refs/pull/101/head': unable to resolve reference
//! ```
//!
//! To update a fork PR, you must push to the **fork's branch**, not the PR ref.
//!
//! ## Push Strategy (No Remote Required)
//!
//! Git's `branch.<name>.pushRemote` config accepts a URL directly, not just a
//! named remote. This means we can set up push tracking without adding remotes:
//!
//! ```text
//! branch.contributor/feature.remote = origin
//! branch.contributor/feature.merge = refs/pull/101/head
//! branch.contributor/feature.pushRemote = git@github.com:contributor/repo.git
//! ```
//!
//! This configuration gives us:
//! - `git pull` fetches from origin's PR ref (stays up to date with PR)
//! - `git push` pushes to the fork URL (updates the PR)
//! - No stray remotes cluttering `git remote -v`
//!
//! ## Checkout Flow (Fork PRs)
//!
//! ```text
//! 1. Get PR metadata from gh
//!      │
//!      ▼
//! 2. Fetch PR head from origin
//!    git fetch origin pull/101/head
//!      │
//!      ▼
//! 3. Create local branch from FETCH_HEAD
//!    git branch <local-branch> FETCH_HEAD
//!      │
//!      ▼
//! 4. Configure branch tracking
//!    git config branch.<local-branch>.remote origin
//!    git config branch.<local-branch>.merge refs/pull/101/head
//!    git config branch.<local-branch>.pushRemote <fork-url>
//!      │
//!      ▼
//! 5. Create worktree for the branch
//! ```
//!
//! ## Local Branch Naming
//!
//! To avoid collisions when multiple PRs have the same branch name (common with
//! branches like `fix`, `update`, etc.), we use a naming scheme:
//!
//! - **Same-repo PR**: Use `headRefName` directly (e.g., `feature-auth`)
//! - **Fork PR**: Use `<owner>/<headRefName>` (e.g., `contributor/feature-auth`)
//!
//! The `<owner>/` prefix ensures uniqueness and makes it clear which fork the
//! branch comes from.
//!
//! ## Push Behavior
//!
//! After checkout, `git push` sends to the fork URL:
//!
//! ```text
//! $ git push
//! # Pushes to git@github.com:contributor/repo.git
//! # PR automatically updates on GitHub
//! ```
//!
//! No named remote is added — the URL is used directly via `pushRemote`.
//!
//! # Error Handling
//!
//! ## PR Not Found
//!
//! ```text
//! ✗ PR #101 not found
//! ↳ Run gh repo set-default --view to check which repo is being queried
//! ```
//!
//! This often happens when `origin` points to a fork but `gh` hasn't been
//! configured to look at the upstream repo. Fix with `gh repo set-default`.
//!
//! ## gh Not Authenticated
//!
//! ```text
//! ✗ GitHub CLI not authenticated
//! ↳ Run gh auth login to authenticate
//! ```
//!
//! ## gh Not Installed
//!
//! ```text
//! ✗ GitHub CLI (gh) required for pr: syntax
//! ↳ Install from https://cli.github.com/
//! ```
//!
//! ## --create Conflict
//!
//! ```text
//! ✗ Cannot use --create with pr: syntax
//! ↳ The PR's branch already exists; remove --create
//! ```
//!
//! # Edge Cases
//!
//! ## Branch Name Collisions
//!
//! If user already has a local branch named `contributor/feature`:
//!
//! - Check if it tracks the same PR ref → reuse it
//! - Otherwise → error with suggestion to delete the branch or use a different name
//!
//! ## Worktree Already Exists
//!
//! If worktree already exists for the resolved branch:
//!
//! - Switch to it (normal `wt switch` behavior)
//! - Don't re-fetch or re-configure
//!
//! ## Draft PRs
//!
//! Draft PRs are checkable like regular PRs. The `isDraft` field could be
//! shown in output but doesn't affect behavior.
//!
//! ## Renamed Branches
//!
//! If the PR's head branch was renamed after PR creation, `headRefName`
//! reflects the current name. We always use the current name.
//!
//! # Platform Support
//!
//! This feature is GitHub-specific. GitLab has similar concepts:
//!
//! - `glab mr view <number>` with `source_branch` field
//! - Different permission model (no exact "maintainer edits" equivalent)
//!
//! Future work could add `mr:<number>` syntax for GitLab, following the same
//! patterns but using `glab` CLI.
//!
//! # Implementation Notes
//!
//! ## Repository Resolution
//!
//! `gh pr view` needs to know which GitHub repo to query. For fork workflows
//! where `origin` points to a fork, `gh` needs to know to look at the parent
//! repo for PRs.
//!
//! The `gh` CLI handles this via `gh repo set-default`:
//!
//! ```text
//! # Stores in git config: remote.origin.gh-resolved = base
//! gh repo set-default owner/upstream-repo
//!
//! # View current setting
//! gh repo set-default --view
//! ```
//!
//! If `gh-resolved` is not set, `gh` may prompt interactively or use heuristics
//! (checking if the repo is a fork and using its parent).
//!
//! **Diagnostics:** `wt config show` should display the resolved repo so users
//! understand which repo PR lookups will query.
//!
//! ## GitHub API Fields
//!
//! We use `gh api repos/{owner}/{repo}/pulls/<number>` which returns:
//! - `head.ref`, `head.repo.owner.login`, `head.repo.name` — PR branch info
//! - `base.repo.owner.login`, `base.repo.name` — target repo (where PR refs live)
//! - `html_url` — PR web URL
//!
//! ## Remote URL Construction
//!
//! For SSH remotes:
//! ```text
//! git@github.com:<owner>/<repo>.git
//! ```
//!
//! For HTTPS remotes:
//! ```text
//! https://github.com/<owner>/<repo>.git
//! ```
//!
//! We match the protocol of the existing `origin` remote to be consistent
//! with the user's authentication setup.
//!
//! ## Caching
//!
//! PR metadata is not cached — we always fetch fresh to ensure we have
//! current state (PR might have been closed, branch might have been pushed).
//!
//! # Testing Strategy
//!
//! ## Unit Tests
//!
//! - PR number parsing from `pr:<number>` syntax
//! - Local branch name generation
//! - URL construction matching origin protocol
//!
//! ## Integration Tests (with mock gh)
//!
//! - Same-repo PR checkout
//! - Fork PR checkout
//! - Existing worktree reuse
//! - Error cases: PR not found, gh not authenticated
//!
//! ## Manual Testing
//!
//! - Fork PR push/pull cycle
//! - Interaction with `wt merge`
//! - Multiple fork PRs with same branch name

use anyhow::{Context, bail};
use serde::Deserialize;

use crate::shell_exec::Cmd;

/// Information about a PR retrieved from GitHub.
#[derive(Debug, Clone)]
pub struct PrInfo {
    /// The PR number.
    pub number: u32,
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
    /// The PR's web URL.
    pub url: String,
}

/// Raw JSON response from `gh api repos/{owner}/{repo}/pulls/{number}`.
#[derive(Debug, Deserialize)]
struct GhApiPrResponse {
    head: GhPrRef,
    base: GhPrRef,
    html_url: String,
}

#[derive(Debug, Deserialize)]
struct GhPrRef {
    #[serde(rename = "ref")]
    ref_name: String,
    repo: GhPrRepo,
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
            let error_str = e.to_string();
            if error_str.contains("No such file")
                || error_str.contains("not found")
                || error_str.contains("cannot find")
            {
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

        bail!("gh api failed: {}", stderr.trim());
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

    // Compute is_cross_repository by comparing base and head repos
    let is_cross_repository = response.base.repo.owner.login != response.head.repo.owner.login
        || response.base.repo.name != response.head.repo.name;

    Ok(PrInfo {
        number: pr_number,
        head_ref_name: response.head.ref_name,
        head_owner: response.head.repo.owner.login,
        head_repo: response.head.repo.name,
        base_owner: response.base.repo.owner.login,
        base_repo: response.base.repo.name,
        is_cross_repository,
        url: response.html_url,
    })
}

/// Generate the local branch name for a PR.
///
/// - Same-repo PRs: use `headRefName` directly
/// - Fork PRs: use `<owner>/<headRefName>` to avoid collisions
pub fn local_branch_name(pr: &PrInfo) -> String {
    if pr.is_cross_repository {
        format!("{}/{}", pr.head_owner, pr.head_ref_name)
    } else {
        pr.head_ref_name.clone()
    }
}

/// Construct the remote URL for a fork, matching the protocol of the primary remote.
///
/// If remote uses SSH (`git@github.com:`), returns SSH URL.
/// If remote uses HTTPS (`https://github.com/`), returns HTTPS URL.
pub fn fork_remote_url(owner: &str, repo: &str, remote_url: &str) -> String {
    if remote_url.starts_with("git@") || remote_url.contains("ssh://") {
        format!("git@github.com:{}/{}.git", owner, repo)
    } else {
        format!("https://github.com/{}/{}.git", owner, repo)
    }
}

/// Check if a branch is tracking a specific PR.
///
/// Returns `Some(true)` if the branch is configured to track `refs/pull/<pr_number>/head`.
/// Returns `Some(false)` if the branch exists but tracks something else.
/// Returns `None` if the branch doesn't exist.
pub fn branch_tracks_pr(repo_root: &std::path::Path, branch: &str, pr_number: u32) -> Option<bool> {
    use crate::shell_exec::Cmd;

    let config_key = format!("branch.{}.merge", branch);
    let output = Cmd::new("git")
        .args(["config", "--get", &config_key])
        .current_dir(repo_root)
        .run()
        .ok()?;

    if !output.status.success() {
        // Config key doesn't exist - branch might not track anything
        // Check if branch exists at all
        let branch_exists = Cmd::new("git")
            .args([
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{}", branch),
            ])
            .current_dir(repo_root)
            .run()
            .map(|o| o.status.success())
            .unwrap_or(false);

        return if branch_exists { Some(false) } else { None };
    }

    let merge_ref = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let expected_ref = format!("refs/pull/{}/head", pr_number);

    Some(merge_ref == expected_ref)
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
            head_ref_name: "feature-auth".to_string(),
            head_owner: "owner".to_string(),
            head_repo: "repo".to_string(),
            base_owner: "owner".to_string(),
            base_repo: "repo".to_string(),
            is_cross_repository: false,
            url: "https://github.com/owner/repo/pull/101".to_string(),
        };
        assert_eq!(local_branch_name(&pr), "feature-auth");
    }

    #[test]
    fn test_local_branch_name_fork() {
        let pr = PrInfo {
            number: 101,
            head_ref_name: "feature-auth".to_string(),
            head_owner: "contributor".to_string(),
            head_repo: "repo".to_string(),
            base_owner: "owner".to_string(),
            base_repo: "repo".to_string(),
            is_cross_repository: true,
            url: "https://github.com/owner/repo/pull/101".to_string(),
        };
        assert_eq!(local_branch_name(&pr), "contributor/feature-auth");
    }

    #[test]
    fn test_fork_remote_url_ssh() {
        let url = fork_remote_url("contributor", "repo", "git@github.com:owner/repo.git");
        assert_eq!(url, "git@github.com:contributor/repo.git");
    }

    #[test]
    fn test_fork_remote_url_https() {
        let url = fork_remote_url("contributor", "repo", "https://github.com/owner/repo.git");
        assert_eq!(url, "https://github.com/contributor/repo.git");
    }
}
