//! Tests for Repository cache sharing.
//!
//! These tests verify that when Repository is cloned, the cache is shared
//! across clones via Arc. This is important for performance in `wt list`
//! where parallel tasks share cached git data.

use rstest::rstest;
use worktrunk::git::Repository;

mod common {
    pub use crate::common::*;
}
use common::{TestRepo, repo};

/// Test that cloning a Repository shares the same cache via Arc.
#[rstest]
fn test_repository_clone_shares_cache(repo: TestRepo) {
    let repo1 = Repository::at(repo.root_path()).unwrap();
    let repo2 = repo1.clone();

    // Both should point to the same cache (Arc pointer equality)
    assert!(
        repo1.shares_cache_with(&repo2),
        "Cloned repositories should share the same cache"
    );
}

/// Test that cached values computed by one clone are visible to another.
#[rstest]
fn test_cache_shared_between_clones(repo: TestRepo) {
    let repo1 = Repository::at(repo.root_path()).unwrap();
    let repo2 = repo1.clone();

    // Call default_branch() on repo1 - this caches the result
    let default1 = repo1.default_branch().unwrap();

    // Call default_branch() on repo2 - should return cached value
    let default2 = repo2.default_branch().unwrap();

    assert_eq!(default1, default2);
    assert_eq!(default1, "main"); // TestRepo uses main as default branch
}

/// Test that merge_base cache is shared across clones.
#[rstest]
fn test_merge_base_cache_shared(mut repo: TestRepo) {
    // Create a branch with a commit
    repo.add_worktree("feature");
    let feature_path = repo.worktree_path("feature");
    repo.commit_in_worktree(feature_path, "feature.txt", "content", "feature commit");

    let repo1 = Repository::at(repo.root_path()).unwrap();
    let repo2 = repo1.clone();

    // Get HEAD commits
    let main_head = repo.head_sha();
    let feature_head = repo.head_sha_in(feature_path);

    // Call merge_base on repo1 - caches the result
    let base1 = repo1.merge_base(&main_head, &feature_head).unwrap();

    // Call merge_base on repo2 - should use cached value
    let base2 = repo2.merge_base(&main_head, &feature_head).unwrap();

    assert_eq!(base1, base2);
    // The merge base should be main's HEAD since feature branched from there
    assert_eq!(base1, main_head);
}

/// Test that parallel tasks share the cache when cloning Repository.
#[rstest]
fn test_parallel_tasks_share_cache(mut repo: TestRepo) {
    use std::thread;

    // Create multiple worktrees
    repo.add_worktree("feature-a");
    repo.add_worktree("feature-b");

    let repo1 = Repository::at(repo.root_path()).unwrap();

    // Spawn threads that clone the repo and access cached values
    let handles: Vec<_> = (0..4)
        .map(|i| {
            let repo_clone = repo1.clone();
            thread::spawn(move || {
                // Each thread accesses the same cached default_branch
                let default = repo_clone.default_branch().unwrap();
                (i, default)
            })
        })
        .collect();

    // All threads should get the same cached value
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    for (_, branch) in &results {
        assert_eq!(branch, "main");
    }
}

/// Test that separate Repository instances (not clones) have separate caches.
#[rstest]
fn test_separate_repositories_have_separate_caches(repo: TestRepo) {
    // Create two separate Repository instances (not clones)
    let repo1 = Repository::at(repo.root_path()).unwrap();
    let repo2 = Repository::at(repo.root_path()).unwrap();

    // They should NOT share the same cache
    assert!(
        !repo1.shares_cache_with(&repo2),
        "Separately created repositories should have independent caches"
    );

    // But they should compute the same values
    let default1 = repo1.default_branch().unwrap();
    let default2 = repo2.default_branch().unwrap();
    assert_eq!(default1, default2);
}
