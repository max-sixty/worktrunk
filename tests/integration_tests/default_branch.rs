use crate::common::{TestRepo, repo, repo_with_remote};
use rstest::rstest;
use worktrunk::git::{GitRemoteUrl, Repository};

#[rstest]
fn test_get_default_branch_with_origin_head(#[from(repo_with_remote)] repo: TestRepo) {
    // origin/HEAD should be set automatically by setup_remote
    assert!(repo.has_origin_head());

    // Test that we can get the default branch
    let branch = Repository::at(repo.root_path())
        .unwrap()
        .default_branch()
        .unwrap();
    assert_eq!(branch, "main");
}

#[rstest]
fn test_get_default_branch_without_origin_head(#[from(repo_with_remote)] repo: TestRepo) {
    // Clear origin/HEAD to force remote query
    repo.clear_origin_head();
    assert!(!repo.has_origin_head());

    // Should still work by querying remote
    let branch = Repository::at(repo.root_path())
        .unwrap()
        .default_branch()
        .unwrap();
    assert_eq!(branch, "main");

    // Verify that worktrunk's cache is now set
    let cached = repo
        .git_command()
        .args(["config", "--get", "worktrunk.default-branch"])
        .run()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&cached.stdout).trim(), "main");
}

#[rstest]
fn test_get_default_branch_caches_result(#[from(repo_with_remote)] repo: TestRepo) {
    // Clear both caches to force remote query
    repo.clear_origin_head();
    let _ = repo
        .git_command()
        .args(["config", "--unset", "worktrunk.default-branch"])
        .run();

    // First call queries remote and caches to worktrunk config
    Repository::at(repo.root_path())
        .unwrap()
        .default_branch()
        .unwrap();
    let cached = repo
        .git_command()
        .args(["config", "--get", "worktrunk.default-branch"])
        .run()
        .unwrap();
    assert!(cached.status.success());

    // Second call uses cache (fast path)
    let branch = Repository::at(repo.root_path())
        .unwrap()
        .default_branch()
        .unwrap();
    assert_eq!(branch, "main");
}

#[rstest]
fn test_get_default_branch_no_remote(repo: TestRepo) {
    // Remove origin (fixture has it) for this no-remote test
    repo.run_git(&["remote", "remove", "origin"]);

    // No remote configured, should infer from local branches
    // Since there's only one local branch, it should return that
    let result = Repository::at(repo.root_path()).unwrap().default_branch();
    assert!(result.is_some());

    // The inferred branch should match the current branch
    let inferred_branch = result.unwrap();
    let repo_instance = Repository::at(repo.root_path()).unwrap();
    let current_branch = repo_instance
        .worktree_at(repo.root_path())
        .branch()
        .unwrap()
        .unwrap();
    assert_eq!(inferred_branch, current_branch);
}

#[rstest]
fn test_get_default_branch_with_custom_remote(mut repo: TestRepo) {
    repo.setup_custom_remote("upstream", "main");

    // Test that we can get the default branch from a custom remote
    let branch = Repository::at(repo.root_path())
        .unwrap()
        .default_branch()
        .unwrap();
    assert_eq!(branch, "main");
}

#[rstest]
fn test_primary_remote_detects_custom_remote(mut repo: TestRepo) {
    // Remove origin (fixture has it) so upstream becomes the primary
    repo.run_git(&["remote", "remove", "origin"]);

    // Use "main" since that's the local branch - the test only cares about remote name detection
    repo.setup_custom_remote("upstream", "main");

    // Test that primary_remote detects the custom remote name
    let git_repo = Repository::at(repo.root_path()).unwrap();
    let remote = git_repo.primary_remote().unwrap();
    assert_eq!(remote, "upstream");
}

#[rstest]
fn test_branch_exists_with_custom_remote(mut repo: TestRepo) {
    repo.setup_custom_remote("upstream", "main");

    let git_repo = Repository::at(repo.root_path()).unwrap();

    // Should find the branch on the custom remote
    assert!(git_repo.branch("main").exists().unwrap());

    // Should not find non-existent branch
    assert!(!git_repo.branch("nonexistent").exists().unwrap());
}

#[rstest]
fn test_get_default_branch_no_remote_common_names_fallback(repo: TestRepo) {
    // Remove origin (fixture has it) for this no-remote test
    repo.run_git(&["remote", "remove", "origin"]);

    // Create additional branches (no remote configured)
    repo.git_command()
        .args(["branch", "feature"])
        .run()
        .unwrap();
    repo.git_command().args(["branch", "bugfix"]).run().unwrap();

    // Now we have multiple branches: main, feature, bugfix
    // Should detect "main" from the common names list
    let branch = Repository::at(repo.root_path())
        .unwrap()
        .default_branch()
        .unwrap();
    assert_eq!(branch, "main");
}

#[rstest]
fn test_get_default_branch_no_remote_master_fallback(repo: TestRepo) {
    // Remove origin (fixture has it) for this no-remote test
    repo.run_git(&["remote", "remove", "origin"]);

    // Rename main to master, then create other branches
    repo.git_command()
        .args(["branch", "-m", "main", "master"])
        .run()
        .unwrap();
    repo.git_command()
        .args(["branch", "feature"])
        .run()
        .unwrap();
    repo.git_command().args(["branch", "bugfix"]).run().unwrap();

    // Now we have: master, feature, bugfix (no "main")
    // Should detect "master" from the common names list
    let branch = Repository::at(repo.root_path())
        .unwrap()
        .default_branch()
        .unwrap();
    assert_eq!(branch, "master");
}

#[rstest]
fn test_default_branch_no_remote_uses_init_config(repo: TestRepo) {
    // Remove origin (fixture has it) for this no-remote test
    repo.run_git(&["remote", "remove", "origin"]);

    // Rename main to something non-standard, create the configured default
    repo.git_command()
        .args(["branch", "-m", "main", "primary"])
        .run()
        .unwrap();
    repo.git_command()
        .args(["branch", "feature"])
        .run()
        .unwrap();

    // Set init.defaultBranch - this should be checked before common names
    repo.git_command()
        .args(["config", "init.defaultBranch", "primary"])
        .run()
        .unwrap();

    // Now we have: primary, feature (no common names like main/master)
    // Should detect "primary" via init.defaultBranch config
    let branch = Repository::at(repo.root_path())
        .unwrap()
        .default_branch()
        .unwrap();
    assert_eq!(branch, "primary");
}

#[rstest]
fn test_configured_default_branch_does_not_exist_returns_none(repo: TestRepo) {
    // Configure a non-existent branch
    repo.git_command()
        .args(["config", "worktrunk.default-branch", "nonexistent-branch"])
        .run()
        .unwrap();

    // Should return None when configured branch doesn't exist locally
    let result = Repository::at(repo.root_path()).unwrap().default_branch();
    assert!(
        result.is_none(),
        "Expected None when configured branch doesn't exist, got: {:?}",
        result
    );
}

#[rstest]
fn test_invalid_default_branch_config_returns_configured_value(repo: TestRepo) {
    // Configure a non-existent branch
    repo.git_command()
        .args(["config", "worktrunk.default-branch", "nonexistent-branch"])
        .run()
        .unwrap();

    // Should report the invalid configuration
    let invalid = Repository::at(repo.root_path())
        .unwrap()
        .invalid_default_branch_config();
    assert_eq!(invalid, Some("nonexistent-branch".to_string()));
}

#[rstest]
fn test_invalid_default_branch_config_returns_none_when_valid(repo: TestRepo) {
    // Configure the existing "main" branch
    repo.git_command()
        .args(["config", "worktrunk.default-branch", "main"])
        .run()
        .unwrap();

    // Should return None since the configured branch exists
    let invalid = Repository::at(repo.root_path())
        .unwrap()
        .invalid_default_branch_config();
    assert!(
        invalid.is_none(),
        "Expected None when configured branch exists, got: {:?}",
        invalid
    );
}

#[rstest]
fn test_get_default_branch_no_remote_fails_when_no_match(repo: TestRepo) {
    // Remove origin (fixture has it) for this no-remote test
    repo.run_git(&["remote", "remove", "origin"]);

    // Rename main to something non-standard
    repo.git_command()
        .args(["branch", "-m", "main", "xyz"])
        .run()
        .unwrap();
    repo.git_command().args(["branch", "abc"]).run().unwrap();
    repo.git_command().args(["branch", "def"]).run().unwrap();

    // Now we have: xyz, abc, def - no common names, no init.defaultBranch
    // In normal repos (not bare), symbolic-ref HEAD isn't used because HEAD
    // points to the current branch, not the default branch.
    // Should return None when default branch cannot be determined
    let result = Repository::at(repo.root_path()).unwrap().default_branch();
    assert!(
        result.is_none(),
        "Expected None when default branch cannot be determined, got: {:?}",
        result
    );
}

#[rstest]
fn test_resolve_caret_fails_when_default_branch_unavailable(repo: TestRepo) {
    // Remove origin (fixture has it) for this no-remote test
    repo.run_git(&["remote", "remove", "origin"]);

    // Rename main to something non-standard so default branch can't be determined
    repo.git_command()
        .args(["branch", "-m", "main", "xyz"])
        .run()
        .unwrap();
    repo.git_command().args(["branch", "abc"]).run().unwrap();
    repo.git_command().args(["branch", "def"]).run().unwrap();

    // Now resolving "^" should fail with an error
    let git_repo = Repository::at(repo.root_path()).unwrap();
    let result = git_repo.resolve_worktree_name("^");
    assert!(
        result.is_err(),
        "Expected error when resolving ^ without default branch"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("Cannot determine default branch"),
        "Error should mention cannot determine default branch, got: {}",
        err_msg
    );
}

// --- Forge URL resolution helpers ---

/// Configure a remote with a custom hostname and an insteadOf rewrite to a real forge.
///
/// Simulates the multi-key SSH pattern: custom host in .git/config, real forge via insteadOf.
fn setup_insteadof(repo: &TestRepo, remote: &str, custom_url: &str, real_prefix: &str) {
    // Extract the org prefix from the custom URL for the insteadOf mapping
    let custom_prefix = custom_url
        .rsplit_once('/')
        .map(|(p, _)| p)
        .unwrap_or(custom_url);
    repo.run_git(&["config", &format!("remote.{remote}.url"), custom_url]);
    repo.run_git(&[
        "config",
        &format!("url.{real_prefix}.insteadOf"),
        custom_prefix,
    ]);
}

/// Set up push tracking so `branch.push_remote()` and `github_push_url()` work.
fn setup_push_tracking(repo: &TestRepo, branch: &str, remote: &str) {
    repo.run_git(&["config", &format!("branch.{branch}.remote"), remote]);
    repo.run_git(&[
        "config",
        &format!("branch.{branch}.merge"),
        &format!("refs/heads/{branch}"),
    ]);
    repo.run_git(&[
        "update-ref",
        &format!("refs/remotes/{remote}/{branch}"),
        branch,
    ]);
}

/// Test effective_remote_url: insteadOf resolves custom hostname to real forge.
#[rstest]
fn test_effective_remote_url_insteadof(repo: TestRepo) {
    setup_insteadof(
        &repo,
        "origin",
        "git@work-ssh:org/repo.git",
        "git@github.com:org",
    );

    let git_repo = Repository::at(repo.root_path()).unwrap();

    // Raw URL has the custom hostname
    assert_eq!(
        git_repo.remote_url("origin").unwrap(),
        "git@work-ssh:org/repo.git"
    );
    // Effective URL has the real forge hostname
    let effective = git_repo.effective_remote_url("origin").unwrap();
    assert_eq!(effective, "git@github.com:org/repo.git");

    let parsed = GitRemoteUrl::parse(&effective).unwrap();
    assert!(parsed.is_github());
    assert_eq!(parsed.host(), "github.com");
    assert_eq!(parsed.owner(), "org");
    assert_eq!(parsed.repo(), "repo");
}

/// Test effective_remote_url: matches raw URL when no insteadOf is configured.
#[rstest]
fn test_effective_remote_url_without_insteadof(repo: TestRepo) {
    let git_repo = Repository::at(repo.root_path()).unwrap();
    assert_eq!(
        git_repo.remote_url("origin").unwrap(),
        git_repo.effective_remote_url("origin").unwrap()
    );
}

/// Test effective_remote_url: returns None for nonexistent remote.
#[rstest]
fn test_effective_remote_url_nonexistent_remote(repo: TestRepo) {
    let git_repo = Repository::at(repo.root_path()).unwrap();
    assert!(git_repo.effective_remote_url("nonexistent").is_none());
}

/// Test effective_remote_url: result is cached (same value on repeated calls).
#[rstest]
fn test_effective_remote_url_is_cached(repo: TestRepo) {
    let git_repo = Repository::at(repo.root_path()).unwrap();
    let first = git_repo.effective_remote_url("origin");
    let second = git_repo.effective_remote_url("origin");
    assert_eq!(first, second);
}

/// Test find_remote_for_repo: resolves through insteadOf to match owner/repo.
#[rstest]
fn test_find_remote_for_repo_insteadof(repo: TestRepo) {
    setup_insteadof(
        &repo,
        "origin",
        "git@work-ssh:org/repo.git",
        "git@github.com:org",
    );

    let git_repo = Repository::at(repo.root_path()).unwrap();

    // Raw URL has custom hostname — find_remote_for_repo should match via the
    // effective URL (github.com), which reveals the real forge after insteadOf
    let found = git_repo.find_remote_for_repo(Some("github.com"), "org", "repo");
    assert_eq!(found.as_deref(), Some("origin"));
}

/// Test find_remote_for_repo: case-insensitive matching works with insteadOf.
#[rstest]
fn test_find_remote_for_repo_insteadof_case_insensitive(repo: TestRepo) {
    setup_insteadof(
        &repo,
        "origin",
        "git@work-ssh:MyOrg/MyRepo.git",
        "git@github.com:MyOrg",
    );

    let git_repo = Repository::at(repo.root_path()).unwrap();
    let found = git_repo.find_remote_for_repo(Some("github.com"), "myorg", "myrepo");
    assert_eq!(found.as_deref(), Some("origin"));
}

/// Test find_remote_for_repo: matches without host constraint via insteadOf.
#[rstest]
fn test_find_remote_for_repo_insteadof_no_host(repo: TestRepo) {
    setup_insteadof(
        &repo,
        "origin",
        "git@work-ssh:org/repo.git",
        "git@github.com:org",
    );

    let git_repo = Repository::at(repo.root_path()).unwrap();
    // host=None should match any forge host
    let found = git_repo.find_remote_for_repo(None, "org", "repo");
    assert_eq!(found.as_deref(), Some("origin"));
}

/// Test find_remote_for_repo: picks the correct remote among multiple with insteadOf.
#[rstest]
fn test_find_remote_for_repo_insteadof_multiple_remotes(repo: TestRepo) {
    // origin → github.com:org/repo via insteadOf
    setup_insteadof(
        &repo,
        "origin",
        "git@work-ssh:org/repo.git",
        "git@github.com:org",
    );
    // upstream → github.com:upstream-org/repo via insteadOf
    repo.run_git(&[
        "config",
        "remote.upstream.url",
        "git@work-ssh-2:upstream-org/repo.git",
    ]);
    repo.run_git(&[
        "config",
        "url.git@github.com:upstream-org.insteadOf",
        "git@work-ssh-2:upstream-org",
    ]);

    let git_repo = Repository::at(repo.root_path()).unwrap();
    assert_eq!(
        git_repo
            .find_remote_for_repo(Some("github.com"), "upstream-org", "repo")
            .as_deref(),
        Some("upstream")
    );
    assert_eq!(
        git_repo
            .find_remote_for_repo(Some("github.com"), "org", "repo")
            .as_deref(),
        Some("origin")
    );
}

/// Test find_remote_by_url: resolves through insteadOf.
#[rstest]
fn test_find_remote_by_url_insteadof(repo: TestRepo) {
    setup_insteadof(
        &repo,
        "origin",
        "git@work-ssh:org/repo.git",
        "git@github.com:org",
    );

    let git_repo = Repository::at(repo.root_path()).unwrap();

    // target_url uses the real forge hostname (as API responses would)
    let found = git_repo.find_remote_by_url("git@github.com:org/repo.git");
    assert_eq!(found.as_deref(), Some("origin"));

    // HTTPS variant should also match
    let found = git_repo.find_remote_by_url("https://github.com/org/repo.git");
    assert_eq!(found.as_deref(), Some("origin"));
}

/// Test github_push_url: resolves through insteadOf on push remote.
#[rstest]
fn test_github_push_url_insteadof_fallback(repo: TestRepo) {
    setup_insteadof(
        &repo,
        "origin",
        "git@work-ssh:org/repo.git",
        "git@github.com:org",
    );
    setup_push_tracking(&repo, "main", "origin");

    let git_repo = Repository::at(repo.root_path()).unwrap();
    let url = git_repo
        .branch("main")
        .github_push_url()
        .expect("github_push_url should resolve via insteadOf");
    let parsed = GitRemoteUrl::parse(&url).unwrap();
    assert!(parsed.is_github());
    assert_eq!(parsed.host(), "github.com");
}

/// Test github_push_url: returns None for non-GitHub forge (GitLab).
#[rstest]
fn test_github_push_url_non_github_forge_returns_none(repo: TestRepo) {
    repo.run_git(&["config", "remote.origin.url", "git@gitlab.com:org/repo.git"]);
    setup_push_tracking(&repo, "main", "origin");

    let git_repo = Repository::at(repo.root_path()).unwrap();
    assert!(git_repo.branch("main").github_push_url().is_none());
}

/// Test github_push_url: returns None when insteadOf resolves to GitLab (not GitHub).
#[rstest]
fn test_github_push_url_unknown_host_non_github_insteadof(repo: TestRepo) {
    setup_insteadof(
        &repo,
        "origin",
        "git@work-ssh:org/repo.git",
        "git@gitlab.com:org",
    );
    setup_push_tracking(&repo, "main", "origin");

    let git_repo = Repository::at(repo.root_path()).unwrap();
    assert!(git_repo.branch("main").github_push_url().is_none());
}
