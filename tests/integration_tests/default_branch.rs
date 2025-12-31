use crate::common::{TestRepo, repo, repo_with_remote};
use rstest::rstest;
use worktrunk::git::Repository;

#[rstest]
fn test_get_default_branch_with_origin_head(#[from(repo_with_remote)] repo: TestRepo) {
    // origin/HEAD should be set automatically by setup_remote
    assert!(repo.has_origin_head());

    // Test that we can get the default branch
    let branch = Repository::at(repo.root_path()).default_branch().unwrap();
    assert_eq!(branch, "main");
}

#[rstest]
fn test_get_default_branch_without_origin_head(#[from(repo_with_remote)] repo: TestRepo) {
    // Clear origin/HEAD to force remote query
    repo.clear_origin_head();
    assert!(!repo.has_origin_head());

    // Should still work by querying remote
    let branch = Repository::at(repo.root_path()).default_branch().unwrap();
    assert_eq!(branch, "main");

    // Verify that worktrunk's cache is now set
    let cached = repo
        .git_command()
        .args(["config", "--get", "worktrunk.default-branch"])
        .output()
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
        .output();

    // First call queries remote and caches to worktrunk config
    Repository::at(repo.root_path()).default_branch().unwrap();
    let cached = repo
        .git_command()
        .args(["config", "--get", "worktrunk.default-branch"])
        .output()
        .unwrap();
    assert!(cached.status.success());

    // Second call uses cache (fast path)
    let branch = Repository::at(repo.root_path()).default_branch().unwrap();
    assert_eq!(branch, "main");
}

#[rstest]
fn test_get_default_branch_no_remote(repo: TestRepo) {
    // No remote configured, should infer from local branches
    // Since there's only one local branch, it should return that
    let result = Repository::at(repo.root_path()).default_branch();
    assert!(result.is_ok());

    // The inferred branch should match the current branch
    let inferred_branch = result.unwrap();
    let repo_instance = Repository::at(repo.root_path());
    let current_branch = repo_instance.current_branch().unwrap().unwrap();
    assert_eq!(inferred_branch, current_branch);
}

#[rstest]
fn test_get_default_branch_with_custom_remote(mut repo: TestRepo) {
    repo.setup_custom_remote("upstream", "main");

    // Test that we can get the default branch from a custom remote
    let branch = Repository::at(repo.root_path()).default_branch().unwrap();
    assert_eq!(branch, "main");
}

#[rstest]
fn test_primary_remote_detects_custom_remote(mut repo: TestRepo) {
    repo.setup_custom_remote("upstream", "develop");

    // Test that primary_remote detects the custom remote name
    let git_repo = Repository::at(repo.root_path());
    let remote = git_repo.primary_remote().unwrap();
    assert_eq!(remote, "upstream");
}

#[rstest]
fn test_branch_exists_with_custom_remote(mut repo: TestRepo) {
    repo.setup_custom_remote("upstream", "main");

    let git_repo = Repository::at(repo.root_path());

    // Should find the branch on the custom remote
    assert!(git_repo.branch_exists("main").unwrap());

    // Should not find non-existent branch
    assert!(!git_repo.branch_exists("nonexistent").unwrap());
}

#[rstest]
fn test_get_default_branch_no_remote_common_names_fallback(repo: TestRepo) {
    // Create additional branches (no remote configured)
    repo.git_command()
        .args(["branch", "feature"])
        .status()
        .unwrap();
    repo.git_command()
        .args(["branch", "bugfix"])
        .status()
        .unwrap();

    // Now we have multiple branches: main, feature, bugfix
    // Should detect "main" from the common names list
    let branch = Repository::at(repo.root_path()).default_branch().unwrap();
    assert_eq!(branch, "main");
}

#[rstest]
fn test_get_default_branch_no_remote_master_fallback(repo: TestRepo) {
    // Rename main to master, then create other branches
    repo.git_command()
        .args(["branch", "-m", "main", "master"])
        .status()
        .unwrap();
    repo.git_command()
        .args(["branch", "feature"])
        .status()
        .unwrap();
    repo.git_command()
        .args(["branch", "bugfix"])
        .status()
        .unwrap();

    // Now we have: master, feature, bugfix (no "main")
    // Should detect "master" from the common names list
    let branch = Repository::at(repo.root_path()).default_branch().unwrap();
    assert_eq!(branch, "master");
}

#[rstest]
fn test_get_default_branch_no_remote_init_default_branch_config(repo: TestRepo) {
    // Rename main to something non-standard, create the configured default
    repo.git_command()
        .args(["branch", "-m", "main", "primary"])
        .status()
        .unwrap();
    repo.git_command()
        .args(["branch", "feature"])
        .status()
        .unwrap();

    // Set init.defaultBranch - this should be checked before common names
    repo.git_command()
        .args(["config", "init.defaultBranch", "primary"])
        .status()
        .unwrap();

    // Now we have: primary, feature (no common names like main/master)
    // Should detect "primary" via init.defaultBranch config
    let branch = Repository::at(repo.root_path()).default_branch().unwrap();
    assert_eq!(branch, "primary");
}

#[rstest]
fn test_get_default_branch_no_remote_fails_when_no_match(repo: TestRepo) {
    // Rename main to something non-standard
    repo.git_command()
        .args(["branch", "-m", "main", "xyz"])
        .status()
        .unwrap();
    repo.git_command().args(["branch", "abc"]).status().unwrap();
    repo.git_command().args(["branch", "def"]).status().unwrap();

    // Now we have: xyz, abc, def - no common names, no init.defaultBranch
    // Should fail with an error
    let result = Repository::at(repo.root_path()).default_branch();
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("Could not infer default branch"),
        "Expected error about inferring default branch, got: {}",
        err
    );
}
