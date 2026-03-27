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

/// Test that forge_remote_url falls back to effective URL when raw URL has a custom hostname.
///
/// Simulates the url.insteadOf pattern used for multi-key SSH setups:
///   .git/config: url = git@work-ssh:org/repo.git  (custom hostname)
///   local config: url."git@github.com:org".insteadOf "git@work-ssh:org"
///   git remote get-url → git@github.com:org/repo.git  (real hostname via insteadOf)
#[rstest]
fn test_forge_remote_url_insteadof_fallback(repo: TestRepo) {
    // Set the raw remote URL to use a custom hostname (not a known forge).
    // The hostname must NOT contain "github" or "gitlab" since is_known_forge()
    // uses substring matching (e.g., "github-work" would match as GitHub).
    repo.run_git(&["config", "remote.origin.url", "git@work-ssh:org/repo.git"]);

    // Configure insteadOf to map the custom hostname to github.com.
    // This simulates a user's SSH multi-key setup where ~/.ssh/config
    // defines custom hosts and git config rewrites URLs.
    repo.run_git(&[
        "config",
        "url.git@github.com:org.insteadOf",
        "git@work-ssh:org",
    ]);

    let git_repo = Repository::at(repo.root_path()).unwrap();

    // raw remote_url should return the custom hostname
    let raw_url = git_repo.remote_url("origin").unwrap();
    assert_eq!(raw_url, "git@work-ssh:org/repo.git");
    assert!(
        !GitRemoteUrl::parse(&raw_url).unwrap().is_known_forge(),
        "Raw URL should have an unrecognized hostname"
    );

    // effective_remote_url should apply insteadOf and return the real hostname
    let effective_url = git_repo.effective_remote_url("origin").unwrap();
    assert_eq!(effective_url, "git@github.com:org/repo.git");

    // forge_remote_url should detect the unknown hostname and fall back to the effective URL
    let forge_url = git_repo.forge_remote_url("origin").unwrap();
    let parsed = GitRemoteUrl::parse(&forge_url).unwrap();
    assert!(
        parsed.is_known_forge(),
        "forge_remote_url should resolve to a known forge, got: {}",
        forge_url
    );
    assert_eq!(parsed.host(), "github.com");
    assert_eq!(parsed.owner(), "org");
    assert_eq!(parsed.repo(), "repo");
}

/// Test that forge_remote_url returns raw URL when it already has a known forge hostname.
#[rstest]
fn test_forge_remote_url_known_forge_no_fallback(repo: TestRepo) {
    let git_repo = Repository::at(repo.root_path()).unwrap();

    // The fixture already has a github.com remote URL
    let raw_url = git_repo.remote_url("origin").unwrap();
    let forge_url = git_repo.forge_remote_url("origin").unwrap();

    // Should return the raw URL directly (no fallback needed)
    assert_eq!(forge_url, raw_url);
}

/// Test that forge_remote_url returns raw URL when neither raw nor effective URL is a known forge.
#[rstest]
fn test_forge_remote_url_unknown_forge_returns_raw(repo: TestRepo) {
    // Set both raw and effective URLs to unknown forges (no insteadOf configured)
    repo.run_git(&[
        "config",
        "remote.origin.url",
        "git@bitbucket.org:org/repo.git",
    ]);

    let git_repo = Repository::at(repo.root_path()).unwrap();

    let forge_url = git_repo.forge_remote_url("origin").unwrap();
    assert_eq!(
        forge_url, "git@bitbucket.org:org/repo.git",
        "Should return raw URL when neither is a known forge"
    );
}

/// Test that primary_forge_remote_url resolves through insteadOf.
#[rstest]
fn test_primary_forge_remote_url_with_insteadof(repo: TestRepo) {
    repo.run_git(&["config", "remote.origin.url", "git@work-ssh:org/repo.git"]);
    repo.run_git(&[
        "config",
        "url.git@github.com:org.insteadOf",
        "git@work-ssh:org",
    ]);

    let git_repo = Repository::at(repo.root_path()).unwrap();
    let url = git_repo.primary_forge_remote_url().unwrap();
    let parsed = GitRemoteUrl::parse(&url).unwrap();
    assert!(parsed.is_github());
    assert_eq!(parsed.host(), "github.com");
}

/// Test that effective_remote_url returns the same URL when no insteadOf is configured.
#[rstest]
fn test_effective_remote_url_without_insteadof(repo: TestRepo) {
    let git_repo = Repository::at(repo.root_path()).unwrap();

    let raw = git_repo.remote_url("origin").unwrap();
    let effective = git_repo.effective_remote_url("origin").unwrap();
    // Without insteadOf, raw and effective should be the same
    assert_eq!(raw, effective);
}

/// Test find_forge_remote with insteadOf alias.
///
/// Verifies the two-pass iteration: raw URLs first, then effective URLs.
#[rstest]
fn test_find_forge_remote_insteadof(repo: TestRepo) {
    repo.run_git(&["config", "remote.origin.url", "git@work-ssh:org/repo.git"]);
    repo.run_git(&[
        "config",
        "url.git@github.com:org.insteadOf",
        "git@work-ssh:org",
    ]);

    let git_repo = Repository::at(repo.root_path()).unwrap();

    // Should find GitHub via insteadOf fallback
    let result = git_repo.find_forge_remote(|parsed| parsed.is_github());
    assert!(
        result.is_some(),
        "Should find GitHub via insteadOf fallback"
    );
    let (remote_name, url) = result.unwrap();
    assert_eq!(remote_name, "origin");
    let parsed = GitRemoteUrl::parse(&url).unwrap();
    assert_eq!(parsed.host(), "github.com");

    // Should NOT find GitLab
    let result = git_repo.find_forge_remote(|parsed| parsed.is_gitlab());
    assert!(result.is_none(), "Should not find GitLab");
}

/// Test find_forge_remote with a known forge hostname (fast path).
#[rstest]
fn test_find_forge_remote_known_forge(repo: TestRepo) {
    // Set origin to a GitHub URL so the fast path (raw URL check) works
    repo.run_git(&["config", "remote.origin.url", "git@github.com:org/repo.git"]);

    let git_repo = Repository::at(repo.root_path()).unwrap();
    let result = git_repo.find_forge_remote(|parsed| parsed.is_github());
    assert!(result.is_some(), "Should find GitHub on fast path");
    let (name, url) = result.unwrap();
    assert_eq!(name, "origin");
    assert_eq!(url, "git@github.com:org/repo.git");
}

/// Test github_push_url with insteadOf alias on push remote.
///
/// When the push remote URL has a custom hostname, github_push_url should
/// fall back to the forge-aware URL resolution.
#[rstest]
fn test_github_push_url_insteadof_fallback(repo: TestRepo) {
    // Set origin URL to custom hostname
    repo.run_git(&["config", "remote.origin.url", "git@work-ssh:org/repo.git"]);
    // Configure insteadOf to map to github.com
    repo.run_git(&[
        "config",
        "url.git@github.com:org.insteadOf",
        "git@work-ssh:org",
    ]);
    // Set up push tracking for main branch (both config and remote-tracking ref)
    repo.run_git(&["config", "branch.main.remote", "origin"]);
    repo.run_git(&["config", "branch.main.merge", "refs/heads/main"]);
    repo.run_git(&["update-ref", "refs/remotes/origin/main", "main"]);

    let git_repo = Repository::at(repo.root_path()).unwrap();
    let branch = git_repo.branch("main");
    let push_url = branch.github_push_url();

    // Should resolve through insteadOf to a GitHub URL
    assert!(
        push_url.is_some(),
        "github_push_url should resolve via insteadOf"
    );
    let url = push_url.unwrap();
    let parsed = GitRemoteUrl::parse(&url).unwrap();
    assert!(parsed.is_github());
    assert_eq!(parsed.host(), "github.com");
}

/// Test github_push_url returns None when push remote is a non-GitHub forge.
///
/// Even after insteadOf fallback, if the URL resolves to GitLab (not GitHub),
/// github_push_url should return None.
#[rstest]
fn test_github_push_url_non_github_forge_returns_none(repo: TestRepo) {
    // Set origin URL to a GitLab remote
    repo.run_git(&["config", "remote.origin.url", "git@gitlab.com:org/repo.git"]);
    // Set up push tracking for main branch
    repo.run_git(&["config", "branch.main.remote", "origin"]);
    repo.run_git(&["config", "branch.main.merge", "refs/heads/main"]);
    repo.run_git(&["update-ref", "refs/remotes/origin/main", "main"]);

    let git_repo = Repository::at(repo.root_path()).unwrap();
    let branch = git_repo.branch("main");

    // GitLab URL is a known forge but not GitHub — should return None
    assert!(
        branch.github_push_url().is_none(),
        "github_push_url should return None for GitLab remotes"
    );
}

/// Test github_push_url returns None when push remote has unknown hostname
/// and insteadOf resolves to a non-GitHub forge.
#[rstest]
fn test_github_push_url_unknown_host_non_github_insteadof(repo: TestRepo) {
    // Set origin URL to custom hostname
    repo.run_git(&["config", "remote.origin.url", "git@work-ssh:org/repo.git"]);
    // insteadOf maps to GitLab, not GitHub
    repo.run_git(&[
        "config",
        "url.git@gitlab.com:org.insteadOf",
        "git@work-ssh:org",
    ]);
    // Set up push tracking
    repo.run_git(&["config", "branch.main.remote", "origin"]);
    repo.run_git(&["config", "branch.main.merge", "refs/heads/main"]);
    repo.run_git(&["update-ref", "refs/remotes/origin/main", "main"]);

    let git_repo = Repository::at(repo.root_path()).unwrap();
    let branch = git_repo.branch("main");

    // Fallback resolves to GitLab, not GitHub — should return None
    assert!(
        branch.github_push_url().is_none(),
        "github_push_url should return None when insteadOf resolves to GitLab"
    );
}

/// Test find_forge_remote returns None when no remotes are configured.
#[rstest]
fn test_find_forge_remote_no_remotes(repo: TestRepo) {
    // Remove the origin remote
    repo.run_git(&["remote", "remove", "origin"]);

    let git_repo = Repository::at(repo.root_path()).unwrap();
    let result = git_repo.find_forge_remote(|parsed| parsed.is_github());
    assert!(result.is_none(), "Should return None with no remotes");
}

/// Test forge_remote_url when effective URL differs from raw but is also not a known forge.
///
/// When insteadOf rewrites to another custom hostname (not github/gitlab),
/// forge_remote_url should still return the raw URL as best-effort.
#[rstest]
fn test_forge_remote_url_insteadof_to_unknown_forge(repo: TestRepo) {
    repo.run_git(&[
        "config",
        "remote.origin.url",
        "git@custom-host:org/repo.git",
    ]);
    // insteadOf rewrites to another unknown host
    repo.run_git(&[
        "config",
        "url.git@other-host:org.insteadOf",
        "git@custom-host:org",
    ]);

    let git_repo = Repository::at(repo.root_path()).unwrap();

    let forge_url = git_repo.forge_remote_url("origin").unwrap();
    // Neither raw nor effective is a known forge — should return raw URL
    assert_eq!(
        forge_url, "git@custom-host:org/repo.git",
        "Should return raw URL when insteadOf also resolves to unknown forge"
    );
}

/// Test effective_remote_url returns None for nonexistent remote.
#[rstest]
fn test_effective_remote_url_nonexistent_remote(repo: TestRepo) {
    let git_repo = Repository::at(repo.root_path()).unwrap();
    assert!(
        git_repo.effective_remote_url("nonexistent").is_none(),
        "Should return None for nonexistent remote"
    );
}

/// Test forge_remote_url returns None for nonexistent remote.
#[rstest]
fn test_forge_remote_url_nonexistent_remote(repo: TestRepo) {
    let git_repo = Repository::at(repo.root_path()).unwrap();
    assert!(
        git_repo.forge_remote_url("nonexistent").is_none(),
        "Should return None for nonexistent remote"
    );
}
