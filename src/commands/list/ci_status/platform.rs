//! Forge dispatch for CI status detection.
//!
//! Given a [`CiPlatform`] (resolved by [`Repository::ci_platform`]), routes to
//! the GitHub (`gh`), GitLab (`glab`), Gitea (`tea`), or Azure DevOps (`az`)
//! backend and checks whether that CLI is installed.

use std::sync::OnceLock;

use worktrunk::git::{CiPlatform, Repository};

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
fn is_tool_available(platform: CiPlatform) -> bool {
    match platform {
        CiPlatform::GitHub => CiToolsAvailable::get().gh,
        CiPlatform::GitLab => CiToolsAvailable::get().glab,
        CiPlatform::Gitea => CiToolsAvailable::get().tea,
        CiPlatform::AzureDevOps => CiToolsAvailable::get().az,
    }
}

/// Detect CI status from a PR/MR.
fn detect_pr_mr(
    platform: CiPlatform,
    repo: &Repository,
    branch: &CiBranchName,
    local_head: &str,
) -> Option<PrStatus> {
    match platform {
        CiPlatform::GitHub => github::detect_github(repo, branch, local_head),
        CiPlatform::GitLab => gitlab::detect_gitlab(repo, branch, local_head),
        CiPlatform::Gitea => gitea::detect_gitea_pr(repo, branch, local_head),
        CiPlatform::AzureDevOps => azure::detect_azure_pr(repo, branch, local_head),
    }
}

/// Detect CI status from a branch workflow/pipeline (fallback when no PR/MR).
fn detect_branch(
    platform: CiPlatform,
    repo: &Repository,
    branch: &CiBranchName,
    local_head: &str,
) -> Option<PrStatus> {
    match platform {
        CiPlatform::GitHub => github::detect_github_commit_checks(repo, branch, local_head),
        // GitLab pipelines use the bare branch name (not "origin/feature").
        CiPlatform::GitLab => gitlab::detect_gitlab_pipeline(repo, &branch.name, local_head),
        // Gitea queries the combined commit status by SHA, but owner/repo come
        // from the branch's own remote (so remote-only rows hit the right repo).
        CiPlatform::Gitea => gitea::detect_gitea_commit_status(repo, branch, local_head),
        CiPlatform::AzureDevOps => azure::detect_azure_pipeline(repo, branch, local_head),
    }
}

/// Detect CI status: PR/MR first, then branch workflow/pipeline if `has_upstream`.
///
/// Returns `None` if the CLI tool isn't installed or no CI status is found.
pub(super) fn detect_ci(
    platform: CiPlatform,
    repo: &Repository,
    branch: &CiBranchName,
    local_head: &str,
    has_upstream: bool,
) -> Option<PrStatus> {
    if !is_tool_available(platform) {
        return None;
    }
    if let Some(status) = detect_pr_mr(platform, repo, branch, local_head) {
        return Some(status);
    }
    if has_upstream {
        return detect_branch(platform, repo, branch, local_head);
    }
    None
}
