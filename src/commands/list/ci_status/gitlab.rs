//! GitLab CI status detection.
//!
//! Detects CI status from GitLab MRs and pipelines using the `glab` CLI.
//!
//! # Two-Step MR Resolution
//!
//! Getting complete MR details (including pipeline status) requires two `glab` calls:
//!
//! 1. `glab mr list --source-branch <branch>` - Returns basic MR info including `iid`
//!    but NOT `head_pipeline` or `pipeline` fields.
//!
//! 2. `glab mr view <iid> --output json` - Returns complete MR details including
//!    `head_pipeline` and `pipeline` fields.
//!
//! See: <https://github.com/max-sixty/worktrunk/issues/764>

use serde::Deserialize;
use std::path::Path;
use worktrunk::git::Repository;

use super::{
    CiBranchName, CiSource, CiStatus, MAX_PRS_TO_FETCH, PrStatus, is_retriable_error,
    non_interactive_cmd, parse_json,
};

/// Get the GitLab project ID for a repository.
///
/// Used for client-side filtering of MRs by source project.
/// This is the GitLab equivalent of `get_origin_owner` for GitHub.
///
/// Returns None if glab is not configured for this repo (e.g., non-GitLab
/// remote, auth issues).
fn get_gitlab_project_id(repo: &Repository) -> Option<u64> {
    let repo_root = repo.current_worktree().root().ok()?;

    // Use glab repo view to get the project info as JSON
    // Disable color/pager to avoid ANSI noise in JSON output
    let output = non_interactive_cmd("glab")
        .args(["repo", "view", "--output", "json"])
        .current_dir(&repo_root)
        .env("PAGER", "cat")
        .run()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    // Parse the JSON to extract the project ID
    #[derive(Deserialize)]
    struct RepoInfo {
        id: u64,
    }

    serde_json::from_slice::<RepoInfo>(&output.stdout)
        .ok()
        .map(|info| info.id)
}

/// Detect GitLab MR CI status for a branch.
///
/// # Filtering Strategy
///
/// Similar to GitHub (see `detect_github`), we need to find MRs where the
/// source branch comes from *our* project, not just MRs we authored.
///
/// Since `glab mr list` doesn't support filtering by source project, we:
/// 1. Get the current project ID via `glab repo view`
/// 2. Fetch all open MRs with matching branch name (up to 20)
/// 3. Filter client-side by comparing `source_project_id` to our project ID
pub(super) fn detect_gitlab(
    repo: &Repository,
    branch: &CiBranchName,
    local_head: &str,
) -> Option<PrStatus> {
    let repo_root = repo.current_worktree().root().ok()?;

    // Get current project ID for filtering
    let project_id = get_gitlab_project_id(repo);
    if project_id.is_none() {
        log::debug!("Could not determine GitLab project ID");
    }

    // Fetch MRs with matching source branch.
    // IMPORTANT: Use the bare branch name (branch.name), not the full remote ref.
    // `glab mr list --source-branch origin/feature` won't find anything - it needs just "feature".
    // Note: glab mr list returns open MRs by default, no --state flag needed.
    // We filter client-side by source_project_id (numeric project ID comparison).
    let output = match non_interactive_cmd("glab")
        .args([
            "mr",
            "list",
            "--source-branch",
            &branch.name, // Use bare branch name, not "origin/feature"
            &format!("--per-page={}", MAX_PRS_TO_FETCH),
            "--output",
            "json",
        ])
        .current_dir(&repo_root)
        .run()
    {
        Ok(output) => output,
        Err(e) => {
            log::warn!(
                "glab mr list failed to execute for branch {}: {}",
                branch.full_name,
                e
            );
            return None;
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Return error status for retriable failures (rate limit, network) so they
        // surface as warnings instead of being cached as "no CI"
        if is_retriable_error(&stderr) {
            return Some(PrStatus::error());
        }
        return None;
    }

    // Step 1: Parse mr list output to find matching MR.
    // Note: glab mr list does NOT return head_pipeline/pipeline fields.
    let mr_list: Vec<GitLabMrListEntry> =
        parse_json(&output.stdout, "glab mr list", &branch.full_name)?;

    // Filter to MRs from our project (numeric project ID comparison)
    let mr_entry = if let Some(proj_id) = project_id {
        let matched = mr_list
            .iter()
            .find(|mr| mr.source_project_id == Some(proj_id));
        if matched.is_none() && !mr_list.is_empty() {
            log::debug!(
                "Found {} MRs for branch {} but none from project ID {}",
                mr_list.len(),
                branch.full_name,
                proj_id
            );
        }
        matched
    } else if mr_list.len() == 1 {
        // If we can't determine project ID but there's only one MR, it's unambiguous
        mr_list.first()
    } else if mr_list.is_empty() {
        // No MRs found
        None
    } else {
        // Multiple MRs exist but we can't determine which project we're in.
        // Don't guess - return None to avoid showing wrong project's CI status.
        log::debug!(
            "Found {} MRs for branch {} but no project ID to filter - skipping to avoid ambiguity",
            mr_list.len(),
            branch.full_name
        );
        None
    }?;

    // Step 2: Fetch full MR details to get pipeline status.
    // This requires a second glab call because mr list doesn't include head_pipeline.
    let mr_info = fetch_mr_details(mr_entry.iid, &repo_root);

    // Determine CI status using priority: conflicts > running > pipeline status > no_ci
    // Use mr_entry for basic info (available from list), mr_info for pipeline status
    //
    // Note: "ci_must_pass" is a policy constraint ("CI must pass to merge"), NOT a failure
    // indicator. We let it fall through to the actual pipeline status.
    let ci_status = if mr_entry.has_conflicts
        || mr_entry.detailed_merge_status.as_deref() == Some("conflict")
    {
        CiStatus::Conflicts
    } else if mr_entry.detailed_merge_status.as_deref() == Some("ci_still_running") {
        CiStatus::Running
    } else if let Some(ref info) = mr_info {
        info.ci_status()
    } else {
        // Found MR but couldn't fetch details - treat as error so it surfaces
        // (not NoCI, which would imply no MR exists)
        log::debug!("Could not fetch MR details for !{}", mr_entry.iid);
        return Some(PrStatus::error());
    };

    let is_stale = mr_entry.sha != local_head;

    Some(PrStatus {
        ci_status,
        source: CiSource::PullRequest,
        is_stale,
        url: mr_entry.web_url.clone(),
    })
}

/// Detect GitLab pipeline status for a branch (when no MR exists).
pub(super) fn detect_gitlab_pipeline(branch: &str, local_head: &str) -> Option<PrStatus> {
    // Get most recent pipeline for the branch using JSON output
    let output = match non_interactive_cmd("glab")
        .args([
            "ci",
            "list",
            "--ref",
            branch,
            "--per-page",
            "1",
            "--output",
            "json",
        ])
        .run()
    {
        Ok(output) => output,
        Err(e) => {
            log::warn!(
                "glab ci list failed to execute for branch {}: {}",
                branch,
                e
            );
            return None;
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Return error status for retriable failures (rate limit, network) so they
        // surface as warnings instead of being cached as "no CI"
        if is_retriable_error(&stderr) {
            return Some(PrStatus::error());
        }
        return None;
    }

    let pipelines: Vec<GitLabPipeline> = parse_json(&output.stdout, "glab ci list", branch)?;
    let pipeline = pipelines.first()?;

    // Check if the pipeline matches our local HEAD commit
    let is_stale = pipeline
        .sha
        .as_ref()
        .map(|pipeline_sha| pipeline_sha != local_head)
        .unwrap_or(true); // If no SHA, consider it stale

    let ci_status = pipeline.ci_status();

    Some(PrStatus {
        ci_status,
        source: CiSource::Branch,
        is_stale,
        url: pipeline.web_url.clone(),
    })
}

/// Basic MR info from `glab mr list --output json`.
///
/// Note: `glab mr list` does NOT return `head_pipeline` or `pipeline` fields.
/// Use [`fetch_mr_details`] with the `iid` to get complete MR info.
///
/// We include `source_project_id` for client-side filtering by source project.
/// See [`worktrunk::git::parse_remote_owner`] for why we filter by source, not by author.
#[derive(Debug, Deserialize)]
struct GitLabMrListEntry {
    /// The internal MR ID (used to fetch full details via `glab mr view <iid>`)
    pub iid: u64,
    pub sha: String,
    pub has_conflicts: bool,
    pub detailed_merge_status: Option<String>,
    /// The source project ID (the project the MR's branch comes from).
    pub source_project_id: Option<u64>,
    /// URL to the MR page for clickable links
    pub web_url: Option<String>,
}

/// Full MR info from `glab mr view <iid> --output json`.
///
/// This includes pipeline status that isn't available from `glab mr list`.
/// We only need the pipeline fields here since basic MR info comes from
/// [`GitLabMrListEntry`].
#[derive(Debug, Deserialize)]
pub(super) struct GitLabMrInfo {
    pub head_pipeline: Option<GitLabPipeline>,
    pub pipeline: Option<GitLabPipeline>,
}

impl GitLabMrInfo {
    pub fn ci_status(&self) -> CiStatus {
        self.head_pipeline
            .as_ref()
            .or(self.pipeline.as_ref())
            .map(GitLabPipeline::ci_status)
            .unwrap_or(CiStatus::NoCI)
    }
}

/// Fetch full MR details using `glab mr view <iid>`.
///
/// This is the second step in the two-step MR resolution process.
/// Returns None if the command fails or returns invalid JSON.
fn fetch_mr_details(iid: u64, repo_root: &Path) -> Option<GitLabMrInfo> {
    let output = non_interactive_cmd("glab")
        .args(["mr", "view", &iid.to_string(), "--output", "json"])
        .current_dir(repo_root)
        .run()
        .ok()?;

    if !output.status.success() {
        log::debug!("glab mr view {} failed", iid);
        return None;
    }

    parse_json(&output.stdout, "glab mr view", &iid.to_string())
}

#[derive(Debug, Deserialize)]
pub(super) struct GitLabPipeline {
    pub status: Option<String>,
    /// Only present in `glab ci list` output, not in MR view embedded pipeline
    #[serde(default)]
    pub sha: Option<String>,
    /// URL to the pipeline page for clickable links
    #[serde(default)]
    pub web_url: Option<String>,
}

fn parse_gitlab_status(status: Option<&str>) -> CiStatus {
    match status {
        // "manual" = pipeline waiting for user to trigger a manual job (not failed)
        Some(
            "running"
            | "pending"
            | "preparing"
            | "waiting_for_resource"
            | "created"
            | "scheduled"
            | "manual",
        ) => CiStatus::Running,
        Some("failed" | "canceled") => CiStatus::Failed,
        Some("success") => CiStatus::Passed,
        Some("skipped") | None => CiStatus::NoCI,
        _ => CiStatus::NoCI,
    }
}

impl GitLabPipeline {
    pub fn ci_status(&self) -> CiStatus {
        parse_gitlab_status(self.status.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_gitlab_status() {
        // Running states (includes "manual" - waiting for user to trigger)
        for status in [
            "running",
            "pending",
            "preparing",
            "waiting_for_resource",
            "created",
            "scheduled",
            "manual",
        ] {
            assert_eq!(
                parse_gitlab_status(Some(status)),
                CiStatus::Running,
                "status={status}"
            );
        }

        // Failed states
        for status in ["failed", "canceled"] {
            assert_eq!(
                parse_gitlab_status(Some(status)),
                CiStatus::Failed,
                "status={status}"
            );
        }

        // Success
        assert_eq!(parse_gitlab_status(Some("success")), CiStatus::Passed);

        // NoCI states
        assert_eq!(parse_gitlab_status(Some("skipped")), CiStatus::NoCI);
        assert_eq!(parse_gitlab_status(None), CiStatus::NoCI);
        assert_eq!(parse_gitlab_status(Some("unknown")), CiStatus::NoCI);
    }

    #[test]
    fn test_gitlab_mr_info_ci_status() {
        // No pipeline = NoCI
        let mr = GitLabMrInfo {
            head_pipeline: None,
            pipeline: None,
        };
        assert_eq!(mr.ci_status(), CiStatus::NoCI);

        // head_pipeline takes precedence
        let mr = GitLabMrInfo {
            head_pipeline: Some(GitLabPipeline {
                status: Some("success".into()),
                sha: None,
                web_url: None,
            }),
            pipeline: Some(GitLabPipeline {
                status: Some("failed".into()),
                sha: None,
                web_url: None,
            }),
        };
        assert_eq!(mr.ci_status(), CiStatus::Passed);

        // Falls back to pipeline if no head_pipeline
        let mr = GitLabMrInfo {
            head_pipeline: None,
            pipeline: Some(GitLabPipeline {
                status: Some("running".into()),
                sha: None,
                web_url: None,
            }),
        };
        assert_eq!(mr.ci_status(), CiStatus::Running);
    }
}
