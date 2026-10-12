//! Forge dispatch for CI status detection.
//!
//! Given a [`ForgeKind`] (resolved by [`Repository::ci_platform`]), routes to
//! the GitHub (`gh`), GitLab (`glab`), Gitea (`tea`), or Azure DevOps (`az`)
//! backend and checks whether that CLI is installed.
//!
//! A failed call shows the error indicator only when the CLI holds a login for
//! the forge's host. Without one, the user isn't using that CLI here, and the
//! cell stays blank. `gh` says so itself, with exit code 4 for the host it
//! resolved for the request; for the other CLIs, a login check reads local
//! state for the remote's host. Either way, a lost connection still shows the
//! error.

use std::sync::LazyLock;

use dashmap::DashMap;
use worktrunk::git::{ForgeKind, Repository, remote_ref};

use super::{
    CiBranchName, CiStatus, PrStatus, auth_host, azure, branch_remote_url, gitea, github, gitlab,
    tool_available,
};

/// Whether each forge's CLI is installed, probed once per process.
static INSTALLED: LazyLock<DashMap<ForgeKind, bool>> = LazyLock::new(DashMap::new);

/// Whether each forge's CLI holds a login for a host, asked once per process,
/// and only once a call has failed.
static LOGGED_IN: LazyLock<DashMap<(ForgeKind, String), bool>> = LazyLock::new(DashMap::new);

/// Whether the CLI tool for this platform is installed (cached).
fn is_tool_available(platform: ForgeKind) -> bool {
    *INSTALLED
        .entry(platform)
        .or_insert_with(|| tool_available(platform.cli(), &["--version"]))
}

/// Whether the platform's CLI holds a login for the branch's forge host
/// (cached). A host that can't be resolved, or a check that can't finish,
/// counts as logged in, leaving the failure on display.
fn is_logged_in(platform: ForgeKind, repo: &Repository, branch: &CiBranchName) -> bool {
    // A `gh` call without a login never reaches here: the backend reads its
    // exit code, which follows SSH host aliases where a check by host can't.
    if platform == ForgeKind::GitHub {
        return true;
    }
    let Some(host) = auth_host(repo, branch_remote_url(repo, branch)) else {
        return true;
    };
    *LOGGED_IN
        .entry((platform, host.clone()))
        .or_insert_with(|| remote_ref::is_authed_for(platform, &host).unwrap_or(true))
}

/// Detect CI status from a PR/MR.
fn detect_pr_mr(
    platform: ForgeKind,
    repo: &Repository,
    branch: &CiBranchName,
    local_head: &str,
) -> Option<PrStatus> {
    match platform {
        ForgeKind::GitHub => github::detect_github(repo, branch, local_head),
        ForgeKind::GitLab => gitlab::detect_gitlab(repo, branch, local_head),
        ForgeKind::Gitea => gitea::detect_gitea_pr(repo, branch, local_head),
        ForgeKind::AzureDevOps => azure::detect_azure_pr(repo, branch, local_head),
    }
}

/// Whether a PR/MR could have this branch as its head.
///
/// Every forge opens a PR/MR from a branch it hosts, so a local branch that was
/// never pushed can't head one, and the forge call is skipped. Each skip saves
/// a ~450ms round trip and ~65ms of CPU, which adds up in a repo that keeps
/// many local-only branches. A remote row is on its remote by definition.
///
/// A local branch counts as never pushed when no remote has it under its name
/// and its push remote would have recorded it there: git updates
/// `refs/remotes/` on push only where the remote's fetch refspecs map the
/// branch. A `--single-branch` or `--depth` clone maps only the branch it
/// cloned, and a URL push remote (a fork's PR checked out with
/// `gh pr checkout`) maps nothing, so for those only the forge can answer.
///
/// A PR opened from a branch pushed by another clone shows once this clone
/// fetches it.
fn may_head_pr(repo: &Repository, branch: &CiBranchName) -> bool {
    if branch.is_remote() {
        return true;
    }
    let handle = repo.branch(&branch.name);
    if handle.remotes().map_or(true, |r| !r.is_empty()) {
        return true;
    }
    // Where `git push` sends it, as `branch_remote_url` resolves it.
    let push_remote = handle.push_remote().or_else(|| repo.primary_remote().ok());
    !push_remote.is_some_and(|remote| repo.fetch_tracks_branch(&remote, &branch.name))
}

/// Detect CI status from a branch workflow/pipeline (fallback when no PR/MR).
fn detect_branch(
    platform: ForgeKind,
    repo: &Repository,
    branch: &CiBranchName,
    local_head: &str,
) -> Option<PrStatus> {
    match platform {
        ForgeKind::GitHub => github::detect_github_commit_checks(repo, branch, local_head),
        // GitLab pipelines use the bare branch name (not "origin/feature").
        ForgeKind::GitLab => gitlab::detect_gitlab_pipeline(repo, &branch.name, local_head),
        // Gitea queries the combined commit status by SHA, but owner/repo come
        // from the branch's own remote (so remote-only rows hit the right repo).
        ForgeKind::Gitea => gitea::detect_gitea_commit_status(repo, branch, local_head),
        ForgeKind::AzureDevOps => azure::detect_azure_pipeline(repo, branch, local_head),
    }
}

/// Detect CI status: PR/MR first when the branch [may head one](may_head_pr),
/// then branch workflow/pipeline if `has_upstream`.
///
/// Returns `None` if the CLI tool isn't installed, no CI status is found, or a
/// call failed and the CLI holds no login for the host.
pub(super) fn detect_ci(
    platform: ForgeKind,
    repo: &Repository,
    branch: &CiBranchName,
    local_head: &str,
    has_upstream: bool,
) -> Option<PrStatus> {
    if !is_tool_available(platform) {
        return None;
    }
    let status = may_head_pr(repo, branch)
        .then(|| detect_pr_mr(platform, repo, branch, local_head))
        .flatten()
        .or_else(|| {
            has_upstream
                .then(|| detect_branch(platform, repo, branch, local_head))
                .flatten()
        })?;
    if status.ci_status == CiStatus::Error && !is_logged_in(platform, repo, branch) {
        return None;
    }
    Some(status)
}
