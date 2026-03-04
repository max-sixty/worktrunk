//! Azure DevOps PR provider.
//!
//! Implements `RemoteRefProvider` for Azure DevOps Pull Requests using the `az` CLI.

use std::io::ErrorKind;
use std::path::Path;

use anyhow::{Context, bail};
use serde::Deserialize;

use super::{PlatformData, RemoteRefInfo, RemoteRefProvider};
use crate::git::RefType;
use crate::git::error::GitError;
use crate::git::url::GitRemoteUrl;
use crate::shell_exec::Cmd;

/// Azure DevOps Pull Request provider.
#[derive(Debug, Clone, Copy)]
pub struct AzureDevOpsProvider;

impl RemoteRefProvider for AzureDevOpsProvider {
    fn ref_type(&self) -> RefType {
        RefType::Pr
    }

    fn fetch_info(&self, number: u32, repo_root: &Path) -> anyhow::Result<RemoteRefInfo> {
        fetch_pr_info(number, repo_root)
    }

    fn ref_path(&self, number: u32) -> String {
        format!("pull/{}/head", number)
    }
}

/// Raw JSON response from `az repos pr show --id <N> --output json`.
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

/// Detect Azure DevOps organization from git remote URLs.
fn detect_azure_org(repo_root: &Path) -> Option<String> {
    let output = Cmd::new("git")
        .args(["remote", "-v"])
        .current_dir(repo_root)
        .run()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let Some(url) = line.split_whitespace().nth(1) else {
            continue;
        };
        if let Some(parsed) = GitRemoteUrl::parse(url)
            && let Some(org) = parsed.azure_organization()
        {
            return Some(org.to_string());
        }
    }
    None
}

/// Fetch PR information from Azure DevOps using the `az` CLI.
fn fetch_pr_info(pr_number: u32, repo_root: &Path) -> anyhow::Result<RemoteRefInfo> {
    let mut args = vec![
        "repos".to_string(),
        "pr".to_string(),
        "show".to_string(),
        "--id".to_string(),
        pr_number.to_string(),
        "--output".to_string(),
        "json".to_string(),
    ];

    // Auto-detect org from git remote URL for zero-config experience
    if let Some(org) = detect_azure_org(repo_root) {
        args.extend(["--org".to_string(), format!("https://dev.azure.com/{}", org)]);
    }

    let output = match Cmd::new("az").args(&args).current_dir(repo_root).run() {
        Ok(output) => output,
        Err(e) => {
            if e.kind() == ErrorKind::NotFound {
                bail!(
                    "Azure CLI (az) not installed; install from https://aka.ms/installazurecliwindows \
                     or run: brew install azure-cli"
                );
            }
            return Err(anyhow::Error::from(e).context("Failed to run az repos pr show"));
        }
    };

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
                "Azure DevOps CLI extension not installed; run: az extension add --name azure-devops"
            );
        }

        return Err(GitError::CliApiError {
            ref_type: RefType::Pr,
            message: format!("az repos pr show failed for PR #{}", pr_number),
            stderr: if stderr.is_empty() {
                stdout_str
            } else {
                stderr
            },
        }
        .into());
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
            .or(fork.repository.remote_url.clone())
    });

    let project = response.repository.project.name.clone();
    let repo_name = response.repository.name.clone();

    // Extract org and host from web_url or fall back to defaults
    let (organization, host, pr_url) = if let Some(web_url) = &response.repository.web_url {
        // web_url format: https://dev.azure.com/{org}/{project}/_git/{repo}
        //            or:  https://{org}.visualstudio.com/{project}/_git/{repo}
        let parsed_host = web_url
            .strip_prefix("https://")
            .or_else(|| web_url.strip_prefix("http://"))
            .and_then(|s| s.split('/').next())
            .unwrap_or("dev.azure.com")
            .to_string();
        let org = web_url
            .strip_prefix("https://dev.azure.com/")
            .and_then(|s| s.split('/').next())
            .unwrap_or(&project)
            .to_string();
        let url = format!(
            "https://dev.azure.com/{}/{}/_git/{}/pullrequest/{}",
            org, project, repo_name, pr_number
        );
        (org, parsed_host, url)
    } else {
        let url = format!(
            "https://dev.azure.com/{}/{}/_git/{}/pullrequest/{}",
            project, project, repo_name, pr_number
        );
        (project.clone(), "dev.azure.com".to_string(), url)
    };

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
}
