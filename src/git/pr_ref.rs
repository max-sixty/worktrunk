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
//!   │     └─▶ Branch exists in primary remote, use directly
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
//! When `base.repo == head.repo`, the PR's branch exists in the primary remote:
//!
//! 1. Resolve `head.ref` (e.g., `"feature-auth"`)
//! 2. Check if worktree exists for that branch → switch to it
//! 3. Otherwise, create worktree for the branch (DWIM from remote)
//! 4. Pushing works normally: `git push`
//!
//! This is equivalent to `wt switch feature-auth` — the `pr:` syntax is just
//! a convenience for looking up the branch name.
//!
//! # Fork PRs
//!
//! When `base.repo != head.repo`, the branch exists in a fork, not the base repo.
//!
//! ## The Problem: PR Refs Are Read-Only
//!
//! GitHub's `refs/pull/<N>/head` refs are **read-only** and cannot be pushed to.
//! This is explicitly documented by GitHub — the `refs/pull/` namespace is a
//! "hidden ref" that GitHub manages automatically:
//!
//! ```text
//! $ git push origin HEAD:refs/pull/101/head
//! ! [remote rejected] HEAD -> refs/pull/101/head (deny updating a hidden ref)
//! ```
//!
//! There is no alternative writable ref on the base repo. The only way to update
//! a fork PR is to push directly to the fork's branch.
//!
//! The "Allow edits from maintainers" feature grants push access to the fork's
//! branch itself — it's a permission change on the fork, not a proxy through
//! the base repo's refs.
//!
//! ## Push Strategy (No Remote Required)
//!
//! Git's `branch.<name>.pushRemote` config accepts a URL directly, not just a
//! named remote. This means we can set up push tracking without adding remotes:
//!
//! ```text
//! branch.contributor/feature.remote = upstream
//! branch.contributor/feature.merge = refs/pull/101/head
//! branch.contributor/feature.pushRemote = git@github.com:contributor/repo.git
//! ```
//!
//! This configuration gives us:
//! - `git pull` fetches from the base repo's PR ref (stays up to date with PR)
//! - `git push` pushes to the fork URL (updates the PR)
//! - No stray remotes cluttering `git remote -v`
//!
//! ## Checkout Flow (Fork PRs)
//!
//! ```text
//! 1. Get PR metadata from gh api
//!      │
//!      ▼
//! 2. Find remote for base repo (where PR refs live)
//!    e.g., upstream → github.com/owner/repo
//!      │
//!      ▼
//! 3. Fetch PR head from that remote
//!    git fetch upstream pull/101/head
//!      │
//!      ▼
//! 4. Create local branch from FETCH_HEAD
//!    git branch <local-branch> FETCH_HEAD
//!      │
//!      ▼
//! 5. Configure branch tracking
//!    git config branch.<local-branch>.remote upstream
//!    git config branch.<local-branch>.merge refs/pull/101/head
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
//! a same-named branch on the pushRemote. If the names differ, push fails:
//!
//! ```text
//! # If local branch is "contributor/feature" but fork has "feature":
//! $ git push
//! error: src refspec contributor/feature does not match any
//! ```
//!
//! Git has no per-branch configuration for "push to a differently-named branch."
//! The only options are explicit refspecs (`git push HEAD:feature`) or matching
//! names. We choose matching names so `git push` "just works."
//!
//! - **Same-repo PR**: Use `headRefName` directly (e.g., `feature-auth`)
//! - **Fork PR**: Use `headRefName` directly (e.g., `feature-auth`)
//!
//! This means two fork PRs with the same branch name would conflict. The
//! `branch_tracks_pr()` check handles this by erroring if a branch exists
//! but tracks a different PR.
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
//! This often happens when the primary remote points to a fork but `gh` hasn't
//! been configured to look at the upstream repo. Fix with `gh repo set-default`.
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
//! If user already has a local branch with the same name as the PR's branch:
//!
//! - Check if it tracks the same PR ref → reuse it
//! - Otherwise → error with suggestion to rename their branch first
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
//! `gh api` needs to know which GitHub repo to query. For fork workflows
//! where the primary remote points to a fork, `gh` needs to know to look at
//! the parent repo for PRs.
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
//! We match the protocol of the existing primary remote to be consistent
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
//! - URL construction matching primary remote protocol
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

use super::GitRemoteUrl;
use super::error::GitError;
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

        // Unknown error - show full output in gutter for debugging
        return Err(GitError::GhApiError {
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

    Ok(PrInfo {
        number: pr_number,
        head_ref_name: response.head.ref_name,
        head_owner: head_repo.owner.login,
        head_repo: head_repo.name,
        base_owner: base_repo.owner.login,
        base_repo: base_repo.name,
        is_cross_repository,
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

/// Construct the remote URL for a fork, matching the protocol and host of the reference URL.
///
/// If reference uses SSH (`git@host:`), returns SSH URL.
/// If reference uses HTTPS (`https://host/`), returns HTTPS URL.
/// Falls back to `github.com` if the reference URL cannot be parsed.
pub fn fork_remote_url(owner: &str, repo: &str, reference_url: &str) -> String {
    let host = GitRemoteUrl::parse(reference_url)
        .map(|u| u.host().to_string())
        .unwrap_or_else(|| "github.com".to_string());

    if reference_url.starts_with("git@") || reference_url.contains("ssh://") {
        format!("git@{}:{}/{}.git", host, owner, repo)
    } else {
        format!("https://{}/{}/{}.git", host, owner, repo)
    }
}

/// Check if a branch is tracking a specific PR.
///
/// Returns `Some(true)` if the branch is configured to track `refs/pull/<pr_number>/head`.
/// Returns `Some(false)` if the branch exists but tracks something else.
/// Returns `None` if the branch doesn't exist.
pub fn branch_tracks_pr(repo_root: &std::path::Path, branch: &str, pr_number: u32) -> Option<bool> {
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
        // Fork PRs also use headRefName directly (not owner/branch) because
        // the local branch name must match the fork's branch for git push to work
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
        assert_eq!(local_branch_name(&pr), "feature-auth");
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

    #[test]
    fn test_fork_remote_url_github_enterprise_ssh() {
        let url = fork_remote_url(
            "contributor",
            "repo",
            "git@github.example.com:owner/repo.git",
        );
        assert_eq!(url, "git@github.example.com:contributor/repo.git");
    }

    #[test]
    fn test_fork_remote_url_github_enterprise_https() {
        let url = fork_remote_url(
            "contributor",
            "repo",
            "https://github.example.com/owner/repo.git",
        );
        assert_eq!(url, "https://github.example.com/contributor/repo.git");
    }
}
