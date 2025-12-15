//! Tests for CI status detection and parsing
//!
//! These tests verify that the CI status parsing code correctly handles
//! JSON responses from GitHub (gh) and GitLab (glab) CLI tools.
//!
//! TODO: Re-enable on Windows once we have a reliable way to mock the `gh` command.
//! Currently skipped because Rust's `Command::new("gh")` doesn't use PATHEXT for
//! resolution - it looks for `.exe` directly. Our mock `gh.bat` scripts aren't
//! found, so we can't mock the gh command reliably without creating a real `.exe`
//! wrapper or modifying the source to use shell execution.

#![cfg(not(windows))]

use crate::common::{TestRepo, make_snapshot_cmd, repo, setup_snapshot_settings};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::process::Command;

/// Get the HEAD commit SHA for a branch
fn get_branch_sha(repo: &TestRepo, branch: &str) -> String {
    let output = repo
        .git_command(&["rev-parse", branch])
        .output()
        .expect("Failed to get commit SHA");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Test CI status detection with GitHub PR showing passed checks
#[rstest]
fn test_list_full_with_github_pr_passed(mut repo: TestRepo) {
    // Add GitHub remote
    repo.git_command(&[
        "remote",
        "add",
        "origin",
        "https://github.com/test-owner/test-repo.git",
    ])
    .output()
    .unwrap();

    // Create a feature branch and push it
    repo.add_worktree("feature");
    repo.push_branch("feature");

    // Get actual commit SHA so PR isn't marked as stale
    let head_sha = get_branch_sha(&repo, "feature");

    // Setup mock gh that returns PR data with passed checks
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
        assert_cmd_snapshot!(cmd);
    });
}

/// Test CI status detection with GitHub PR showing failed checks
#[rstest]
fn test_list_full_with_github_pr_failed(mut repo: TestRepo) {
    // Add GitHub remote
    repo.git_command(&[
        "remote",
        "add",
        "origin",
        "https://github.com/test-owner/test-repo.git",
    ])
    .output()
    .unwrap();

    // Create a feature branch and push it
    repo.add_worktree("feature");
    repo.push_branch("feature");

    // Get actual commit SHA so PR isn't marked as stale
    let head_sha = get_branch_sha(&repo, "feature");

    // Setup mock gh that returns PR data with failed checks
    let pr_json = format!(
        r#"[{{
        "headRefOid": "{}",
        "mergeStateStatus": "BLOCKED",
        "statusCheckRollup": [
            {{"status": "COMPLETED", "conclusion": "FAILURE"}}
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
        assert_cmd_snapshot!(cmd);
    });
}

/// Test CI status detection with GitHub PR showing running checks
#[rstest]
fn test_list_full_with_github_pr_running(mut repo: TestRepo) {
    // Add GitHub remote
    repo.git_command(&[
        "remote",
        "add",
        "origin",
        "https://github.com/test-owner/test-repo.git",
    ])
    .output()
    .unwrap();

    // Create a feature branch and push it
    repo.add_worktree("feature");
    repo.push_branch("feature");

    // Get actual commit SHA so PR isn't marked as stale
    let head_sha = get_branch_sha(&repo, "feature");

    // Setup mock gh that returns PR data with running checks
    let pr_json = format!(
        r#"[{{
        "headRefOid": "{}",
        "mergeStateStatus": "UNKNOWN",
        "statusCheckRollup": [
            {{"status": "IN_PROGRESS", "conclusion": null}}
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
        assert_cmd_snapshot!(cmd);
    });
}

/// Test CI status detection with GitHub PR showing conflicts
#[rstest]
fn test_list_full_with_github_pr_conflicts(mut repo: TestRepo) {
    // Add GitHub remote
    repo.git_command(&[
        "remote",
        "add",
        "origin",
        "https://github.com/test-owner/test-repo.git",
    ])
    .output()
    .unwrap();

    // Create a feature branch and push it
    repo.add_worktree("feature");
    repo.push_branch("feature");

    // Get actual commit SHA so PR isn't marked as stale
    let head_sha = get_branch_sha(&repo, "feature");

    // Setup mock gh that returns PR data with merge conflicts
    let pr_json = format!(
        r#"[{{
        "headRefOid": "{}",
        "mergeStateStatus": "DIRTY",
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
        assert_cmd_snapshot!(cmd);
    });
}

/// Test CI status detection with StatusContext (external CI like pre-commit.ci)
#[rstest]
fn test_list_full_with_status_context_pending(mut repo: TestRepo) {
    // Add GitHub remote
    repo.git_command(&[
        "remote",
        "add",
        "origin",
        "https://github.com/test-owner/test-repo.git",
    ])
    .output()
    .unwrap();

    // Create a feature branch and push it
    repo.add_worktree("feature");
    repo.push_branch("feature");

    // Get actual commit SHA so PR isn't marked as stale
    let head_sha = get_branch_sha(&repo, "feature");

    // Setup mock gh with StatusContext (external CI) pending
    let pr_json = format!(
        r#"[{{
        "headRefOid": "{}",
        "mergeStateStatus": "UNKNOWN",
        "statusCheckRollup": [
            {{"state": "PENDING"}}
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
        assert_cmd_snapshot!(cmd);
    });
}

/// Test CI status detection with StatusContext failure (external CI)
#[rstest]
fn test_list_full_with_status_context_failure(mut repo: TestRepo) {
    // Add GitHub remote
    repo.git_command(&[
        "remote",
        "add",
        "origin",
        "https://github.com/test-owner/test-repo.git",
    ])
    .output()
    .unwrap();

    // Create a feature branch and push it
    repo.add_worktree("feature");
    repo.push_branch("feature");

    // Get actual commit SHA so PR isn't marked as stale
    let head_sha = get_branch_sha(&repo, "feature");

    // Setup mock gh with StatusContext (external CI) failure
    let pr_json = format!(
        r#"[{{
        "headRefOid": "{}",
        "mergeStateStatus": "BLOCKED",
        "statusCheckRollup": [
            {{"state": "FAILURE"}}
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
        assert_cmd_snapshot!(cmd);
    });
}

/// Test CI status detection with no PR but workflow run
#[rstest]
fn test_list_full_with_github_workflow_run(mut repo: TestRepo) {
    // Add GitHub remote
    repo.git_command(&[
        "remote",
        "add",
        "origin",
        "https://github.com/test-owner/test-repo.git",
    ])
    .output()
    .unwrap();

    // Create a feature branch and push it (so has_upstream is true)
    repo.add_worktree("feature");
    repo.push_branch("feature");

    // Get actual commit SHA so workflow run isn't marked as stale
    let head_sha = get_branch_sha(&repo, "feature");

    // Setup mock gh with no PR but workflow run
    let pr_json = "[]"; // No PR
    let run_json = format!(
        r#"[{{
        "status": "completed",
        "conclusion": "success",
        "headSha": "{}"
    }}]"#,
        head_sha
    );
    repo.setup_mock_gh_with_ci_data(pr_json, &run_json);

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "list", &["--full"], None);
        repo.configure_mock_commands(&mut cmd);
        assert_cmd_snapshot!(cmd);
    });
}

/// Test CI status detection with workflow run in progress
#[rstest]
fn test_list_full_with_github_workflow_running(mut repo: TestRepo) {
    // Add GitHub remote
    repo.git_command(&[
        "remote",
        "add",
        "origin",
        "https://github.com/test-owner/test-repo.git",
    ])
    .output()
    .unwrap();

    // Create a feature branch and push it
    repo.add_worktree("feature");
    repo.push_branch("feature");

    // Get actual commit SHA so workflow run isn't marked as stale
    let head_sha = get_branch_sha(&repo, "feature");

    // Setup mock gh with workflow run in progress
    let pr_json = "[]"; // No PR
    let run_json = format!(
        r#"[{{
        "status": "in_progress",
        "conclusion": null,
        "headSha": "{}"
    }}]"#,
        head_sha
    );
    repo.setup_mock_gh_with_ci_data(pr_json, &run_json);

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "list", &["--full"], None);
        repo.configure_mock_commands(&mut cmd);
        assert_cmd_snapshot!(cmd);
    });
}

/// Test CI status with stale PR (local HEAD differs from PR HEAD)
#[rstest]
fn test_list_full_with_stale_pr(mut repo: TestRepo) {
    // Add GitHub remote
    repo.git_command(&[
        "remote",
        "add",
        "origin",
        "https://github.com/test-owner/test-repo.git",
    ])
    .output()
    .unwrap();

    // Create a feature branch and push it
    repo.add_worktree("feature");
    repo.push_branch("feature");

    // Make additional commit locally (not pushed)
    let worktree_path = repo.worktrees.get("feature").unwrap();
    std::fs::write(worktree_path.join("new_file.txt"), "new content").unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "."])
        .current_dir(worktree_path)
        .output()
        .unwrap();
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Local commit"])
        .current_dir(worktree_path)
        .output()
        .unwrap();

    // Setup mock gh with PR data - use fake SHA to simulate stale PR
    // (PR was created before the local commit, so PR HEAD differs from local HEAD)
    let pr_json = r#"[{
        "headRefOid": "old_sha_from_before_local_commit",
        "mergeStateStatus": "CLEAN",
        "statusCheckRollup": [
            {"status": "COMPLETED", "conclusion": "SUCCESS"}
        ],
        "url": "https://github.com/test-owner/test-repo/pull/1",
        "headRepositoryOwner": {"login": "test-owner"}
    }]"#;
    let run_json = "[]";
    repo.setup_mock_gh_with_ci_data(pr_json, run_json);

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "list", &["--full"], None);
        repo.configure_mock_commands(&mut cmd);
        assert_cmd_snapshot!(cmd);
    });
}

/// Test CI status detection with mixed CheckRun and StatusContext results
#[rstest]
fn test_list_full_with_mixed_check_types(mut repo: TestRepo) {
    // Add GitHub remote
    repo.git_command(&[
        "remote",
        "add",
        "origin",
        "https://github.com/test-owner/test-repo.git",
    ])
    .output()
    .unwrap();

    // Create a feature branch and push it
    repo.add_worktree("feature");
    repo.push_branch("feature");

    // Get actual commit SHA so PR isn't marked as stale
    let head_sha = get_branch_sha(&repo, "feature");

    // Setup mock gh with mixed check types (CheckRun + StatusContext)
    // One passed CheckRun, one pending StatusContext
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
    let run_json = "[]";
    repo.setup_mock_gh_with_ci_data(&pr_json, run_json);

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "list", &["--full"], None);
        repo.configure_mock_commands(&mut cmd);
        assert_cmd_snapshot!(cmd);
    });
}

/// Test CI status detection when PR has no checks configured
#[rstest]
fn test_list_full_with_no_ci_checks(mut repo: TestRepo) {
    // Add GitHub remote
    repo.git_command(&[
        "remote",
        "add",
        "origin",
        "https://github.com/test-owner/test-repo.git",
    ])
    .output()
    .unwrap();

    // Create a feature branch and push it
    repo.add_worktree("feature");
    repo.push_branch("feature");

    // Get actual commit SHA so PR isn't marked as stale
    let head_sha = get_branch_sha(&repo, "feature");

    // Setup mock gh with PR but no CI checks
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
    let run_json = "[]";
    repo.setup_mock_gh_with_ci_data(&pr_json, run_json);

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "list", &["--full"], None);
        repo.configure_mock_commands(&mut cmd);
        assert_cmd_snapshot!(cmd);
    });
}

/// Test filtering PRs by repository owner
#[rstest]
fn test_list_full_filters_by_repo_owner(mut repo: TestRepo) {
    // Add GitHub remote with specific owner
    repo.git_command(&[
        "remote",
        "add",
        "origin",
        "https://github.com/my-org/test-repo.git",
    ])
    .output()
    .unwrap();

    // Create a feature branch and push it
    repo.add_worktree("feature");
    repo.push_branch("feature");

    // Get actual commit SHA so PR isn't marked as stale
    let head_sha = get_branch_sha(&repo, "feature");

    // Setup mock gh with multiple PRs - only one from our org
    // The first PR is from a different org, second is from our org
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
    let run_json = "[]";
    repo.setup_mock_gh_with_ci_data(&pr_json, run_json);

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "list", &["--full"], None);
        repo.configure_mock_commands(&mut cmd);
        // Should show passed (green) because it filters to my-org's PR
        assert_cmd_snapshot!(cmd);
    });
}
