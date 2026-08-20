//! GitHub CI status detection.
//!
//! Detects CI status from GitHub PRs and workflow runs using the `gh` CLI.

use serde::Deserialize;
use worktrunk::git::Repository;

use super::{
    CiBranchName, CiSource, CiStatus, MAX_PRS_TO_FETCH, PrRef, PrStatus, ReviewState,
    branch_owner_repo, non_interactive_cmd, parse_json, retriable_pr_error,
};
// CI detection primes the picker's on-disk comments cache: this `gh pr list`
// call already transfers the comment thread (we count it for `comment_count`),
// so feeding it to the cache the picker's `comments` tab reads spares that tab
// its own `gh pr view --json comments` fetch. See `prime_comments_cache`.
use crate::commands::picker::preview_cache::{self, CommentEntry};

/// Detect GitHub PR CI status for a branch.
///
/// # Filtering Strategy
///
/// We need to find PRs where the head branch comes from *our* fork, not just
/// PRs we authored. The `--author` flag filters by PR creator, but we want
/// to filter by source repository.
///
/// Since `gh pr list --head` doesn't support `owner:branch` format, we:
/// 1. Fetch all open PRs with matching branch name (up to 20)
/// 2. Include `headRepositoryOwner` in the JSON output
/// 3. Filter client-side by comparing `headRepositoryOwner.login` to the branch's push remote owner
///
/// This correctly handles:
/// - Fork workflows (PRs from your fork to upstream)
/// - Organization repos (PRs from org branches)
/// - Multiple users with same branch name
/// - Remote-only branches (e.g., "origin/feature")
pub(super) fn detect_github(
    repo: &Repository,
    branch: &CiBranchName,
    local_head: &str,
) -> Option<PrStatus> {
    let repo_root = repo.current_worktree().root().ok()?;

    // Get the owner of the branch's push remote for filtering PRs by source repository.
    // For local branches: resolves through pushRemote → remote.pushDefault → tracking remote.
    // For remote branches: use the remote's effective URL (handles insteadOf aliases).
    let branch_owner = branch_owner_repo(repo, branch).map(|(owner, _)| owner);

    let Some(branch_owner) = branch_owner else {
        tracing::debug!(
            branch = %branch.full_name,
            "Branch {} has no resolvable push remote; skipping PR-based CI detection",
            branch.full_name
        );
        return None;
    };

    // Use `gh pr list --head` instead of `gh pr view` to handle numeric branch names correctly.
    // When branch name is all digits (e.g., "4315"), `gh pr view` interprets it as a PR number,
    // but `gh pr list --head` correctly treats it as a branch name.
    //
    // IMPORTANT: Use the bare branch name (branch.name), not the full remote ref.
    // `gh pr list --head origin/feature` won't find anything - it needs just "feature".
    //
    // We fetch up to MAX_PRS_TO_FETCH PRs to handle branch name collisions, then filter
    // client-side by headRepositoryOwner to find PRs from our fork.
    let output = match non_interactive_cmd("gh")
        .args([
            "pr",
            "list",
            "--head",
            &branch.name, // Use bare branch name, not "origin/feature"
            "--state",
            "open",
            "--limit",
            &MAX_PRS_TO_FETCH.to_string(),
            "--json",
            // title,body,author and the comments array ride this existing call so
            // the picker's `pr` preview pane and matcher text can use them — no
            // extra round-trip. `gh pr list` has no comment-count field, so we
            // request the array and count it; for a `--head <branch>` call that's
            // typically one PR. `updatedAt` rides too: it keys the picker's
            // on-disk comments cache (see `PrStatus::updated_at`).
            "number,title,body,author,comments,headRefOid,mergeStateStatus,statusCheckRollup,url,headRepositoryOwner,reviewDecision,isDraft,updatedAt",
        ])
        .current_dir(&repo_root)
        .run()
    {
        Ok(output) => output,
        Err(e) => {
            tracing::warn!(
                branch = %branch.full_name,
                error = %e,
                "gh pr list failed to execute for branch {}: {}",
                branch.full_name,
                e
            );
            return None;
        }
    };

    if !output.status.success() {
        return retriable_pr_error(&output);
    }

    // gh pr list returns an array - find the first PR from our origin
    let pr_list: Vec<GitHubPrInfo> = parse_json(&output.stdout, "gh pr list", &branch.full_name)?;

    // Filter to PRs from our origin (case-insensitive comparison for GitHub usernames).
    // If headRepositoryOwner is missing (older GH CLI, Enterprise, or permissions),
    // treat it as a potential match to avoid false negatives.
    let pr_info = pr_list.iter().find(|pr| {
        pr.head_repository_owner
            .as_ref()
            .map(|h| h.login.eq_ignore_ascii_case(&branch_owner))
            .unwrap_or(true) // Missing owner field = potential match
    });
    if pr_info.is_none() && !pr_list.is_empty() {
        tracing::debug!(
            count = %pr_list.len(),
            branch = %branch.full_name,
            owner = %branch_owner,
            "Found {} PRs for branch {} but none from owner {}",
            pr_list.len(),
            branch.full_name,
            branch_owner
        );
    }
    let pr_info = pr_info?;

    // Determine CI status using priority: conflicts > running > failed > passed > no_ci
    let ci_status = if pr_info.merge_state_status.as_deref() == Some("DIRTY") {
        CiStatus::Conflicts
    } else {
        pr_info.ci_status()
    };

    let is_stale = pr_info
        .head_ref_oid
        .as_ref()
        .map(|pr_head| pr_head != local_head)
        .unwrap_or(false);

    // The comment thread is already in hand from the call above — hand it to the
    // picker's comments cache so its `comments` tab need not re-fetch it.
    prime_comments_cache(repo, pr_info);

    Some(PrStatus {
        ci_status,
        source: CiSource::PullRequest,
        is_stale,
        is_priming: false,
        url: pr_info.url.clone(),
        number: pr_info.number.map(PrRef::pr),
        review_state: pr_info.review_state(),
        title: pr_info.title.clone(),
        body: pr_info.body.clone(),
        author: pr_info.author.as_ref().map(|a| a.login.clone()),
        comment_count: pr_info.comment_count(),
        updated_at: pr_info.updated_at.clone(),
    })
}

/// `gh pr list` returns at most one page (100) of a PR's `comments` connection
/// and does *not* paginate nested connections (unlike `gh pr view`, which
/// follows every page). A returned full page may therefore be truncated.
const GH_LIST_COMMENTS_PAGE: usize = 100;

/// Prime the picker's on-disk `comments` cache from the thread this `gh pr list`
/// call already transferred (the same array [`GitHubPrInfo::comment_count`]
/// counts). The picker's worktree-row `comments` tab keys off the PR's
/// `updatedAt`, so a matching prime turns its per-row `gh pr view --json
/// comments` fetch into a cache hit — including the common zero-comment PR (an
/// empty thread is cached, so the tab resolves to "No comments" with no fetch).
///
/// GitHub only: the key needs `updatedAt`, the content signature GitLab's
/// throttled, delete-blind timestamp can't provide (see [`PrStatus::updated_at`]).
/// A PR with no resolved number or no `updatedAt` is skipped — there's no key to
/// write under, and the tab falls back to its own fetch.
///
/// A full page of comments ([`GH_LIST_COMMENTS_PAGE`]) is also skipped: it may be
/// truncated, and caching a partial thread would stop the tab's *paginating*
/// `gh pr view` fetch (which gets all of them) from ever running. Sub-page
/// threads — the overwhelming majority — are known-complete and cached.
fn prime_comments_cache(repo: &Repository, pr_info: &GitHubPrInfo) {
    let (Some(number), Some(updated_at)) = (pr_info.number, pr_info.updated_at.as_deref()) else {
        return;
    };
    if pr_info.comments.len() >= GH_LIST_COMMENTS_PAGE {
        return;
    }
    let entries: Vec<CommentEntry> = pr_info.comments.iter().map(CommentEntry::from).collect();
    preview_cache::write_comments(repo, number as u32, updated_at, &entries);
}

/// Detect CI status for a commit using GitHub's check-runs API.
///
/// This queries all check runs for the commit SHA, giving us the same data
/// that `statusCheckRollup` provides for PRs. This correctly aggregates
/// status across multiple workflows (e.g., `ci` and `publish-docs`).
pub(super) fn detect_github_commit_checks(
    repo: &Repository,
    branch: &CiBranchName,
    local_head: &str,
) -> Option<PrStatus> {
    let repo_root = repo.current_worktree().root().ok()?;
    let (owner, repo_name) = branch_owner_repo(repo, branch)?;

    // Only pass --hostname when explicitly configured (for GHE / self-hosted)
    let hostname = repo.forge_hostname();

    // Use GitHub's check-runs API to get all checks for this commit
    let api_path = format!("repos/{owner}/{repo_name}/commits/{local_head}/check-runs");
    let mut args = vec!["api", api_path.as_str()];
    if let Some(h) = &hostname {
        args.extend(["--hostname", h.as_str()]);
    }
    args.extend(["--jq", ".check_runs | map({status, conclusion})"]);

    let output = match non_interactive_cmd("gh")
        .args(args)
        .current_dir(&repo_root)
        .run()
    {
        Ok(output) => output,
        Err(e) => {
            tracing::warn!(
                head = %local_head,
                error = %e,
                "gh api check-runs failed to execute for {}: {}",
                local_head,
                e
            );
            return None;
        }
    };

    if !output.status.success() {
        return retriable_pr_error(&output);
    }

    let checks: Vec<GitHubCheck> = parse_json(&output.stdout, "gh api check-runs", local_head)?;

    if checks.is_empty() {
        return None;
    }

    // Aggregate status: any failed → Failed, any running → Running, else Passed
    let ci_status = aggregate_github_checks(&checks);

    Some(PrStatus {
        ci_status,
        source: CiSource::Branch,
        is_stale: false, // We're querying by SHA, so always current
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

/// GitHub PR info from `gh pr list --json ...`
///
/// Note: We include `headRepositoryOwner` for client-side filtering by source fork.
/// See the `# Filtering Strategy` docs on [`detect_github`] for why this is necessary.
///
/// Note: We don't include `state` because we already filter with `--state open`.
#[derive(Debug, Deserialize)]
pub(crate) struct GitHubPrInfo {
    pub number: Option<u64>,
    /// PR title; shown in the picker's `pr` preview pane. Rides this call.
    pub title: Option<String>,
    /// PR description; shown in the `pr` preview pane. Rides this call.
    pub body: Option<String>,
    /// PR author; folded into the row's matcher text. Requested by both the
    /// worktree-row [`detect_github`] call and the `--prs` list call.
    #[serde(default)]
    pub author: Option<GitHubAuthor>,
    /// Conversation comments on the PR. Requested only on the worktree-row
    /// [`detect_github`] call; the `--prs` call omits `comments` to keep its
    /// 50-PR payload light, so this stays empty there (`#[serde(default)]`). Its
    /// length feeds [`comment_count`](Self::comment_count) (the `pr` pane's
    /// `comments` line), and the bodies prime the picker's on-disk comments cache
    /// via [`prime_comments_cache`] — the same thread the `comments` tab would
    /// otherwise re-fetch with `gh pr view --json comments`.
    #[serde(default)]
    pub comments: Vec<GitHubComment>,
    #[serde(rename = "headRefOid")]
    pub head_ref_oid: Option<String>,
    #[serde(rename = "mergeStateStatus")]
    pub merge_state_status: Option<String>,
    #[serde(rename = "statusCheckRollup")]
    pub status_check_rollup: Option<Vec<GitHubCheck>>,
    pub url: Option<String>,
    /// The owner of the repository the PR's head branch comes from.
    /// Used to filter PRs by source fork (see the `# Filtering Strategy` docs on [`detect_github`]).
    #[serde(rename = "headRepositoryOwner")]
    pub head_repository_owner: Option<HeadRepositoryOwner>,
    /// GraphQL review decision: "APPROVED", "CHANGES_REQUESTED",
    /// "REVIEW_REQUIRED", or empty when no reviews exist and none are required.
    #[serde(rename = "reviewDecision")]
    pub review_decision: Option<String>,
    #[serde(rename = "isDraft")]
    pub is_draft: Option<bool>,
    /// PR `updatedAt` — the forge's "last modified" timestamp (RFC 3339). Rides
    /// both the worktree-row [`detect_github`] call and the `--prs` list call;
    /// keys the picker's on-disk comments cache (see [`PrStatus::updated_at`]).
    #[serde(rename = "updatedAt", default)]
    pub updated_at: Option<String>,
}

/// Owner info for the head repository of a PR.
#[derive(Debug, Deserialize)]
pub(crate) struct HeadRepositoryOwner {
    /// The login (username/org name) of the repository owner.
    pub login: String,
}

/// PR author from `gh pr list --json author` (`{"login": ...}`).
#[derive(Debug, Default, Deserialize)]
pub(crate) struct GitHubAuthor {
    #[serde(default)]
    pub login: String,
}

/// A single GitHub conversation comment. The per-element shape is identical for
/// `gh pr list --json comments` (the worktree CI call, which primes the cache —
/// see [`prime_comments_cache`]) and `gh pr view --json comments` (the picker's
/// lazy fetch in `commands::picker::prs`), so both parse into this one type. Only
/// the fields the picker's `comments` pane renders are kept — author,
/// body, and timestamp; the rest of GitHub's comment object is ignored.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct GitHubComment {
    #[serde(default)]
    pub author: GitHubAuthor,
    #[serde(default)]
    pub body: String,
    #[serde(rename = "createdAt", default)]
    pub created_at: String,
}

impl From<&GitHubComment> for CommentEntry {
    fn from(c: &GitHubComment) -> Self {
        CommentEntry {
            author: c.author.login.clone(),
            body: c.body.clone(),
            created_at: c.created_at.clone(),
        }
    }
}

/// A single check from `statusCheckRollup`.
///
/// This is a union of two GitHub API types with different field structures:
/// - `CheckRun` (GitHub Actions): has `status` ("COMPLETED", "IN_PROGRESS") and
///   `conclusion` ("SUCCESS", "FAILURE", "CANCELLED", "SKIPPED")
/// - `StatusContext` (external CI like pre-commit.ci): has `state` only
///   ("SUCCESS", "FAILURE", "PENDING", "ERROR")
///
/// We parse all three fields and check whichever is present. Using
/// `gh pr checks <number> --json state` would require a second call after PR
/// discovery, while `statusCheckRollup` returns PR metadata and CI status
/// together. A branch without a PR has no rollup, so
/// [`detect_github_commit_checks`] queries the commit's check-runs API separately.
#[derive(Debug, Deserialize)]
pub(crate) struct GitHubCheck {
    /// CheckRun only: "COMPLETED", "IN_PROGRESS", "QUEUED", etc.
    pub status: Option<String>,
    /// CheckRun only: "SUCCESS", "FAILURE", "CANCELLED", "SKIPPED", etc.
    pub conclusion: Option<String>,
    /// StatusContext only: "SUCCESS", "FAILURE", "PENDING", "ERROR"
    pub state: Option<String>,
}

impl GitHubPrInfo {
    /// Map `isDraft` + `reviewDecision` to a [`ReviewState`].
    ///
    /// Draft wins over the review decision: a draft is intentionally parked,
    /// so its review verdict shouldn't demand attention. An empty
    /// `reviewDecision` means no review signal and maps to `None`.
    pub fn review_state(&self) -> Option<ReviewState> {
        if self.is_draft == Some(true) {
            return Some(ReviewState::Draft);
        }
        match self.review_decision.as_deref() {
            Some("APPROVED") => Some(ReviewState::Approved),
            Some("CHANGES_REQUESTED") => Some(ReviewState::ChangesRequested),
            Some("REVIEW_REQUIRED") => Some(ReviewState::Pending),
            _ => None,
        }
    }

    pub fn ci_status(&self) -> CiStatus {
        match &self.status_check_rollup {
            None => CiStatus::NoCI,
            Some(checks) if checks.is_empty() => CiStatus::NoCI,
            Some(checks) => aggregate_github_checks(checks),
        }
    }

    /// The conversation-comment count for [`PrStatus::comment_count`], or `None`
    /// when there are none — zero is flattened so a PR with no comments shows
    /// nothing in the `pr` pane.
    pub fn comment_count(&self) -> Option<u32> {
        u32::try_from(self.comments.len()).ok().filter(|&n| n > 0)
    }

    /// Build a [`PrStatus`] from this open-PR entry, for callers that already
    /// hold the open-PR list (the `--prs` picker) and want the same CI-column
    /// treatment [`detect_github`] produces per branch. PR rows have no local
    /// checkout to diff against, so the result is never marked stale.
    ///
    /// Only the `--prs` picker calls this.
    pub(crate) fn open_pr_status(&self) -> PrStatus {
        let ci_status = if self.merge_state_status.as_deref() == Some("DIRTY") {
            CiStatus::Conflicts
        } else {
            self.ci_status()
        };
        PrStatus {
            ci_status,
            source: CiSource::PullRequest,
            is_stale: false,
            is_priming: false,
            url: self.url.clone(),
            number: self.number.map(PrRef::pr),
            review_state: self.review_state(),
            // The `--prs` pane reads title/body from the `PrEntry`, not this status
            // (which feeds only the CI column), so they stay absent here. Likewise
            // the comment count: the `--prs` rows surface comments in their own
            // background-fetched comments tab, and the list call omits `comments`.
            title: None,
            body: None,
            author: None,
            comment_count: None,
            // Unlike title/body/comment_count, `updated_at` IS read off this
            // status: it keys the `--prs` row's on-disk comments cache, so it
            // rides through here from the same `gh pr list` call.
            updated_at: self.updated_at.clone(),
        }
    }
}

/// Aggregate CI status from multiple GitHub checks (case-insensitive).
///
/// Priority: running > failed > passed > no-ci.
/// Handles both `statusCheckRollup` (uppercase) and check-runs API (lowercase).
///
/// Each field draws from its own GitHub enum, and every non-terminal value in
/// each one has to reach `has_running` — a check the aggregate reads as
/// terminal when it isn't turns the whole row green early:
///
/// - `status` — `CheckStatusState`: `REQUESTED`, `QUEUED`, `IN_PROGRESS`,
///   `COMPLETED`, `WAITING`, `PENDING`. All but `COMPLETED` are non-terminal.
///   `WAITING` is the one a deployment-protection rule or required reviewer
///   parks a run in, so it can sit there for hours with every other check green.
/// - `state` — `StatusState`: `EXPECTED`, `ERROR`, `FAILURE`, `PENDING`,
///   `SUCCESS`. `EXPECTED` is a required context branch protection knows about
///   but no one has posted yet, so it is non-terminal too.
/// - `conclusion` — `CheckConclusionState`: `ACTION_REQUIRED`, `TIMED_OUT`,
///   `CANCELLED`, `FAILURE`, `SUCCESS`, `NEUTRAL`, `SKIPPED`,
///   `STARTUP_FAILURE`, `STALE`. `STARTUP_FAILURE` is a run that never got off
///   the ground, which is a failure like any other.
///
/// `SKIPPED` and `NEUTRAL` deliberately contribute to neither pass nor fail, and
/// `STALE` joins them: it marks a result GitHub itself no longer considers
/// current, so it is not a verdict on this commit. Each branch also tolerates
/// the other fields' vocabulary (`expected` under `status`, `error` under
/// `conclusion`) — those combinations don't occur, but the cross-checks cost
/// nothing and keep a mislabeled payload from being read as terminal.
pub(super) fn aggregate_github_checks(checks: &[GitHubCheck]) -> CiStatus {
    let mut has_running = false;
    let mut has_failure = false;
    let mut has_success = false;

    for check in checks {
        // CheckRun: status field indicates in-progress states
        if let Some(status) = &check.status {
            let s = status.to_ascii_lowercase();
            if matches!(
                s.as_str(),
                "in_progress" | "queued" | "pending" | "waiting" | "requested" | "expected"
            ) {
                has_running = true;
            }
        }

        // StatusContext: state field indicates pending
        if let Some(state) = &check.state {
            let s = state.to_ascii_lowercase();
            if matches!(s.as_str(), "pending" | "expected") {
                has_running = true;
            } else if matches!(s.as_str(), "failure" | "error") {
                has_failure = true;
            } else if s == "success" {
                has_success = true;
            }
        }

        // CheckRun: conclusion field indicates final result
        if let Some(conclusion) = &check.conclusion {
            let c = conclusion.to_ascii_lowercase();
            match c.as_str() {
                "failure" | "error" | "cancelled" | "timed_out" | "action_required"
                | "startup_failure" => {
                    has_failure = true;
                }
                "success" => {
                    has_success = true;
                }
                // "skipped", "neutral", "stale" - ignored
                _ => {}
            }
        }
    }

    if has_running {
        CiStatus::Running
    } else if has_failure {
        CiStatus::Failed
    } else if has_success {
        CiStatus::Passed
    } else {
        CiStatus::NoCI
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `DIRTY` merge state (merge conflicts) reports `Conflicts` regardless of
    /// the check rollup — the `--prs` picker's CI column treatment.
    #[test]
    fn open_pr_status_dirty_merge_state_reports_conflicts() {
        let pr = GitHubPrInfo {
            number: Some(7),
            head_ref_oid: None,
            merge_state_status: Some("DIRTY".to_string()),
            status_check_rollup: None,
            url: None,
            head_repository_owner: None,
            title: None,
            body: None,
            comments: Vec::new(),
            review_decision: None,
            is_draft: None,
            updated_at: None,
            author: None,
        };
        assert_eq!(pr.open_pr_status().ci_status, CiStatus::Conflicts);
    }

    #[test]
    fn test_github_pr_info_ci_status() {
        let cases = [
            (r#"{}"#, CiStatus::NoCI),
            (r#"{"statusCheckRollup":[]}"#, CiStatus::NoCI),
            (
                r#"{"statusCheckRollup":[{"status":"IN_PROGRESS"}]}"#,
                CiStatus::Running,
            ),
            (
                r#"{"statusCheckRollup":[{"status":"QUEUED"}]}"#,
                CiStatus::Running,
            ),
            (
                r#"{"statusCheckRollup":[{"status":"PENDING"}]}"#,
                CiStatus::Running,
            ),
            (
                r#"{"statusCheckRollup":[{"status":"EXPECTED"}]}"#,
                CiStatus::Running,
            ),
            (
                r#"{"statusCheckRollup":[{"state":"PENDING"}]}"#,
                CiStatus::Running,
            ),
            (
                r#"{"statusCheckRollup":[{"status":"COMPLETED","conclusion":"FAILURE"}]}"#,
                CiStatus::Failed,
            ),
            (
                r#"{"statusCheckRollup":[{"status":"COMPLETED","conclusion":"ERROR"}]}"#,
                CiStatus::Failed,
            ),
            (
                r#"{"statusCheckRollup":[{"status":"COMPLETED","conclusion":"CANCELLED"}]}"#,
                CiStatus::Failed,
            ),
            (
                r#"{"statusCheckRollup":[{"state":"FAILURE"}]}"#,
                CiStatus::Failed,
            ),
            (
                r#"{"statusCheckRollup":[{"state":"ERROR"}]}"#,
                CiStatus::Failed,
            ),
            (
                r#"{"statusCheckRollup":[{"status":"COMPLETED","conclusion":"SUCCESS"}]}"#,
                CiStatus::Passed,
            ),
        ];

        for (json, expected) in cases {
            let pr: GitHubPrInfo = serde_json::from_str(json).expect("valid GitHub PR JSON");
            assert_eq!(pr.ci_status(), expected, "json={json}");
        }
    }

    #[test]
    fn test_github_pr_info_review_state() {
        let pr = |review_decision: Option<&str>, is_draft: Option<bool>| GitHubPrInfo {
            number: None,
            head_ref_oid: None,
            merge_state_status: None,
            status_check_rollup: None,
            url: None,
            head_repository_owner: None,
            title: None,
            body: None,
            comments: Vec::new(),
            review_decision: review_decision.map(Into::into),
            is_draft,
            updated_at: None,
            author: None,
        };

        assert_eq!(
            pr(Some("APPROVED"), None).review_state(),
            Some(ReviewState::Approved)
        );
        assert_eq!(
            pr(Some("CHANGES_REQUESTED"), Some(false)).review_state(),
            Some(ReviewState::ChangesRequested)
        );
        assert_eq!(
            pr(Some("REVIEW_REQUIRED"), None).review_state(),
            Some(ReviewState::Pending)
        );
        // Empty decision = no review signal, not pending
        assert_eq!(pr(Some(""), None).review_state(), None);
        assert_eq!(pr(None, None).review_state(), None);
        // Draft wins over the decision
        assert_eq!(
            pr(Some("APPROVED"), Some(true)).review_state(),
            Some(ReviewState::Draft)
        );
    }

    #[test]
    fn test_github_pr_info_comment_count() {
        // The count comes from the length of the requested `comments` array.
        let with = |n: usize| GitHubPrInfo {
            number: None,
            head_ref_oid: None,
            merge_state_status: None,
            status_check_rollup: None,
            url: None,
            head_repository_owner: None,
            title: None,
            body: None,
            comments: std::iter::repeat_with(GitHubComment::default)
                .take(n)
                .collect(),
            review_decision: None,
            is_draft: None,
            updated_at: None,
            author: None,
        };

        // Zero comments flatten to None so a PR with no comments shows nothing.
        assert_eq!(with(0).comment_count(), None);
        assert_eq!(with(1).comment_count(), Some(1));
        assert_eq!(with(4).comment_count(), Some(4));
    }

    /// `gh pr list --json comments` carries each comment's author, body, and
    /// `createdAt` (note the rename) — parsing them is what lets the worktree CI
    /// call prime the picker's comments cache. Pins the JSON shape.
    #[test]
    fn github_pr_info_parses_comment_bodies() {
        let json = br#"[{
            "number": 1,
            "comments": [
                {"author": {"login": "bob"}, "body": "ship it", "createdAt": "2026-06-28T00:00:00Z"}
            ],
            "updatedAt": "2026-06-28T01:00:00Z"
        }]"#;
        let prs: Vec<GitHubPrInfo> = serde_json::from_slice(json).expect("parse");
        let pr = &prs[0];
        assert_eq!(pr.comments.len(), 1);
        assert_eq!(pr.comments[0].author.login, "bob");
        assert_eq!(pr.comments[0].body, "ship it");
        assert_eq!(pr.comments[0].created_at, "2026-06-28T00:00:00Z");
        assert_eq!(pr.updated_at.as_deref(), Some("2026-06-28T01:00:00Z"));
    }

    /// `prime_comments_cache` writes the thread under `(number, updatedAt)`, the
    /// key the picker's `comments` tab reads — so the tab serves it without its
    /// own `gh pr view --json comments` fetch.
    #[test]
    fn prime_comments_cache_writes_under_signature() {
        use worktrunk::testing::TestRepo;

        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        let json = br#"[{
            "number": 42,
            "comments": [
                {"author": {"login": "alice"}, "body": "lgtm", "createdAt": "2026-06-28T18:00:00Z"}
            ],
            "updatedAt": "2026-06-28T18:30:00Z"
        }]"#;
        let prs: Vec<GitHubPrInfo> = serde_json::from_slice(json).expect("parse");
        prime_comments_cache(&repo, &prs[0]);

        let cached = preview_cache::read_comments(&repo, 42, "2026-06-28T18:30:00Z")
            .expect("primed entry exists");
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].author, "alice");
        assert_eq!(cached[0].body, "lgtm");
        assert_eq!(cached[0].created_at, "2026-06-28T18:00:00Z");
    }

    /// A zero-comment PR is primed too (an empty thread), so the common
    /// no-comments case skips the fetch as well.
    #[test]
    fn prime_comments_cache_caches_empty_thread() {
        use worktrunk::testing::TestRepo;

        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        let json = br#"[{"number": 7, "comments": [], "updatedAt": "2026-06-28T00:00:00Z"}]"#;
        let prs: Vec<GitHubPrInfo> = serde_json::from_slice(json).expect("parse");
        prime_comments_cache(&repo, &prs[0]);

        let cached =
            preview_cache::read_comments(&repo, 7, "2026-06-28T00:00:00Z").expect("empty entry");
        assert!(cached.is_empty());
    }

    /// A full page of comments may be truncated (`gh pr list` doesn't paginate
    /// nested connections), so the prime skips it — the comments tab's own
    /// paginating `gh pr view` fetch gets the complete thread instead.
    #[test]
    fn prime_comments_cache_skips_a_possibly_truncated_full_page() {
        use worktrunk::testing::TestRepo;

        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        let pr = GitHubPrInfo {
            number: Some(5),
            comments: std::iter::repeat_with(GitHubComment::default)
                .take(GH_LIST_COMMENTS_PAGE)
                .collect(),
            updated_at: Some("2026-06-28T00:00:00Z".to_string()),
            head_ref_oid: None,
            merge_state_status: None,
            status_check_rollup: None,
            url: None,
            head_repository_owner: None,
            title: None,
            body: None,
            review_decision: None,
            is_draft: None,
            author: None,
        };
        prime_comments_cache(&repo, &pr);

        assert_eq!(
            preview_cache::count_all(&repo),
            0,
            "a full (possibly truncated) page is not cached"
        );
    }

    /// No `updatedAt` means no cache key, so nothing is written — the tab falls
    /// back to its own fetch rather than caching under a missing signature.
    #[test]
    fn prime_comments_cache_skips_without_updated_at() {
        use worktrunk::testing::TestRepo;

        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        let json = br#"[{
            "number": 9,
            "comments": [{"author": {"login": "x"}, "body": "y", "createdAt": "z"}]
        }]"#;
        let prs: Vec<GitHubPrInfo> = serde_json::from_slice(json).expect("parse");
        prime_comments_cache(&repo, &prs[0]);

        assert_eq!(
            preview_cache::count_all(&repo),
            0,
            "no updatedAt → no key → nothing written"
        );
    }

    #[test]
    fn test_aggregate_github_checks() {
        // Helper to create a check without state field (like check-runs API)
        fn check(status: &str, conclusion: Option<&str>) -> GitHubCheck {
            GitHubCheck {
                status: Some(status.into()),
                conclusion: conclusion.map(|c| c.into()),
                state: None,
            }
        }

        // Empty checks = NoCI
        assert_eq!(aggregate_github_checks(&[]), CiStatus::NoCI);

        // All skipped = NoCI (skipped doesn't count as success). "stale" joins
        // them: GitHub marks a result stale when it no longer reflects the
        // commit, so it is not a verdict either way.
        let checks = vec![
            check("completed", Some("skipped")),
            check("completed", Some("neutral")),
            check("completed", Some("stale")),
        ];
        assert_eq!(aggregate_github_checks(&checks), CiStatus::NoCI);

        // Any running = Running. Every CheckStatusState but COMPLETED is
        // non-terminal — "waiting" is a run parked by a deployment-protection
        // rule, "requested" one created but not yet queued.
        for status in ["in_progress", "queued", "pending", "waiting", "requested"] {
            let checks = vec![check("completed", Some("success")), check(status, None)];
            assert_eq!(
                aggregate_github_checks(&checks),
                CiStatus::Running,
                "status={status}"
            );
        }

        // A required context branch protection expects but nobody has posted
        // yet is non-terminal, same as a pending one.
        let checks = vec![
            check("completed", Some("success")),
            GitHubCheck {
                status: None,
                conclusion: None,
                state: Some("EXPECTED".into()),
            },
        ];
        assert_eq!(aggregate_github_checks(&checks), CiStatus::Running);

        // Any failure among completed checks = Failed. "startup_failure" is a
        // run that never got off the ground.
        for conclusion in [
            "failure",
            "cancelled",
            "timed_out",
            "action_required",
            "startup_failure",
        ] {
            let checks = vec![
                check("completed", Some("success")),
                check("completed", Some(conclusion)),
            ];
            assert_eq!(
                aggregate_github_checks(&checks),
                CiStatus::Failed,
                "conclusion={conclusion}"
            );
        }

        // Running takes priority over failure (build might still succeed)
        let checks = vec![
            check("in_progress", None),
            check("completed", Some("failure")),
        ];
        assert_eq!(aggregate_github_checks(&checks), CiStatus::Running);

        // A pending StatusContext also takes priority over a successful CheckRun
        let checks = vec![
            check("completed", Some("success")),
            GitHubCheck {
                status: None,
                conclusion: None,
                state: Some("PENDING".into()),
            },
        ];
        assert_eq!(aggregate_github_checks(&checks), CiStatus::Running);

        // All success = Passed
        let checks = vec![
            check("completed", Some("success")),
            check("completed", Some("success")),
        ];
        assert_eq!(aggregate_github_checks(&checks), CiStatus::Passed);

        // Mix of success and skipped = Passed (skipped doesn't block)
        let checks = vec![
            check("completed", Some("success")),
            check("completed", Some("skipped")),
        ];
        assert_eq!(aggregate_github_checks(&checks), CiStatus::Passed);

        // Case insensitivity (handles both PR uppercase and API lowercase)
        let checks = vec![check("COMPLETED", Some("FAILURE"))];
        assert_eq!(aggregate_github_checks(&checks), CiStatus::Failed);

        // StatusContext via state field (used by external CI like pre-commit.ci)
        let checks = vec![GitHubCheck {
            status: None,
            conclusion: None,
            state: Some("PENDING".into()),
        }];
        assert_eq!(aggregate_github_checks(&checks), CiStatus::Running);

        let checks = vec![GitHubCheck {
            status: None,
            conclusion: None,
            state: Some("failure".into()),
        }];
        assert_eq!(aggregate_github_checks(&checks), CiStatus::Failed);
    }
}
