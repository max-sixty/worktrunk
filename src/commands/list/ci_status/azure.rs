//! Azure DevOps CI status detection.
//!
//! Detects CI status from Azure DevOps PRs and pipeline runs using the `az` CLI.
//! Requires the `azure-devops` extension (`az extension add --name azure-devops`).

use serde::Deserialize;
use worktrunk::git::remote_ref::azure as az_url;
use worktrunk::git::{GitRemoteUrl, Repository};

use super::{
    CiBranchName, CiSource, CiStatus, PrRef, PrStatus, branch_remote_url, non_interactive_cmd,
    parse_json, retriable_pr_error,
};

/// Resolve the Azure DevOps context (host, org, project, `--org` URL) for this
/// branch's `az` invocations.
///
/// Walks the shared [`branch_remote_url`] chain first — so a remote-branch
/// row from `wt list --remotes --full` queries the right tenant in
/// multi-org setups. Falls back to scanning every configured remote for
/// any Azure URL when the resolved remote isn't Azure DevOps (e.g., a
/// branch tracks a non-Azure mirror but another remote is the real one).
/// Returns `None` if no remote points at Azure DevOps.
fn azure_context(repo: &Repository, branch: &CiBranchName) -> Option<AzureContext> {
    let try_url = |url: &str| -> Option<AzureContext> {
        let parsed = GitRemoteUrl::parse(url)?;
        if !parsed.is_azure_devops() {
            return None;
        }
        let host = parsed.host().to_string();
        let organization = parsed.azure_organization()?.to_string();
        let project = parsed.azure_project()?.to_string();
        let org_url = az_url::az_org_url(&host, &organization);
        Some(AzureContext {
            host,
            organization,
            project,
            org_url,
        })
    };

    if let Some(url) = branch_remote_url(repo, branch)
        && let Some(ctx) = try_url(&url)
    {
        return Some(ctx);
    }
    for (_, url) in repo.all_remote_urls() {
        if let Some(ctx) = try_url(&url) {
            return Some(ctx);
        }
    }
    None
}

struct AzureContext {
    host: String,
    organization: String,
    project: String,
    org_url: String,
}

/// Detect Azure DevOps PR CI status for a branch.
///
/// Uses `az repos pr list` to find an open PR for the branch.
pub(super) fn detect_azure_pr(
    repo: &Repository,
    branch: &CiBranchName,
    local_head: &str,
) -> Option<PrStatus> {
    let repo_root = repo.repo_path().ok()?;
    let ctx = azure_context(repo, branch)?;

    // `az repos pr list --source-branch` expects a full ref name.
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
            &ctx.project,
            "--org",
            &ctx.org_url,
            "--output",
            "json",
        ])
        .current_dir(repo_root)
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
        return retriable_pr_error(&output);
    }

    let pr_list: Vec<AzPrListEntry> =
        parse_json(&output.stdout, "az repos pr list", &branch.full_name)?;
    let pr = pr_list.first()?;

    // mergeStatus reflects merge feasibility, not pipeline result. We surface
    // conflicts and queued states; everything else shows as NoCI. The
    // pipelines fallback below never runs for an open PR — detect_ci returns
    // on the first Some — so pipeline pass/fail is not surfaced here.
    // TODO(azure-pr-pipeline): fetch the PR's pipeline status instead of NoCI.
    let ci_status = match pr.merge_status.as_deref() {
        Some("conflicts") => CiStatus::Conflicts,
        Some("queued") => CiStatus::Running,
        _ => CiStatus::NoCI,
    };

    let is_stale = pr
        .last_merge_source_commit
        .as_ref()
        .and_then(|c| c.commit_id.as_ref())
        .map(|sha| sha != local_head)
        .unwrap_or(true);

    let url = pr.url_for(&ctx);

    Some(PrStatus {
        ci_status,
        source: CiSource::PullRequest,
        is_stale,
        is_priming: false,
        url,
        number: Some(PrRef::pr(u64::from(pr.pull_request_id))),
        review_state: None,
    })
}

/// Detect Azure Pipelines status for a branch (fallback when no PR exists).
///
/// Uses `az pipelines runs list --branch <branch>` to get the most recent run.
/// Note: `--top 1` returns the most recently queued run, which may be a retry
/// from a different SHA than `local_head`; `is_stale` flags that case so the UI
/// can dim the indicator rather than reporting fresh status against stale data.
pub(super) fn detect_azure_pipeline(
    repo: &Repository,
    branch: &CiBranchName,
    local_head: &str,
) -> Option<PrStatus> {
    let repo_root = repo.repo_path().ok()?;
    let ctx = azure_context(repo, branch)?;

    let branch_ref = format!("refs/heads/{}", branch.name);
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
            &ctx.project,
            "--org",
            &ctx.org_url,
            "--output",
            "json",
        ])
        .current_dir(repo_root)
        .run()
    {
        Ok(output) => output,
        Err(e) => {
            log::warn!(
                "az pipelines runs list failed to execute for branch {}: {}",
                branch.full_name,
                e
            );
            return None;
        }
    };

    if !output.status.success() {
        return retriable_pr_error(&output);
    }

    let runs: Vec<AzPipelineRun> =
        parse_json(&output.stdout, "az pipelines runs list", &branch.full_name)?;
    let run = runs.first()?;

    let ci_status = parse_azure_pipeline_status(run.status.as_deref(), run.result.as_deref());

    let is_stale = run
        .source_version
        .as_ref()
        .map(|sha| sha != local_head)
        .unwrap_or(true);

    // The `url` field in the API response is a REST endpoint, not a browser URL —
    // construct the web URL from the host/org/project/build ID instead.
    let web_url = Some(az_url::build_web_url(
        &ctx.host,
        &ctx.organization,
        &ctx.project,
        run.id,
    ));

    Some(PrStatus {
        ci_status,
        source: CiSource::Branch,
        is_stale,
        is_priming: false,
        url: web_url,
        number: None,
        review_state: None,
    })
}

/// Map Azure Pipelines run status/result to [`CiStatus`].
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
    fn url_for(&self, ctx: &AzureContext) -> Option<String> {
        Some(az_url::pr_web_url(
            &ctx.host,
            &ctx.organization,
            &self.repository.project.name,
            &self.repository.name,
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
    use worktrunk::testing::TestRepo;

    /// `azure_context` first walks the branch's remote, then scans every
    /// configured remote for any Azure URL — covering the case where a
    /// branch tracks (or the primary remote is) something non-Azure but
    /// another remote is the real Azure DevOps mirror.
    #[test]
    fn test_azure_context_falls_back_to_all_remote_urls() {
        let test = TestRepo::with_initial_commit();
        // Primary remote isn't Azure — the branch-aware path returns None.
        test.run_git(&[
            "remote",
            "add",
            "origin",
            "https://github.com/owner/test-repo.git",
        ]);
        // Secondary Azure remote — the all_remote_urls scan must find this.
        test.run_git(&[
            "remote",
            "add",
            "azure",
            "https://dev.azure.com/myorg/myproject/_git/myrepo",
        ]);
        let repo = Repository::at(test.root_path()).unwrap();
        let branch = CiBranchName {
            full_name: "ghost-local".to_string(),
            remote: None,
            name: "ghost-local".to_string(),
        };

        let ctx = azure_context(&repo, &branch).expect("scan should find the azure remote");
        assert_eq!(ctx.organization, "myorg");
        assert_eq!(ctx.project, "myproject");
    }

    /// No Azure remote anywhere → `None`.
    #[test]
    fn test_azure_context_returns_none_without_azure_remote() {
        let test = TestRepo::with_initial_commit();
        test.run_git(&[
            "remote",
            "add",
            "origin",
            "https://github.com/owner/test-repo.git",
        ]);
        let repo = Repository::at(test.root_path()).unwrap();
        let branch = CiBranchName {
            full_name: "ghost-local".to_string(),
            remote: None,
            name: "ghost-local".to_string(),
        };
        assert!(azure_context(&repo, &branch).is_none());
    }

    #[test]
    fn test_parse_azure_pipeline_status() {
        assert_eq!(
            parse_azure_pipeline_status(Some("inProgress"), None),
            CiStatus::Running
        );
        assert_eq!(
            parse_azure_pipeline_status(Some("notStarted"), None),
            CiStatus::Running
        );
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
        assert_eq!(
            parse_azure_pipeline_status(Some("cancelling"), None),
            CiStatus::Failed
        );
        assert_eq!(parse_azure_pipeline_status(None, None), CiStatus::NoCI);
    }
}
