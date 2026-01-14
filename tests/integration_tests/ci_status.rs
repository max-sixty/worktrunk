//! Tests for CI status detection and parsing
//!
//! These tests verify that the CI status parsing code correctly handles
//! JSON responses from GitHub (gh) and GitLab (glab) CLI tools.
//!
//! ## Windows support
//!
//! On Windows, mock-stub.exe sets MOCK_SCRIPT_DIR so the mock gh script can
//! reliably locate its JSON data files. Use MOCK_DEBUG=1 to troubleshoot
//! path issues.

use crate::common::{TestRepo, make_snapshot_cmd, repo, setup_snapshot_settings};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;

/// Get the HEAD commit SHA for a branch
fn get_branch_sha(repo: &TestRepo, branch: &str) -> String {
    repo.git_output(&["rev-parse", branch])
}

/// Helper to run a CI status test with the given mock data
fn run_ci_status_test(repo: &mut TestRepo, snapshot_name: &str, pr_json: &str, run_json: &str) {
    repo.setup_mock_gh_with_ci_data(pr_json, run_json);

    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(repo, "list", &["--full"], None);
        repo.configure_mock_commands(&mut cmd);
        assert_cmd_snapshot!(snapshot_name, cmd);
    });
}

/// Setup a repo with GitHub remote and feature worktree, returns head SHA
fn setup_github_repo_with_feature(repo: &mut TestRepo) -> String {
    // Set origin URL (origin already exists from fixture, just update URL)
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://github.com/test-owner/test-repo.git",
    ]);
    repo.add_worktree("feature");
    get_branch_sha(repo, "feature")
}

// =============================================================================
// PR status tests (CheckRun format)
// =============================================================================

#[rstest]
#[case::passed("CLEAN", "COMPLETED", "SUCCESS", "github_pr_passed")]
#[case::failed("BLOCKED", "COMPLETED", "FAILURE", "github_pr_failed")]
#[case::running("UNKNOWN", "IN_PROGRESS", "null", "github_pr_running")]
#[case::conflicts("DIRTY", "COMPLETED", "SUCCESS", "github_pr_conflicts")]
fn test_list_full_with_github_pr_status(
    mut repo: TestRepo,
    #[case] merge_state: &str,
    #[case] status: &str,
    #[case] conclusion: &str,
    #[case] snapshot_name: &str,
) {
    let head_sha = setup_github_repo_with_feature(&mut repo);

    // Format conclusion - use raw value for null, quoted for strings
    let conclusion_json = if conclusion == "null" {
        "null".to_string()
    } else {
        format!("\"{}\"", conclusion)
    };

    let pr_json = format!(
        r#"[{{
        "headRefOid": "{}",
        "mergeStateStatus": "{}",
        "statusCheckRollup": [
            {{"status": "{}", "conclusion": {}}}
        ],
        "url": "https://github.com/test-owner/test-repo/pull/1",
        "headRepositoryOwner": {{"login": "test-owner"}}
    }}]"#,
        head_sha, merge_state, status, conclusion_json
    );

    run_ci_status_test(&mut repo, snapshot_name, &pr_json, "[]");
}

// =============================================================================
// StatusContext tests (external CI systems like Jenkins)
// =============================================================================

#[rstest]
#[case::pending("UNKNOWN", "PENDING", "status_context_pending")]
#[case::failure("BLOCKED", "FAILURE", "status_context_failure")]
fn test_list_full_with_status_context(
    mut repo: TestRepo,
    #[case] merge_state: &str,
    #[case] state: &str,
    #[case] snapshot_name: &str,
) {
    let head_sha = setup_github_repo_with_feature(&mut repo);

    let pr_json = format!(
        r#"[{{
        "headRefOid": "{}",
        "mergeStateStatus": "{}",
        "statusCheckRollup": [
            {{"state": "{}"}}
        ],
        "url": "https://github.com/test-owner/test-repo/pull/1",
        "headRepositoryOwner": {{"login": "test-owner"}}
    }}]"#,
        head_sha, merge_state, state
    );

    run_ci_status_test(&mut repo, snapshot_name, &pr_json, "[]");
}

// =============================================================================
// Workflow run tests (no PR, just workflow runs)
// =============================================================================

#[rstest]
#[case::completed("completed", "success", "github_workflow_run")]
#[case::running("in_progress", "null", "github_workflow_running")]
fn test_list_full_with_github_workflow(
    mut repo: TestRepo,
    #[case] status: &str,
    #[case] conclusion: &str,
    #[case] snapshot_name: &str,
) {
    let head_sha = setup_github_repo_with_feature(&mut repo);

    let conclusion_json = if conclusion == "null" {
        "null".to_string()
    } else {
        format!("\"{}\"", conclusion)
    };

    let run_json = format!(
        r#"[{{
        "status": "{}",
        "conclusion": {},
        "headSha": "{}"
    }}]"#,
        status, conclusion_json, head_sha
    );

    run_ci_status_test(&mut repo, snapshot_name, "[]", &run_json);
}

// =============================================================================
// Special case tests (unique scenarios that don't fit parameterization)
// =============================================================================

#[rstest]
fn test_list_full_with_stale_pr(mut repo: TestRepo) {
    setup_github_repo_with_feature(&mut repo);

    // Make additional commit locally (not pushed)
    let worktree_path = repo.worktrees.get("feature").unwrap().clone();
    std::fs::write(worktree_path.join("new_file.txt"), "new content").unwrap();
    repo.stage_all(&worktree_path);
    repo.run_git_in(&worktree_path, &["commit", "-m", "Local commit"]);

    // PR HEAD differs from local HEAD - simulates stale PR
    let pr_json = r#"[{
        "headRefOid": "old_sha_from_before_local_commit",
        "mergeStateStatus": "CLEAN",
        "statusCheckRollup": [
            {"status": "COMPLETED", "conclusion": "SUCCESS"}
        ],
        "url": "https://github.com/test-owner/test-repo/pull/1",
        "headRepositoryOwner": {"login": "test-owner"}
    }]"#;

    run_ci_status_test(&mut repo, "stale_pr", pr_json, "[]");
}

#[rstest]
fn test_list_full_with_mixed_check_types(mut repo: TestRepo) {
    let head_sha = setup_github_repo_with_feature(&mut repo);

    // Mixed: CheckRun (passed) + StatusContext (pending)
    let pr_json = format!(
        r#"[{{
        "headRefOid": "{}",
        "mergeStateStatus": "UNKNOWN",
        "statusCheckRollup": [
            {{"status": "COMPLETED", "conclusion": "SUCCESS"}},
            {{"state": "PENDING"}}
        ],
        "url": "https://github.com/test-owner/test-repo/pull/1",
        "headRepositoryOwner": {{"login": "test-owner"}}
    }}]"#,
        head_sha
    );

    run_ci_status_test(&mut repo, "mixed_check_types", &pr_json, "[]");
}

#[rstest]
fn test_list_full_with_no_ci_checks(mut repo: TestRepo) {
    let head_sha = setup_github_repo_with_feature(&mut repo);

    let pr_json = format!(
        r#"[{{
        "headRefOid": "{}",
        "mergeStateStatus": "CLEAN",
        "statusCheckRollup": [],
        "url": "https://github.com/test-owner/test-repo/pull/1",
        "headRepositoryOwner": {{"login": "test-owner"}}
    }}]"#,
        head_sha
    );

    run_ci_status_test(&mut repo, "no_ci_checks", &pr_json, "[]");
}

#[rstest]
fn test_list_full_filters_by_repo_owner(mut repo: TestRepo) {
    // Use different org name
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://github.com/my-org/test-repo.git",
    ]);
    repo.add_worktree("feature");
    let head_sha = get_branch_sha(&repo, "feature");

    // Multiple PRs - only one from our org (should filter to my-org's PR)
    let pr_json = format!(
        r#"[
        {{
            "headRefOid": "wrong_sha",
            "mergeStateStatus": "CLEAN",
            "statusCheckRollup": [{{"status": "COMPLETED", "conclusion": "FAILURE"}}],
            "url": "https://github.com/other-org/test-repo/pull/99",
            "headRepositoryOwner": {{"login": "other-org"}}
        }},
        {{
            "headRefOid": "{}",
            "mergeStateStatus": "CLEAN",
            "statusCheckRollup": [{{"status": "COMPLETED", "conclusion": "SUCCESS"}}],
            "url": "https://github.com/my-org/test-repo/pull/1",
            "headRepositoryOwner": {{"login": "my-org"}}
        }}
    ]"#,
        head_sha
    );

    run_ci_status_test(&mut repo, "filters_by_repo_owner", &pr_json, "[]");
}

#[rstest]
fn test_list_full_with_platform_override_github(mut repo: TestRepo) {
    // Set a non-GitHub remote (bitbucket) - platform won't be auto-detected
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://bitbucket.org/test-owner/test-repo.git",
    ]);

    // Set platform override in project config
    repo.write_project_config(
        r#"
[ci]
platform = "github"
"#,
    );

    // Create a feature branch
    repo.add_worktree("feature");

    // Get actual commit SHA
    let head_sha = get_branch_sha(&repo, "feature");

    // Setup mock gh with PR data - this should work because platform is overridden to github
    let pr_json = format!(
        r#"[{{
        "headRefOid": "{}",
        "mergeStateStatus": "CLEAN",
        "statusCheckRollup": [
            {{"status": "COMPLETED", "conclusion": "SUCCESS"}}
        ],
        "url": "https://github.com/test-owner/test-repo/pull/1",
        "headRepositoryOwner": {{"login": "test-owner"}}
    }}]"#,
        head_sha
    );
    let run_json = "[]";
    repo.setup_mock_gh_with_ci_data(&pr_json, run_json);

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "list", &["--full"], None);
        repo.configure_mock_commands(&mut cmd);
        // Platform override should force GitHub detection even with bitbucket remote
        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_list_full_with_gitlab_remote(mut repo: TestRepo) {
    // Set GitLab remote URL - tests get_gitlab_host_for_repo path
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://gitlab.example.com/test-owner/test-repo.git",
    ]);

    // Create a feature branch
    repo.add_worktree("feature");

    // No mock glab setup - this tests the hint path when glab isn't available
    // The get_gitlab_host_for_repo function is called to detect GitLab platform

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "list", &["--full"], None);
        // Don't configure mocks - we want to test the "no CI tool" hint path
        // which exercises get_gitlab_host_for_repo
        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_list_full_with_invalid_platform_override(mut repo: TestRepo) {
    // Set GitHub remote URL
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://github.com/test-owner/test-repo.git",
    ]);

    // Set INVALID platform override - should warn and fall back to URL detection
    repo.write_project_config(
        r#"
[ci]
platform = "invalid_platform"
"#,
    );

    // Create a feature branch
    repo.add_worktree("feature");
    let head_sha = get_branch_sha(&repo, "feature");

    // Setup mock gh - platform should fall back to GitHub via URL detection
    let pr_json = format!(
        r#"[{{
        "headRefOid": "{}",
        "mergeStateStatus": "CLEAN",
        "statusCheckRollup": [
            {{"status": "COMPLETED", "conclusion": "SUCCESS"}}
        ],
        "url": "https://github.com/test-owner/test-repo/pull/1",
        "headRepositoryOwner": {{"login": "test-owner"}}
    }}]"#,
        head_sha
    );
    repo.setup_mock_gh_with_ci_data(&pr_json, "[]");

    let mut settings = setup_snapshot_settings(&repo);
    // Normalize worker thread ID prefix in log output (e.g., [n], [z], [A] -> [W])
    settings.add_filter(r"\[[a-zA-Z]\]", "[W]");
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "list", &["--full"], None);
        repo.configure_mock_commands(&mut cmd);
        // Invalid platform should fall back to URL detection (GitHub)
        assert_cmd_snapshot!(cmd);
    });
}

// =============================================================================
// GitLab MR status tests
// =============================================================================

/// Helper to run a GitLab CI status test with the given mock data
fn run_gitlab_ci_status_test(
    repo: &mut TestRepo,
    snapshot_name: &str,
    mr_json: &str,
    project_id: Option<u64>,
) {
    repo.setup_mock_glab_with_ci_data(mr_json, project_id);

    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(repo, "list", &["--full"], None);
        repo.configure_mock_commands(&mut cmd);
        assert_cmd_snapshot!(snapshot_name, cmd);
    });
}

/// Setup a repo with GitLab remote and feature worktree, returns head SHA
fn setup_gitlab_repo_with_feature(repo: &mut TestRepo) -> String {
    // Set origin URL (origin already exists from fixture, just update URL)
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://gitlab.com/test-group/test-project.git",
    ]);
    repo.add_worktree("feature");
    get_branch_sha(repo, "feature")
}

#[rstest]
#[case::passed("success", false, "gitlab_mr_passed")]
#[case::failed("failed", false, "gitlab_mr_failed")]
#[case::running("running", false, "gitlab_mr_running")]
#[case::pending("pending", false, "gitlab_mr_pending")]
#[case::conflicts("success", true, "gitlab_mr_conflicts")]
fn test_list_full_with_gitlab_mr_status(
    mut repo: TestRepo,
    #[case] pipeline_status: &str,
    #[case] has_conflicts: bool,
    #[case] snapshot_name: &str,
) {
    let head_sha = setup_gitlab_repo_with_feature(&mut repo);

    let mr_json = format!(
        r#"[{{
        "sha": "{}",
        "has_conflicts": {},
        "detailed_merge_status": null,
        "head_pipeline": {{"status": "{}"}},
        "source_project_id": 12345,
        "web_url": "https://gitlab.com/test-group/test-project/-/merge_requests/1"
    }}]"#,
        head_sha, has_conflicts, pipeline_status
    );

    run_gitlab_ci_status_test(&mut repo, snapshot_name, &mr_json, Some(12345));
}

#[rstest]
fn test_list_full_with_gitlab_stale_mr(mut repo: TestRepo) {
    setup_gitlab_repo_with_feature(&mut repo);

    // Make additional commit locally (not pushed)
    let worktree_path = repo.worktrees.get("feature").unwrap().clone();
    std::fs::write(worktree_path.join("new_file.txt"), "new content").unwrap();
    repo.stage_all(&worktree_path);
    repo.run_git_in(&worktree_path, &["commit", "-m", "Local commit"]);

    // MR HEAD differs from local HEAD - simulates stale MR
    let mr_json = r#"[{
        "sha": "old_sha_from_before_local_commit",
        "has_conflicts": false,
        "detailed_merge_status": null,
        "head_pipeline": {"status": "success"},
        "source_project_id": 12345,
        "web_url": "https://gitlab.com/test-group/test-project/-/merge_requests/1"
    }]"#;

    run_gitlab_ci_status_test(&mut repo, "gitlab_stale_mr", mr_json, Some(12345));
}

#[rstest]
fn test_list_full_with_gitlab_no_ci(mut repo: TestRepo) {
    let head_sha = setup_gitlab_repo_with_feature(&mut repo);

    // MR with no pipeline
    let mr_json = format!(
        r#"[{{
        "sha": "{}",
        "has_conflicts": false,
        "detailed_merge_status": null,
        "head_pipeline": null,
        "source_project_id": 12345,
        "web_url": "https://gitlab.com/test-group/test-project/-/merge_requests/1"
    }}]"#,
        head_sha
    );

    run_gitlab_ci_status_test(&mut repo, "gitlab_no_ci", &mr_json, Some(12345));
}

#[rstest]
fn test_list_full_with_gitlab_filters_by_project_id(mut repo: TestRepo) {
    // Use a specific project for our repo
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://gitlab.com/my-group/my-project.git",
    ]);
    repo.add_worktree("feature");
    let head_sha = get_branch_sha(&repo, "feature");

    // Multiple MRs - only one from our project (should filter to project 99999)
    let mr_json = format!(
        r#"[
        {{
            "sha": "wrong_sha",
            "has_conflicts": false,
            "detailed_merge_status": null,
            "head_pipeline": {{"status": "failed"}},
            "source_project_id": 11111,
            "web_url": "https://gitlab.com/other-group/other-project/-/merge_requests/99"
        }},
        {{
            "sha": "{}",
            "has_conflicts": false,
            "detailed_merge_status": null,
            "head_pipeline": {{"status": "success"}},
            "source_project_id": 99999,
            "web_url": "https://gitlab.com/my-group/my-project/-/merge_requests/1"
        }}
    ]"#,
        head_sha
    );

    run_gitlab_ci_status_test(
        &mut repo,
        "gitlab_filters_by_project_id",
        &mr_json,
        Some(99999),
    );
}
