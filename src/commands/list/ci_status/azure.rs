//! Azure DevOps CI status detection.
//!
//! Detects CI status from Azure DevOps PRs and pipeline runs using the `az` CLI.
//! Requires the `azure-devops` extension (`az extension add --name azure-devops`).

use serde::Deserialize;
use worktrunk::git::{GitRemoteUrl, Repository};

use super::{
    CiBranchName, CiSource, CiStatus, PrStatus, is_retriable_error, non_interactive_cmd,
    parse_json,
};

/// Get Azure DevOps org URL from any Azure remote.
///
/// Scans all remotes for an Azure DevOps URL and extracts the organization.
/// Returns the `--org` URL (e.g., `https://dev.azure.com/myorg`).
fn get_azure_org_url(repo: &Repository) -> Option<String> {
    for (_, url) in repo.all_remote_urls() {
        if let Some(parsed) = GitRemoteUrl::parse(&url)
            && parsed.is_azure_devops()
            && let Some(org) = parsed.azure_organization()
        {
            return Some(format!("https://dev.azure.com/{}", org));
        }
    }
    None
}

/// Get Azure DevOps project name from any Azure remote.
fn get_azure_project(repo: &Repository) -> Option<String> {
    for (_, url) in repo.all_remote_urls() {
        if let Some(parsed) = GitRemoteUrl::parse(&url)
            && parsed.is_azure_devops()
            && let Some(project) = parsed.azure_project()
        {
            return Some(project.to_string());
        }
    }
    None
}

/// Detect Azure DevOps PR CI status for a branch.
///
/// Uses `az repos pr list` to find open PRs for the branch, then checks
/// the merge status for CI information.
pub(super) fn detect_azure_pr(
    repo: &Repository,
    branch: &CiBranchName,
    local_head: &str,
) -> Option<PrStatus> {
    let repo_root = repo.current_worktree().root().ok()?;
    let org_url = get_azure_org_url(repo)?;
    let project = get_azure_project(repo)?;

    // Use `az repos pr list` to find PRs with matching source branch.
    // The source-branch filter expects a full ref name.
    let source_ref = format!("refs/heads/{}", branch.name);
    let output = match non_interactive_cmd("az")
        .args([
            "repos",
            "pr",
            "list",
            "--source-branch",
            &source_ref,
            "--status",
            "active",
            "--project",
            &project,
            "--org",
            &org_url,
            "--output",
            "json",
        ])
        .current_dir(&repo_root)
        .run()
    {
        Ok(output) => output,
        Err(e) => {
            log::warn!(
                "az repos pr list failed to execute for branch {}: {}",
                branch.full_name,
                e
            );
            return None;
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if is_retriable_error(&stderr) {
            return Some(PrStatus::error());
        }
        return None;
    }

    let pr_list: Vec<AzPrListEntry> =
        parse_json(&output.stdout, "az repos pr list", &branch.full_name)?;

    let pr = pr_list.first()?;

    // Azure DevOps PR merge status maps to CI status
    let ci_status = match pr.merge_status.as_deref() {
        Some("conflicts") => CiStatus::Conflicts,
        Some("succeeded") => {
            // Merge succeeded means no conflicts, but check PR status for CI
            // The mergeStatus field relates to merge feasibility, not CI.
            // Fall through to check if we have any pipeline info from the PR.
            CiStatus::NoCI
        }
        Some("queued") => CiStatus::Running,
        _ => CiStatus::NoCI,
    };

    // Check staleness by comparing last merge source commit
    let is_stale = pr
        .last_merge_source_commit
        .as_ref()
        .and_then(|c| c.commit_id.as_ref())
        .map(|sha| sha != local_head)
        .unwrap_or(true);

    // Build PR URL
    let url = pr.url_from_web(repo);

    Some(PrStatus {
        ci_status,
        source: CiSource::PullRequest,
        is_stale,
        url,
    })
}

/// Detect Azure Pipelines status for a branch (when no PR exists).
///
/// Uses `az pipelines runs list --branch <branch>` to get the most recent
/// pipeline run for the branch.
pub(super) fn detect_azure_pipeline(
    repo: &Repository,
    branch: &str,
    local_head: &str,
) -> Option<PrStatus> {
    let repo_root = repo.current_worktree().root().ok()?;
    let org_url = get_azure_org_url(repo)?;
    let project = get_azure_project(repo)?;

    let branch_ref = format!("refs/heads/{}", branch);
    let output = match non_interactive_cmd("az")
        .args([
            "pipelines",
            "runs",
            "list",
            "--branch",
            &branch_ref,
            "--top",
            "1",
            "--project",
            &project,
            "--org",
            &org_url,
            "--output",
            "json",
        ])
        .current_dir(&repo_root)
        .run()
    {
        Ok(output) => output,
        Err(e) => {
            log::warn!(
                "az pipelines runs list failed to execute for branch {}: {}",
                branch,
                e
            );
            return None;
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if is_retriable_error(&stderr) {
            return Some(PrStatus::error());
        }
        return None;
    }

    let runs: Vec<AzPipelineRun> = parse_json(&output.stdout, "az pipelines runs list", branch)?;
    let run = runs.first()?;

    let ci_status = parse_azure_pipeline_status(run.status.as_deref(), run.result.as_deref());

    let is_stale = run
        .source_version
        .as_ref()
        .map(|sha| sha != local_head)
        .unwrap_or(true);

    // Construct web URL from org/project/build ID
    // (the `url` field in the API response is a REST API URL, not a browser link)
    let web_url = Some(format!(
        "{}/{}/_build/results?buildId={}",
        org_url, project, run.id
    ));

    Some(PrStatus {
        ci_status,
        source: CiSource::Branch,
        is_stale,
        url: web_url,
    })
}

/// Map Azure Pipelines run status/result to CiStatus.
fn parse_azure_pipeline_status(status: Option<&str>, result: Option<&str>) -> CiStatus {
    match status {
        Some("inProgress" | "notStarted") => CiStatus::Running,
        Some("completed") => match result {
            Some("succeeded") => CiStatus::Passed,
            Some("failed" | "canceled") => CiStatus::Failed,
            _ => CiStatus::NoCI,
        },
        Some("cancelling") => CiStatus::Failed,
        _ => CiStatus::NoCI,
    }
}

/// PR list entry from `az repos pr list --output json`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AzPrListEntry {
    pull_request_id: u32,
    #[serde(default)]
    merge_status: Option<String>,
    #[serde(default)]
    last_merge_source_commit: Option<AzCommitRef>,
    repository: AzPrRepository,
}

impl AzPrListEntry {
    /// Build a web URL for this PR from repository info.
    fn url_from_web(&self, repo: &Repository) -> Option<String> {
        let org = get_azure_org_url(repo)?;
        let org_name = org.strip_prefix("https://dev.azure.com/")?;
        Some(format!(
            "https://dev.azure.com/{}/{}/_git/{}/pullrequest/{}",
            org_name,
            self.repository.project.name,
            self.repository.name,
            self.pull_request_id,
        ))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AzCommitRef {
    #[serde(default)]
    commit_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AzPrRepository {
    name: String,
    project: AzPrProject,
}

#[derive(Debug, Deserialize)]
struct AzPrProject {
    name: String,
}

/// Pipeline run from `az pipelines runs list --output json`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AzPipelineRun {
    id: u32,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    source_version: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_azure_pipeline_status() {
        // Running states
        assert_eq!(
            parse_azure_pipeline_status(Some("inProgress"), None),
            CiStatus::Running
        );
        assert_eq!(
            parse_azure_pipeline_status(Some("notStarted"), None),
            CiStatus::Running
        );

        // Completed states
        assert_eq!(
            parse_azure_pipeline_status(Some("completed"), Some("succeeded")),
            CiStatus::Passed
        );
        assert_eq!(
            parse_azure_pipeline_status(Some("completed"), Some("failed")),
            CiStatus::Failed
        );
        assert_eq!(
            parse_azure_pipeline_status(Some("completed"), Some("canceled")),
            CiStatus::Failed
        );

        // Cancelling
        assert_eq!(
            parse_azure_pipeline_status(Some("cancelling"), None),
            CiStatus::Failed
        );

        // No status
        assert_eq!(parse_azure_pipeline_status(None, None), CiStatus::NoCI);
    }
}
