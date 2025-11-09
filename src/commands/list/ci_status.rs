use serde::{Deserialize, Serialize};
use std::process::Command;

/// CI status from GitHub/GitLab checks
/// Matches the statusline.sh color scheme:
/// - Passed: Green (all checks passed)
/// - Running: Blue (checks in progress)
/// - Failed: Red (checks failed)
/// - Conflicts: Yellow (merge conflicts)
/// - NoCI: Gray (no PR/checks)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CiStatus {
    Passed,
    Running,
    Failed,
    Conflicts,
    NoCI,
}

/// PR/MR status including CI state and staleness
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrStatus {
    pub ci_status: CiStatus,
    /// True if local HEAD differs from PR HEAD (unpushed changes)
    pub is_stale: bool,
}

impl PrStatus {
    /// Detect CI status for a branch using gh/glab CLI
    /// First tries to find PR/MR status, then falls back to workflow/pipeline runs
    /// Returns None if no CI found or CLI tools unavailable
    pub fn detect(branch: &str, local_head: &str) -> Option<Self> {
        // Get GitHub repo for gh commands (from git remote)
        let github_repo = Self::get_github_repo();

        // Get git repo root directory for setting working directory
        let repo_root = Self::get_repo_root();

        // Try GitHub PR first
        if let Some(status) = Self::detect_github(
            branch,
            local_head,
            github_repo.as_deref(),
            repo_root.as_deref(),
        ) {
            return Some(status);
        }

        // Try GitHub workflow runs (for branches without PRs)
        if let Some(status) = Self::detect_github_workflow(
            branch,
            local_head,
            github_repo.as_deref(),
            repo_root.as_deref(),
        ) {
            return Some(status);
        }

        // Try GitLab MR
        if let Some(status) = Self::detect_gitlab(branch, local_head) {
            return Some(status);
        }

        // Fall back to GitLab pipeline (for branches without MRs)
        Self::detect_gitlab_pipeline(branch, local_head)
    }

    /// Get git repository root directory
    fn get_repo_root() -> Option<String> {
        let output = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        Some(String::from_utf8(output.stdout).ok()?.trim().to_string())
    }

    /// Extract GitHub repository owner/name from git remote URL
    /// Returns None if not a GitHub repo or if git command fails
    fn get_github_repo() -> Option<String> {
        let output = Command::new("git")
            .args(["remote", "get-url", "origin"])
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let url = String::from_utf8(output.stdout).ok()?.trim().to_string();

        // Parse GitHub URL - handles both HTTPS and SSH formats
        // HTTPS: https://github.com/owner/repo.git
        // SSH: git@github.com:owner/repo.git
        if !url.contains("github.com") {
            return None;
        }

        // Find the part after "github.com:" or "github.com/"
        let after_github = if let Some(pos) = url.find("github.com:") {
            &url[pos + "github.com:".len()..]
        } else if let Some(pos) = url.find("github.com/") {
            &url[pos + "github.com/".len()..]
        } else {
            return None;
        };

        // Remove .git suffix if present
        let repo_part = after_github.strip_suffix(".git").unwrap_or(after_github);

        // Extract owner/repo (should be of form "owner/repo")
        if repo_part.split('/').count() >= 2 {
            Some(repo_part.to_string())
        } else {
            None
        }
    }

    fn detect_github(
        branch: &str,
        local_head: &str,
        repo: Option<&str>,
        repo_root: Option<&str>,
    ) -> Option<Self> {
        // Check if gh is available and authenticated
        if !Command::new("gh")
            .args(["auth", "status"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return None;
        }

        // Get PR info for the branch
        let mut cmd = Command::new("gh");
        cmd.args([
            "pr",
            "view",
            branch,
            "--json",
            "state,headRefOid,mergeStateStatus,statusCheckRollup",
        ]);

        // Remove environment variables that force color output
        cmd.env_remove("CLICOLOR_FORCE");
        cmd.env_remove("GH_FORCE_TTY");
        cmd.env("NO_COLOR", "1");
        cmd.env("CLICOLOR", "0");

        if let Some(r) = repo {
            cmd.args(["--repo", r]);
        }

        if let Some(root) = repo_root {
            cmd.current_dir(root);
        }

        let output = cmd.output().ok()?;

        if !output.status.success() {
            return None;
        }

        let pr_info: GitHubPrInfo = serde_json::from_slice(&output.stdout).ok()?;

        // Only process open PRs
        if pr_info.state != "OPEN" {
            return None;
        }

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
            is_stale,
        })
    }

    fn detect_gitlab(branch: &str, local_head: &str) -> Option<Self> {
        // Check if glab is available
        if !Command::new("glab")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return None;
        }

        // Get MR info for the branch
        let output = Command::new("glab")
            .args(["mr", "view", branch, "--output", "json"])
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let mr_info: GitLabMrInfo = serde_json::from_slice(&output.stdout).ok()?;

        // Only process open MRs
        if mr_info.state != "opened" {
            return None;
        }

        // Determine CI status using priority: conflicts > running > failed > passed > no_ci
        let ci_status = if mr_info.has_conflicts
            || mr_info.detailed_merge_status.as_deref() == Some("conflict")
        {
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
            is_stale,
        })
    }

    fn detect_github_workflow(
        branch: &str,
        _local_head: &str,
        repo: Option<&str>,
        repo_root: Option<&str>,
    ) -> Option<Self> {
        // Check if gh is available and authenticated
        if !Command::new("gh")
            .args(["auth", "status"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return None;
        }

        // Get most recent workflow run for the branch
        let mut cmd = Command::new("gh");
        cmd.args([
            "run",
            "list",
            "--branch",
            branch,
            "--limit",
            "1",
            "--json",
            "status,conclusion",
        ]);

        // Remove environment variables that force color output even when piped
        // CLICOLOR_FORCE and GH_FORCE_TTY override NO_COLOR and TTY detection
        cmd.env_remove("CLICOLOR_FORCE");
        cmd.env_remove("GH_FORCE_TTY");
        // Request no color output
        cmd.env("NO_COLOR", "1");
        cmd.env("CLICOLOR", "0");

        if let Some(r) = repo {
            cmd.args(["--repo", r]);
        }

        if let Some(root) = repo_root {
            cmd.current_dir(root);
        }

        let output = cmd.output().ok()?;

        if !output.status.success() {
            return None;
        }

        let runs: Vec<GitHubWorkflowRun> = serde_json::from_slice(&output.stdout).ok()?;
        let run = runs.first()?;

        // Analyze workflow run status
        let ci_status = run.ci_status();

        // Workflow runs don't have staleness concept (no PR to compare against)
        Some(PrStatus {
            ci_status,
            is_stale: false,
        })
    }

    fn detect_gitlab_pipeline(branch: &str, _local_head: &str) -> Option<Self> {
        // Check if glab is available
        if !Command::new("glab")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return None;
        }

        // Get most recent pipeline for the branch
        let output = Command::new("glab")
            .args(["ci", "list", "--per-page", "1"])
            .env("BRANCH", branch) // glab ci list uses BRANCH env var
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        // Parse glab ci list output (format: "• (<status>) <pipeline-info>")
        let output_str = String::from_utf8(output.stdout).ok()?;
        let first_line = output_str.lines().next()?;

        // Extract status from format like "• (running) #12345"
        let status_start = first_line.find('(')?;
        let status_end = first_line.find(')')?;
        let status = first_line[status_start + 1..status_end].to_string();
        let ci_status = GitLabPipeline {
            status: Some(status),
        }
        .ci_status();

        Some(PrStatus {
            ci_status,
            is_stale: false,
        })
    }
}

#[derive(Debug, Deserialize)]
struct GitHubPrInfo {
    state: String,
    #[serde(rename = "headRefOid")]
    head_ref_oid: Option<String>,
    #[serde(rename = "mergeStateStatus")]
    merge_state_status: Option<String>,
    #[serde(rename = "statusCheckRollup")]
    status_check_rollup: Option<Vec<GitHubCheck>>,
}

#[derive(Debug, Deserialize)]
struct GitHubCheck {
    status: Option<String>,
    conclusion: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GitHubWorkflowRun {
    status: Option<String>,
    conclusion: Option<String>,
}

impl GitHubPrInfo {
    fn ci_status(&self) -> CiStatus {
        let Some(checks) = &self.status_check_rollup else {
            return CiStatus::NoCI;
        };

        if checks.is_empty() {
            return CiStatus::NoCI;
        }

        let has_pending = checks.iter().any(|c| {
            matches!(
                c.status.as_deref(),
                Some("IN_PROGRESS" | "QUEUED" | "PENDING" | "EXPECTED")
            )
        });

        let has_failure = checks.iter().any(|c| {
            matches!(
                c.conclusion.as_deref(),
                Some("FAILURE" | "ERROR" | "CANCELLED")
            )
        });

        if has_pending {
            CiStatus::Running
        } else if has_failure {
            CiStatus::Failed
        } else {
            CiStatus::Passed
        }
    }
}

impl GitHubWorkflowRun {
    fn ci_status(&self) -> CiStatus {
        match self.status.as_deref() {
            Some("in_progress" | "queued" | "pending" | "waiting") => CiStatus::Running,
            Some("completed") => match self.conclusion.as_deref() {
                Some("success") => CiStatus::Passed,
                Some("failure" | "cancelled" | "timed_out" | "action_required") => CiStatus::Failed,
                Some("skipped" | "neutral") | None => CiStatus::NoCI,
                _ => CiStatus::NoCI,
            },
            _ => CiStatus::NoCI,
        }
    }
}

#[derive(Debug, Deserialize)]
struct GitLabMrInfo {
    state: String,
    sha: String,
    has_conflicts: bool,
    detailed_merge_status: Option<String>,
    head_pipeline: Option<GitLabPipeline>,
    pipeline: Option<GitLabPipeline>,
}

impl GitLabMrInfo {
    fn ci_status(&self) -> CiStatus {
        self.head_pipeline
            .as_ref()
            .or(self.pipeline.as_ref())
            .map(GitLabPipeline::ci_status)
            .unwrap_or(CiStatus::NoCI)
    }
}

#[derive(Debug, Deserialize)]
struct GitLabPipeline {
    status: Option<String>,
}

impl GitLabPipeline {
    fn ci_status(&self) -> CiStatus {
        match self.status.as_deref() {
            Some(
                "running"
                | "pending"
                | "preparing"
                | "waiting_for_resource"
                | "created"
                | "scheduled",
            ) => CiStatus::Running,
            Some("failed" | "canceled" | "manual") => CiStatus::Failed,
            Some("success") => CiStatus::Passed,
            Some("skipped") | None => CiStatus::NoCI,
            _ => CiStatus::NoCI,
        }
    }
}
