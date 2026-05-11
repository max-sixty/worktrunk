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

    fn platform_label(&self) -> &'static str {
        "azure-devops"
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
/// Emits `https://{host}/{org}/{project}/_git/{repo}` for `dev.azure.com` and
/// `https://{host}/{project}/_git/{repo}` for legacy `*.visualstudio.com` hosts
/// (where the org is in the hostname). Azure DevOps SSH URLs require per-org
/// public keys, so HTTPS — which works with both PAT and Azure CLI auth — is
/// the safe default.
pub fn fork_remote_url(host: &str, organization: &str, project: &str, repo: &str) -> String {
    if host.to_ascii_lowercase().ends_with(".visualstudio.com") {
        format!("https://{}/{}/_git/{}", host, project, repo)
    } else {
        format!(
            "https://{}/{}/{}/_git/{}",
            host, organization, project, repo
        )
    }
}

/// Construct the PR web URL for the user's actual host (handles `*.visualstudio.com`).
pub fn pr_web_url(host: &str, organization: &str, project: &str, repo: &str, pr: u32) -> String {
    if host.to_ascii_lowercase().ends_with(".visualstudio.com") {
        format!(
            "https://{}/{}/_git/{}/pullrequest/{}",
            host, project, repo, pr
        )
    } else {
        format!(
            "https://dev.azure.com/{}/{}/_git/{}/pullrequest/{}",
            organization, project, repo, pr
        )
    }
}

/// Construct the build-results web URL for the user's actual host.
pub fn build_web_url(host: &str, organization: &str, project: &str, build_id: u32) -> String {
    if host.to_ascii_lowercase().ends_with(".visualstudio.com") {
        format!(
            "https://{}/{}/_build/results?buildId={}",
            host, project, build_id
        )
    } else {
        format!(
            "https://dev.azure.com/{}/{}/_build/results?buildId={}",
            organization, project, build_id
        )
    }
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

/// Detect the Azure DevOps `(host, organization)` to use for API calls.
///
/// Prefers the primary remote (typically `origin` or whatever the user pushed
/// with) so fork workflows hit the right tenant. Falls back to the first
/// Azure remote found if the primary isn't Azure DevOps.
fn detect_azure_target(repo: &Repository) -> Option<(String, String)> {
    if let Ok(remote) = repo.primary_remote()
        && let Some(url) = repo.effective_remote_url(&remote)
        && let Some(parsed) = GitRemoteUrl::parse(&url)
        && let Some(org) = parsed.azure_organization()
    {
        return Some((parsed.host().to_string(), org.to_string()));
    }
    for (_, url) in repo.all_remote_urls() {
        if let Some(parsed) = GitRemoteUrl::parse(&url)
            && let Some(org) = parsed.azure_organization()
        {
            return Some((parsed.host().to_string(), org.to_string()));
        }
    }
    None
}

/// Build the `--org` URL for the `az` CLI from a host and organization.
///
/// `dev.azure.com` and `ssh.dev.azure.com` both map to the cloud `dev.azure.com`
/// API host. Legacy `*.visualstudio.com` hosts keep their hostname (the API
/// accepts both forms).
pub fn az_org_url(host: &str, organization: &str) -> String {
    let lower = host.to_ascii_lowercase();
    if lower.ends_with(".visualstudio.com") {
        format!("https://{}", host)
    } else {
        format!("https://dev.azure.com/{}", organization)
    }
}

/// Parse `(host, organization)` out of an Azure DevOps web URL.
///
/// Returns `None` if the URL is missing or unrecognised; callers fall back to
/// the org detected from local remotes. We refuse to invent values here — the
/// previous version's `unwrap_or(project_name)` produced wrong but plausible
/// identifiers that propagated into the constructed PR URL.
///
/// Two shapes are recognised:
/// - `https://dev.azure.com/{org}/{project}/_git/{repo}` → `("dev.azure.com", org)`
/// - `https://{org}.visualstudio.com/{project}/_git/{repo}` → `("{org}.visualstudio.com", org)`
fn parse_web_url(web_url: Option<&str>) -> Option<(String, String)> {
    let url = web_url?;
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let (host, path) = rest.split_once('/')?;
    let host_lower = host.to_ascii_lowercase();
    if host_lower == "dev.azure.com" {
        let org = path.split('/').next().filter(|s| !s.is_empty())?;
        Some((host.to_string(), org.to_string()))
    } else if host_lower.ends_with(".visualstudio.com") {
        let org = host.split('.').next().filter(|s| !s.is_empty())?;
        Some((host.to_string(), org.to_string()))
    } else {
        None
    }
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

    // Auto-detect organization from the primary Azure DevOps remote so
    // contributors don't have to pass `--org` explicitly.
    let target = detect_azure_target(repo);
    let org_url = target.as_ref().map(|(host, org)| az_org_url(host, org));
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
            bail!("Azure DevOps PR #{} not found", pr_number);
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

    // Prefer the API response's web_url; fall back to whatever we detected from
    // local remotes. Never invent the org from the project name — Azure orgs
    // and projects share a namespace, so a collision produces a URL that 404s
    // in a hard-to-debug way.
    let (host, organization) = parse_web_url(response.repository.web_url.as_deref())
        .or_else(|| target.clone())
        .with_context(|| {
            format!(
                "Could not determine Azure DevOps org/host for PR #{}: \
                 response had no web_url and no local Azure remote is configured.",
                pr_number
            )
        })?;

    let pr_url = pr_web_url(&host, &organization, &project, &repo_name, pr_number);

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
        let parsed = parse_web_url(Some("https://dev.azure.com/myorg/myproject/_git/myrepo"));
        assert_eq!(
            parsed,
            Some(("dev.azure.com".to_string(), "myorg".to_string()))
        );
    }

    #[test]
    fn test_parse_web_url_visualstudio() {
        // Legacy *.visualstudio.com URLs encode the org in the hostname.
        let parsed = parse_web_url(Some("https://myorg.visualstudio.com/myproject/_git/myrepo"));
        assert_eq!(
            parsed,
            Some(("myorg.visualstudio.com".to_string(), "myorg".to_string()))
        );
    }

    #[test]
    fn test_parse_web_url_missing_or_unknown() {
        assert_eq!(parse_web_url(None), None);
        assert_eq!(parse_web_url(Some("https://github.com/owner/repo")), None);
        assert_eq!(parse_web_url(Some("not-a-url")), None);
    }

    #[test]
    fn test_fork_remote_url_format() {
        // dev.azure.com gets the canonical {host}/{org}/{project}/_git/{repo} layout.
        assert_eq!(
            fork_remote_url("dev.azure.com", "myorg", "myproject", "myrepo"),
            "https://dev.azure.com/myorg/myproject/_git/myrepo"
        );
        // *.visualstudio.com URLs omit the org (it's already in the hostname).
        assert_eq!(
            fork_remote_url("myorg.visualstudio.com", "myorg", "myproject", "myrepo"),
            "https://myorg.visualstudio.com/myproject/_git/myrepo"
        );
    }

    #[test]
    fn test_pr_web_url_format() {
        assert_eq!(
            pr_web_url("dev.azure.com", "myorg", "myproject", "myrepo", 42),
            "https://dev.azure.com/myorg/myproject/_git/myrepo/pullrequest/42"
        );
        assert_eq!(
            pr_web_url("myorg.visualstudio.com", "myorg", "myproject", "myrepo", 42),
            "https://myorg.visualstudio.com/myproject/_git/myrepo/pullrequest/42"
        );
    }

    #[test]
    fn test_az_org_url_format() {
        assert_eq!(
            az_org_url("dev.azure.com", "myorg"),
            "https://dev.azure.com/myorg"
        );
        assert_eq!(
            az_org_url("myorg.visualstudio.com", "myorg"),
            "https://myorg.visualstudio.com"
        );
        // ssh.dev.azure.com is the API's cloud host — still routes through dev.azure.com.
        assert_eq!(
            az_org_url("ssh.dev.azure.com", "myorg"),
            "https://dev.azure.com/myorg"
        );
    }
}
