//! GitLab CI status detection.
//!
//! Detects CI status from GitLab MRs and pipelines using the `glab` CLI.

use serde::Deserialize;
use worktrunk::git::Repository;
use worktrunk::shell_exec::Cmd;

use super::{
    CiSource, CiStatus, MAX_PRS_TO_FETCH, PrStatus, is_retriable_error, non_interactive_cmd,
    parse_json, tool_available,
};

/// Get the GitLab project ID for a repository.
///
/// Used for client-side filtering of MRs by source project.
/// This is the GitLab equivalent of `get_origin_owner` for GitHub.
///
/// Returns None if glab is not available or not configured for this repo.
///
/// # Performance Note
///
/// This function is called during GitLab detection regardless of whether
/// the repo is actually GitLab-hosted. If glab is installed but the repo
/// is GitHub, this adds an unnecessary CLI call. A future optimization
/// could check the remote URL first and skip for non-GitLab remotes.
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
pub(super) fn detect_gitlab(repo: &Repository, branch: &str, local_head: &str) -> Option<PrStatus> {
    if !tool_available("glab", &["--version"]) {
        return None;
    }

    let repo_root = repo.current_worktree().root().ok()?;

    // Get current project ID for filtering
    let project_id = get_gitlab_project_id(repo);
    if project_id.is_none() {
        log::debug!("Could not determine GitLab project ID");
    }

    // Fetch MRs with matching source branch.
    // We filter client-side by source_project_id (numeric project ID comparison).
    let output = match Cmd::new("glab")
        .args([
            "mr",
            "list",
            "--source-branch",
            branch,
            "--state=opened",
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

    // glab mr list returns an array - find the first MR from our project
    let mr_list: Vec<GitLabMrInfo> = parse_json(&output.stdout, "glab mr list", branch)?;

    // Filter to MRs from our project (numeric project ID comparison)
    let mr_info = if let Some(proj_id) = project_id {
        let matched = mr_list
            .iter()
            .find(|mr| mr.source_project_id == Some(proj_id));
        if matched.is_none() && !mr_list.is_empty() {
            log::debug!(
                "Found {} MRs for branch {} but none from project ID {}",
                mr_list.len(),
                branch,
                proj_id
            );
        }
        matched
    } else {
        // If we can't determine project ID, fall back to first MR
        log::debug!(
            "No project ID for {}, using first MR for branch {}",
            repo_root.display(),
            branch
        );
        mr_list.first()
    }?;

    // Determine CI status using priority: conflicts > running > failed > passed > no_ci
    let ci_status =
        if mr_info.has_conflicts || mr_info.detailed_merge_status.as_deref() == Some("conflict") {
            CiStatus::Conflicts
        } else if mr_info.detailed_merge_status.as_deref() == Some("ci_still_running") {
            CiStatus::Running
        } else if mr_info.detailed_merge_status.as_deref() == Some("ci_must_pass") {
            CiStatus::Failed
        } else {
            mr_info.ci_status()
        };

    let is_stale = mr_info.sha != local_head;

    Some(PrStatus {
        ci_status,
        source: CiSource::PullRequest,
        is_stale,
        url: mr_info.web_url.clone(),
    })
}

/// Detect GitLab pipeline status for a branch (when no MR exists).
pub(super) fn detect_gitlab_pipeline(branch: &str, local_head: &str) -> Option<PrStatus> {
    if !tool_available("glab", &["--version"]) {
        return None;
    }

    // Get most recent pipeline for the branch using JSON output
    let output = match Cmd::new("glab")
        .args(["ci", "list", "--per-page", "1", "--output", "json"])
        .env("BRANCH", branch) // glab ci list uses BRANCH env var
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

/// GitLab MR info from `glab mr list --output json`
///
/// Note: We include `source_project_id` for client-side filtering by source project.
/// See [`worktrunk::git::parse_remote_owner`] for why we filter by source, not by author.
#[derive(Debug, Deserialize)]
pub(super) struct GitLabMrInfo {
    pub sha: String,
    pub has_conflicts: bool,
    pub detailed_merge_status: Option<String>,
    pub head_pipeline: Option<GitLabPipeline>,
    pub pipeline: Option<GitLabPipeline>,
    /// The source project ID (the project the MR's branch comes from).
    /// Used to filter MRs by source project.
    pub source_project_id: Option<u64>,
    /// URL to the MR page for clickable links
    pub web_url: Option<String>,
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
        Some(
            "running" | "pending" | "preparing" | "waiting_for_resource" | "created" | "scheduled",
        ) => CiStatus::Running,
        Some("failed" | "canceled" | "manual") => CiStatus::Failed,
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
        // Running states
        for status in [
            "running",
            "pending",
            "preparing",
            "waiting_for_resource",
            "created",
            "scheduled",
        ] {
            assert_eq!(
                parse_gitlab_status(Some(status)),
                CiStatus::Running,
                "status={status}"
            );
        }

        // Failed states
        for status in ["failed", "canceled", "manual"] {
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
            sha: "abc".into(),
            has_conflicts: false,
            detailed_merge_status: None,
            head_pipeline: None,
            pipeline: None,
            source_project_id: None,
            web_url: None,
        };
        assert_eq!(mr.ci_status(), CiStatus::NoCI);

        // head_pipeline takes precedence
        let mr = GitLabMrInfo {
            sha: "abc".into(),
            has_conflicts: false,
            detailed_merge_status: None,
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
            source_project_id: None,
            web_url: None,
        };
        assert_eq!(mr.ci_status(), CiStatus::Passed);

        // Falls back to pipeline if no head_pipeline
        let mr = GitLabMrInfo {
            sha: "abc".into(),
            has_conflicts: false,
            detailed_merge_status: None,
            head_pipeline: None,
            pipeline: Some(GitLabPipeline {
                status: Some("running".into()),
                sha: None,
                web_url: None,
            }),
            source_project_id: None,
            web_url: None,
        };
        assert_eq!(mr.ci_status(), CiStatus::Running);
    }
}
