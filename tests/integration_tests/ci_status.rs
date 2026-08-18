//! Tests for CI status detection and parsing
//!
//! These tests verify that the CI status parsing code correctly handles
//! JSON responses from GitHub (gh) and GitLab (glab) CLI tools.

use crate::common::{
    TestRepo, make_snapshot_cmd,
    mock_commands::{MockConfig, MockResponse},
    repo, setup_snapshot_settings, wt_command,
};
use ansi_str::AnsiStr;
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::path::Path;
use std::process::Command;

/// Get the HEAD commit SHA for a branch
fn branch_sha(repo: &TestRepo, branch: &str) -> String {
    repo.git_output(&["rev-parse", branch])
}

/// Set up tracking for all branches so `push_remote_url()` resolves a push
/// destination.
///
/// `%(push:remotename)` reads the `branch.<name>.{remote,merge}` config. The
/// remote-tracking ref that fetch/push would normally create is set up
/// alongside it, so the fixture matches a real clone.
fn setup_tracking_for_all_branches(repo: &TestRepo, remote: &str) {
    for branch in ["feature", "feature-a", "feature-b", "feature-c", "main"] {
        repo.run_git(&["config", &format!("branch.{}.remote", branch), remote]);
        repo.run_git(&[
            "config",
            &format!("branch.{}.merge", branch),
            &format!("refs/heads/{}", branch),
        ]);
        // Create the remote-tracking ref
        repo.run_git(&[
            "update-ref",
            &format!("refs/remotes/{}/{}", remote, branch),
            branch,
        ]);
    }
}

/// Helper to run a CI status test with the given mock data
fn run_ci_status_test(repo: &mut TestRepo, snapshot_name: &str, pr_json: &str) {
    repo.setup_mock_gh_with_ci_data(pr_json);

    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(repo, "list", &["--full"], None);
        repo.configure_mock_commands(&mut cmd);
        assert_cmd_snapshot!(snapshot_name, cmd);
    });
}

/// Setup mock `gh` with configurable `pr list` and `api repos/.../check-runs`
/// responses.
fn setup_mock_gh_with_api_data(
    repo: &TestRepo,
    pr_json: &str,
    api_responses: &[(&str, &str)],
) -> std::path::PathBuf {
    let mock_bin = repo.root_path().join("mock-bin");
    std::fs::create_dir_all(&mock_bin).unwrap();

    let mut gh = MockConfig::new("gh")
        .version("gh version 2.0.0 (mock)")
        .command("pr list", MockResponse::output(pr_json));

    for (command, response) in api_responses {
        gh = gh.command(command, MockResponse::output(response));
    }

    gh.command("_default", MockResponse::exit(1))
        .write(&mock_bin);
    MockConfig::new("glab")
        .version("glab version 1.0.0 (mock)")
        .command("_default", MockResponse::exit(1))
        .write(&mock_bin);

    mock_bin
}

/// Configure command environment for local gh/glab mocks.
fn configure_mock_ci_env(cmd: &mut Command, mock_bin: &Path) {
    cmd.env("WORKTRUNK_TEST_MOCK_CONFIG_DIR", mock_bin);

    let (path_var_name, current_path) = std::env::vars_os()
        .find(|(k, _)| k.eq_ignore_ascii_case("PATH"))
        .map(|(k, v)| (k.to_string_lossy().into_owned(), Some(v)))
        .unwrap_or(("PATH".to_string(), None));

    let mut paths: Vec<std::path::PathBuf> = current_path
        .as_deref()
        .map(|p| std::env::split_paths(p).collect())
        .unwrap_or_default();
    paths.insert(0, mock_bin.to_path_buf());
    let new_path = std::env::join_paths(&paths).unwrap();
    cmd.env(path_var_name, new_path);
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
    setup_tracking_for_all_branches(repo, "origin");
    branch_sha(repo, "feature")
}

// =============================================================================
// PR conflict status
// =============================================================================

#[rstest]
fn test_list_full_with_github_pr_conflicts(mut repo: TestRepo) {
    let head_sha = setup_github_repo_with_feature(&mut repo);

    let pr_json = format!(
        r#"[{{
        "number": 1,
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

    run_ci_status_test(&mut repo, "github_pr_conflicts", &pr_json);
}

#[rstest]
fn test_list_full_json_ci_repo_from_pr_url(mut repo: TestRepo) {
    let head_sha = setup_github_repo_with_feature(&mut repo);

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
    repo.setup_mock_gh_with_ci_data(&pr_json);

    let mut cmd = repo.wt_command();
    cmd.args(["list", "--full", "--format=json"]);
    repo.configure_mock_commands(&mut cmd);
    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "wt list --full --format=json should succeed\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let rows: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    let row = rows
        .iter()
        .find(|row| row["branch"].as_str() == Some("feature"))
        .expect("feature row should be present");
    let ci = &row["ci"];
    assert_eq!(
        ci["repo_url"].as_str(),
        Some("https://github.com/test-owner/test-repo")
    );
    assert_eq!(
        ci["repo"]["url"].as_str(),
        Some("https://github.com/test-owner/test-repo")
    );
    assert_eq!(ci["repo"]["provider"].as_str(), Some("github"));
    assert_eq!(ci["repo"]["host"].as_str(), Some("github.com"));
    assert_eq!(ci["repo"]["owner"].as_str(), Some("test-owner"));
    assert_eq!(ci["repo"]["name"].as_str(), Some("test-repo"));
    assert!(
        ci["repo"].get("remote").is_none(),
        "ci.repo should not include the local remote name"
    );
}

#[rstest]
fn test_list_full_json_ci_repo_uses_configured_provider_for_opaque_host(mut repo: TestRepo) {
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://git.company.com/test-owner/test-repo.git",
    ]);
    repo.write_project_config("[forge]\nplatform = \"github\"\n");
    repo.add_worktree("feature");
    setup_tracking_for_all_branches(&repo, "origin");
    let head_sha = branch_sha(&repo, "feature");

    let pr_json = format!(
        r#"[{{
        "headRefOid": "{}",
        "mergeStateStatus": "CLEAN",
        "statusCheckRollup": [
            {{"status": "COMPLETED", "conclusion": "SUCCESS"}}
        ],
        "url": "https://git.company.com/test-owner/test-repo/pull/1",
        "headRepositoryOwner": {{"login": "test-owner"}}
    }}]"#,
        head_sha
    );
    repo.setup_mock_gh_with_ci_data(&pr_json);

    let mut cmd = repo.wt_command();
    cmd.args(["list", "--full", "--format=json"]);
    repo.configure_mock_commands(&mut cmd);
    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "wt list --full --format=json should succeed\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let rows: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    let row = rows
        .iter()
        .find(|row| row["branch"].as_str() == Some("feature"))
        .expect("feature row should be present");
    assert_eq!(row["repo"]["provider"].as_str(), Some("github"));
    assert_eq!(row["repo"]["host"].as_str(), Some("git.company.com"));

    let ci = &row["ci"];
    assert_eq!(
        ci["repo_url"].as_str(),
        Some("https://git.company.com/test-owner/test-repo")
    );
    assert_eq!(
        ci["repo"]["url"].as_str(),
        Some("https://git.company.com/test-owner/test-repo")
    );
    assert_eq!(ci["repo"]["provider"].as_str(), Some("github"));
    assert_eq!(ci["repo"]["host"].as_str(), Some("git.company.com"));
    assert_eq!(ci["repo"]["owner"].as_str(), Some("test-owner"));
    assert_eq!(ci["repo"]["name"].as_str(), Some("test-repo"));
}

// =============================================================================
// Review state
// =============================================================================

#[rstest]
fn test_list_full_with_github_changes_requested(mut repo: TestRepo) {
    let head_sha = setup_github_repo_with_feature(&mut repo);

    let pr_json = format!(
        r#"[{{
        "number": 1,
        "headRefOid": "{}",
        "mergeStateStatus": "CLEAN",
        "statusCheckRollup": [
            {{"status": "COMPLETED", "conclusion": "SUCCESS"}}
        ],
        "url": "https://github.com/test-owner/test-repo/pull/1",
        "headRepositoryOwner": {{"login": "test-owner"}},
        "reviewDecision": "CHANGES_REQUESTED",
        "isDraft": false
    }}]"#,
        head_sha
    );

    run_ci_status_test(&mut repo, "github_pr_changes_requested", &pr_json);
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
        "number": 1,
        "headRefOid": "old_sha_from_before_local_commit",
        "mergeStateStatus": "CLEAN",
        "statusCheckRollup": [
            {"status": "COMPLETED", "conclusion": "SUCCESS"}
        ],
        "url": "https://github.com/test-owner/test-repo/pull/1",
        "headRepositoryOwner": {"login": "test-owner"}
    }]"#;

    run_ci_status_test(&mut repo, "stale_pr", pr_json);
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
    setup_tracking_for_all_branches(&repo, "origin");
    let head_sha = branch_sha(&repo, "feature");

    // Multiple PRs - only one from our org (should filter to my-org's PR)
    let pr_json = format!(
        r#"[
        {{
            "number": 99,
            "headRefOid": "wrong_sha",
            "mergeStateStatus": "CLEAN",
            "statusCheckRollup": [{{"status": "COMPLETED", "conclusion": "FAILURE"}}],
            "url": "https://github.com/other-org/test-repo/pull/99",
            "headRepositoryOwner": {{"login": "other-org"}}
        }},
        {{
            "number": 1,
            "headRefOid": "{}",
            "mergeStateStatus": "CLEAN",
            "statusCheckRollup": [{{"status": "COMPLETED", "conclusion": "SUCCESS"}}],
            "url": "https://github.com/my-org/test-repo/pull/1",
            "headRepositoryOwner": {{"login": "my-org"}}
        }}
    ]"#,
        head_sha
    );

    run_ci_status_test(&mut repo, "filters_by_repo_owner", &pr_json);
}

#[rstest]
fn test_list_full_with_configured_platform_github(mut repo: TestRepo) {
    // Set a non-GitHub remote (bitbucket) as origin - platform won't be auto-detected
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://bitbucket.org/test-owner/test-repo.git",
    ]);

    // Add a GitHub remote for PR detection (the configured platform still needs a
    // GitHub remote to determine which repo's PRs to check)
    repo.run_git(&[
        "remote",
        "add",
        "github",
        "https://github.com/test-owner/test-repo.git",
    ]);

    // Set the platform explicitly in project config
    repo.write_project_config(
        r#"
[ci]
platform = "github"
"#,
    );

    // Create a feature branch with tracking to the github remote
    repo.add_worktree("feature");
    setup_tracking_for_all_branches(&repo, "github");

    // Get actual commit SHA
    let head_sha = branch_sha(&repo, "feature");

    // Setup mock gh with PR data - this should work because the platform is set to github
    let pr_json = format!(
        r#"[{{
        "number": 1,
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
    repo.setup_mock_gh_with_ci_data(&pr_json);

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "list", &["--full"], None);
        repo.configure_mock_commands(&mut cmd);
        // The configured platform should force GitHub detection even with a bitbucket remote
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
fn test_list_full_with_gitea_forge_platform(mut repo: TestRepo) {
    // `forge.platform = "gitea"` resolves to the (experimental) Gitea CI
    // backend, but without `tea` installed there's nothing to show — CI stays
    // blank, and `wt list` must not warn that the value is "invalid". (Gitea CI
    // detection with a mocked `tea` is covered by the `gitea_*` tests below.)
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://gitea.example.com/test-owner/test-repo.git",
    ]);
    repo.write_project_config("[forge]\nplatform = \"gitea\"\n");

    repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "list", &["--full"], None);
        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_list_full_with_invalid_configured_platform(mut repo: TestRepo) {
    // Set GitHub remote URL
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://github.com/test-owner/test-repo.git",
    ]);

    // Set an invalid platform value - should warn and fall back to URL detection
    repo.write_project_config(
        r#"
[ci]
platform = "invalid_platform"
"#,
    );

    // Create a feature branch with tracking
    repo.add_worktree("feature");
    setup_tracking_for_all_branches(&repo, "origin");
    let head_sha = branch_sha(&repo, "feature");

    // Setup mock gh - platform should fall back to GitHub via URL detection
    let pr_json = format!(
        r#"[{{
        "number": 1,
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
    repo.setup_mock_gh_with_ci_data(&pr_json);

    let mut settings = setup_snapshot_settings(&repo);
    // Normalize worker thread ID prefix in log output (e.g., [n], [z], [A] -> [W]).
    // `label_for_thread_index` emits `0`, `a`-`z`, `A`-`Z`, and `?` (the latter
    // for thread IDs above 52, which appear on high-core machines where the
    // rayon pools span enough threads); the filter covers the whole alphabet.
    settings.add_filter(r"\[[a-zA-Z0-9?]\]", "[W]");
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "list", &["--full"], None);
        repo.configure_mock_commands(&mut cmd);
        // The snapshot asserts on the `log::warn!` diagnostic for the
        // invalid platform value, which is suppressed at the default
        // baseline filter (`-v 0` → Off). Opt this test into Warn-level
        // logging via `RUST_LOG`, which `logging::init` merges with the
        // verbose flag via `tracing_subscriber::EnvFilter` (env wins
        // when set).
        cmd.env("RUST_LOG", "warn");
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
    setup_tracking_for_all_branches(repo, "origin");
    branch_sha(repo, "feature")
}

#[rstest]
fn test_list_full_with_gitlab_mr_conflicts(mut repo: TestRepo) {
    let head_sha = setup_gitlab_repo_with_feature(&mut repo);

    let mr_json = format!(
        r#"[{{
        "iid": 1,
        "sha": "{}",
        "has_conflicts": true,
        "detailed_merge_status": null,
        "head_pipeline": {{"status": "success"}},
        "source_project_id": 12345,
        "web_url": "https://gitlab.com/test-group/test-project/-/merge_requests/1"
    }}]"#,
        head_sha
    );

    run_gitlab_ci_status_test(&mut repo, "gitlab_mr_conflicts", &mr_json, Some(12345));
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
        "iid": 1,
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
fn test_list_full_with_gitlab_filters_by_project_id(mut repo: TestRepo) {
    // Use a specific project for our repo
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://gitlab.com/my-group/my-project.git",
    ]);
    repo.add_worktree("feature");
    setup_tracking_for_all_branches(&repo, "origin");
    let head_sha = branch_sha(&repo, "feature");

    // Multiple MRs - only one from our project (should filter to project 99999)
    // The "other" MR is listed first to prove filtering works (not just taking first element)
    let mr_json = format!(
        r#"[
        {{
            "iid": 99,
            "sha": "wrong_sha",
            "has_conflicts": false,
            "detailed_merge_status": null,
            "head_pipeline": {{"status": "failed"}},
            "source_project_id": 11111,
            "web_url": "https://gitlab.com/other-group/other-project/-/merge_requests/99"
        }},
        {{
            "iid": 1,
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

// =============================================================================
// GitLab project ID edge cases (PR #846 panic prevention)
// =============================================================================

/// Test that single MR without project ID works (unambiguous case).
///
/// When `glab repo view` fails to return a project ID but there's only one MR,
/// we can safely use it since there's no ambiguity.
#[rstest]
fn test_list_full_with_gitlab_single_mr_no_project_id(mut repo: TestRepo) {
    let head_sha = setup_gitlab_repo_with_feature(&mut repo);

    // Single MR - should work even without project ID to filter by
    let mr_json = format!(
        r#"[{{
        "iid": 1,
        "sha": "{}",
        "has_conflicts": false,
        "detailed_merge_status": null,
        "head_pipeline": {{"status": "success"}},
        "source_project_id": 12345,
        "web_url": "https://gitlab.com/test-group/test-project/-/merge_requests/1"
    }}]"#,
        head_sha
    );

    // Pass None for project_id to trigger "no project ID" path
    run_gitlab_ci_status_test(&mut repo, "gitlab_single_mr_no_project_id", &mr_json, None);
}

/// Test that empty MR list without project ID returns None gracefully.
///
/// When no MRs are found and we don't have a project ID, the code should
/// return None without panicking. This falls through to pipeline detection.
#[rstest]
fn test_list_full_with_gitlab_empty_mr_list_no_project_id(mut repo: TestRepo) {
    setup_gitlab_repo_with_feature(&mut repo);

    // Empty MR list + no project ID -> falls through to pipeline check
    run_gitlab_ci_status_test(
        &mut repo,
        "gitlab_empty_mr_list_no_project_id",
        "[]", // Empty MR list
        None, // No project ID
    );
}

/// Test that multiple MRs without project ID are skipped (ambiguous case).
///
/// When there are multiple MRs with the same branch name and we can't determine
/// which project we're in, we skip CI detection rather than showing the wrong one.
/// This falls through to pipeline detection via `glab ci list`.
#[rstest]
fn test_list_full_with_gitlab_multiple_mrs_no_project_id(mut repo: TestRepo) {
    let head_sha = setup_gitlab_repo_with_feature(&mut repo);

    // Multiple MRs from different projects - ambiguous without project ID
    let mr_json = format!(
        r#"[
        {{
            "iid": 1,
            "sha": "{}",
            "has_conflicts": false,
            "detailed_merge_status": null,
            "head_pipeline": {{"status": "failed"}},
            "source_project_id": 11111,
            "web_url": "https://gitlab.com/org-a/project/-/merge_requests/1"
        }},
        {{
            "iid": 2,
            "sha": "{}",
            "has_conflicts": false,
            "detailed_merge_status": null,
            "head_pipeline": {{"status": "success"}},
            "source_project_id": 22222,
            "web_url": "https://gitlab.com/org-b/project/-/merge_requests/2"
        }}
    ]"#,
        head_sha, head_sha
    );

    // Pass None for project_id - should skip MR detection due to ambiguity
    // and fall through to pipeline detection (which will show NoCI since
    // our mock returns empty pipeline list)
    run_gitlab_ci_status_test(
        &mut repo,
        "gitlab_multiple_mrs_no_project_id",
        &mr_json,
        None,
    );
}

// =============================================================================
// URL-based pushremote tests (gh pr checkout scenario)
// =============================================================================

/// Test that CI status works when pushremote is a URL instead of a remote name.
///
/// This simulates the `gh pr checkout` scenario where git sets:
/// - branch.<name>.pushremote = https://github.com/fork-owner/repo.git (a URL)
/// - branch.<name>.merge = refs/pull/123/head (a PR ref)
///
/// Git's @{push} syntax fails when the push remote is a URL, so
/// `push_remote_url()` reads `%(push:remotename)`, which returns the URL directly.
#[rstest]
fn test_list_full_with_url_based_pushremote(mut repo: TestRepo) {
    // Set origin URL (the upstream repo where PRs are opened)
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://github.com/upstream-owner/test-repo.git",
    ]);
    repo.add_worktree("feature");
    let head_sha = branch_sha(&repo, "feature");

    // Simulate `gh pr checkout` behavior:
    // - Sets pushremote to the fork URL (not a remote name)
    // - Sets merge to a PR ref (not a normal branch ref)
    repo.run_git(&[
        "config",
        "branch.feature.pushremote",
        "https://github.com/fork-owner/test-repo.git", // URL, not remote name
    ]);
    repo.run_git(&[
        "config",
        "branch.feature.merge",
        "refs/pull/123/head", // PR ref, not branch ref
    ]);

    // The PR comes from the fork owner (matches pushremote URL)
    let pr_json = format!(
        r#"[{{
        "number": 1,
        "headRefOid": "{}",
        "mergeStateStatus": "CLEAN",
        "statusCheckRollup": [
            {{"status": "COMPLETED", "conclusion": "SUCCESS"}}
        ],
        "url": "https://github.com/upstream-owner/test-repo/pull/123",
        "headRepositoryOwner": {{"login": "fork-owner"}}
    }}]"#,
        head_sha
    );

    run_ci_status_test(&mut repo, "url_based_pushremote", &pr_json);
}

/// When a branch has no PR yet, fallback check-runs detection should query the
/// branch's pushremote repository rather than the first GitHub remote in the
/// repo.
#[rstest]
fn test_list_full_with_branch_fallback_using_fork_pushremote(mut repo: TestRepo) {
    setup_github_repo_with_feature(&mut repo);

    let feature_a_sha = branch_sha(&repo, "feature-a");
    repo.run_git(&[
        "config",
        "branch.feature-a.pushremote",
        "https://github.com/fork-owner/test-repo.git",
    ]);

    let fork_checks = r#"[{"status":"COMPLETED","conclusion":"SUCCESS"}]"#;
    let mock_bin = setup_mock_gh_with_api_data(
        &repo,
        "[]",
        &[
            (
                &format!("api repos/upstream-owner/test-repo/commits/{feature_a_sha}/check-runs"),
                "[]",
            ),
            (
                &format!("api repos/fork-owner/test-repo/commits/{feature_a_sha}/check-runs"),
                fork_checks,
            ),
        ],
    );

    let mut cmd = make_snapshot_cmd(&repo, "list", &["--full"], None);
    configure_mock_ci_env(&mut cmd, &mock_bin);
    let output = cmd.output().unwrap();
    assert!(output.status.success(), "wt list should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout)
        .ansi_strip()
        .into_owned();
    let feature_a_line = stdout
        .lines()
        .find(|line| line.contains("feature-a"))
        .expect("expected feature-a line in wt list output");
    assert!(
        feature_a_line.contains("#"),
        "expected feature-a to show passed branch CI from the fork repo fallback\nstdout:\n{stdout}",
    );
}

// =============================================================================
// GitLab error path tests
// =============================================================================

/// Test that when `glab mr view` fails after finding an MR, we show error status (not NoCI).
#[rstest]
fn test_list_full_with_gitlab_mr_view_failure(mut repo: TestRepo) {
    let head_sha = setup_gitlab_repo_with_feature(&mut repo);

    // Set up mock where mr list succeeds but mr view fails
    let mr_list_json = format!(
        r#"[{{
        "iid": 1,
        "sha": "{}",
        "has_conflicts": false,
        "detailed_merge_status": null,
        "source_project_id": 12345,
        "web_url": "https://gitlab.com/test/repo/-/merge_requests/1"
    }}]"#,
        head_sha
    );

    repo.setup_mock_glab_with_failing_mr_view(&mr_list_json, Some(12345));

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "list", &["--full"], None);
        repo.configure_mock_commands(&mut cmd);
        assert_cmd_snapshot!("gitlab_mr_view_failure", cmd);
    });
}

/// Test that rate limit errors in `glab ci list` show error status (not NoCI).
///
/// This exercises the `is_retriable_error` check in `detect_gitlab_pipeline`,
/// which is the fallback path when no MR exists for a branch.
#[rstest]
fn test_list_full_with_gitlab_ci_rate_limit(mut repo: TestRepo) {
    setup_gitlab_repo_with_feature(&mut repo);

    // Mock returns empty MR list (no MRs), so we fall through to ci list,
    // which returns a rate limit error
    repo.setup_mock_glab_with_ci_rate_limit(Some(12345));

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "list", &["--full"], None);
        repo.configure_mock_commands(&mut cmd);
        assert_cmd_snapshot!("gitlab_ci_rate_limit", cmd);
    });
}

/// A branch pipeline with no MR renders the bare `#` colored by pipeline
/// status, hyperlinked to the pipeline page (the success path of
/// `detect_gitlab_pipeline`; the rate-limit test above covers its error path).
#[rstest]
fn test_list_full_with_gitlab_pipeline(mut repo: TestRepo) {
    let head_sha = setup_gitlab_repo_with_feature(&mut repo);

    let pipeline_json = format!(
        r#"[{{
        "status": "success",
        "sha": "{head_sha}",
        "web_url": "https://gitlab.com/test-group/test-project/-/pipelines/123"
    }}]"#
    );
    repo.setup_mock_glab_with_pipeline(&pipeline_json, Some(12345));

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "list", &["--full"], None);
        repo.configure_mock_commands(&mut cmd);
        assert_cmd_snapshot!("gitlab_pipeline_success", cmd);
    });
}

/// An expired CI cache entry is refetched, not served: a stale "failed"
/// entry past its 30-60s TTL must not mask the fresh status.
#[rstest]
fn test_ci_cache_expired_entry_refetches(mut repo: TestRepo) {
    use crate::common::TEST_EPOCH;

    let head_sha = setup_github_repo_with_feature(&mut repo);

    // Fresh fetch reports passed
    let pr_json = format!(
        r#"[{{
        "number": 1,
        "headRefOid": "{head_sha}",
        "mergeStateStatus": "CLEAN",
        "statusCheckRollup": [{{"status": "COMPLETED", "conclusion": "SUCCESS"}}],
        "url": "https://github.com/test-owner/test-repo/pull/1",
        "headRepositoryOwner": {{"login": "test-owner"}}
    }}]"#
    );
    repo.setup_mock_gh_with_ci_data(&pr_json);

    // Expired cache entry claiming the CI failed
    let git_dir = repo.git_output(&["rev-parse", "--git-common-dir"]);
    let git_path = if Path::new(&git_dir).is_absolute() {
        std::path::PathBuf::from(&git_dir)
    } else {
        repo.root_path().join(&git_dir)
    };
    let cache_dir = git_path.join("wt").join("cache").join("ci-status");
    std::fs::create_dir_all(&cache_dir).unwrap();
    let expired_at = TEST_EPOCH - 3600;
    std::fs::write(
        cache_dir.join("feature.json"),
        format!(
            r##"{{"status":{{"ci_status":"failed","source":"pr","is_stale":false,"number":{{"number":1,"sigil":"#"}}}},"checked_at":{expired_at},"head":"{head_sha}","branch":"feature"}}"##
        ),
    )
    .unwrap();

    let mut cmd = make_snapshot_cmd(&repo, "list", &["--full"], None);
    repo.configure_mock_commands(&mut cmd);
    // Debug logging evaluates the cache-expiry diagnostics' format args
    cmd.env("RUST_LOG", "worktrunk=debug");
    cmd.env("CLICOLOR_FORCE", "1");
    let output = cmd.output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Fresh fetch wins: green #1, and the stale red #1 is gone
    assert!(
        stdout.contains("\x1b[32m#1"),
        "expired entry should refetch as green #1:\n{stdout}"
    );
    assert!(
        !stdout.contains("\x1b[31m#1"),
        "stale failed entry must not be served:\n{stdout}"
    );
}

// =============================================================================
// Azure DevOps CI status tests
// =============================================================================

/// Set up a repo with an Azure DevOps remote and a `feature` worktree.
/// Returns the `feature` branch HEAD SHA.
fn setup_azure_repo_with_feature(repo: &mut TestRepo) -> String {
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://dev.azure.com/myorg/myproject/_git/test-repo",
    ]);
    repo.add_worktree("feature");
    setup_tracking_for_all_branches(repo, "origin");
    branch_sha(repo, "feature")
}

/// Run an Azure DevOps CI status test with the given `az repos pr list` and
/// `az pipelines runs list` mock responses.
fn run_azure_ci_status_test(
    repo: &mut TestRepo,
    snapshot_name: &str,
    pr_list_json: &str,
    runs_json: &str,
) {
    repo.setup_mock_az_with_ci_data(pr_list_json, runs_json);

    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(repo, "list", &["--full"], None);
        repo.configure_mock_commands(&mut cmd);
        assert_cmd_snapshot!(snapshot_name, cmd);
    });
}

/// An active PR with `mergeStatus: "conflicts"` surfaces as a Conflicts
/// indicator (exercises `detect_azure_pr`).
#[rstest]
fn test_list_full_with_azure_pr_conflicts(mut repo: TestRepo) {
    let head_sha = setup_azure_repo_with_feature(&mut repo);

    let pr_list_json = format!(
        r#"[{{
        "pullRequestId": 7,
        "mergeStatus": "conflicts",
        "lastMergeSourceCommit": {{"commitId": "{}"}},
        "repository": {{"name": "test-repo", "project": {{"name": "myproject"}}}}
    }}]"#,
        head_sha
    );

    run_azure_ci_status_test(&mut repo, "azure_pr_conflicts", &pr_list_json, "[]");
}

/// An active PR with `mergeStatus: "queued"` surfaces as a Running indicator.
#[rstest]
fn test_list_full_with_azure_pr_queued(mut repo: TestRepo) {
    let head_sha = setup_azure_repo_with_feature(&mut repo);

    let pr_list_json = format!(
        r#"[{{
        "pullRequestId": 7,
        "mergeStatus": "queued",
        "lastMergeSourceCommit": {{"commitId": "{}"}},
        "repository": {{"name": "test-repo", "project": {{"name": "myproject"}}}}
    }}]"#,
        head_sha
    );

    run_azure_ci_status_test(&mut repo, "azure_pr_queued", &pr_list_json, "[]");
}

/// No PR for the branch falls back to the latest pipeline run
/// (exercises `detect_azure_pipeline` via `parse_azure_pipeline_status`).
#[rstest]
fn test_list_full_with_azure_passed_pipeline(mut repo: TestRepo) {
    let head_sha = setup_azure_repo_with_feature(&mut repo);

    let runs_json = format!(
        r#"[{{
        "id": 4242,
        "status": "completed",
        "result": "succeeded",
        "sourceVersion": "{}"
    }}]"#,
        head_sha
    );

    run_azure_ci_status_test(&mut repo, "azure_pipeline_passed", "[]", &runs_json);
}

/// A pipeline run from a different SHA than local HEAD is marked stale (dimmed).
#[rstest]
fn test_list_full_with_azure_stale_pipeline(mut repo: TestRepo) {
    setup_azure_repo_with_feature(&mut repo);

    let runs_json = r#"[{
        "id": 4242,
        "status": "completed",
        "result": "succeeded",
        "sourceVersion": "0000000000000000000000000000000000000000"
    }]"#;

    run_azure_ci_status_test(&mut repo, "azure_stale_pipeline", "[]", runs_json);
}

/// No PR and no pipeline runs → no CI indicator.
#[rstest]
fn test_list_full_with_azure_no_ci(mut repo: TestRepo) {
    setup_azure_repo_with_feature(&mut repo);
    run_azure_ci_status_test(&mut repo, "azure_no_ci", "[]", "[]");
}

/// A retriable error from `az repos pr list` (e.g., HTTP 429) surfaces as an
/// error indicator rather than NoCI (exercises the `is_retriable_error` branch
/// in `detect_azure_pr`).
#[rstest]
fn test_list_full_with_azure_pr_list_retriable_error(mut repo: TestRepo) {
    setup_azure_repo_with_feature(&mut repo);
    repo.setup_mock_az_with_detection_errors(
        Some("ERROR: HTTP error 429: Too Many Requests"),
        None,
    );

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "list", &["--full"], None);
        repo.configure_mock_commands(&mut cmd);
        assert_cmd_snapshot!("azure_pr_list_retriable_error", cmd);
    });
}

/// A retriable error from `az pipelines runs list` surfaces as an error
/// indicator (exercises the `is_retriable_error` branch in
/// `detect_azure_pipeline`, reached when no PR exists for the branch).
#[rstest]
fn test_list_full_with_azure_pipeline_retriable_error(mut repo: TestRepo) {
    setup_azure_repo_with_feature(&mut repo);
    repo.setup_mock_az_with_detection_errors(
        None,
        Some("ERROR: HTTP error 429: Too Many Requests"),
    );

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "list", &["--full"], None);
        repo.configure_mock_commands(&mut cmd);
        assert_cmd_snapshot!("azure_pipeline_retriable_error", cmd);
    });
}

// =============================================================================
// Gitea CI status tests
// =============================================================================

/// Set up a repo with a Gitea remote and a `feature` worktree carrying its own
/// commit (so its HEAD SHA differs from `main`'s, keeping the per-branch
/// commit-status lookups distinct). Returns the `feature` HEAD SHA.
fn setup_gitea_repo_with_feature(repo: &mut TestRepo) -> String {
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://gitea.example.com/owner/test-repo.git",
    ]);
    let feature_wt = repo.add_worktree("feature");
    repo.commit_in_worktree(
        &feature_wt,
        "gitea-ci.txt",
        "gitea ci test",
        "feat: gitea feature",
    );
    setup_tracking_for_all_branches(repo, "origin");
    branch_sha(repo, "feature")
}

/// Run a Gitea CI status test with the given `tea api .../pulls` and
/// `tea api .../commits/{sha}/status` mock responses, each an
/// `(HTTP status, body)` pair.
fn run_gitea_ci_status_test(
    repo: &mut TestRepo,
    snapshot_name: &str,
    head_sha: &str,
    pulls: (&str, &str),
    status: (&str, &str),
) {
    repo.setup_mock_tea_with_ci_data("owner", "test-repo", head_sha, pulls, status);

    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(repo, "list", &["--full"], None);
        repo.configure_mock_commands(&mut cmd);
        assert_cmd_snapshot!(snapshot_name, cmd);
    });
}

/// Gitea's `APIError` body, which the API returns in place of the resource
/// whenever a request fails — and which `tea api` copies to stdout with exit 0,
/// since it never reads the HTTP status. The message is a 500's wrapped
/// internal error (Gitea passes it through for an admin token), so
/// `is_retriable_error` recognizes the cause.
const GITEA_API_ERROR_BODY: &str = r#"{
    "message": "pq: dial tcp 10.0.0.5:5432: connect: connection refused",
    "url": "https://gitea.example.com/api/swagger"
}"#;

/// The same body a production Gitea sends a non-admin token for that 500:
/// `APIError` with the message blanked. Nothing in it says "error", which is
/// why the status line rather than the body is what the backend reads.
const GITEA_BLANK_ERROR_BODY: &str = r#"{
    "message": "",
    "url": "https://gitea.example.com/api/swagger"
}"#;

/// Build a one-PR `tea api .../pulls` response for the `feature` branch.
fn gitea_feature_pr_json(head_sha: &str, mergeable: bool) -> String {
    format!(
        r#"[{{
        "number": 7,
        "mergeable": {mergeable},
        "html_url": "https://gitea.example.com/owner/test-repo/pulls/7",
        "head": {{
            "ref": "feature",
            "sha": "{head_sha}",
            "repo": {{"owner": {{"login": "owner"}}}}
        }}
    }}]"#
    )
}

/// An open PR with `mergeable: false` surfaces as a Conflicts indicator
/// (exercises `detect_gitea_pr`).
#[rstest]
fn test_list_full_with_gitea_pr_conflicts(mut repo: TestRepo) {
    let head_sha = setup_gitea_repo_with_feature(&mut repo);
    run_gitea_ci_status_test(
        &mut repo,
        "gitea_pr_conflicts",
        &head_sha,
        ("200 OK", &gitea_feature_pr_json(&head_sha, false)),
        ("200 OK", r#"{"state":"","total_count":0}"#),
    );
}

/// No PR for the branch falls back to the HEAD commit's combined status
/// (exercises `detect_gitea_commit_status` and the `failure` state mapping).
#[rstest]
fn test_list_full_with_gitea_commit_status(mut repo: TestRepo) {
    let head_sha = setup_gitea_repo_with_feature(&mut repo);
    run_gitea_ci_status_test(
        &mut repo,
        "gitea_commit_status",
        &head_sha,
        ("200 OK", "[]"),
        ("200 OK", r#"{"state":"failure","total_count":1}"#),
    );
}

/// No PR and no commit statuses → no CI indicator.
#[rstest]
fn test_list_full_with_gitea_no_ci(mut repo: TestRepo) {
    let head_sha = setup_gitea_repo_with_feature(&mut repo);
    run_gitea_ci_status_test(
        &mut repo,
        "gitea_no_ci",
        &head_sha,
        ("200 OK", "[]"),
        ("200 OK", r#"{"state":"","total_count":0}"#),
    );
}

/// A Gitea 500 whose `APIError` body names a retriable cause surfaces as an
/// error indicator rather than NoCI. `tea api` exits 0 here — it copies the
/// response body through whatever the status — so the status line `--include`
/// puts on stderr is what separates this from a PR list.
#[rstest]
fn test_list_full_with_gitea_pr_error_body(mut repo: TestRepo) {
    let head_sha = setup_gitea_repo_with_feature(&mut repo);
    run_gitea_ci_status_test(
        &mut repo,
        "gitea_pr_error_body",
        &head_sha,
        ("500 Internal Server Error", GITEA_API_ERROR_BODY),
        ("200 OK", r#"{"state":"","total_count":0}"#),
    );
}

/// The same `APIError` body from the commit-status lookup (when no PR exists
/// for the branch). Read as data this one is the quieter bug: every field of
/// `GiteaCombinedStatus` defaults, so the error body would deserialize as "no
/// statuses" and paint a blank cell.
#[rstest]
fn test_list_full_with_gitea_commit_status_error_body(mut repo: TestRepo) {
    let head_sha = setup_gitea_repo_with_feature(&mut repo);
    run_gitea_ci_status_test(
        &mut repo,
        "gitea_commit_status_error_body",
        &head_sha,
        ("200 OK", "[]"),
        ("500 Internal Server Error", GITEA_API_ERROR_BODY),
    );
}

/// A 500 the message of which Gitea blanked still reaches the cell as an error.
/// The status is the whole basis: the body says nothing, so the text sniff that
/// used to decide had nothing to match and painted the same blank cell as a
/// healthy branch with no CI.
#[rstest]
fn test_list_full_with_gitea_blanked_500(mut repo: TestRepo) {
    let head_sha = setup_gitea_repo_with_feature(&mut repo);
    run_gitea_ci_status_test(
        &mut repo,
        "gitea_blanked_500",
        &head_sha,
        ("500 Internal Server Error", GITEA_BLANK_ERROR_BODY),
        ("200 OK", r#"{"state":"","total_count":0}"#),
    );
}

/// A 404 is the other half of that: also an error, also carrying no useful
/// text, but nothing a later `wt list` would answer differently — so the cell
/// stays blank rather than showing an indicator that never clears. Pairs with
/// the 500 above; between them the status is doing the deciding, not the body.
#[rstest]
fn test_list_full_with_gitea_not_found(mut repo: TestRepo) {
    let head_sha = setup_gitea_repo_with_feature(&mut repo);
    run_gitea_ci_status_test(
        &mut repo,
        "gitea_not_found",
        &head_sha,
        (
            "404 Not Found",
            r#"{"message":"user redirect does not exist [name: owner]"}"#,
        ),
        ("200 OK", r#"{"state":"","total_count":0}"#),
    );
}

/// Run `wt list --full` against the given `tea api .../pulls` response — an
/// `(HTTP status, body)` pair — and return stderr.
///
/// `RUST_LOG=warn` because `parse_json`'s warning is a `tracing` record and the
/// stderr layer is off at the default verbosity; `-v` would turn it on but bury
/// it under a template expansion per worktree.
fn gitea_ci_status_stderr(repo: &mut TestRepo, head_sha: &str, pulls: (&str, &str)) -> String {
    repo.setup_mock_tea_with_ci_data(
        "owner",
        "test-repo",
        head_sha,
        pulls,
        ("200 OK", r#"{"state":"","total_count":0}"#),
    );

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.env("RUST_LOG", "warn");
    cmd.args(["list", "--full"]).current_dir(repo.root_path());
    let output = cmd.output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "wt list --full failed: {stderr}");
    stderr
}

/// "May indicate a Gitea API change" is reserved for a 2xx whose body isn't the
/// resource, which is the only response that is one.
///
/// The status decides, so the two bodies that used to be the hard cases are
/// now read by what accompanied them. A 500 whose message Gitea blanked says
/// nothing about being an error, and a proxy's HTML page isn't Gitea's shape at
/// all; both reach the PR list as errors on the strength of the status line
/// alone, and the CI cell stays blank because neither carries retriable text.
/// The 200 case is the control: it pins where the warning does belong, and
/// keeps the others from passing merely because nothing logs at all.
///
/// One case per repo, not several runs in one: `wt list` caches CI status per
/// branch for 30s, so a second run against the same HEAD never reaches `tea`.
#[rstest]
#[case::blanked_500_is_an_error(
    ("500 Internal Server Error", GITEA_BLANK_ERROR_BODY),
    false
)]
#[case::proxy_page_is_an_error(("502 Bad Gateway", "<html>Bad Gateway</html>"), false)]
#[case::unknown_200_body_is_a_parse_failure(("200 OK", r#"{"unexpected":1}"#), true)]
fn test_list_full_gitea_parse_warning_is_reserved_for_unknown_bodies(
    mut repo: TestRepo,
    #[case] pulls: (&str, &str),
    #[case] expect_parse_warning: bool,
) {
    let head_sha = setup_gitea_repo_with_feature(&mut repo);
    let stderr = gitea_ci_status_stderr(&mut repo, &head_sha, pulls);

    assert_eq!(
        stderr.contains("Failed to parse tea api pulls JSON"),
        expect_parse_warning,
        "wrong diagnosis for {pulls:?}: {stderr}"
    );
}

/// `tea` itself failing on `tea api .../pulls` surfaces as an error indicator
/// rather than NoCI (exercises the `is_retriable_error` branch in
/// `detect_gitea_pr`). A transport failure is the case that does exit non-zero,
/// and `tea` names it on stderr.
#[rstest]
fn test_list_full_with_gitea_retriable_error(mut repo: TestRepo) {
    setup_gitea_repo_with_feature(&mut repo);
    repo.setup_mock_tea_with_detection_error(
        r#"Error: Get "https://gitea.example.com/api/v1/repos/owner/test-repo/pulls": dial tcp 10.0.0.5:443: connect: connection refused"#,
    );

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "list", &["--full"], None);
        repo.configure_mock_commands(&mut cmd);
        assert_cmd_snapshot!("gitea_retriable_error", cmd);
    });
}

/// A retriable error from the commit-status lookup (when no PR exists for the
/// branch) surfaces as an error indicator (exercises the `is_retriable_error`
/// branch in `fetch_combined_status`, reached via `detect_gitea_commit_status`).
#[rstest]
fn test_list_full_with_gitea_commit_status_retriable_error(mut repo: TestRepo) {
    let head_sha = setup_gitea_repo_with_feature(&mut repo);
    repo.setup_mock_tea_commit_status_error(
        &head_sha,
        r#"Error: Get "https://gitea.example.com/api/v1/repos/owner/test-repo/commits/HEAD/status": dial tcp 10.0.0.5:443: connect: connection refused"#,
    );

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "list", &["--full"], None);
        repo.configure_mock_commands(&mut cmd);
        assert_cmd_snapshot!("gitea_commit_status_retriable_error", cmd);
    });
}

/// `wt list --remotes --full` exercises the commit-status fallback for a
/// remote-only branch and proves it queries the branch's own remote, not the
/// primary one. Two Gitea remotes (`origin` → `owner/test-repo`, `fork` →
/// `forkowner/test-repo`) plus a remote-only `fork/feature-remote` ref. The
/// mock answers only `forkowner/test-repo` SHA-status requests, so a primary-
/// remote lookup in the fallback would return no CI; the green `#` in the
/// snapshot proves the SHA-status path honors `branch.remote`. (The PR path
/// intentionally uses the primary remote, mirroring the `gh` backend; for this
/// branch it returns no PR and we fall through to the SHA-status path.)
#[rstest]
fn test_list_remotes_full_with_gitea_remote_branch(mut repo: TestRepo) {
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://gitea.example.com/owner/test-repo.git",
    ]);
    repo.run_git(&[
        "remote",
        "add",
        "fork",
        "https://gitea.example.com/forkowner/test-repo.git",
    ]);

    // Build the remote-only `fork/feature-remote` ref in a temporary local
    // branch (mirroring how `test_list_with_remotes_and_full` does it for
    // origin), then drop the local copy so the row appears as remote-only.
    repo.run_git(&["checkout", "-b", "feature-remote"]);
    std::fs::write(repo.root_path().join("gitea-fork.txt"), "fork content").unwrap();
    repo.run_git(&["add", "."]);
    repo.run_git(&["commit", "-m", "feat: fork feature"]);
    let head_sha = branch_sha(&repo, "feature-remote");
    repo.run_git(&["update-ref", "refs/remotes/fork/feature-remote", &head_sha]);
    repo.run_git(&["checkout", "main"]);
    repo.run_git(&["branch", "-D", "feature-remote"]);

    repo.setup_mock_tea_with_ci_data(
        "forkowner",
        "test-repo",
        &head_sha,
        ("200 OK", "[]"),
        ("200 OK", r#"{"state":"success","total_count":1}"#),
    );

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "list", &["--remotes", "--full"], None);
        repo.configure_mock_commands(&mut cmd);
        assert_cmd_snapshot!("gitea_remote_branch_uses_branch_remote", cmd);
    });
}

/// A PR opened from a fork (head owner ≠ the queried upstream owner) is
/// matched: `wt list --full` lists the *primary* remote's open PRs and filters
/// by the branch's push owner against `head.repo.owner.login`. Two Gitea
/// remotes (`origin` → `upstream/test-repo`, `fork` → `forkowner/test-repo`)
/// and the `feature` branch pushes to `fork`. The mock returns two open PRs for
/// the `feature` ref — a decoy from `other-owner` that must be filtered out and
/// the real one from `forkowner` — and answers the upstream's commit-status
/// lookup with `success`. The green `#7` proves the fork PR matched; a revert to
/// querying/filtering by the branch's own remote would query `forkowner`'s
/// (unmocked) `/pulls` and lose the indicator. Mirrors the GitHub backend's
/// `test_list_full_filters_by_repo_owner`.
#[rstest]
fn test_list_full_with_gitea_fork_pr(mut repo: TestRepo) {
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://gitea.example.com/upstream/test-repo.git",
    ]);
    repo.run_git(&[
        "remote",
        "add",
        "fork",
        "https://gitea.example.com/forkowner/test-repo.git",
    ]);
    let feature_wt = repo.add_worktree("feature");
    repo.commit_in_worktree(
        &feature_wt,
        "gitea-fork-ci.txt",
        "gitea fork ci test",
        "feat: gitea fork feature",
    );
    setup_tracking_for_all_branches(&repo, "fork");
    let head_sha = branch_sha(&repo, "feature");

    let pulls_json = format!(
        r#"[
        {{
            "number": 98,
            "mergeable": true,
            "html_url": "https://gitea.example.com/upstream/test-repo/pulls/98",
            "head": {{"ref": "feature", "sha": "wrong_sha", "repo": {{"owner": {{"login": "other-owner"}}}}}}
        }},
        {{
            "number": 7,
            "mergeable": true,
            "html_url": "https://gitea.example.com/upstream/test-repo/pulls/7",
            "head": {{"ref": "feature", "sha": "{head_sha}", "repo": {{"owner": {{"login": "forkowner"}}}}}}
        }}
    ]"#
    );

    repo.setup_mock_tea_with_ci_data(
        "upstream",
        "test-repo",
        &head_sha,
        ("200 OK", &pulls_json),
        ("200 OK", r#"{"state":"success","total_count":1}"#),
    );

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "list", &["--full"], None);
        repo.configure_mock_commands(&mut cmd);
        assert_cmd_snapshot!("gitea_fork_pr", cmd);
    });
}
