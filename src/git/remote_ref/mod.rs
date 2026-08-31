//! Unified PR/MR reference resolution.
//!
//! This module resolves GitHub PRs, Gitea PRs, GitLab MRs, and Azure DevOps PRs
//! to local branches. [`ForgeKind`] is the forge identity throughout; all
//! platforms follow the same workflow:
//!
//! 1. Parse `pr:<number>` or `mr:<number>` syntax
//! 2. Fetch metadata from the platform API
//! 3. Check if a local branch already tracks this ref
//! 4. Create/configure the branch if needed
//!
//! # Usage
//!
//! ```no_run
//! use worktrunk::git::Repository;
//! use worktrunk::git::{ForgeKind, remote_ref};
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let repo = Repository::at(".")?;
//! let info = remote_ref::fetch_info(ForgeKind::GitHub, 123, &repo)?;
//! println!("PR #{}: {}", info.number, info.title);
//! # Ok(())
//! # }
//! ```
//!
//! # Error Messages
//!
//! When a forge CLI reports a failure, the message the user sees is the CLI's
//! own, in a gutter under one line of our context — see `cli_api_error` below.
//! `gh` and `glab` both render an API failure as the API's own message plus the
//! status (`gh: Not Found (HTTP 404)`, `glab: 401 Unauthorized (HTTP 401)`);
//! `az` writes its own prose, which names the remedy where there is one. Either
//! way that line stays accurate across locale, CLI version, and API rewording in
//! a way our paraphrase of it would not.
//!
//! A backend writes a message of its own only where the CLI structurally cannot
//! report the condition:
//!
//! - **GitHub's 404** answers about an owner/repo that is *our* choice — from
//!   `gh repo set-default` or from a remote — so it often means the choice was
//!   wrong rather than that the PR is missing. `gh` reports the miss; only we
//!   know what we asked for and where the question came from.
//! - **Azure DevOps' missing extension** is a precondition, not a guess: the
//!   whole `az repos` command group ships in the `azure-devops` extension, and
//!   `az extension list` answers for it. `az` itself never names it.
//!
//! Everything else forwards. Reading more nicely than the CLI does not qualify:
//! "GitLab CLI not authenticated" restated `glab: 401 Unauthorized (HTTP 401)`
//! with the status dropped, and cost a `starts_with("401")` test against prose
//! to select it.
//!
//! Where a backend still classifies, it keys on structure and falls through to
//! forwarding when the structure isn't there: `gh` puts a `status` field in its
//! error body, while `glab` puts the status only inside the message text, so
//! there is nothing there to key on. Gitea is the one backend whose response
//! *shape* is the error channel at all, because `tea api` copies the body to
//! stdout and exits 0 whatever the status — see [`gitea`].
//!
//! # Platform-Specific Notes
//!
//! ## GitHub
//!
//! Uses `gh api repos/{owner}/{repo}/pulls/<number>` which returns head/base repo info.
//! For fork workflows, `gh repo set-default` controls which repo is queried.
//!
//! ## GitLab
//!
//! Uses `glab api projects/:id/merge_requests/<number>`. Fork MRs require additional
//! API calls to fetch source/target project URLs.
//!
//! ## Gitea (experimental)
//!
//! Uses `tea api repos/{owner}/{repo}/pulls/<number>`. Unlike `gh`, `tea`'s
//! `{owner}`/`{repo}` template expansion depends on local repo context, so the
//! backend resolves owner/repo from a matching Gitea remote and passes a
//! pre-expanded path.
//!
//! ## Azure DevOps
//!
//! Uses `az repos pr show --id <number> --output json`. Auto-detects the organisation
//! from configured Azure DevOps remotes. Requires the `azure-devops` extension
//! (`az extension add --name azure-devops`).

pub mod azure;
pub mod gitea;
pub mod github;
pub mod gitlab;
mod info;

pub use info::{PlatformData, RemoteRefInfo};

use std::io::ErrorKind;
use std::path::Path;
use std::process::Output;

use anyhow::{Context, bail};

use crate::git::ci_platform::{host_is_within, normalized_hostname};
use crate::git::error::GitError;
use crate::git::url::authority_host;
use crate::git::{ForgeKind, GitRepoInfo, GitRepoProvider, RefType, Repository};
use crate::shell_exec::Cmd;

/// Fetch PR/MR metadata through the forge's CLI.
pub fn fetch_info(
    forge: ForgeKind,
    number: u32,
    repo: &Repository,
) -> anyhow::Result<RemoteRefInfo> {
    match forge {
        ForgeKind::GitHub => github::fetch_pr_info(number, repo),
        ForgeKind::GitLab => gitlab::fetch_mr_info(number, repo),
        ForgeKind::Gitea => gitea::fetch_pr_info(number, repo),
        ForgeKind::AzureDevOps => azure::fetch_pr_info(number, repo),
    }
}

/// Get the fetch ref path for a forge PR/MR.
pub fn ref_path(forge: ForgeKind, number: u32) -> String {
    match forge {
        ForgeKind::GitLab => format!("merge-requests/{number}/head"),
        ForgeKind::GitHub | ForgeKind::Gitea | ForgeKind::AzureDevOps => {
            format!("pull/{number}/head")
        }
    }
}

/// Get the full tracking ref for a forge PR/MR.
pub fn tracking_ref(forge: ForgeKind, number: u32) -> String {
    format!("refs/{}", ref_path(forge, number))
}

pub(super) struct CliApiRequest<'a> {
    pub tool: &'a str,
    pub args: &'a [&'a str],
    pub repo_root: &'a Path,
    pub prompt_env: (&'a str, &'a str),
    pub install_hint: &'a str,
    pub run_context: &'a str,
}

pub(super) fn run_cli_api(request: CliApiRequest<'_>) -> anyhow::Result<Output> {
    match Cmd::new(request.tool)
        .args(request.args.iter().copied())
        .current_dir(request.repo_root)
        .env(request.prompt_env.0, request.prompt_env.1)
        .run()
    {
        Ok(output) => Ok(output),
        Err(error) => {
            if error.kind() == ErrorKind::NotFound {
                bail!("{}", request.install_hint);
            }
            Err(anyhow::Error::from(error).context(request.run_context.to_string()))
        }
    }
}

/// The CLI's own account of the failure: stderr when it wrote any, else stdout.
///
/// `gh` and `glab` write a formatted line to stderr (`gh: Not Found (HTTP 404)`)
/// and the raw API body to stdout, so preferring stderr picks the human form
/// over the JSON. The stdout fallback covers a CLI that writes only a body.
pub(super) fn cli_api_error_details(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.trim().is_empty() {
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    } else {
        stderr.trim().to_string()
    }
}

/// Wrap a failed forge-CLI invocation, rendering `message` above the CLI's own
/// output in a gutter.
///
/// Every backend's failure path ends here, and forwarding is the default:
/// `message` names the request we made ("gh api failed for PR #123") and the
/// gutter carries the CLI's verdict on it. A backend with something of its own
/// to say — the cases in the module docs — passes it as `message` rather than
/// bailing, so its words are added to the CLI's rather than substituted for
/// them.
pub(super) fn cli_api_error(ref_type: RefType, message: String, output: &Output) -> anyhow::Error {
    GitError::CliApiError {
        ref_type,
        message,
        stderr: cli_api_error_details(output),
    }
    .into()
}

/// Extract the host (e.g. `github.com`) from a PR/MR `html_url` returned by
/// the forge API. Both GitHub and Gitea responses use the same `https://host/...`
/// shape, so we share the parser.
pub(super) fn extract_host_from_html_url(html_url: &str) -> anyhow::Result<String> {
    html_url
        .strip_prefix("https://")
        .or_else(|| html_url.strip_prefix("http://"))
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .and_then(authority_host)
        .map(String::from)
        .with_context(|| format!("Failed to parse host from PR URL: {html_url}"))
}

pub(super) fn cli_config_value(tool: &str, key: &str) -> Option<String> {
    Cmd::new(tool)
        .args(["config", "get", key])
        .run()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Find the local remote that points to the base (target) project for a PR/MR.
///
/// Matches by owner/repo only (host is not required to match). This handles
/// SSH host aliases where the local hostname differs from the API hostname.
/// The suggested URL in the error respects each platform's configured git
/// protocol (SSH vs HTTPS).
pub fn find_remote(repo: &Repository, info: &RemoteRefInfo) -> Result<String, GitError> {
    // Azure DevOps URLs don't fit the host/owner/repo shape that `find_remote_for_repo`
    // assumes (the parser stores `{org}/{project}/_git` in owner). Use the dedicated
    // Azure matcher for that platform; GitHub, Gitea, and GitLab share the owner/repo match.
    let (matched, owner, repo_name) = match &info.platform_data {
        PlatformData::GitHub {
            base_owner,
            base_repo,
            ..
        }
        | PlatformData::Gitea {
            base_owner,
            base_repo,
            ..
        }
        | PlatformData::GitLab {
            base_owner,
            base_repo,
            ..
        } => (
            repo.find_remote_for_repo(None, base_owner, base_repo),
            base_owner.as_str(),
            base_repo.as_str(),
        ),
        PlatformData::AzureDevOps {
            organization,
            project,
            repo_name,
            ..
        } => (
            repo.find_remote_for_azure(organization, project, repo_name),
            organization.as_str(),
            repo_name.as_str(),
        ),
    };

    matched.ok_or_else(|| {
        let suggested_url = match &info.platform_data {
            PlatformData::GitHub {
                host,
                base_owner,
                base_repo,
                ..
            } => github::fork_remote_url(host, base_owner, base_repo),
            PlatformData::Gitea {
                host,
                base_owner,
                base_repo,
                ..
            } => gitea::fork_remote_url(host, base_owner, base_repo),
            PlatformData::GitLab {
                host,
                base_owner,
                base_repo,
                ..
            } => gitlab::fork_remote_url(host, base_owner, base_repo),
            PlatformData::AzureDevOps {
                host,
                organization,
                project,
                repo_name,
            } => azure::fork_remote_url(host, organization, project, repo_name),
        };
        GitError::NoRemoteForRepo {
            owner: owner.to_string(),
            repo: repo_name.to_string(),
            suggested_url,
        }
    })
}

/// A forge PR/MR web URL decomposed into its parts.
///
/// Detection is shape-based, not host-based: the URL must use `http(s)://`
/// and contain `/pull/N`, `/pulls/N`, `/-/merge_requests/N`, or
/// `/pullrequest/N` in its path. Trailing path segments (e.g. `/files`,
/// `/commits`), query strings, and fragments are ignored. Host is not
/// inspected, so self-hosted GitHub Enterprise / Gitea / GitLab instances
/// work without a hostname allow-list.
struct RefUrlParts<'a> {
    /// URL scheme: `"https"` or `"http"`.
    scheme: &'a str,
    /// Non-empty path segments after the scheme (host, owner, …, marker, N).
    segments: Vec<&'a str>,
    /// Index into `segments` of the marker segment (`pull` / `pulls` /
    /// `pullrequest` / `merge_requests`).
    marker_index: usize,
    /// `"pr"` for GitHub/Gitea/Azure, `"mr"` for GitLab.
    kind: &'static str,
    /// The PR/MR number.
    number: u32,
}

/// Shape-based parse of a forge PR/MR web URL across all four supported forges
/// (GitHub including Enterprise, GitLab, Gitea, Azure DevOps).
///
/// Shared by [`parse_ref_url`] (which formats the `pr:`/`mr:` shortcut) and
/// [`repo_info_from_ref_url_with_provider`] (which keeps the path up to the marker).
fn parse_ref_url_parts(input: &str) -> Option<RefUrlParts<'_>> {
    let trimmed = input.trim();
    let scheme_end = trimmed.find("://")?;
    let scheme = &trimmed[..scheme_end];
    if scheme != "https" && scheme != "http" {
        return None;
    }
    let rest = &trimmed[scheme_end + 3..];

    // Drop query/fragment so trailing `?foo=bar` / `#discussion_r...` don't
    // break the segment match.
    let path = rest.split(['?', '#']).next()?;
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    // Minimum shape: host / owner / repo / kind / N (5 segments). Rejects e.g.
    // `https://example.com/pull/1`, too shallow to be a real PR URL.
    if segments.len() < 5 {
        return None;
    }

    for (marker_index, pair) in segments.windows(2).enumerate() {
        let Ok(number) = pair[1].parse::<u32>() else {
            continue;
        };
        let kind = match pair[0] {
            // GitHub `pull`, Gitea `pulls`, Azure DevOps `pullrequest`.
            "pull" | "pulls" | "pullrequest" => "pr",
            // GitLab `merge_requests` (always preceded by `/-/`).
            "merge_requests" => "mr",
            _ => continue,
        };
        return Some(RefUrlParts {
            scheme,
            segments,
            marker_index,
            kind,
            number,
        });
    }

    None
}

/// Parse a forge PR/MR web URL into the equivalent `pr:N` / `mr:N` shortcut
/// string.
///
/// Callers substitute the returned shortcut for the original input, so URL
/// handling flows through the same `pr:` / `mr:` parsing path as a literal
/// shortcut, with no duplicate dispatch. Detection is shape-based, not
/// host-based, so self-hosted instances work without an allow-list (see
/// `parse_ref_url_parts`).
pub fn parse_ref_url(input: &str) -> Option<String> {
    let parts = parse_ref_url_parts(input)?;
    Some(format!("{}:{}", parts.kind, parts.number))
}

/// Derive repository metadata from a PR/MR URL with an optional configured
/// `[forge].platform` override.
///
/// The repository URL truncates the PR/MR path (`/pull/N`, `/pulls/N`,
/// `/pullrequest/N`, or `/-/merge_requests/N`). Provider and owner/name fields
/// are derived from the same shape. The URL names the **target** repository:
/// for a fork PR it is the upstream repo the PR was opened against, not the
/// contributor's fork.
pub fn repo_info_from_ref_url_with_provider(
    input: &str,
    provider_override: Option<&str>,
) -> Option<GitRepoInfo> {
    let parts = parse_ref_url_parts(input)?;

    // The repository path is everything before the marker segment. GitLab
    // places a `/-/` separator before `merge_requests`; drop the trailing `-`
    // so the repo URL doesn't end in it.
    let mut repo_segments = &parts.segments[..parts.marker_index];
    if repo_segments.last() == Some(&"-") {
        repo_segments = &repo_segments[..repo_segments.len() - 1];
    }
    // Need host + at least owner/repo (Azure: org/project/_git/repo).
    if repo_segments.len() < 3 {
        return None;
    }
    let host = authority_host(repo_segments[0])?.to_string();
    let url = format!(
        "{}://{}/{}",
        parts.scheme,
        host,
        repo_segments[1..].join("/")
    );
    let marker = parts.segments[parts.marker_index];
    let provider_override = provider_override.and_then(|value| value.parse::<ForgeKind>().ok());

    let provider = match marker {
        "pull" => {
            if provider_override == Some(ForgeKind::GitHub)
                || ForgeKind::from_host(&host) == Some(ForgeKind::GitHub)
            {
                GitRepoProvider::GitHub
            } else {
                GitRepoProvider::Unknown
            }
        }
        "pulls" => GitRepoProvider::Gitea,
        "merge_requests" => GitRepoProvider::GitLab,
        "pullrequest" => {
            if provider_override == Some(ForgeKind::AzureDevOps)
                || ForgeKind::from_host(&host) == Some(ForgeKind::AzureDevOps)
            {
                GitRepoProvider::AzureDevOps
            } else {
                GitRepoProvider::Unknown
            }
        }
        _ => GitRepoProvider::Unknown,
    };

    if provider == GitRepoProvider::AzureDevOps {
        let allow_generic_host = provider_override == Some(ForgeKind::AzureDevOps);
        if let Some((owner, project, name)) =
            azure_repo_segments(&host, &repo_segments[1..], allow_generic_host)
        {
            return Some(GitRepoInfo {
                url,
                provider,
                host,
                owner,
                name,
                project: Some(project),
                remote: None,
            });
        }

        let name = repo_segments.last()?.to_string();
        let owner = repo_segments[1..repo_segments.len() - 1].join("/");
        return Some(GitRepoInfo {
            url,
            provider: GitRepoProvider::Unknown,
            host,
            owner,
            name,
            project: None,
            remote: None,
        });
    }

    let name = repo_segments.last()?.to_string();
    let owner = repo_segments[1..repo_segments.len() - 1].join("/");
    Some(GitRepoInfo {
        url,
        provider,
        host,
        owner,
        name,
        project: None,
        remote: None,
    })
}

fn azure_repo_segments(
    host: &str,
    segments: &[&str],
    allow_generic_host: bool,
) -> Option<(String, String, String)> {
    let host_lower = normalized_hostname(host);
    if host_lower == "ssh.dev.azure.com" {
        if segments.len() >= 4 && segments.first() == Some(&"v3") {
            return Some((
                segments[1].to_string(),
                segments[2].to_string(),
                segments[3].to_string(),
            ));
        }
    } else if host_is_within(&host_lower, "dev.azure.com") {
        if segments.len() >= 4 && segments.get(2) == Some(&"_git") {
            return Some((
                segments[0].to_string(),
                segments[1].to_string(),
                segments[3].to_string(),
            ));
        }
    } else if host_is_within(&host_lower, "visualstudio.com") {
        if segments.len() >= 3 && segments.get(1) == Some(&"_git") {
            let organization = host.split('.').next()?.to_string();
            return Some((
                organization,
                segments[0].to_string(),
                segments[2].to_string(),
            ));
        }
    } else if allow_generic_host && segments.len() >= 4 && segments.get(2) == Some(&"_git") {
        return Some((
            segments[0].to_string(),
            segments[1].to_string(),
            segments[3].to_string(),
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_url_host_uses_the_network_host_after_userinfo() {
        assert_eq!(
            extract_host_from_html_url("https://github.com@attacker.example/owner/repo/pull/1")
                .unwrap(),
            "attacker.example"
        );
        assert_eq!(
            extract_host_from_html_url("http://user:token@github.com:8443/owner/repo/pull/1")
                .unwrap(),
            "github.com:8443"
        );
        assert!(extract_host_from_html_url("https://github.com@/owner/repo/pull/1").is_err());
    }

    #[test]
    fn tracking_refs_follow_forge_namespaces() {
        assert_eq!(tracking_ref(ForgeKind::GitHub, 123), "refs/pull/123/head");
        assert_eq!(tracking_ref(ForgeKind::Gitea, 7), "refs/pull/7/head");
        assert_eq!(
            tracking_ref(ForgeKind::AzureDevOps, 550),
            "refs/pull/550/head"
        );
        assert_eq!(
            tracking_ref(ForgeKind::GitLab, 42),
            "refs/merge-requests/42/head"
        );
    }

    #[test]
    fn parse_ref_url_github() {
        assert_eq!(
            parse_ref_url("https://github.com/owner/repo/pull/123").as_deref(),
            Some("pr:123")
        );
        // GitHub Enterprise host.
        assert_eq!(
            parse_ref_url("https://github.acme.com/team/repo/pull/9").as_deref(),
            Some("pr:9")
        );
        // Trailing segments (e.g. /files, /commits) and fragments.
        assert_eq!(
            parse_ref_url("https://github.com/owner/repo/pull/2895/files").as_deref(),
            Some("pr:2895")
        );
        assert_eq!(
            parse_ref_url("https://github.com/owner/repo/pull/77#discussion_r1").as_deref(),
            Some("pr:77")
        );
        // http:// is accepted too.
        assert_eq!(
            parse_ref_url("http://github.com/owner/repo/pull/1").as_deref(),
            Some("pr:1")
        );
    }

    #[test]
    fn parse_ref_url_gitlab() {
        assert_eq!(
            parse_ref_url("https://gitlab.com/group/repo/-/merge_requests/42").as_deref(),
            Some("mr:42")
        );
        // Nested subgroups.
        assert_eq!(
            parse_ref_url("https://gitlab.com/group/sub/repo/-/merge_requests/7").as_deref(),
            Some("mr:7")
        );
        // Self-hosted GitLab with trailing diff path.
        assert_eq!(
            parse_ref_url("https://gitlab.example.com/team/repo/-/merge_requests/12/diffs")
                .as_deref(),
            Some("mr:12")
        );
    }

    #[test]
    fn parse_ref_url_gitea() {
        // Gitea / Codeberg use /pulls/N rather than GitHub's /pull/N.
        assert_eq!(
            parse_ref_url("https://codeberg.org/owner/repo/pulls/55").as_deref(),
            Some("pr:55")
        );
        assert_eq!(
            parse_ref_url("https://gitea.example.com/team/repo/pulls/3").as_deref(),
            Some("pr:3")
        );
    }

    #[test]
    fn parse_ref_url_azure_devops() {
        assert_eq!(
            parse_ref_url("https://dev.azure.com/org/project/_git/repo/pullrequest/9").as_deref(),
            Some("pr:9")
        );
        // Legacy `*.visualstudio.com` host: org in the hostname, one fewer path
        // segment than `dev.azure.com`. Still resolves to the same shortcut.
        assert_eq!(
            parse_ref_url("https://myorg.visualstudio.com/myproject/_git/repo/pullrequest/9")
                .as_deref(),
            Some("pr:9")
        );
    }

    #[test]
    fn parse_ref_url_rejects_non_urls() {
        // Plain branch names — no protocol prefix.
        assert_eq!(parse_ref_url("pr:123"), None);
        assert_eq!(parse_ref_url("feature/pull/7"), None);
        assert_eq!(parse_ref_url("pull/123"), None);
        // Too shallow even with a protocol prefix.
        assert_eq!(parse_ref_url("https://example.com/pull/1"), None);
        // Wrong kind segment.
        assert_eq!(parse_ref_url("https://github.com/o/r/issues/5"), None);
        // Non-numeric tail (e.g. PR creation URL).
        assert_eq!(parse_ref_url("https://github.com/o/r/pull/new"), None);
        // Empty/whitespace.
        assert_eq!(parse_ref_url(""), None);
        assert_eq!(parse_ref_url("   "), None);
    }

    #[test]
    fn repo_info_url_per_forge() {
        let cases = [
            // GitHub, including a fork PR (target repo = the upstream owner).
            (
                "https://github.com/upstream/repo/pull/123",
                "https://github.com/upstream/repo",
            ),
            // Trailing segments and fragments are ignored.
            (
                "https://github.com/owner/repo/pull/2895/files",
                "https://github.com/owner/repo",
            ),
            (
                "https://github.com/owner/repo/pull/77#discussion_r1",
                "https://github.com/owner/repo",
            ),
            // GitHub Enterprise host.
            (
                "https://github.acme.com/team/repo/pull/9",
                "https://github.acme.com/team/repo",
            ),
            // GitLab drops the `/-/` separator; nested subgroups preserved.
            (
                "https://gitlab.com/group/sub/repo/-/merge_requests/7/diffs",
                "https://gitlab.com/group/sub/repo",
            ),
            // Gitea / Codeberg `/pulls/N`.
            (
                "https://codeberg.org/owner/repo/pulls/55",
                "https://codeberg.org/owner/repo",
            ),
            // Azure DevOps keeps the `_git` segment.
            (
                "https://dev.azure.com/org/project/_git/repo/pullrequest/9",
                "https://dev.azure.com/org/project/_git/repo",
            ),
            // http:// is preserved.
            (
                "http://github.com/owner/repo/pull/1",
                "http://github.com/owner/repo",
            ),
        ];
        for (input, expected) in cases {
            let info = repo_info_from_ref_url_with_provider(input, None)
                .expect("ref URL should produce repo info");
            assert_eq!(info.url, expected, "input: {input}");
        }
    }

    #[test]
    fn repo_info_rejects_non_pr_urls() {
        for input in [
            "https://github.com/owner/repo",   // Not a PR/MR link.
            "https://github.com/o/r/issues/5", // Not a PR/MR link.
            "https://example.com/pull/1",      // Too shallow.
            "pr:123",                          // Not a URL.
            "",
        ] {
            assert_eq!(repo_info_from_ref_url_with_provider(input, None), None);
        }
    }

    #[test]
    fn repo_info_from_ref_url_per_forge() {
        let cases = [
            (
                "https://github.com/owner/repo/pull/123",
                "https://github.com/owner/repo",
                GitRepoProvider::GitHub,
                "github.com",
                "owner",
                "repo",
                None,
            ),
            (
                "https://gitlab.com/group/sub/repo/-/merge_requests/7/diffs",
                "https://gitlab.com/group/sub/repo",
                GitRepoProvider::GitLab,
                "gitlab.com",
                "group/sub",
                "repo",
                None,
            ),
            (
                "https://gitea.example.com/owner/repo/pulls/55",
                "https://gitea.example.com/owner/repo",
                GitRepoProvider::Gitea,
                "gitea.example.com",
                "owner",
                "repo",
                None,
            ),
            (
                "https://dev.azure.com/org/project/_git/repo/pullrequest/9",
                "https://dev.azure.com/org/project/_git/repo",
                GitRepoProvider::AzureDevOps,
                "dev.azure.com",
                "org",
                "repo",
                Some("project"),
            ),
            (
                "https://ssh.dev.azure.com/v3/org/project/repo/pullrequest/9",
                "https://ssh.dev.azure.com/v3/org/project/repo",
                GitRepoProvider::AzureDevOps,
                "ssh.dev.azure.com",
                "org",
                "repo",
                Some("project"),
            ),
            (
                "https://org.visualstudio.com/project/_git/repo/pullrequest/9",
                "https://org.visualstudio.com/project/_git/repo",
                GitRepoProvider::AzureDevOps,
                "org.visualstudio.com",
                "org",
                "repo",
                Some("project"),
            ),
        ];

        for (input, url, provider, host, owner, name, project) in cases {
            let info = repo_info_from_ref_url_with_provider(input, None)
                .expect("ref URL should produce repo info");
            assert_eq!(info.url, url, "input: {input}");
            assert_eq!(info.provider, provider, "input: {input}");
            assert_eq!(info.host, host, "input: {input}");
            assert_eq!(info.owner, owner, "input: {input}");
            assert_eq!(info.name, name, "input: {input}");
            assert_eq!(info.project.as_deref(), project, "input: {input}");
        }
    }

    #[test]
    fn repo_info_from_ref_url_unknown_pull_host() {
        let info =
            repo_info_from_ref_url_with_provider("https://git.example.com/owner/repo/pull/1", None)
                .expect("shape is still parseable");
        assert_eq!(info.url, "https://git.example.com/owner/repo");
        assert_eq!(info.provider, GitRepoProvider::Unknown);
        assert_eq!(info.host, "git.example.com");
        assert_eq!(info.owner, "owner");
        assert_eq!(info.name, "repo");
        assert_eq!(info.project, None);
    }

    #[test]
    fn repo_info_from_ref_url_respects_host_boundaries_and_ports() {
        let github =
            repo_info_from_ref_url_with_provider("https://github.com:8443/owner/repo/pull/1", None)
                .expect("GitHub PR URL");
        assert_eq!(github.provider, GitRepoProvider::GitHub);

        let azure = repo_info_from_ref_url_with_provider(
            "https://dev.azure.com:8443/org/project/_git/repo/pullrequest/9",
            None,
        )
        .expect("Azure DevOps PR URL");
        assert_eq!(azure.provider, GitRepoProvider::AzureDevOps);
        assert_eq!(azure.owner, "org");
        assert_eq!(azure.project.as_deref(), Some("project"));

        let azure_with_forge_label = repo_info_from_ref_url_with_provider(
            "https://github.dev.azure.com/org/project/_git/repo/pullrequest/9",
            None,
        )
        .expect("Azure DevOps PR URL");
        assert_eq!(
            azure_with_forge_label.provider,
            GitRepoProvider::AzureDevOps
        );

        let hostile_userinfo = repo_info_from_ref_url_with_provider(
            "https://github.com@attacker.example/owner/repo/pull/1",
            None,
        )
        .expect("ref URL is structurally valid");
        assert_eq!(hostile_userinfo.provider, GitRepoProvider::Unknown);
        assert_eq!(hostile_userinfo.host, "attacker.example");
        assert_eq!(hostile_userinfo.url, "https://attacker.example/owner/repo");

        // A brand anywhere in the host classifies here too.
        let branded = repo_info_from_ref_url_with_provider(
            "https://github-mirror.example/owner/repo/pull/1",
            None,
        )
        .expect("ref URL is structurally valid");
        assert_eq!(branded.provider, GitRepoProvider::GitHub);

        // The Azure service domains stay bounded by suffix, and a host outside
        // them carries no brand to fall back on.
        for input in [
            "https://dev.azure.com.attacker.example/org/project/_git/repo/pullrequest/9",
            "https://evil-visualstudio.com/org/project/_git/repo/pullrequest/9",
        ] {
            let info = repo_info_from_ref_url_with_provider(input, None)
                .expect("ref URL is structurally valid");
            assert_eq!(info.provider, GitRepoProvider::Unknown, "{input}");
        }
    }

    #[test]
    fn repo_info_from_ref_url_uses_configured_github_for_opaque_host() {
        let input = "https://git.example.com/owner/repo/pull/1";
        let info = repo_info_from_ref_url_with_provider(input, Some("github"))
            .expect("shape is still parseable");
        assert_eq!(info.url, "https://git.example.com/owner/repo");
        assert_eq!(info.provider, GitRepoProvider::GitHub);
        assert_eq!(info.host, "git.example.com");
        assert_eq!(info.owner, "owner");
        assert_eq!(info.name, "repo");
        assert_eq!(info.project, None);
    }

    #[test]
    fn repo_info_from_ref_url_uses_configured_azure_for_opaque_host() {
        let input = "https://git.example.com/org/project/_git/repo/pullrequest/9";
        let info = repo_info_from_ref_url_with_provider(input, Some("azure-devops"))
            .expect("shape is still parseable");
        assert_eq!(info.url, "https://git.example.com/org/project/_git/repo");
        assert_eq!(info.provider, GitRepoProvider::AzureDevOps);
        assert_eq!(info.host, "git.example.com");
        assert_eq!(info.owner, "org");
        assert_eq!(info.name, "repo");
        assert_eq!(info.project.as_deref(), Some("project"));
    }

    #[test]
    fn repo_info_from_ref_url_malformed_azure_is_unknown() {
        let input = "https://dev.azure.com/org/repo/pullrequest/9";
        let info =
            repo_info_from_ref_url_with_provider(input, None).expect("shape is still parseable");
        assert_eq!(info.url, "https://dev.azure.com/org/repo");
        assert_eq!(info.provider, GitRepoProvider::Unknown);
        assert_eq!(info.host, "dev.azure.com");
        assert_eq!(info.owner, "org");
        assert_eq!(info.name, "repo");
        assert_eq!(info.project, None);
    }
}
