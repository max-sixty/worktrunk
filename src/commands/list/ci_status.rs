use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::process::Command;

/// Configure command to disable color output
fn disable_color_output(cmd: &mut Command) {
    cmd.env_remove("CLICOLOR_FORCE");
    cmd.env_remove("GH_FORCE_TTY");
    cmd.env("NO_COLOR", "1");
    cmd.env("CLICOLOR", "0");
}

/// Check if a CLI tool is available
fn tool_available(tool: &str, args: &[&str]) -> bool {
    Command::new(tool)
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Parse JSON output from CLI tools
fn parse_json<T: DeserializeOwned>(stdout: &[u8], command: &str, branch: &str) -> Option<T> {
    serde_json::from_slice(stdout)
        .map_err(|e| log::warn!("Failed to parse {} JSON for {}: {}", command, branch, e))
        .ok()
}

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

/// Source of CI status
///
/// TODO: Current visual distinction (● for PR, ○ for branch) means main branch
/// always shows hollow circle when running branch CI. This may not be ideal.
/// Possible improvements:
/// - Use different symbols entirely (e.g., ● vs ◎ double circle, ● vs ⊙ circled dot)
/// - Add a third state for "primary branch" (main/master)
/// - Use different shape families (e.g., ● circle vs ■ square, ● vs ◆ diamond)
/// - Consider directional symbols for branch CI (e.g., ▶ right arrow)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CiSource {
    /// Pull request or merge request
    PullRequest,
    /// Branch workflow/pipeline (no PR/MR)
    Branch,
}

/// PR/MR status including CI state and staleness
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrStatus {
    pub ci_status: CiStatus,
    /// Source of the CI status (PR/MR or branch workflow)
    pub source: CiSource,
    /// True if local HEAD differs from PR HEAD (unpushed changes)
    pub is_stale: bool,
    /// URL to the PR/MR (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl PrStatus {
    /// Detect CI status for a branch using gh/glab CLI
    /// First tries to find PR/MR status, then falls back to workflow/pipeline runs
    /// Returns None if no CI found or CLI tools unavailable
    ///
    /// # Fork Support
    /// Runs gh commands from the repository directory to enable auto-detection of
    /// upstream repositories for forks. This ensures PRs opened against upstream
    /// repos are properly detected.
    ///
    /// # Arguments
    /// * `repo_path` - Repository root path from `Repository::worktree_root()`
    pub fn detect(branch: &str, local_head: &str, repo_path: &std::path::Path) -> Option<Self> {
        // We run gh/glab commands from the repo directory to let them auto-detect the correct repo
        // (including upstream repos for forks)
        let repo_root = repo_path.to_str().expect("repo path is not valid UTF-8");

        // Try GitHub PR first
        if let Some(status) = Self::detect_github(branch, local_head, repo_root) {
            return Some(status);
        }

        // Try GitHub workflow runs (for branches without PRs)
        if let Some(status) = Self::detect_github_workflow(branch, local_head, repo_root) {
            return Some(status);
        }

        // Try GitLab MR
        if let Some(status) = Self::detect_gitlab(branch, local_head) {
            return Some(status);
        }

        // Fall back to GitLab pipeline (for branches without MRs)
        Self::detect_gitlab_pipeline(branch, local_head)
    }

    fn detect_github(branch: &str, local_head: &str, repo_root: &str) -> Option<Self> {
        // Check if gh is available and authenticated
        let auth = Command::new("gh").args(["auth", "status"]).output();
        match auth {
            Err(e) => {
                log::debug!("gh not available for {}: {}", branch, e);
                return None;
            }
            Ok(o) if !o.status.success() => {
                log::debug!("gh not authenticated for {}", branch);
                return None;
            }
            _ => {}
        }

        // Use `gh pr list --head` instead of `gh pr view` to handle numeric branch names correctly.
        // When branch name is all digits (e.g., "4315"), `gh pr view` interprets it as a PR number,
        // but `gh pr list --head` correctly treats it as a branch name.
        let mut cmd = Command::new("gh");
        cmd.args([
            "pr",
            "list",
            "--head",
            branch,
            "--limit",
            "1",
            "--json",
            "state,headRefOid,mergeStateStatus,statusCheckRollup,url",
        ]);

        disable_color_output(&mut cmd);
        cmd.current_dir(repo_root);

        let output = match cmd.output() {
            Ok(output) => output,
            Err(e) => {
                log::warn!("gh pr list failed to execute for branch {}: {}", branch, e);
                return None;
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            log::debug!("gh pr list failed for {}: {}", branch, stderr.trim());
            return None;
        }

        // gh pr list returns an array, take the first (and only) item
        let pr_list: Vec<GitHubPrInfo> = parse_json(&output.stdout, "gh pr list", branch)?;
        let pr_info = pr_list.first()?;

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
            source: CiSource::PullRequest,
            is_stale,
            url: pr_info.url.clone(),
        })
    }

    fn detect_gitlab(branch: &str, local_head: &str) -> Option<Self> {
        if !tool_available("glab", &["--version"]) {
            return None;
        }

        // Get MR info for the branch
        let output = match Command::new("glab")
            .args(["mr", "view", branch, "--output", "json"])
            .output()
        {
            Ok(output) => output,
            Err(e) => {
                log::warn!(
                    "glab mr view failed to execute for branch {}: {}",
                    branch,
                    e
                );
                return None;
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            log::debug!("glab mr view failed for {}: {}", branch, stderr.trim());
            return None;
        }

        let mr_info: GitLabMrInfo = parse_json(&output.stdout, "glab mr view", branch)?;

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
            source: CiSource::PullRequest,
            is_stale,
            // TODO: Fetch GitLab MR URL from glab output to enable clickable links
            // Currently only GitHub PRs have clickable underlined indicators
            url: None,
        })
    }

    fn detect_github_workflow(branch: &str, local_head: &str, repo_root: &str) -> Option<Self> {
        // Note: We don't log auth failures here since detect_github already logged them
        if !tool_available("gh", &["auth", "status"]) {
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
            "status,conclusion,headSha",
        ]);

        disable_color_output(&mut cmd);
        cmd.current_dir(repo_root);

        let output = match cmd.output() {
            Ok(output) => output,
            Err(e) => {
                log::warn!("gh run list failed to execute for branch {}: {}", branch, e);
                return None;
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            log::debug!("gh run list failed for {}: {}", branch, stderr.trim());
            return None;
        }

        let runs: Vec<GitHubWorkflowRun> = parse_json(&output.stdout, "gh run list", branch)?;
        let run = runs.first()?;

        // Check if the workflow run matches our local HEAD commit
        let is_stale = run
            .head_sha
            .as_ref()
            .map(|run_sha| run_sha != local_head)
            .unwrap_or(true); // If no SHA, consider it stale

        // Analyze workflow run status
        let ci_status = run.ci_status();

        Some(PrStatus {
            ci_status,
            source: CiSource::Branch,
            is_stale,
            url: None, // Workflow runs don't have a PR URL
        })
    }

    fn detect_gitlab_pipeline(branch: &str, local_head: &str) -> Option<Self> {
        if !tool_available("glab", &["--version"]) {
            return None;
        }

        // Get most recent pipeline for the branch using JSON output
        let output = match Command::new("glab")
            .args(["ci", "list", "--per-page", "1", "--output", "json"])
            .env("BRANCH", branch) // glab ci list uses BRANCH env var
            .output()
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
            log::debug!("glab ci list failed for {}: {}", branch, stderr.trim());
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
            // TODO: Fetch GitLab pipeline URL to enable clickable links
            url: None,
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
    url: Option<String>,
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
    #[serde(rename = "headSha")]
    head_sha: Option<String>,
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
    /// Only present in `glab ci list` output, not in MR view embedded pipeline
    #[serde(default)]
    sha: Option<String>,
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
    fn ci_status(&self) -> CiStatus {
        parse_gitlab_status(self.status.as_deref())
    }
}
