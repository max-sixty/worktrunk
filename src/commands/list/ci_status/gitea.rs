//! Gitea CI status detection.
//!
//! Detects CI status from Gitea pull requests and commit statuses using the
//! `tea` CLI. Gitea's API is GitHub-compatible:
//!
//! - `GET /repos/{owner}/{repo}/commits/{sha}/status` returns a *combined*
//!   commit status (`state` + `total_count`). Both Gitea Actions and external
//!   CI report into commit statuses, so this is the pass/fail/pending rollup —
//!   the equivalent of GitHub's check-runs API.
//! - `GET /repos/{owner}/{repo}/pulls?state=open` lists open PRs (the `state`
//!   filter is required — the endpoint returns *all* states by default); each
//!   PR carries `mergeable`, which surfaces merge conflicts.
//!
//! ## Two paths (mirrors [`super::azure`])
//!
//! - [`detect_gitea_pr`] — finds an open PR whose head branch matches, surfaces
//!   conflicts from `mergeable` and the real CI state from the PR head commit's
//!   combined status, and yields the PR's `html_url` for the clickable
//!   indicator.
//! - [`detect_gitea_commit_status`] — the branch fallback when no PR exists:
//!   queries the combined status of the local HEAD SHA.
//!
//! When a PR exists, two API calls are made (pulls list + combined status);
//! results are cached for 30–60s by the caller, so `wt list` over many branches
//! doesn't refetch.
//!
//! ## Known limitations (experimental)
//!
//! - Only the first page of open PRs (Gitea's default page size, newest-first)
//!   is inspected; in a repo with many open PRs, ours could fall off the page.
//! - PRs opened from a fork (head repo owner ≠ the queried repo's owner) aren't
//!   matched — owner/repo comes from the branch's own remote, not the upstream
//!   the PR targets.
//! - `mergeable` is computed asynchronously by Gitea, so a freshly-opened PR can
//!   briefly report a false conflict until the check completes (self-corrects
//!   when the cache expires).

use serde::Deserialize;
use std::process::Output;
use worktrunk::git::{Repository, parse_owner_repo};

use super::{
    CiBranchName, CiSource, CiStatus, PrStatus, is_retriable_error, non_interactive_cmd, parse_json,
};

/// Resolve `(owner, repo)` for the branch's effective Gitea remote.
///
/// For a remote branch ref (e.g. `gitea/feature`), reads the branch's own
/// remote — so a `wt list --remotes --full` row queries the right repo in a
/// mixed-remote setup. Uses [`Repository::effective_remote_url`] to honor
/// `url.insteadOf` rewrites, matching the GitHub path's
/// `github_owner_repo_for_branch`.
///
/// For a local branch (no `branch.remote`), falls back to the primary remote's
/// *raw* URL — matching `git::remote_ref::gitea`'s `pr:` resolver. The raw URL
/// is enough here: only owner/repo (the path) are read, and `tea` resolves the
/// host from its own login config rather than the URL we pass.
fn gitea_owner_repo_for_branch(
    repo: &Repository,
    branch: &CiBranchName,
) -> Option<(String, String)> {
    let url = if let Some(remote_name) = &branch.remote {
        repo.effective_remote_url(remote_name)
    } else {
        let remote = repo.primary_remote().ok()?;
        repo.remote_url(&remote)
    }?;
    parse_owner_repo(&url)
}

/// Run `tea api <path>` from the worktree root.
///
/// `is_tool_available` has already confirmed `tea` is on PATH, so a spawn
/// failure here is an OS-level edge case — fall through to `None` (no CI status
/// for this branch) rather than logging.
fn tea_api(repo: &Repository, path: &str) -> Option<Output> {
    let repo_root = repo.current_worktree().root().ok()?;
    non_interactive_cmd("tea")
        .args(["api", path])
        .current_dir(&repo_root)
        .run()
        .ok()
}

/// Combine stderr and stdout for retriable-error sniffing. `tea` reports API
/// errors as a JSON `{"message": "..."}` on stdout, but transport errors land
/// on stderr — check both.
fn error_text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    )
}

/// Fetch the combined CI status for a commit SHA.
///
/// Returns `Some(CiStatus::Error)` for retriable failures (rate limit, network),
/// `None` when the commit has no statuses or the call fails non-retriably.
fn fetch_combined_status(
    repo: &Repository,
    owner: &str,
    repo_name: &str,
    sha: &str,
) -> Option<CiStatus> {
    let path = format!("repos/{owner}/{repo_name}/commits/{sha}/status");
    let output = tea_api(repo, &path)?;
    if !output.status.success() {
        return is_retriable_error(&error_text(&output)).then_some(CiStatus::Error);
    }
    let combined: GiteaCombinedStatus = parse_json(&output.stdout, "tea api commit status", sha)?;
    if combined.total_count == 0 {
        return None;
    }
    parse_gitea_status_state(&combined.state)
}

/// Detect Gitea PR CI status for a branch.
///
/// Lists open PRs (`tea api repos/{owner}/{repo}/pulls?state=open`), finds the
/// one whose head branch matches `branch.name`, then reports conflicts
/// (`mergeable`) and the head commit's combined CI status.
pub(super) fn detect_gitea_pr(
    repo: &Repository,
    branch: &CiBranchName,
    local_head: &str,
) -> Option<PrStatus> {
    let (owner, repo_name) = gitea_owner_repo_for_branch(repo, branch)?;

    // `state=open` is required: the pulls list returns all states by default.
    let path = format!("repos/{owner}/{repo_name}/pulls?state=open");
    let output = tea_api(repo, &path)?;
    if !output.status.success() {
        return is_retriable_error(&error_text(&output)).then(PrStatus::error);
    }

    let prs: Vec<GiteaPr> = parse_json(&output.stdout, "tea api pulls", &branch.full_name)?;

    // Match by head branch name; require the head repo owner (when present) to
    // be the repo we queried — same-repo PRs only, since this branch looks at
    // the primary remote alone (fork PRs are out of scope). A missing owner is
    // treated as a potential match, as in the GitHub path.
    let pr = prs.iter().find(|pr| {
        pr.head.ref_name == branch.name
            && pr
                .head
                .repo
                .as_ref()
                .map(|r| r.owner.login.eq_ignore_ascii_case(&owner))
                .unwrap_or(true)
    })?;

    let base_status = fetch_combined_status(
        repo,
        &owner,
        &repo_name,
        pr.head.sha.as_deref().unwrap_or(local_head),
    )
    .unwrap_or(CiStatus::NoCI);
    let ci_status = if pr.mergeable == Some(false) {
        CiStatus::Conflicts
    } else {
        base_status
    };

    let is_stale = pr
        .head
        .sha
        .as_deref()
        .map(|sha| sha != local_head)
        .unwrap_or(false);

    Some(PrStatus {
        ci_status,
        source: CiSource::PullRequest,
        is_stale,
        url: Some(pr.html_url.clone()),
    })
}

/// Detect Gitea CI status for a branch's HEAD commit (fallback when no PR).
///
/// Queries the combined commit status of `local_head` directly, so the result
/// is never stale.
pub(super) fn detect_gitea_commit_status(
    repo: &Repository,
    branch: &CiBranchName,
    local_head: &str,
) -> Option<PrStatus> {
    let (owner, repo_name) = gitea_owner_repo_for_branch(repo, branch)?;
    let ci_status = fetch_combined_status(repo, &owner, &repo_name, local_head)?;
    Some(PrStatus {
        ci_status,
        source: CiSource::Branch,
        is_stale: false,
        url: None,
    })
}

/// Map a Gitea combined-status `state` to [`CiStatus`].
///
/// Gitea collapses the per-commit statuses into one combined value — in
/// practice `success` / `pending` / `failure` (a status-less commit reports
/// `pending`, which the caller already excludes via `total_count == 0`).
/// `error` and `warning` are per-status values handled defensively in case a
/// Gitea version surfaces them here; unknown values map to `None`.
fn parse_gitea_status_state(state: &str) -> Option<CiStatus> {
    match state {
        "success" => Some(CiStatus::Passed),
        "pending" => Some(CiStatus::Running),
        "failure" | "error" | "warning" => Some(CiStatus::Failed),
        _ => None,
    }
}

/// Combined commit status from `GET /repos/{owner}/{repo}/commits/{ref}/status`.
#[derive(Debug, Deserialize)]
struct GiteaCombinedStatus {
    #[serde(default)]
    state: String,
    #[serde(default)]
    total_count: u32,
}

/// A pull request from `GET /repos/{owner}/{repo}/pulls`.
#[derive(Debug, Deserialize)]
struct GiteaPr {
    #[serde(default)]
    mergeable: Option<bool>,
    html_url: String,
    head: GiteaPrBranch,
}

#[derive(Debug, Deserialize)]
struct GiteaPrBranch {
    #[serde(rename = "ref", default)]
    ref_name: String,
    #[serde(default)]
    sha: Option<String>,
    #[serde(default)]
    repo: Option<GiteaPrRepo>,
}

#[derive(Debug, Deserialize)]
struct GiteaPrRepo {
    owner: GiteaOwner,
}

#[derive(Debug, Deserialize)]
struct GiteaOwner {
    login: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_gitea_status_state() {
        assert_eq!(parse_gitea_status_state("success"), Some(CiStatus::Passed));
        assert_eq!(parse_gitea_status_state("pending"), Some(CiStatus::Running));
        assert_eq!(parse_gitea_status_state("failure"), Some(CiStatus::Failed));
        assert_eq!(parse_gitea_status_state("error"), Some(CiStatus::Failed));
        assert_eq!(parse_gitea_status_state("warning"), Some(CiStatus::Failed));
        assert_eq!(parse_gitea_status_state(""), None);
        assert_eq!(parse_gitea_status_state("bogus"), None);
    }
}
