//! Gitea CI status detection.
//!
//! Detects CI status from Gitea PRs and commit statuses using the `tea` CLI.
//! Experimental.

use serde::Deserialize;
use std::process::Output;
use worktrunk::git::remote_ref::gitea::api_status;
use worktrunk::git::{Repository, parse_owner_repo};

use super::{
    CiBranchName, CiSource, CiStatus, MAX_PRS_TO_FETCH, PrRef, PrStatus, branch_owner_repo,
    is_retriable_error, non_interactive_cmd, output_error_text, parse_json,
};

/// Run `tea api --include <path>` from the worktree root.
///
/// `--include` puts the HTTP status line on stderr, which is the only place the
/// status reaches us — see the [`gitea`](worktrunk::git::remote_ref::gitea)
/// module docs.
///
/// `is_tool_available` has already confirmed `tea` is on PATH, so a spawn
/// failure here is an OS-level edge case — fall through to `None` (no CI status
/// for this branch) rather than logging.
fn tea_api(repo: &Repository, path: &str) -> Option<Output> {
    let repo_root = repo.current_worktree().root().ok()?;
    non_interactive_cmd("tea")
        .args(["api", "--include", path])
        .current_dir(&repo_root)
        .run()
        .ok()
}

/// Whether a `tea api` call failed, and if so whether a later `wt list` could
/// get an answer where this one didn't. `None` when the response carries the
/// resource.
///
/// Neither failure is an exit code. `tea api` exits 0 for every HTTP response,
/// so a non-zero exit means `tea` itself failed and every HTTP error arrives as
/// a successful spawn carrying a body in place of the resource — which
/// [`api_status`] identifies. Branching on the exit code alone would let that
/// body through as data: every field of [`GiteaCombinedStatus`] defaults, so a
/// 500 would deserialize as "no statuses" and paint a blank CI cell.
///
/// Retriability then comes from whichever channel knows:
///
/// - **`tea` itself failed** — no response, so its own stderr is the only
///   account of why (transport error, no login configured), and
///   [`is_retriable_error`] reads it as it does for every other backend's CLI.
/// - **the API answered** — the status settles it outright: 429 and 5xx are the
///   server saying "later", while every other 4xx is a token or a missing
///   resource that repeating the call won't change. Gitea is the only backend
///   that can answer this way; `gh` and `glab` leave the status inside prose,
///   which is why they still sniff for it.
/// - **no status line from an exit-0 `tea`** — the response never arrived, and
///   a call that can't be classified won't classify on a retry either.
///
/// Gitea's message is read nowhere here. It reached the user only through this
/// sniff, and the status answers better than the sniff did: a 500 whose message
/// Gitea blanked for a non-admin token used to leave nothing to match, so a
/// server error painted the same blank cell as a healthy branch with no CI.
fn tea_api_failure_is_retriable(output: &Output) -> Option<bool> {
    if !output.status.success() {
        return Some(is_retriable_error(&output_error_text(output)));
    }
    match api_status(&output.stderr) {
        Some(status) if status < 400 => None,
        Some(status) => Some(status == 429 || status >= 500),
        None => Some(false),
    }
}

/// Fetch the combined CI status for a commit SHA.
///
/// Returns `Some(CiStatus::Error)` for retriable failures (rate limit, network),
/// `None` when the commit has no statuses or the call fails non-retriably.
fn fetch_combined_status(
    repo: &Repository,
    owner: &str,
    repo_name: &str,
    sha: &str,
) -> Option<CiStatus> {
    let path = format!("repos/{owner}/{repo_name}/commits/{sha}/status");
    let output = tea_api(repo, &path)?;
    if let Some(retriable) = tea_api_failure_is_retriable(&output) {
        // The PR-status warning from `retriable_pr_error` is the wrong shape
        // here (this returns just CiStatus).
        return retriable.then_some(CiStatus::Error);
    }
    let combined: GiteaCombinedStatus = parse_json(&output.stdout, "tea api commit status", sha)?;
    if combined.total_count == 0 {
        return None;
    }
    parse_gitea_status_state(&combined.state)
}

/// Detect Gitea PR CI status for a branch.
///
/// Lists open PRs on the primary remote, finds the one whose head branch +
/// head owner matches, then reports conflicts (`mergeable`) and the head
/// commit's combined CI status.
pub(super) fn detect_gitea_pr(
    repo: &Repository,
    branch: &CiBranchName,
    local_head: &str,
) -> Option<PrStatus> {
    // Query the primary remote (typically the upstream), then filter by the
    // branch's push owner — same pattern as github.rs, so fork PRs match.
    let primary_remote = repo.primary_remote().ok()?;
    let primary_url = repo.effective_remote_url(&primary_remote)?;
    let (query_owner, query_repo) = parse_owner_repo(&primary_url)?;
    let branch_owner = branch_owner_repo(repo, branch).map(|(owner, _)| owner)?;

    // `state=open` required (default returns all states); `limit` matches the
    // github backend's MAX_PRS_TO_FETCH so both have identical page semantics.
    let path =
        format!("repos/{query_owner}/{query_repo}/pulls?state=open&limit={MAX_PRS_TO_FETCH}");
    let output = tea_api(repo, &path)?;
    if let Some(retriable) = tea_api_failure_is_retriable(&output) {
        return retriable.then(PrStatus::error);
    }

    let prs: Vec<GiteaPr> = parse_json(&output.stdout, "tea api pulls", &branch.full_name)?;

    // Match by head branch + head owner. Missing owner → potential match,
    // mirroring github.rs.
    let pr = prs.iter().find(|pr| {
        pr.head.ref_name == branch.name
            && pr
                .head
                .repo
                .as_ref()
                .map(|r| r.owner.login.eq_ignore_ascii_case(&branch_owner))
                .unwrap_or(true)
    })?;

    let base_status = fetch_combined_status(
        repo,
        &query_owner,
        &query_repo,
        pr.head.sha.as_deref().unwrap_or(local_head),
    )
    .unwrap_or(CiStatus::NoCI);
    // `mergeable` is computed asynchronously by Gitea — a freshly-opened PR
    // can briefly report `Some(false)` until the check completes (self-
    // corrects when the cache expires).
    let ci_status = if pr.mergeable == Some(false) {
        CiStatus::Conflicts
    } else {
        base_status
    };

    let is_stale = pr
        .head
        .sha
        .as_deref()
        .map(|sha| sha != local_head)
        .unwrap_or(false);

    Some(PrStatus {
        ci_status,
        source: CiSource::PullRequest,
        is_stale,
        is_priming: false,
        url: Some(pr.html_url.clone()),
        number: pr.number.map(PrRef::pr),
        review_state: None,
        title: pr.title.clone(),
        body: pr.body.clone(),
        author: None,
        comment_count: pr.comment_count(),
        updated_at: None,
    })
}

/// Detect Gitea CI status for a branch's HEAD commit (fallback when no PR).
///
/// Queries the combined commit status of `local_head` directly, so the result
/// is never stale.
pub(super) fn detect_gitea_commit_status(
    repo: &Repository,
    branch: &CiBranchName,
    local_head: &str,
) -> Option<PrStatus> {
    let (owner, repo_name) = branch_owner_repo(repo, branch)?;
    let ci_status = fetch_combined_status(repo, &owner, &repo_name, local_head)?;
    Some(PrStatus {
        ci_status,
        source: CiSource::Branch,
        is_stale: false,
        is_priming: false,
        url: None,
        number: None,
        review_state: None,
        title: None,
        body: None,
        author: None,
        comment_count: None,
        updated_at: None,
    })
}

/// Map a Gitea combined-status `state` to [`CiStatus`].
///
/// Gitea collapses the per-commit statuses into one combined value — in
/// practice `success` / `pending` / `failure` (a status-less commit reports
/// `pending`, which the caller already excludes via `total_count == 0`).
/// `error` and `warning` are per-status values handled defensively in case a
/// Gitea version surfaces them here; unknown values map to `None`.
fn parse_gitea_status_state(state: &str) -> Option<CiStatus> {
    match state {
        "success" => Some(CiStatus::Passed),
        "pending" => Some(CiStatus::Running),
        "failure" | "error" | "warning" => Some(CiStatus::Failed),
        _ => None,
    }
}

/// Combined commit status from `GET /repos/{owner}/{repo}/commits/{ref}/status`.
#[derive(Debug, Deserialize)]
struct GiteaCombinedStatus {
    #[serde(default)]
    state: String,
    #[serde(default)]
    total_count: u32,
}

/// A pull request from `GET /repos/{owner}/{repo}/pulls`.
#[derive(Debug, Deserialize)]
struct GiteaPr {
    #[serde(default)]
    number: Option<u64>,
    #[serde(default)]
    mergeable: Option<bool>,
    html_url: String,
    head: GiteaPrBranch,
    /// PR title; shown in the picker's `pr` preview pane. Rides this call.
    #[serde(default)]
    title: Option<String>,
    /// PR description; rendered as markdown in the `pr` preview pane.
    #[serde(default)]
    body: Option<String>,
    /// Comment count. Gitea's PR object carries it directly, so the `comments`
    /// line in the `pr` pane rides this call with no extra round-trip.
    #[serde(default)]
    comments: Option<u32>,
}

impl GiteaPr {
    /// Comment count for [`PrStatus::comment_count`], zero flattened to `None`
    /// so a PR with no comments shows nothing in the `pr` pane.
    fn comment_count(&self) -> Option<u32> {
        self.comments.filter(|&n| n > 0)
    }
}

#[derive(Debug, Deserialize)]
struct GiteaPrBranch {
    #[serde(rename = "ref", default)]
    ref_name: String,
    #[serde(default)]
    sha: Option<String>,
    #[serde(default)]
    repo: Option<GiteaPrRepo>,
}

#[derive(Debug, Deserialize)]
struct GiteaPrRepo {
    owner: GiteaOwner,
}

#[derive(Debug, Deserialize)]
struct GiteaOwner {
    login: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use worktrunk::testing::TestRepo;

    #[test]
    fn test_gitea_pr_comment_count() {
        let pr = |comments: Option<u32>| GiteaPr {
            number: Some(1),
            mergeable: None,
            html_url: String::new(),
            head: GiteaPrBranch {
                ref_name: String::new(),
                sha: None,
                repo: None,
            },
            title: None,
            body: None,
            comments,
        };

        // Zero (or a missing count) flattens to None; a positive count carries through.
        assert_eq!(pr(Some(0)).comment_count(), None);
        assert_eq!(pr(None).comment_count(), None);
        assert_eq!(pr(Some(2)).comment_count(), Some(2));
    }

    /// The status line answers both questions the exit code can't: whether the
    /// call failed, and whether repeating it could help. The body is not read —
    /// each case below pairs a status with a body that would have said something
    /// different, so a sniff surviving anywhere would show up here.
    #[test]
    fn test_tea_api_failure_is_retriable_reads_the_http_status() {
        // `ExitStatus::default()` is success on every platform, so these all
        // exercise the exit-0 path — the one the exit code can't classify.
        let response = |status: &str, stdout: &str| Output {
            status: Default::default(),
            stdout: stdout.as_bytes().to_vec(),
            stderr: format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\r\n")
                .into_bytes(),
        };
        let retriable =
            |status: &str, stdout: &str| tea_api_failure_is_retriable(&response(status, stdout));

        // 2xx is the resource, whatever it holds — an `APIError` body served
        // with a 200 is a Gitea API change, not an API error.
        assert_eq!(retriable("200 OK", "[]"), None);
        assert_eq!(retriable("200 OK", r#"{"message":"nope"}"#), None);

        // The server saying "later".
        assert_eq!(
            retriable("500 Internal Server Error", r#"{"message":""}"#),
            Some(true)
        );
        assert_eq!(
            retriable("502 Bad Gateway", "<html>Bad Gateway</html>"),
            Some(true)
        );
        assert_eq!(retriable("429 Too Many Requests", "{}"), Some(true));

        // A token or a missing resource: repeating the call changes nothing,
        // even when the message reads like a network fault.
        assert_eq!(
            retriable("401 Unauthorized", r#"{"message":"token is required"}"#),
            Some(false)
        );
        assert_eq!(
            retriable("403 Forbidden", r#"{"message":"connection timeout"}"#),
            Some(false)
        );
        assert_eq!(
            retriable("404 Not Found", r#"{"message":"does not exist"}"#),
            Some(false)
        );

        // No status line from an exit-0 `tea`: a failure, never the resource,
        // and nothing a retry would classify differently.
        assert_eq!(
            tea_api_failure_is_retriable(&Output {
                status: Default::default(),
                stdout: b"[]".to_vec(),
                stderr: Vec::new(),
            }),
            Some(false)
        );
    }

    #[test]
    fn test_parse_gitea_status_state() {
        assert_eq!(parse_gitea_status_state("success"), Some(CiStatus::Passed));
        assert_eq!(parse_gitea_status_state("pending"), Some(CiStatus::Running));
        assert_eq!(parse_gitea_status_state("failure"), Some(CiStatus::Failed));
        assert_eq!(parse_gitea_status_state("error"), Some(CiStatus::Failed));
        assert_eq!(parse_gitea_status_state("warning"), Some(CiStatus::Failed));
        assert_eq!(parse_gitea_status_state(""), None);
        assert_eq!(parse_gitea_status_state("bogus"), None);
    }

    /// Local branches walk the push-destination chain; with no explicit
    /// pushRemote and no tracking, the primary remote is the fallback.
    /// Asserts directly against the shared resolver so coverage doesn't
    /// depend on the spawn-based `tea --version` probe in the integration
    /// tests.
    #[test]
    fn test_branch_owner_repo_local_uses_primary_remote() {
        let test = TestRepo::with_initial_commit();
        test.run_git(&[
            "remote",
            "add",
            "origin",
            "https://gitea.example.com/owner/test-repo.git",
        ]);
        let repo = Repository::at(test.root_path()).unwrap();

        // Use a name that doesn't exist locally — `push_remote_url` returns
        // None for it, so we land on the primary-remote fallback.
        let branch = CiBranchName {
            full_name: "ghost-local".to_string(),
            remote: None,
            name: "ghost-local".to_string(),
        };
        assert_eq!(
            branch_owner_repo(&repo, &branch),
            Some(("owner".to_string(), "test-repo".to_string()))
        );
    }

    /// A branch whose remote has no URL (e.g., a stale ref to a deleted
    /// remote) propagates `None` through `branch_remote_url`.
    #[test]
    fn test_branch_owner_repo_returns_none_when_remote_missing() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        let branch = CiBranchName {
            full_name: "ghost/feature".to_string(),
            remote: Some("ghost".to_string()),
            name: "feature".to_string(),
        };
        assert_eq!(branch_owner_repo(&repo, &branch), None);
    }

    /// Remote-branch refs read from `branch.remote`'s effective URL. In a
    /// mixed-remote repo, this must pick the branch's remote, not the
    /// primary one.
    #[test]
    fn test_branch_owner_repo_remote_uses_branch_remote() {
        let test = TestRepo::with_initial_commit();
        test.run_git(&[
            "remote",
            "add",
            "origin",
            "https://gitea.example.com/owner/test-repo.git",
        ]);
        test.run_git(&[
            "remote",
            "add",
            "fork",
            "https://gitea.example.com/forkowner/test-repo.git",
        ]);
        let repo = Repository::at(test.root_path()).unwrap();

        let branch = CiBranchName {
            full_name: "fork/feature".to_string(),
            remote: Some("fork".to_string()),
            name: "feature".to_string(),
        };
        assert_eq!(
            branch_owner_repo(&repo, &branch),
            Some(("forkowner".to_string(), "test-repo".to_string()))
        );
    }

    /// A Forgejo / non-canonically-named Gitea host (e.g. `codeberg.org`)
    /// must still resolve through `branch_owner_repo` — the platform comes
    /// from explicit `forge.platform` config, so the host heuristic
    /// (`is_gitea` substring check) is not used here. Re-introducing such a
    /// filter would silently drop CI status for legitimate Forgejo users
    /// (see `src/git/ci_platform.rs:186`).
    #[test]
    fn test_branch_owner_repo_resolves_non_canonical_gitea_host() {
        let test = TestRepo::with_initial_commit();
        test.run_git(&[
            "remote",
            "add",
            "origin",
            "https://codeberg.org/owner/test-repo.git",
        ]);
        let repo = Repository::at(test.root_path()).unwrap();

        let branch = CiBranchName {
            full_name: "ghost-local".to_string(),
            remote: None,
            name: "ghost-local".to_string(),
        };
        assert_eq!(
            branch_owner_repo(&repo, &branch),
            Some(("owner".to_string(), "test-repo".to_string()))
        );
    }
}
