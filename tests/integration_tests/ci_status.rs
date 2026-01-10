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
    repo.run_git(&[
        "remote",
        "add",
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
        "add",
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
