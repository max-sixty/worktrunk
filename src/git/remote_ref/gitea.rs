//! Gitea PR provider.
//!
//! Implements `RemoteRefProvider` for Gitea Pull Requests using the `tea` CLI.
//!
//! ## API path resolution
//!
//! `tea api <path>` does support `{owner}` and `{repo}` placeholders, but their
//! values come from `tea`'s own repo-context resolver, which depends on the
//! local git remote being a Gitea-accessible URL and on the user having set up
//! `tea login add` first. We resolve owner/repo from the primary remote URL
//! ourselves and pass an already-expanded path so the call works regardless of
//! how `tea` resolves its own context.

use anyhow::{Context, bail};
use serde::Deserialize;

use super::{
    CliApiRequest, PlatformData, RemoteRefInfo, RemoteRefProvider, cli_api_error,
    extract_host_from_html_url, run_cli_api,
};
use crate::git::{RefType, Repository};

/// Gitea Pull Request provider.
#[derive(Debug, Clone, Copy)]
pub struct GiteaProvider;

impl RemoteRefProvider for GiteaProvider {
    fn ref_type(&self) -> RefType {
        RefType::Pr
    }

    fn platform_label(&self) -> &'static str {
        "gitea"
    }

    fn fetch_info(&self, number: u32, repo: &Repository) -> anyhow::Result<RemoteRefInfo> {
        fetch_pr_info(number, repo)
    }

    fn ref_path(&self, number: u32) -> String {
        format!("pull/{}/head", number)
    }
}

/// Raw JSON response from `tea api repos/{owner}/{repo}/pulls/{number}`.
#[derive(Debug, Deserialize)]
struct TeaApiPrResponse {
    title: String,
    user: TeaUser,
    state: String,
    #[serde(default)]
    draft: bool,
    head: TeaPrRef,
    base: TeaPrRef,
    html_url: String,
}

/// Error response from Gitea API.
#[derive(Debug, Deserialize)]
struct TeaApiErrorResponse {
    #[serde(default)]
    message: String,
}

#[derive(Debug, Deserialize)]
struct TeaUser {
    login: String,
}

#[derive(Debug, Deserialize)]
struct TeaPrRef {
    #[serde(default)]
    label: String,
    #[serde(rename = "ref")]
    #[serde(default)]
    ref_name: String,
    repo: Option<TeaPrRepo>,
}

#[derive(Debug, Deserialize)]
struct TeaPrRepo {
    name: String,
    owner: TeaOwner,
}

#[derive(Debug, Deserialize)]
struct TeaOwner {
    login: String,
}

/// Fetch PR information from Gitea using the `tea` CLI.
fn fetch_pr_info(pr_number: u32, repo: &Repository) -> anyhow::Result<RemoteRefInfo> {
    let repo_root = repo.repo_path()?;

    // Resolve owner/repo from the Gitea remote — which may be non-primary in
    // a mixed-remote repo — so we pass a fully expanded path to `tea api`.
    // See module docstring for the raw-URL rationale.
    let parsed = repo
        .forge_remote_parsed_url(|u| u.is_gitea())
        .ok_or_else(|| anyhow::anyhow!("No Gitea remote configured"))?;

    let api_path = format!(
        "repos/{}/{}/pulls/{}",
        parsed.owner(),
        parsed.repo(),
        pr_number,
    );

    let output = run_cli_api(CliApiRequest {
        tool: "tea",
        args: &["api", &api_path],
        repo_root,
        // tea reads no prompt-disable env var; pass a no-op key/value so the
        // shared helper has something to set without inventing a fake var.
        prompt_env: ("TEA_NO_PROMPT", "1"),
        install_hint: "Gitea CLI (tea) not installed; install from https://gitea.com/gitea/tea",
        run_context: "Failed to run tea api",
    })?;

    if !output.status.success() {
        if let Ok(error_response) = serde_json::from_slice::<TeaApiErrorResponse>(&output.stdout) {
            let message_lower = error_response.message.to_ascii_lowercase();
            if message_lower.contains("404") || message_lower.contains("not found") {
                bail!(
                    "Gitea PR #{} not found on {}/{}",
                    pr_number,
                    parsed.owner(),
                    parsed.repo()
                );
            }
            if message_lower.contains("401") || message_lower.contains("unauthorized") {
                bail!("Gitea CLI not authenticated; run tea login add");
            }
            if message_lower.contains("403") || message_lower.contains("forbidden") {
                bail!("Gitea API access forbidden for PR #{}", pr_number);
            }
        }

        return Err(cli_api_error(
            RefType::Pr,
            format!("tea api failed for PR #{}", pr_number),
            &output,
        ));
    }

    let response: TeaApiPrResponse = serde_json::from_slice(&output.stdout).with_context(|| {
        format!(
            "Failed to parse Gitea API response for PR #{}. \
             This may indicate a Gitea API change.",
            pr_number
        )
    })?;

    // Check head.repo before extract_source_branch so deleted-source PRs hit
    // the specific "source repository was deleted" message instead of falling
    // back to the generic "no source branch" path.
    let base_repo = response.base.repo.context(
        "Gitea PR base repository is null; this is unexpected and may indicate a Gitea API issue",
    )?;

    let TeaPrRef {
        label: head_label,
        ref_name: head_ref_name,
        repo: head_repo_opt,
    } = response.head;

    let head_repo = head_repo_opt.ok_or_else(|| {
        anyhow::anyhow!(
            "Gitea PR #{} source repository was deleted. \
             The fork that this PR was opened from no longer exists, \
             so the branch cannot be checked out.",
            pr_number
        )
    })?;

    let source_branch =
        extract_source_branch_from_parts(&head_label, &head_ref_name).ok_or_else(|| {
            anyhow::anyhow!(
                "Gitea PR #{} has no usable source branch — head.label/head.ref \
                 carry placeholders, so the PR may be in an invalid state",
                pr_number
            )
        })?;

    let is_cross_repo = !base_repo
        .owner
        .login
        .eq_ignore_ascii_case(&head_repo.owner.login)
        || !base_repo.name.eq_ignore_ascii_case(&head_repo.name);

    let host = extract_host_from_html_url(&response.html_url)?;

    let fork_push_url =
        is_cross_repo.then(|| fork_remote_url(&host, &head_repo.owner.login, &head_repo.name));

    Ok(RemoteRefInfo {
        ref_type: RefType::Pr,
        number: pr_number,
        title: response.title,
        author: response.user.login,
        state: response.state,
        draft: response.draft,
        source_branch,
        is_cross_repo,
        url: response.html_url,
        fork_push_url,
        platform_data: PlatformData::Gitea {
            host,
            head_owner: head_repo.owner.login,
            head_repo: head_repo.name,
            base_owner: base_repo.owner.login,
            base_repo: base_repo.name,
        },
    })
}

/// Extract the source branch name from a PR's head ref/label.
///
/// Prefers `label` (Gitea returns `owner:branch` for forks, `branch` otherwise).
/// Falls back to `ref`, which Gitea returns as the bare branch name (e.g.
/// `feature-auth`); the `refs/heads/` strip handles Gitea instances that
/// happen to return a fully-qualified ref.
///
/// When `head.repo` is null Gitea may still emit placeholder strings here
/// (`label = "unknown repository"`, `ref = "refs/pull/<n>/head"`). `fetch_pr_info`
/// checks `head.repo` before calling us, so by the time this runs we expect a
/// real branch name; placeholders that slip through return None and bail.
fn extract_source_branch_from_parts(label: &str, ref_name: &str) -> Option<String> {
    if !label.is_empty() {
        let candidate = label
            .split_once(':')
            .map(|(_, b)| b)
            .unwrap_or(label)
            .trim();
        if is_real_branch_name(candidate) {
            return Some(candidate.to_string());
        }
    }

    let candidate = ref_name
        .strip_prefix("refs/heads/")
        .unwrap_or(ref_name)
        .trim();
    is_real_branch_name(candidate).then(|| candidate.to_string())
}

/// A branch name candidate is real when it's non-empty, has no whitespace
/// (placeholders like `"unknown repository"` carry a space), and isn't a
/// PR-tracking ref like `refs/pull/<n>/head` or `pulls/<n>/head`.
fn is_real_branch_name(s: &str) -> bool {
    !s.is_empty()
        && !s.contains(char::is_whitespace)
        && !s.starts_with("refs/")
        && !s.starts_with("pulls/")
        && !s.starts_with("pull/")
}

/// Construct the remote URL for a Gitea repository.
pub fn fork_remote_url(host: &str, owner: &str, repo: &str) -> String {
    format!("https://{}/{}/{}.git", host, owner, repo)
}

/// Whether `tea` has a login configured for `host`.
///
/// Used by the switch dispatcher to decide which provider to try when the
/// remote URL doesn't unambiguously identify the forge. Reads tea's config
/// file directly — `$XDG_CONFIG_HOME/tea/config.yml` (default
/// `~/.config/tea/config.yml`) with legacy fallback `~/.tea/tea.yml` — and
/// returns true if any `logins[].url` parses to the same host. Pure local
/// I/O; never invokes `tea` (which can trigger an OAuth refresh on lookup).
pub fn is_authed_for(host: &str) -> bool {
    read_tea_config().is_some_and(|content| config_has_login_for(&content, host))
}

/// Pure parser: scan tea's `config.yml` content for a `logins[].url` whose
/// host matches `target`. Extracted from `is_authed_for` so the YAML-shaped
/// matching can be unit-tested without touching the filesystem or env vars.
fn config_has_login_for(content: &str, target: &str) -> bool {
    content.lines().any(|line| {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("url:") else {
            return false;
        };
        let value = rest.trim().trim_matches(|c: char| c == '"' || c == '\'');
        let Some(without_scheme) = value
            .strip_prefix("https://")
            .or_else(|| value.strip_prefix("http://"))
        else {
            return false;
        };
        let host = without_scheme.split(['/', '?', '#']).next().unwrap_or("");
        host.eq_ignore_ascii_case(target)
    })
}

/// Whether `tea` has *any* login configured (host-agnostic).
///
/// Used by `wt config show` diagnostics to report Gitea auth status when the
/// caller has no specific host in hand. Like [`is_authed_for`], reads tea's
/// config file directly rather than invoking `tea` (which can trigger an OAuth
/// refresh on lookup).
pub fn has_any_login() -> bool {
    read_tea_config().is_some_and(|content| content_has_any_login(&content))
}

/// Pure parser: true if any line is a `url:` entry carrying an http(s) URL.
/// Mirrors the line shape `config_has_login_for` matches, minus the host check.
fn content_has_any_login(content: &str) -> bool {
    content.lines().any(|line| {
        let Some(rest) = line.trim_start().strip_prefix("url:") else {
            return false;
        };
        let value = rest.trim().trim_matches(|c: char| c == '"' || c == '\'');
        value.starts_with("https://") || value.starts_with("http://")
    })
}

/// Read tea's config.yml, honoring `$XDG_CONFIG_HOME` and the legacy
/// `~/.tea/tea.yml` fallback. Returns None if neither file is readable.
fn read_tea_config() -> Option<String> {
    let xdg = std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from);
    let home = dirs::home_dir();

    let primary = xdg
        .clone()
        .or_else(|| home.as_ref().map(|h| h.join(".config")))
        .map(|base| base.join("tea").join("config.yml"));
    if let Some(path) = primary
        && let Ok(content) = std::fs::read_to_string(&path)
    {
        return Some(content);
    }

    let legacy = home.map(|h| h.join(".tea").join("tea.yml"));
    if let Some(path) = legacy
        && let Ok(content) = std::fs::read_to_string(&path)
    {
        return Some(content);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ref_path() {
        let provider = GiteaProvider;
        assert_eq!(provider.ref_path(7), "pull/7/head");
        assert_eq!(provider.tracking_ref(7), "refs/pull/7/head");
    }

    #[test]
    fn test_ref_type() {
        let provider = GiteaProvider;
        assert_eq!(provider.ref_type(), RefType::Pr);
    }

    #[test]
    fn test_extract_source_branch_prefers_label() {
        let head = TeaPrRef {
            label: "alice:feature-auth".to_string(),
            ref_name: "refs/pull/42/head".to_string(),
            repo: None,
        };
        assert_eq!(
            extract_source_branch_from_parts(&head.label, &head.ref_name),
            Some("feature-auth".to_string())
        );
    }

    #[test]
    fn test_extract_source_branch_from_plain_label() {
        let head = TeaPrRef {
            label: "feature-auth".to_string(),
            ref_name: "refs/pull/42/head".to_string(),
            repo: None,
        };
        assert_eq!(
            extract_source_branch_from_parts(&head.label, &head.ref_name),
            Some("feature-auth".to_string())
        );
    }

    #[test]
    fn test_extract_source_branch_fallback_to_bare_ref() {
        // Gitea returns just the branch name in head.ref
        let head = TeaPrRef {
            label: "".to_string(),
            ref_name: "feature-auth".to_string(),
            repo: None,
        };
        assert_eq!(
            extract_source_branch_from_parts(&head.label, &head.ref_name),
            Some("feature-auth".to_string())
        );
    }

    #[test]
    fn test_extract_source_branch_fallback_strips_refs_heads() {
        let head = TeaPrRef {
            label: "".to_string(),
            ref_name: "refs/heads/feature-auth".to_string(),
            repo: None,
        };
        assert_eq!(
            extract_source_branch_from_parts(&head.label, &head.ref_name),
            Some("feature-auth".to_string())
        );
    }

    #[test]
    fn test_extract_source_branch_label_with_empty_branch_falls_through() {
        // Label "owner:" → split_once gives ("", ""); after trim it's empty,
        // so the function falls through to the ref-name branch.
        let head = TeaPrRef {
            label: "owner:".to_string(),
            ref_name: "feature-auth".to_string(),
            repo: None,
        };
        assert_eq!(
            extract_source_branch_from_parts(&head.label, &head.ref_name),
            Some("feature-auth".to_string())
        );
    }

    #[test]
    fn test_extract_source_branch_empty_after_strip_returns_none() {
        // Bare "refs/heads/" strips to empty — no branch name available.
        let head = TeaPrRef {
            label: "".to_string(),
            ref_name: "refs/heads/".to_string(),
            repo: None,
        };
        assert_eq!(
            extract_source_branch_from_parts(&head.label, &head.ref_name),
            None
        );
    }

    #[test]
    fn test_extract_source_branch_empty_ref_returns_none() {
        let head = TeaPrRef {
            label: "".to_string(),
            ref_name: "".to_string(),
            repo: None,
        };
        assert_eq!(
            extract_source_branch_from_parts(&head.label, &head.ref_name),
            None
        );
    }

    #[test]
    fn test_extract_source_branch_skips_deleted_branch_ref() {
        // Gitea uses "pulls/<idx>/head" when the source branch is deleted —
        // not a usable branch name.
        let head = TeaPrRef {
            label: "".to_string(),
            ref_name: "pulls/42/head".to_string(),
            repo: None,
        };
        assert_eq!(
            extract_source_branch_from_parts(&head.label, &head.ref_name),
            None
        );
    }

    #[test]
    fn test_extract_source_branch_rejects_placeholders() {
        // Deleted-source PRs: Gitea returns "unknown repository" as the label
        // (contains a space, not a real branch name) and "refs/pull/<n>/head"
        // as the ref (a tracking ref, not a branch). Both must be rejected so
        // fetch_pr_info bails with the deleted-source error rather than
        // proceeding to fetch an invalid branch.
        let head = TeaPrRef {
            label: "unknown repository".to_string(),
            ref_name: "refs/pull/42/head".to_string(),
            repo: None,
        };
        assert_eq!(
            extract_source_branch_from_parts(&head.label, &head.ref_name),
            None
        );

        // Same but with the bare `pull/<n>/head` form some Gitea versions emit.
        let head = TeaPrRef {
            label: "".to_string(),
            ref_name: "pull/42/head".to_string(),
            repo: None,
        };
        assert_eq!(
            extract_source_branch_from_parts(&head.label, &head.ref_name),
            None
        );

        // A bare `refs/pull/...` in the label (no `:` separator) must also fail.
        let head = TeaPrRef {
            label: "refs/pull/42/head".to_string(),
            ref_name: "".to_string(),
            repo: None,
        };
        assert_eq!(
            extract_source_branch_from_parts(&head.label, &head.ref_name),
            None
        );
    }

    #[test]
    fn test_config_has_login_for_matches_known_hosts() {
        // tea writes one entry per `tea login add`. Match by host extracted
        // from the URL — case-insensitive, scheme-agnostic, ignores trailing
        // path/query.
        let yaml = r#"logins:
  - name: gitea-com
    url: https://gitea.com
    default: true
  - name: selfhosted
    url: "https://forge.example.com/"
  - name: with-path
    url: http://other.test/api/v1
"#;
        assert!(config_has_login_for(yaml, "gitea.com"));
        assert!(config_has_login_for(yaml, "GITEA.COM"));
        assert!(config_has_login_for(yaml, "forge.example.com"));
        assert!(config_has_login_for(yaml, "other.test"));
        assert!(!config_has_login_for(yaml, "not-configured.test"));
        // Empty config has no logins.
        assert!(!config_has_login_for("", "gitea.com"));
        // Stray `url:` outside a logins entry must not match — but the parser
        // is line-based and intentionally permissive; document the trade-off
        // by asserting the tea-shaped scheme-prefixed form is required.
        assert!(!config_has_login_for("url: gitea.com\n", "gitea.com"));
    }

    #[test]
    fn test_content_has_any_login() {
        let yaml = r#"logins:
  - name: gitea-com
    url: https://gitea.com
    default: true
"#;
        assert!(content_has_any_login(yaml));
        assert!(content_has_any_login(
            "    url: \"http://forge.example.com/\"\n"
        ));
        // No logins / no scheme-prefixed url.
        assert!(!content_has_any_login(""));
        assert!(!content_has_any_login("logins: []\n"));
        assert!(!content_has_any_login("url: gitea.com\n"));
    }
}
