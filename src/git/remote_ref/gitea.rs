//! Gitea PR backend using the `tea` CLI. It also hosts the `tea`-facing helpers
//! other modules share: [`api_status`] (how
//! every caller separates a failed request from a resource, and decides whether
//! a retry could help), [`is_authed_for`], and [`has_any_login`] — read by the
//! switch dispatcher and the CI-status backend, so a change to one of them is
//! not local.
//!
//! ## Reading the HTTP status
//!
//! `tea api` copies the response body to stdout and exits 0 whatever the status,
//! so the exit code answers only whether `tea` itself ran. `--include` adds the
//! status line and response headers on stderr, and that line is where every
//! caller here reads the status from. Both `tea api` call sites pass the flag.
//!
//! The flag shipped with the `api` subcommand itself in tea v0.12.0 and is
//! unchanged since, so every `tea` that can make this call accepts it — a `tea`
//! old enough to lack `--include` has no `api` subcommand to send it to.
//!
//! It writes the whole header block, not just the status line, so the captured
//! stderr now holds whatever the server sends back — a `Set-Cookie` among it,
//! on a Gitea that issues one. That reaches disk only under `-vv`, which logs
//! every subprocess's output to `subprocess.log`; there is no narrower flag to
//! ask for, and the status is not available any other way.
//!
//! ## API path resolution
//!
//! `tea api <path>` does support `{owner}` and `{repo}` placeholders, but their
//! values come from `tea`'s own repo-context resolver, which depends on the
//! local git remote being a Gitea-accessible URL and on the user having set up
//! `tea login add` first. We resolve owner/repo from a matching Gitea remote
//! and pass an already-expanded path so the call works regardless of how `tea`
//! resolves its own context.

use anyhow::{Context, bail};
use serde::Deserialize;

use super::{
    CliApiRequest, PlatformData, RemoteRefInfo, cli_api_error, extract_host_from_html_url,
    run_cli_api,
};
use crate::git::{ForgeKind, Repository};

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

/// Gitea's `APIError` body, which the API returns in place of the resource
/// whenever the request fails.
#[derive(Debug, Deserialize)]
struct TeaApiErrorResponse {
    message: String,
}

/// The HTTP status of a `tea api --include` response, read from the status
/// line `tea` writes to stderr (`HTTP/1.1 404 Not Found`).
///
/// `None` means no such line arrived. From a `tea` that exited 0 that is
/// structurally impossible — `--include` prints the moment the response does,
/// before the body — so callers treat it as a failed request rather than
/// reading stdout as the resource.
///
/// Scans for the line rather than taking the first, so anything `tea` writes
/// to stderr ahead of it can't hide it.
pub fn api_status(stderr: &[u8]) -> Option<u16> {
    String::from_utf8_lossy(stderr)
        .lines()
        .find_map(|line| line.strip_prefix("HTTP/"))
        .and_then(|rest| rest.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
}

/// Gitea's own account of a failed request, from the `APIError` body it returns
/// in place of the resource.
///
/// Read only after [`api_status`] has said the request failed, so this decides
/// nothing. It reports three states, and [`fetch_pr_info`] — which does have
/// prose to write — says something different for each:
///
/// - `Some(message)` — Gitea said what went wrong.
/// - `Some("")` — Gitea's envelope with nothing in it. Production blanks a 5xx
///   message unless the token belongs to an admin, so the body arrives as
///   `{"message":"","url":"…/api/swagger"}`. There is no more to report, and
///   that is worth saying.
/// - `None` — not an `APIError` at all, so the body didn't come from Gitea's
///   API layer: a reverse proxy's HTML error page, or a shape Gitea doesn't
///   send today.
///
/// Local to this module: the CI-status backend reads only [`api_status`], since
/// a CI cell has no room for the text and the status already decides what the
/// cell shows.
fn api_error_message(stdout: &[u8]) -> Option<String> {
    serde_json::from_slice::<TeaApiErrorResponse>(stdout)
        .ok()
        .map(|error| error.message.trim().to_string())
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
pub(super) fn fetch_pr_info(pr_number: u32, repo: &Repository) -> anyhow::Result<RemoteRefInfo> {
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
        args: &["api", "--include", &api_path],
        repo_root,
        // tea reads no prompt-disable env var; pass a no-op key/value so the
        // shared helper has something to set without inventing a fake var.
        prompt_env: ("TEA_NO_PROMPT", "1"),
        install_hint: "Gitea CLI (tea) not installed; install from https://gitea.com/gitea/tea",
        run_context: "Failed to run tea api",
    })?;

    // `tea api` exits 0 for every HTTP response, so a non-zero exit means `tea`
    // itself failed (no login configured, unresolvable endpoint, transport
    // error) and its own stderr names which. It also means no status line, so
    // this branch comes first.
    if !output.status.success() {
        return Err(cli_api_error(
            ForgeKind::Gitea.ref_type(),
            format!("tea api failed for PR #{}", pr_number),
            &output,
        ));
    }

    // A 404, 401, 403, or 500 therefore arrives here as a successful spawn
    // carrying Gitea's `APIError` body instead of the PR, and the status line
    // from `--include` is what says so.
    let status = api_status(&output.stderr).with_context(|| {
        format!(
            "tea api --include wrote no HTTP status line for PR #{pr_number}, \
             so a PR can't be told from an API error"
        )
    })?;

    if status >= 400 {
        let (owner, repo_name) = (parsed.owner(), parsed.repo());
        let context =
            format!("Gitea API error {status} for PR #{pr_number} on {owner}/{repo_name}");
        match api_error_message(&output.stdout) {
            Some(message) if !message.is_empty() => bail!("{context}: {message}"),
            Some(_) => bail!(
                "{context}, but the response carried no message — Gitea hides 5xx messages \
                 from non-admin tokens"
            ),
            None => bail!("{context}, and the response body is not one Gitea sends"),
        }
    }

    // A 2xx that isn't the resource: report the parse failure, whose source
    // names where the body diverged.
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
/// Used by the switch dispatcher to decide which forge CLI to try when the
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
    let home = crate::path::home_dir();

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

    /// The status line is the discriminator, and it is the second token of the
    /// first `HTTP/` line — reachable past whatever `tea` wrote before it, and
    /// past a status with no reason phrase.
    #[test]
    fn test_api_status_reads_the_status_line() {
        let status = |stderr: &str| api_status(stderr.as_bytes());

        assert_eq!(status("HTTP/1.1 200 OK\r\n\r\n"), Some(200));
        assert_eq!(status("HTTP/2.0 404 Not Found\n\n"), Some(404));
        assert_eq!(status("HTTP/1.1 500\n"), Some(500));
        // Headers follow the status line and must not be mistaken for it.
        assert_eq!(
            status("HTTP/1.1 403 Forbidden\r\nX-Proto: HTTP/1.1 200 OK\r\n\r\n"),
            Some(403)
        );
        // Anything `tea` says first is stepped over.
        assert_eq!(
            status("warning: login token expires soon\nHTTP/1.1 401 Unauthorized\n"),
            Some(401)
        );

        // Nothing to read: no `--include` output at all, or a line that
        // doesn't carry a number where the status belongs.
        assert_eq!(status(""), None);
        assert_eq!(status("Error: dial tcp: connection refused\n"), None);
        assert_eq!(status("HTTP/1.1 OK\n"), None);
    }

    /// The body supplies the error text, and its three states are three
    /// different things to say: Gitea's message, Gitea's envelope with the
    /// message blanked, and a body Gitea didn't write.
    #[test]
    fn test_api_error_message_reads_the_body() {
        let error = |body: &str| api_error_message(body.as_bytes());

        assert_eq!(
            error(
                r#"{"errors":null,"message":"token is required","url":"https://gitea.example.com/api/swagger"}"#
            ),
            Some("token is required".to_string())
        );

        // Gitea's envelope, blanked for a non-admin token. Whitespace-only is
        // the same thing — trimmed, so one branch covers both.
        assert_eq!(
            error(r#"{"message":"","url":"https://gitea.example.com/api/swagger"}"#),
            Some(String::new())
        );
        assert_eq!(error(r#"{"message":"   "}"#), Some(String::new()));

        // Not an `APIError` at all: a resource, an unknown shape, a proxy's
        // error page.
        assert_eq!(error(r#"{"title":"Fix login","state":"open"}"#), None);
        assert_eq!(error("[]"), None);
        assert_eq!(error(r#"{"unexpected":1}"#), None);
        assert_eq!(error("<html>Bad Gateway</html>"), None);
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
