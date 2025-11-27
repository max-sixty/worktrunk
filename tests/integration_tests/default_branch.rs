use crate::common::TestRepo;
use worktrunk::git::Repository;

#[test]
fn test_get_default_branch_with_origin_head() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // origin/HEAD should be set automatically by setup_remote
    assert!(repo.has_origin_head());

    // Test that we can get the default branch
    let branch = Repository::at(repo.root_path()).default_branch().unwrap();
    assert_eq!(branch, "main");
}

#[test]
fn test_get_default_branch_without_origin_head() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Clear origin/HEAD to force remote query
    repo.clear_origin_head();
    assert!(!repo.has_origin_head());

    // Should still work by querying remote
    let branch = Repository::at(repo.root_path()).default_branch().unwrap();
    assert_eq!(branch, "main");

    // Verify that origin/HEAD is now cached
    assert!(repo.has_origin_head());
}

#[test]
fn test_get_default_branch_caches_result() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Clear origin/HEAD
    repo.clear_origin_head();
    assert!(!repo.has_origin_head());

    // First call queries remote and caches
    Repository::at(repo.root_path()).default_branch().unwrap();
    assert!(repo.has_origin_head());

    // Second call uses cache (fast path)
    let branch = Repository::at(repo.root_path()).default_branch().unwrap();
    assert_eq!(branch, "main");
}

#[test]
fn test_get_default_branch_no_remote() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // No remote configured, should infer from local branches
    // Since there's only one local branch, it should return that
    let result = Repository::at(repo.root_path()).default_branch();
    assert!(result.is_ok());

    // The inferred branch should match the current branch
    let inferred_branch = result.unwrap();
    let current_branch = Repository::at(repo.root_path())
        .current_branch()
        .unwrap()
        .unwrap();
    assert_eq!(inferred_branch, current_branch);
}

#[test]
fn test_get_default_branch_with_custom_remote() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_custom_remote("upstream", "main");

    // Test that we can get the default branch from a custom remote
    let branch = Repository::at(repo.root_path()).default_branch().unwrap();
    assert_eq!(branch, "main");
}

#[test]
fn test_primary_remote_detects_custom_remote() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_custom_remote("upstream", "develop");

    // Test that primary_remote detects the custom remote name
    let remote = Repository::at(repo.root_path()).primary_remote().unwrap();
    assert_eq!(remote, "upstream");
}

#[test]
fn test_branch_exists_with_custom_remote() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_custom_remote("upstream", "main");

    let git_repo = Repository::at(repo.root_path());

    // Should find the branch on the custom remote
    assert!(git_repo.branch_exists("main").unwrap());

    // Should not find non-existent branch
    assert!(!git_repo.branch_exists("nonexistent").unwrap());
}
