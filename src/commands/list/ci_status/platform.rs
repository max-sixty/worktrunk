//! Forge dispatch for CI status detection.
//!
//! Given a [`ForgeKind`] (resolved by [`Repository::ci_platform`]), routes to
//! the GitHub (`gh`), GitLab (`glab`), Gitea (`tea`), or Azure DevOps (`az`)
//! backend and checks whether that CLI is installed.

use std::sync::OnceLock;

use worktrunk::git::{ForgeKind, Repository};

use super::{CiBranchName, PrStatus, azure, gitea, github, gitlab, tool_available};

/// Cached availability of CI CLI tools (`gh`, `glab`, `tea`, `az`).
///
/// Probed once on first access via a `--version` check.
static CI_TOOLS: OnceLock<CiToolsAvailable> = OnceLock::new();

struct CiToolsAvailable {
    gh: bool,
    glab: bool,
    tea: bool,
    az: bool,
}

impl CiToolsAvailable {
    fn get() -> &'static Self {
        CI_TOOLS.get_or_init(|| Self {
            gh: tool_available("gh", &["--version"]),
            glab: tool_available("glab", &["--version"]),
            tea: tool_available("tea", &["--version"]),
            az: tool_available("az", &["--version"]),
        })
    }
}

/// Whether the CLI tool for this platform is installed (cached).
fn is_tool_available(platform: ForgeKind) -> bool {
    match platform {
        ForgeKind::GitHub => CiToolsAvailable::get().gh,
        ForgeKind::GitLab => CiToolsAvailable::get().glab,
        ForgeKind::Gitea => CiToolsAvailable::get().tea,
        ForgeKind::AzureDevOps => CiToolsAvailable::get().az,
    }
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
/// Returns `None` if the CLI tool isn't installed or no CI status is found.
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
    if may_head_pr(repo, branch)
        && let Some(status) = detect_pr_mr(platform, repo, branch, local_head)
    {
        return Some(status);
    }
    if has_upstream {
        return detect_branch(platform, repo, branch, local_head);
    }
    None
}
