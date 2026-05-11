//! Azure DevOps PR provider.
//!
//! Implements `RemoteRefProvider` for Azure DevOps Pull Requests using the `az` CLI.
//! Requires the `azure-devops` extension (`az extension add --name azure-devops`).

use anyhow::{Context, bail};
use serde::Deserialize;

use super::{CliApiRequest, PlatformData, RemoteRefInfo, RemoteRefProvider, cli_api_error};
use crate::git::url::GitRemoteUrl;
use crate::git::{RefType, Repository};

/// Azure DevOps Pull Request provider.
#[derive(Debug, Clone, Copy)]
pub struct AzureDevOpsProvider;

impl RemoteRefProvider for AzureDevOpsProvider {
    fn ref_type(&self) -> RefType {
        RefType::Pr
    }

    fn fetch_info(&self, number: u32, repo: &Repository) -> anyhow::Result<RemoteRefInfo> {
        fetch_pr_info(number, repo)
    }

    fn ref_path(&self, number: u32) -> String {
        format!("pull/{}/head", number)
    }
}

/// Construct an Azure DevOps remote URL for a repo.
///
/// Always emits the canonical `https://dev.azure.com/{org}/{project}/_git/{repo}`
/// form. The `host` argument is accepted for parity with the GitHub/GitLab helpers
/// but ignored — Azure DevOps SSH URLs require a per-org public key, and
/// `*.visualstudio.com` hosts redirect to `dev.azure.com` anyway, so emitting the
/// canonical HTTPS form works out of the box with both PAT and Azure CLI auth.
pub fn fork_remote_url(_host: &str, organization: &str, project: &str, repo: &str) -> String {
    format!(
        "https://dev.azure.com/{}/{}/_git/{}",
        organization, project, repo
    )
}

/// Raw JSON response from `az repos pr show --id <N>`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AzPrResponse {
    title: String,
    created_by: AzIdentity,
    status: String,
    #[serde(default)]
    is_draft: bool,
    source_ref_name: String,
    repository: AzRepository,
    #[serde(default)]
    fork_source: Option<AzForkRef>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AzIdentity {
    unique_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AzRepository {
    name: String,
    project: AzProject,
    #[serde(default)]
    web_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AzProject {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AzForkRef {
    repository: AzForkRepository,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AzForkRepository {
    #[serde(default)]
    remote_url: Option<String>,
    #[serde(default)]
    ssh_url: Option<String>,
}

/// Detect Azure DevOps organization from any Azure remote URL on this repo.
fn detect_azure_org(repo: &Repository) -> Option<String> {
    for (_, url) in repo.all_remote_urls() {
        if let Some(parsed) = GitRemoteUrl::parse(&url)
            && let Some(org) = parsed.azure_organization()
        {
            return Some(org.to_string());
        }
    }
    None
}

/// Parse `host` and `organization` out of an Azure DevOps web URL, falling back
/// to defaults derived from the response when the URL is missing or unusual.
fn parse_web_url(web_url: Option<&str>, fallback_org: &str) -> (String, String) {
    let Some(web_url) = web_url else {
        return ("dev.azure.com".to_string(), fallback_org.to_string());
    };
    let host = web_url
        .strip_prefix("https://")
        .or_else(|| web_url.strip_prefix("http://"))
        .and_then(|s| s.split('/').next())
        .unwrap_or("dev.azure.com")
        .to_string();
    let org = web_url
        .strip_prefix("https://dev.azure.com/")
        .and_then(|s| s.split('/').next())
        .unwrap_or(fallback_org)
        .to_string();
    (host, org)
}

fn fetch_pr_info(pr_number: u32, repo: &Repository) -> anyhow::Result<RemoteRefInfo> {
    let repo_root = repo.repo_path()?;
    let pr_id = pr_number.to_string();

    let mut args = vec![
        "repos",
        "pr",
        "show",
        "--id",
        pr_id.as_str(),
        "--output",
        "json",
    ];

    // Auto-detect organization from any Azure DevOps remote so contributors
    // don't have to pass `--org` explicitly.
    let org_url = detect_azure_org(repo).map(|org| format!("https://dev.azure.com/{}", org));
    if let Some(org_url) = &org_url {
        args.extend(["--org", org_url]);
    }

    let output = super::run_cli_api(CliApiRequest {
        tool: "az",
        args: &args,
        repo_root,
        prompt_env: ("AZURE_CORE_NO_COLOR", "true"),
        install_hint: "Azure CLI (az) not installed; install from https://aka.ms/installazurecli",
        run_context: "Failed to run az repos pr show",
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout_str = String::from_utf8_lossy(&output.stdout).trim().to_string();

        if stderr.contains("does not exist") || stdout_str.contains("does not exist") {
            bail!("PR #{} not found", pr_number);
        }
        if stderr.contains("login") || stderr.contains("authenticate") {
            bail!("Azure CLI not authenticated; run az login");
        }
        if stderr.contains("azure-devops") && stderr.contains("extension") {
            bail!(
                "Azure DevOps CLI extension not installed; \
                 run: az extension add --name azure-devops"
            );
        }

        return Err(cli_api_error(
            RefType::Pr,
            format!("az repos pr show failed for PR #{}", pr_number),
            &output,
        ));
    }

    let response: AzPrResponse = serde_json::from_slice(&output.stdout).with_context(|| {
        format!(
            "Failed to parse Azure DevOps API response for PR #{}. \
             This may indicate an az CLI version issue.",
            pr_number
        )
    })?;

    // Strip refs/heads/ prefix from branch names
    let source_branch = response
        .source_ref_name
        .strip_prefix("refs/heads/")
        .unwrap_or(&response.source_ref_name)
        .to_string();

    let is_cross_repo = response.fork_source.is_some();

    let fork_push_url = response.fork_source.as_ref().and_then(|fork| {
        fork.repository
            .ssh_url
            .clone()
            .or_else(|| fork.repository.remote_url.clone())
    });

    let project = response.repository.project.name.clone();
    let repo_name = response.repository.name.clone();

    let fallback_org = org_url
        .as_deref()
        .and_then(|u| u.strip_prefix("https://dev.azure.com/"))
        .unwrap_or(&project);
    let (host, organization) = parse_web_url(response.repository.web_url.as_deref(), fallback_org);

    let pr_url = format!(
        "https://dev.azure.com/{}/{}/_git/{}/pullrequest/{}",
        organization, project, repo_name, pr_number
    );

    Ok(RemoteRefInfo {
        ref_type: RefType::Pr,
        number: pr_number,
        title: response.title,
        author: response.created_by.unique_name,
        state: response.status,
        draft: response.is_draft,
        source_branch,
        is_cross_repo,
        url: pr_url,
        fork_push_url,
        platform_data: PlatformData::AzureDevOps {
            host,
            organization,
            project,
            repo_name,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ref_path() {
        let provider = AzureDevOpsProvider;
        assert_eq!(provider.ref_path(550), "pull/550/head");
        assert_eq!(provider.tracking_ref(550), "refs/pull/550/head");
    }

    #[test]
    fn test_ref_type() {
        let provider = AzureDevOpsProvider;
        assert_eq!(provider.ref_type(), RefType::Pr);
    }

    #[test]
    fn test_parse_web_url_dev_azure() {
        let (host, org) = parse_web_url(
            Some("https://dev.azure.com/myorg/myproject/_git/myrepo"),
            "fallback",
        );
        assert_eq!(host, "dev.azure.com");
        assert_eq!(org, "myorg");
    }

    #[test]
    fn test_parse_web_url_visualstudio() {
        let (host, org) = parse_web_url(
            Some("https://myorg.visualstudio.com/myproject/_git/myrepo"),
            "myproject",
        );
        assert_eq!(host, "myorg.visualstudio.com");
        // visualstudio.com URLs aren't parsed for org — caller uses fallback
        assert_eq!(org, "myproject");
    }

    #[test]
    fn test_parse_web_url_missing_falls_back() {
        let (host, org) = parse_web_url(None, "fallback-org");
        assert_eq!(host, "dev.azure.com");
        assert_eq!(org, "fallback-org");
    }

    #[test]
    fn test_fork_remote_url_format() {
        assert_eq!(
            fork_remote_url("dev.azure.com", "myorg", "myproject", "myrepo"),
            "https://dev.azure.com/myorg/myproject/_git/myrepo"
        );
    }
}
