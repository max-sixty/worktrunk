//! GitHub CI status detection.
//!
//! Detects CI status from GitHub PRs and workflow runs using the `gh` CLI.

use serde::Deserialize;
use worktrunk::git::{Repository, parse_owner_repo, parse_remote_owner};

use super::{
    CiSource, CiStatus, MAX_PRS_TO_FETCH, PrStatus, is_retriable_error, non_interactive_cmd,
    parse_json,
};

/// Get the owner of the origin remote (for GitHub fork detection).
///
/// Used for client-side filtering of PRs by source repository.
/// See [`parse_remote_owner`] for details on why this is necessary.
fn get_origin_owner(repo: &Repository) -> Option<String> {
    let url = repo.primary_remote_url()?;
    parse_remote_owner(&url)
}

/// Get the owner and repo name from the primary remote.
///
/// Used for GitHub API calls that require `repos/{owner}/{repo}/...` paths.
fn get_owner_repo(repo: &Repository) -> Option<(String, String)> {
    let url = repo.primary_remote_url()?;
    parse_owner_repo(&url)
}

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
/// 3. Filter client-side by comparing `headRepositoryOwner.login` to our origin owner
///
/// This correctly handles:
/// - Fork workflows (PRs from your fork to upstream)
/// - Organization repos (PRs from org branches)
/// - Multiple users with same branch name
pub(super) fn detect_github(repo: &Repository, branch: &str, local_head: &str) -> Option<PrStatus> {
    let repo_root = repo.current_worktree().root().ok()?;

    // Get origin owner for filtering (see parse_remote_owner docs for why)
    let origin_owner = get_origin_owner(repo);
    if origin_owner.is_none() {
        log::debug!("Could not determine origin owner for {}", branch);
    }

    // Use `gh pr list --head` instead of `gh pr view` to handle numeric branch names correctly.
    // When branch name is all digits (e.g., "4315"), `gh pr view` interprets it as a PR number,
    // but `gh pr list --head` correctly treats it as a branch name.
    //
    // We fetch up to MAX_PRS_TO_FETCH PRs to handle branch name collisions, then filter
    // client-side by headRepositoryOwner to find PRs from our fork.
    let output = match non_interactive_cmd("gh")
        .args([
            "pr",
            "list",
            "--head",
            branch,
            "--state",
            "open",
            "--limit",
            &MAX_PRS_TO_FETCH.to_string(),
            "--json",
            "headRefOid,mergeStateStatus,statusCheckRollup,url,headRepositoryOwner",
        ])
        .current_dir(&repo_root)
        .run()
    {
        Ok(output) => output,
        Err(e) => {
            log::warn!("gh pr list failed to execute for branch {}: {}", branch, e);
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

    // gh pr list returns an array - find the first PR from our origin
    let pr_list: Vec<GitHubPrInfo> = parse_json(&output.stdout, "gh pr list", branch)?;

    // Filter to PRs from our origin (case-insensitive comparison for GitHub usernames).
    // If headRepositoryOwner is missing (older GH CLI, Enterprise, or permissions),
    // treat it as a potential match to avoid false negatives.
    let pr_info = if let Some(ref owner) = origin_owner {
        let matched = pr_list.iter().find(|pr| {
            pr.head_repository_owner
                .as_ref()
                .map(|h| h.login.eq_ignore_ascii_case(owner))
                .unwrap_or(true) // Missing owner field = potential match
        });
        if matched.is_none() && !pr_list.is_empty() {
            log::debug!(
                "Found {} PRs for branch {} but none from origin owner {}",
                pr_list.len(),
                branch,
                owner
            );
        }
        matched
    } else {
        // If we can't determine origin owner, fall back to first open PR
        // This is less accurate but better than nothing
        log::debug!(
            "No origin owner for {}, using first open PR for branch {}",
            repo_root.display(),
            branch
        );
        pr_list.first()
    }?;

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

    Some(PrStatus {
        ci_status,
        source: CiSource::PullRequest,
        is_stale,
        url: pr_info.url.clone(),
    })
}

/// Detect CI status for a commit using GitHub's check-runs API.
///
/// This queries all check runs for the commit SHA, giving us the same data
/// that `statusCheckRollup` provides for PRs. This correctly aggregates
/// status across multiple workflows (e.g., `ci` and `publish-docs`).
pub(super) fn detect_github_commit_checks(repo: &Repository, local_head: &str) -> Option<PrStatus> {
    let repo_root = repo.current_worktree().root().ok()?;
    let (owner, repo_name) = get_owner_repo(repo)?;

    // Use GitHub's check-runs API to get all checks for this commit
    let output = match non_interactive_cmd("gh")
        .args([
            "api",
            &format!("repos/{owner}/{repo_name}/commits/{local_head}/check-runs"),
            "--jq",
            ".check_runs | map({status, conclusion})",
        ])
        .current_dir(&repo_root)
        .run()
    {
        Ok(output) => output,
        Err(e) => {
            log::warn!(
                "gh api check-runs failed to execute for {}: {}",
                local_head,
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
        url: None,
    })
}

/// GitHub PR info from `gh pr list --json ...`
///
/// Note: We include `headRepositoryOwner` for client-side filtering by source fork.
/// See [`parse_remote_owner`] for why this is necessary.
///
/// Note: We don't include `state` because we already filter with `--state open`.
#[derive(Debug, Deserialize)]
pub(super) struct GitHubPrInfo {
    #[serde(rename = "headRefOid")]
    pub head_ref_oid: Option<String>,
    #[serde(rename = "mergeStateStatus")]
    pub merge_state_status: Option<String>,
    #[serde(rename = "statusCheckRollup")]
    pub status_check_rollup: Option<Vec<GitHubCheck>>,
    pub url: Option<String>,
    /// The owner of the repository the PR's head branch comes from.
    /// Used to filter PRs by source fork (see [`parse_remote_owner`]).
    #[serde(rename = "headRepositoryOwner")]
    pub head_repository_owner: Option<HeadRepositoryOwner>,
}

/// Owner info for the head repository of a PR.
#[derive(Debug, Deserialize)]
pub(super) struct HeadRepositoryOwner {
    /// The login (username/org name) of the repository owner.
    pub login: String,
}

/// A single check from `statusCheckRollup`.
///
/// This is a union of two GitHub API types with different field structures:
/// - `CheckRun` (GitHub Actions): has `status` ("COMPLETED", "IN_PROGRESS") and
///   `conclusion` ("SUCCESS", "FAILURE", "CANCELLED", "SKIPPED")
/// - `StatusContext` (external CI like pre-commit.ci): has `state` only
///   ("SUCCESS", "FAILURE", "PENDING", "ERROR")
///
/// We parse all three fields and check whichever is present. An alternative approach would be
/// `gh pr checks <number> --json state` which returns a flat array with unified `state` field,
/// but that requires a separate API call after finding the PR number. Since we also need
/// `gh run list` for branch-based CI (branches without PRs), keeping the single-call approach
/// here is simpler overall.
#[derive(Debug, Deserialize)]
pub(super) struct GitHubCheck {
    /// CheckRun only: "COMPLETED", "IN_PROGRESS", "QUEUED", etc.
    pub status: Option<String>,
    /// CheckRun only: "SUCCESS", "FAILURE", "CANCELLED", "SKIPPED", etc.
    pub conclusion: Option<String>,
    /// StatusContext only: "SUCCESS", "FAILURE", "PENDING", "ERROR"
    pub state: Option<String>,
}

impl GitHubPrInfo {
    pub fn ci_status(&self) -> CiStatus {
        match &self.status_check_rollup {
            None => CiStatus::NoCI,
            Some(checks) if checks.is_empty() => CiStatus::NoCI,
            Some(checks) => aggregate_github_checks(checks),
        }
    }
}

/// Aggregate CI status from multiple GitHub checks (case-insensitive).
///
/// Priority: running > failed > passed > no-ci.
/// Handles both `statusCheckRollup` (uppercase) and check-runs API (lowercase).
/// Skipped/neutral checks don't contribute to pass/fail.
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
                "in_progress" | "queued" | "pending" | "expected"
            ) {
                has_running = true;
            }
        }

        // StatusContext: state field indicates pending
        if let Some(state) = &check.state {
            let s = state.to_ascii_lowercase();
            if s == "pending" {
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
                "failure" | "error" | "cancelled" | "timed_out" | "action_required" => {
                    has_failure = true;
                }
                "success" => {
                    has_success = true;
                }
                // "skipped", "neutral" - ignored
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

    #[test]
    fn test_github_pr_info_ci_status() {
        // No checks = NoCI
        let pr = GitHubPrInfo {
            head_ref_oid: None,
            merge_state_status: None,
            status_check_rollup: None,
            url: None,
            head_repository_owner: None,
        };
        assert_eq!(pr.ci_status(), CiStatus::NoCI);

        // Empty checks = NoCI
        let pr = GitHubPrInfo {
            head_ref_oid: None,
            merge_state_status: None,
            status_check_rollup: Some(vec![]),
            url: None,
            head_repository_owner: None,
        };
        assert_eq!(pr.ci_status(), CiStatus::NoCI);

        // CheckRun pending states
        for status in ["IN_PROGRESS", "QUEUED", "PENDING", "EXPECTED"] {
            let pr = GitHubPrInfo {
                head_ref_oid: None,
                merge_state_status: None,
                status_check_rollup: Some(vec![GitHubCheck {
                    status: Some(status.into()),
                    conclusion: None,
                    state: None,
                }]),
                url: None,
                head_repository_owner: None,
            };
            assert_eq!(pr.ci_status(), CiStatus::Running, "status={status}");
        }

        // StatusContext pending
        let pr = GitHubPrInfo {
            head_ref_oid: None,
            merge_state_status: None,
            status_check_rollup: Some(vec![GitHubCheck {
                status: None,
                conclusion: None,
                state: Some("PENDING".into()),
            }]),
            url: None,
            head_repository_owner: None,
        };
        assert_eq!(pr.ci_status(), CiStatus::Running);

        // CheckRun failures
        for conclusion in ["FAILURE", "ERROR", "CANCELLED"] {
            let pr = GitHubPrInfo {
                head_ref_oid: None,
                merge_state_status: None,
                status_check_rollup: Some(vec![GitHubCheck {
                    status: Some("COMPLETED".into()),
                    conclusion: Some(conclusion.into()),
                    state: None,
                }]),
                url: None,
                head_repository_owner: None,
            };
            assert_eq!(pr.ci_status(), CiStatus::Failed, "conclusion={conclusion}");
        }

        // StatusContext failures
        for state in ["FAILURE", "ERROR"] {
            let pr = GitHubPrInfo {
                head_ref_oid: None,
                merge_state_status: None,
                status_check_rollup: Some(vec![GitHubCheck {
                    status: None,
                    conclusion: None,
                    state: Some(state.into()),
                }]),
                url: None,
                head_repository_owner: None,
            };
            assert_eq!(pr.ci_status(), CiStatus::Failed, "state={state}");
        }

        // Success
        let pr = GitHubPrInfo {
            head_ref_oid: None,
            merge_state_status: None,
            status_check_rollup: Some(vec![GitHubCheck {
                status: Some("COMPLETED".into()),
                conclusion: Some("SUCCESS".into()),
                state: None,
            }]),
            url: None,
            head_repository_owner: None,
        };
        assert_eq!(pr.ci_status(), CiStatus::Passed);
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

        // All skipped = NoCI (skipped doesn't count as success)
        let checks = vec![
            check("completed", Some("skipped")),
            check("completed", Some("neutral")),
        ];
        assert_eq!(aggregate_github_checks(&checks), CiStatus::NoCI);

        // Any running = Running
        for status in ["in_progress", "queued", "pending"] {
            let checks = vec![check("completed", Some("success")), check(status, None)];
            assert_eq!(
                aggregate_github_checks(&checks),
                CiStatus::Running,
                "status={status}"
            );
        }

        // Any failure among completed checks = Failed
        for conclusion in ["failure", "cancelled", "timed_out", "action_required"] {
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
