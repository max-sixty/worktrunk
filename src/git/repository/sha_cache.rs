//! Persistent cache for SHA-keyed git command results.
//!
//! Caches the results of expensive git operations keyed on pairs of
//! content-addressed SHAs — diffing commit A against commit B is the same
//! today as last week. One variant (working-tree conflict checks) uses a
//! composite key that includes a tree SHA; see
//! [`Repository::has_merge_conflicts_by_tree_with_base_sha`]. No TTL, no invalidation
//! logic, only a per-kind LRU size bound to prevent unbounded growth.
//!
//! Layout: `.git/wt/cache/{kind}/{key}.json` where `kind` is one of
//! `merge-tree-conflicts`, `merge-add-probe`, `is-ancestor`,
//! `has-added-changes`, `diff-stats`, `ahead-behind`, or `merge-base`.
//! Symmetric kinds sort the SHA pair so `(A, B)` and `(B, A)` hit the same
//! entry; asymmetric kinds preserve ordering. See [`crate::cache`] for
//! read/write/clear mechanics, torn-write semantics, and the
//! user-initiated clear error policy.

use super::Repository;
use super::integration::MergeProbeResult;
use crate::cache;
use crate::git::LineDiff;

/// Maximum cached entries per task kind before the LRU sweep removes the
/// oldest entries. 5000 entries × ~80 bytes ≈ 400 KB per kind — small
/// enough to ignore, large enough to cover years of typical use.
const MAX_ENTRIES_PER_KIND: usize = 5000;

const KIND_MERGE_TREE_CONFLICTS: &str = "merge-tree-conflicts";
const KIND_MERGE_ADD_PROBE: &str = "merge-add-probe";
const KIND_IS_ANCESTOR: &str = "is-ancestor";
const KIND_HAS_ADDED_CHANGES: &str = "has-added-changes";
const KIND_DIFF_STATS: &str = "diff-stats";
const KIND_AHEAD_BEHIND: &str = "ahead-behind";
const KIND_MERGE_BASE: &str = "merge-base";

/// All cache kind identifiers, used by [`clear_all`].
const ALL_KINDS: &[&str] = &[
    KIND_MERGE_TREE_CONFLICTS,
    KIND_MERGE_ADD_PROBE,
    KIND_IS_ANCESTOR,
    KIND_HAS_ADDED_CHANGES,
    KIND_DIFF_STATS,
    KIND_AHEAD_BEHIND,
    KIND_MERGE_BASE,
];

/// Build a symmetric filename from a SHA pair (order-independent).
fn symmetric_key(sha1: &str, sha2: &str) -> String {
    if sha1 <= sha2 {
        format!("{sha1}-{sha2}.json")
    } else {
        format!("{sha2}-{sha1}.json")
    }
}

/// Build an asymmetric filename from a SHA pair (order preserved).
fn asymmetric_key(first: &str, second: &str) -> String {
    format!("{first}-{second}.json")
}

// merge-tree conflicts (symmetric)

/// Look up a cached `has_merge_conflicts_by_sha(sha1, sha2)` result.
///
/// The key is order-independent: `(A, B)` and `(B, A)` hit the same entry.
pub(super) fn merge_conflicts(repo: &Repository, sha1: &str, sha2: &str) -> Option<bool> {
    cache::read(repo, KIND_MERGE_TREE_CONFLICTS, &symmetric_key(sha1, sha2))
}

/// Store a `has_merge_conflicts_by_sha(sha1, sha2)` result, triggering an LRU
/// sweep if the per-kind entry bound is exceeded.
pub(super) fn put_merge_conflicts(repo: &Repository, sha1: &str, sha2: &str, value: bool) {
    cache::write_with_lru(
        repo,
        KIND_MERGE_TREE_CONFLICTS,
        &symmetric_key(sha1, sha2),
        &value,
        MAX_ENTRIES_PER_KIND,
    );
}

// merge-add probe (asymmetric)

/// Look up a cached `merge_integration_probe_by_sha(branch, target)` result.
///
/// The key is order-dependent: the merge result is compared against
/// `target`'s tree, so swapping arguments changes the semantics.
pub(super) fn merge_add_probe(
    repo: &Repository,
    branch_sha: &str,
    target_sha: &str,
) -> Option<MergeProbeResult> {
    cache::read(
        repo,
        KIND_MERGE_ADD_PROBE,
        &asymmetric_key(branch_sha, target_sha),
    )
}

/// Store a `merge_integration_probe_by_sha(branch, target)` result, triggering
/// an LRU sweep if the per-kind entry bound is exceeded.
pub(super) fn put_merge_add_probe(
    repo: &Repository,
    branch_sha: &str,
    target_sha: &str,
    value: MergeProbeResult,
) {
    cache::write_with_lru(
        repo,
        KIND_MERGE_ADD_PROBE,
        &asymmetric_key(branch_sha, target_sha),
        &value,
        MAX_ENTRIES_PER_KIND,
    );
}

// is-ancestor (asymmetric)

/// Look up a cached `is_ancestor_by_sha(base_sha, head_sha)` result.
///
/// Asymmetric: "is base ancestor of head?" differs from "is head ancestor of base?".
pub(super) fn is_ancestor(repo: &Repository, base_sha: &str, head_sha: &str) -> Option<bool> {
    cache::read(repo, KIND_IS_ANCESTOR, &asymmetric_key(base_sha, head_sha))
}

/// Store an `is_ancestor_by_sha(base_sha, head_sha)` result.
pub(super) fn put_is_ancestor(repo: &Repository, base_sha: &str, head_sha: &str, value: bool) {
    cache::write_with_lru(
        repo,
        KIND_IS_ANCESTOR,
        &asymmetric_key(base_sha, head_sha),
        &value,
        MAX_ENTRIES_PER_KIND,
    );
}

// has-added-changes (asymmetric)

/// Look up a cached `has_added_changes_by_sha(branch_sha, target_sha)` result.
///
/// Asymmetric: diff from merge-base to branch is directional.
pub(super) fn has_added_changes(
    repo: &Repository,
    branch_sha: &str,
    target_sha: &str,
) -> Option<bool> {
    cache::read(
        repo,
        KIND_HAS_ADDED_CHANGES,
        &asymmetric_key(branch_sha, target_sha),
    )
}

/// Store a `has_added_changes_by_sha(branch_sha, target_sha)` result.
pub(super) fn put_has_added_changes(
    repo: &Repository,
    branch_sha: &str,
    target_sha: &str,
    value: bool,
) {
    cache::write_with_lru(
        repo,
        KIND_HAS_ADDED_CHANGES,
        &asymmetric_key(branch_sha, target_sha),
        &value,
        MAX_ENTRIES_PER_KIND,
    );
}

// diff-stats (asymmetric)

/// Look up cached `branch_diff_stats(base_sha, head_sha)` result.
///
/// Asymmetric: diff from merge-base(base,head)..head is directional.
pub(super) fn diff_stats(repo: &Repository, base_sha: &str, head_sha: &str) -> Option<LineDiff> {
    cache::read(repo, KIND_DIFF_STATS, &asymmetric_key(base_sha, head_sha))
}

/// Store a `branch_diff_stats(base_sha, head_sha)` result.
pub(super) fn put_diff_stats(repo: &Repository, base_sha: &str, head_sha: &str, value: LineDiff) {
    cache::write_with_lru(
        repo,
        KIND_DIFF_STATS,
        &asymmetric_key(base_sha, head_sha),
        &value,
        MAX_ENTRIES_PER_KIND,
    );
}

// ahead-behind (asymmetric)

/// Look up a cached `ahead_behind(base_sha, head_sha)` result `(ahead, behind)`.
///
/// Asymmetric: the counts are relative to `base_sha`, so swapping the
/// arguments swaps `ahead` and `behind` — a different question, a
/// different entry. Content-addressed and never stale: the two commit
/// SHAs fix the merge-base and both rev-list counts.
pub(super) fn ahead_behind(
    repo: &Repository,
    base_sha: &str,
    head_sha: &str,
) -> Option<(usize, usize)> {
    cache::read(repo, KIND_AHEAD_BEHIND, &asymmetric_key(base_sha, head_sha))
}

/// Store an `ahead_behind(base_sha, head_sha)` result.
pub(super) fn put_ahead_behind(
    repo: &Repository,
    base_sha: &str,
    head_sha: &str,
    value: (usize, usize),
) {
    cache::write_with_lru(
        repo,
        KIND_AHEAD_BEHIND,
        &asymmetric_key(base_sha, head_sha),
        &value,
        MAX_ENTRIES_PER_KIND,
    );
}

/// Bulk-write a sequence of `ahead_behind` entries, deferring the LRU
/// sweep to a single pass at the end. Use this for batch seeding (one
/// cache kind, many entries) — `put_ahead_behind` goes through
/// `write_with_lru`, which scans the cache directory on *every* call, so
/// N successive calls cost O(N · dir_size) wall (and on the serial setup
/// path that's O(N²) before the worker pool even opens). The bulk form
/// pays one final `sweep_lru` instead.
pub(super) fn put_ahead_behind_bulk<'a, I>(repo: &Repository, entries: I)
where
    I: IntoIterator<Item = (&'a str, &'a str, (usize, usize))>,
{
    let dir = cache::cache_dir(repo, KIND_AHEAD_BEHIND);
    for (base_sha, head_sha, value) in entries {
        cache::write_json(&dir.join(asymmetric_key(base_sha, head_sha)), &value);
    }
    cache::sweep_lru(&dir, MAX_ENTRIES_PER_KIND);
}

// merge-base (symmetric)

/// Look up a cached `merge_base_by_sha(sha1, sha2)` result.
///
/// The value is `Option<String>`: `Some(sha)` is the common ancestor,
/// `None` is an orphan pair (no common ancestor). So the return type is
/// `Option<Option<String>>`: the outer `None` is a cache miss, the inner
/// `None` is a cached orphan. Symmetric — `(A, B)` and `(B, A)` hit the
/// same entry, matching `merge-base(A, B) == merge-base(B, A)`.
pub(super) fn merge_base(repo: &Repository, sha1: &str, sha2: &str) -> Option<Option<String>> {
    cache::read(repo, KIND_MERGE_BASE, &symmetric_key(sha1, sha2))
}

/// Store a `merge_base_by_sha(sha1, sha2)` result.
pub(super) fn put_merge_base(repo: &Repository, sha1: &str, sha2: &str, value: &Option<String>) {
    cache::write_with_lru(
        repo,
        KIND_MERGE_BASE,
        &symmetric_key(sha1, sha2),
        value,
        MAX_ENTRIES_PER_KIND,
    );
}

// Maintenance

/// Clear all cached SHA-keyed entries, returning the count removed. Called
/// by `wt config state clear`; see [`cache::clear_json_files`] for the
/// missing-dir / concurrent-removal / error-propagation semantics.
pub fn clear_all(repo: &Repository) -> anyhow::Result<usize> {
    let mut cleared = 0;
    for kind in ALL_KINDS {
        cleared += cache::clear_json_files(&cache::cache_dir(repo, kind))?;
    }
    Ok(cleared)
}

/// Count all cached SHA-keyed entries across every kind.
///
/// Called by `wt config state get` to surface the same state that
/// `clear_all` would sweep, preserving get ↔ clear parity.
pub fn count_all(repo: &Repository) -> usize {
    ALL_KINDS
        .iter()
        .map(|kind| cache::count_json_files(&cache::cache_dir(repo, kind)))
        .sum()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::testing::TestRepo;

    // Key formatting

    #[test]
    fn test_symmetric_key_sorts_pair() {
        assert_eq!(symmetric_key("aaaa", "bbbb"), "aaaa-bbbb.json");
        assert_eq!(symmetric_key("bbbb", "aaaa"), "aaaa-bbbb.json");
        assert_eq!(
            symmetric_key("deadbeef", "deadbeef"),
            "deadbeef-deadbeef.json"
        );
    }

    #[test]
    fn test_asymmetric_key_preserves_order() {
        assert_eq!(asymmetric_key("aaaa", "bbbb"), "aaaa-bbbb.json");
        assert_eq!(asymmetric_key("bbbb", "aaaa"), "bbbb-aaaa.json");
    }

    // Round-trip file I/O

    #[test]
    fn test_merge_conflicts_roundtrip() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        assert_eq!(merge_conflicts(&repo, "aaaa", "bbbb"), None);

        put_merge_conflicts(&repo, "aaaa", "bbbb", true);
        assert_eq!(merge_conflicts(&repo, "aaaa", "bbbb"), Some(true));

        // Symmetric: swapped args hit the same entry
        assert_eq!(merge_conflicts(&repo, "bbbb", "aaaa"), Some(true));

        // Overwrite with a new value
        put_merge_conflicts(&repo, "aaaa", "bbbb", false);
        assert_eq!(merge_conflicts(&repo, "aaaa", "bbbb"), Some(false));
    }

    #[test]
    fn test_merge_add_probe_roundtrip() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        assert_eq!(merge_add_probe(&repo, "aaaa", "bbbb"), None);

        let value = MergeProbeResult {
            would_merge_add: true,
            is_patch_id_match: false,
        };
        put_merge_add_probe(&repo, "aaaa", "bbbb", value);
        assert_eq!(merge_add_probe(&repo, "aaaa", "bbbb"), Some(value));

        // Asymmetric: swapped args miss
        assert_eq!(merge_add_probe(&repo, "bbbb", "aaaa"), None);
    }

    #[test]
    fn test_kinds_are_isolated() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        put_merge_conflicts(&repo, "aaaa", "bbbb", true);
        // Same SHA pair in a different kind is a separate entry
        assert_eq!(merge_add_probe(&repo, "aaaa", "bbbb"), None);
    }

    #[test]
    fn test_corrupt_entry_returns_none() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        put_merge_conflicts(&repo, "aaaa", "bbbb", true);

        let path =
            cache::cache_dir(&repo, KIND_MERGE_TREE_CONFLICTS).join(symmetric_key("aaaa", "bbbb"));
        fs::write(&path, "not valid json {{{").unwrap();

        assert_eq!(merge_conflicts(&repo, "aaaa", "bbbb"), None);
    }

    // Cache consultation by has_merge_conflicts

    #[test]
    fn test_has_merge_conflicts_reads_cache() {
        let test = TestRepo::with_initial_commit();

        // Create a feature branch with a clean merge (no conflicts)
        test.run_git(&["checkout", "-b", "feature"]);
        fs::write(test.root_path().join("new.txt"), "content\n").unwrap();
        test.run_git(&["add", "new.txt"]);
        test.run_git(&["commit", "-m", "Add file"]);
        test.run_git(&["checkout", "main"]);

        let main_sha = test.git_output(&["rev-parse", "main"]);
        let feature_sha = test.git_output(&["rev-parse", "feature"]);

        let repo = Repository::at(test.root_path()).unwrap();

        // First call: real computation — clean merge → false
        assert!(
            !repo
                .has_merge_conflicts_by_sha(&main_sha, &feature_sha)
                .unwrap()
        );

        // Tamper with the cache file to return the wrong answer
        let dir = cache::cache_dir(&repo, KIND_MERGE_TREE_CONFLICTS);
        let entries: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_str().is_some_and(|s| s.ends_with(".json")))
            .collect();
        assert_eq!(entries.len(), 1, "exactly one cache entry expected");
        fs::write(entries[0].path(), "true").unwrap();

        // Second call on a fresh Repository reads the tampered value from the
        // persistent cache.
        let repo2 = Repository::at(test.root_path()).unwrap();
        assert!(
            repo2
                .has_merge_conflicts_by_sha(&main_sha, &feature_sha)
                .unwrap()
        );
    }

    #[test]
    fn test_has_merge_conflicts_by_tree_uses_composite_cache_key() {
        let test = TestRepo::with_initial_commit();

        // Create a feature branch with a staged change
        test.run_git(&["checkout", "-b", "feature"]);
        fs::write(test.root_path().join("dirty.txt"), "uncommitted\n").unwrap();
        test.run_git(&["add", "dirty.txt"]);

        let branch_head = test.git_output(&["rev-parse", "HEAD"]);
        let tree_sha = test.git_output(&["write-tree"]);
        let main_sha = test.git_output(&["rev-parse", "main"]);

        let repo = Repository::at(test.root_path()).unwrap();

        // Call with composite keying — computes and caches
        let result = repo
            .has_merge_conflicts_by_tree_with_base_sha(&main_sha, &branch_head, &tree_sha)
            .unwrap();

        // Verify the cache entry uses the composite key
        let dir = cache::cache_dir(&repo, KIND_MERGE_TREE_CONFLICTS);
        let entries: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_str().is_some_and(|s| s.ends_with(".json")))
            .collect();
        assert_eq!(entries.len(), 1, "exactly one cache entry expected");

        let filename = entries[0].file_name().to_string_lossy().into_owned();
        assert!(
            filename.contains(&tree_sha),
            "cache filename should contain tree SHA ({tree_sha}), got: {filename}"
        );

        // Tamper with the cache and verify a fresh repo reads the tampered value
        let tampered = !result;
        fs::write(entries[0].path(), serde_json::to_string(&tampered).unwrap()).unwrap();

        let repo2 = Repository::at(test.root_path()).unwrap();
        let cached = repo2
            .has_merge_conflicts_by_tree_with_base_sha(&main_sha, &branch_head, &tree_sha)
            .unwrap();
        assert_eq!(cached, tampered, "should read tampered value from cache");
    }

    #[test]
    fn test_has_merge_conflicts_by_tree_invalidates_on_branch_head_change() {
        let test = TestRepo::with_initial_commit();

        // Set up a common ancestor with shared.txt
        fs::write(test.root_path().join("shared.txt"), "initial\n").unwrap();
        test.run_git(&["add", "shared.txt"]);
        test.run_git(&["commit", "-m", "base: add shared.txt"]);

        // Create feature branch from this point, then diverge both branches
        test.run_git(&["checkout", "-b", "feature"]);
        fs::write(test.root_path().join("shared.txt"), "feature content\n").unwrap();
        test.run_git(&["add", "shared.txt"]);
        test.run_git(&["commit", "-m", "feature: modify shared.txt"]);

        test.run_git(&["checkout", "main"]);
        fs::write(test.root_path().join("shared.txt"), "main content\n").unwrap();
        test.run_git(&["add", "shared.txt"]);
        test.run_git(&["commit", "-m", "main: modify shared.txt"]);

        // Back to feature, stage an extra file
        test.run_git(&["checkout", "feature"]);
        fs::write(test.root_path().join("extra.txt"), "extra\n").unwrap();
        test.run_git(&["add", "extra.txt"]);

        let head_before = test.git_output(&["rev-parse", "HEAD"]);
        let tree1 = test.git_output(&["write-tree"]);
        let main_sha = test.git_output(&["rev-parse", "main"]);

        let repo = Repository::at(test.root_path()).unwrap();

        // Before rebase: feature conflicts with main (both modified shared.txt)
        let result_before = repo
            .has_merge_conflicts_by_tree_with_base_sha(&main_sha, &head_before, &tree1)
            .unwrap();
        assert!(result_before, "should conflict before rebase");

        // Unstage, rebase, then re-stage (rebase requires a clean index)
        test.run_git(&["reset", "HEAD", "extra.txt"]);
        fs::remove_file(test.root_path().join("extra.txt")).unwrap();
        test.run_git(&["rebase", "main", "--strategy-option=theirs"]);

        // Re-stage the same extra file
        fs::write(test.root_path().join("extra.txt"), "extra\n").unwrap();
        test.run_git(&["add", "extra.txt"]);

        let head_after = test.git_output(&["rev-parse", "HEAD"]);
        let tree2 = test.git_output(&["write-tree"]);

        assert_ne!(
            head_before, head_after,
            "branch HEAD should change after rebase"
        );

        let repo2 = Repository::at(test.root_path()).unwrap();
        let result_after = repo2
            .has_merge_conflicts_by_tree_with_base_sha(&main_sha, &head_after, &tree2)
            .unwrap();

        // After rebase onto main, feature is based on main — no conflicts
        assert!(
            !result_after,
            "should not conflict after rebase (different branch HEAD = different cache key)"
        );
    }

    #[test]
    fn test_merge_integration_probe_reads_cache() {
        let test = TestRepo::with_initial_commit();

        // Create a feature branch that's already merged (fast-forward)
        test.run_git(&["checkout", "-b", "feature"]);
        fs::write(test.root_path().join("new.txt"), "content\n").unwrap();
        test.run_git(&["add", "new.txt"]);
        test.run_git(&["commit", "-m", "Feature"]);
        test.run_git(&["checkout", "main"]);
        test.run_git(&["merge", "feature"]);

        let feature_sha = test.git_output(&["rev-parse", "feature"]);
        let main_sha = test.git_output(&["rev-parse", "main"]);

        let repo = Repository::at(test.root_path()).unwrap();

        // First call: feature is fully integrated → would_merge_add=false
        let real = repo
            .merge_integration_probe_by_sha(&feature_sha, &main_sha)
            .unwrap();
        assert!(!real.would_merge_add);

        // Tamper with the cache to flip the answer
        let dir = cache::cache_dir(&repo, KIND_MERGE_ADD_PROBE);
        let entries: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_str().is_some_and(|s| s.ends_with(".json")))
            .collect();
        assert_eq!(entries.len(), 1, "exactly one cache entry expected");
        let tampered = MergeProbeResult {
            would_merge_add: true,
            is_patch_id_match: true,
        };
        fs::write(entries[0].path(), serde_json::to_string(&tampered).unwrap()).unwrap();

        // Fresh repo reads the tampered cache
        let repo2 = Repository::at(test.root_path()).unwrap();
        let cached = repo2
            .merge_integration_probe_by_sha(&feature_sha, &main_sha)
            .unwrap();
        assert!(cached.would_merge_add);
        assert!(cached.is_patch_id_match);
    }

    // Round-trip: is-ancestor, has-added-changes, diff-stats

    #[test]
    fn test_is_ancestor_roundtrip() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        assert_eq!(is_ancestor(&repo, "aaaa", "bbbb"), None);

        put_is_ancestor(&repo, "aaaa", "bbbb", true);
        assert_eq!(is_ancestor(&repo, "aaaa", "bbbb"), Some(true));

        // Asymmetric: swapped args miss
        assert_eq!(is_ancestor(&repo, "bbbb", "aaaa"), None);
    }

    #[test]
    fn test_has_added_changes_roundtrip() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        assert_eq!(has_added_changes(&repo, "aaaa", "bbbb"), None);

        put_has_added_changes(&repo, "aaaa", "bbbb", false);
        assert_eq!(has_added_changes(&repo, "aaaa", "bbbb"), Some(false));

        // Asymmetric: swapped args miss
        assert_eq!(has_added_changes(&repo, "bbbb", "aaaa"), None);
    }

    #[test]
    fn test_diff_stats_roundtrip() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        assert_eq!(diff_stats(&repo, "aaaa", "bbbb"), None);

        let value = LineDiff {
            added: 42,
            deleted: 7,
        };
        put_diff_stats(&repo, "aaaa", "bbbb", value);
        assert_eq!(diff_stats(&repo, "aaaa", "bbbb"), Some(value));

        // Asymmetric: swapped args miss
        assert_eq!(diff_stats(&repo, "bbbb", "aaaa"), None);
    }

    #[test]
    fn test_ahead_behind_roundtrip() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        assert_eq!(ahead_behind(&repo, "aaaa", "bbbb"), None);

        put_ahead_behind(&repo, "aaaa", "bbbb", (3, 5));
        assert_eq!(ahead_behind(&repo, "aaaa", "bbbb"), Some((3, 5)));

        // Asymmetric: swapped args miss (the question "ahead/behind relative
        // to bbbb" is distinct from "relative to aaaa")
        assert_eq!(ahead_behind(&repo, "bbbb", "aaaa"), None);

        // Overwrite with a new value
        put_ahead_behind(&repo, "aaaa", "bbbb", (0, 0));
        assert_eq!(ahead_behind(&repo, "aaaa", "bbbb"), Some((0, 0)));
    }

    // Cache consultation: is_ancestor, has_added_changes, branch_diff_stats

    #[test]
    fn test_is_ancestor_reads_cache() {
        let test = TestRepo::with_initial_commit();

        test.run_git(&["checkout", "-b", "feature"]);
        fs::write(test.root_path().join("new.txt"), "content\n").unwrap();
        test.run_git(&["add", "new.txt"]);
        test.run_git(&["commit", "-m", "Feature"]);
        test.run_git(&["checkout", "main"]);

        let feature_sha = test.git_output(&["rev-parse", "feature"]);
        let main_sha = test.git_output(&["rev-parse", "main"]);

        let repo = Repository::at(test.root_path()).unwrap();

        // feature is NOT an ancestor of main (main didn't merge feature)
        assert!(!repo.is_ancestor_by_sha(&feature_sha, &main_sha).unwrap());

        // Tamper with cache to flip the answer
        let dir = cache::cache_dir(&repo, KIND_IS_ANCESTOR);
        let entries: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_str().is_some_and(|s| s.ends_with(".json")))
            .collect();
        assert_eq!(entries.len(), 1);
        fs::write(entries[0].path(), "true").unwrap();

        let repo2 = Repository::at(test.root_path()).unwrap();
        assert!(repo2.is_ancestor_by_sha(&feature_sha, &main_sha).unwrap());
    }

    #[test]
    fn test_has_added_changes_reads_cache() {
        let test = TestRepo::with_initial_commit();

        test.run_git(&["checkout", "-b", "feature"]);
        fs::write(test.root_path().join("new.txt"), "content\n").unwrap();
        test.run_git(&["add", "new.txt"]);
        test.run_git(&["commit", "-m", "Feature"]);
        test.run_git(&["checkout", "main"]);

        let feature_sha = test.git_output(&["rev-parse", "feature"]);
        let main_sha = test.git_output(&["rev-parse", "main"]);

        let repo = Repository::at(test.root_path()).unwrap();

        // feature has added changes compared to main
        assert!(
            repo.has_added_changes_by_sha(&feature_sha, &main_sha)
                .unwrap()
        );

        // Tamper with cache to flip the answer
        let dir = cache::cache_dir(&repo, KIND_HAS_ADDED_CHANGES);
        let entries: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_str().is_some_and(|s| s.ends_with(".json")))
            .collect();
        assert_eq!(entries.len(), 1);
        fs::write(entries[0].path(), "false").unwrap();

        let repo2 = Repository::at(test.root_path()).unwrap();
        assert!(
            !repo2
                .has_added_changes_by_sha(&feature_sha, &main_sha)
                .unwrap()
        );
    }

    #[test]
    fn test_ahead_behind_reads_cache() {
        let test = TestRepo::with_initial_commit();

        test.run_git(&["checkout", "-b", "feature"]);
        fs::write(test.root_path().join("new.txt"), "content\n").unwrap();
        test.run_git(&["add", "new.txt"]);
        test.run_git(&["commit", "-m", "Feature"]);

        let main_sha = test.git_output(&["rev-parse", "main"]);
        let feature_sha = test.git_output(&["rev-parse", "feature"]);

        let repo = Repository::at(test.root_path()).unwrap();

        // Real computation: feature is 1 ahead, 0 behind main
        assert_eq!(
            repo.ahead_behind_by_sha(&main_sha, &feature_sha).unwrap(),
            (1, 0)
        );

        // Tamper with cache to return different counts
        let dir = cache::cache_dir(&repo, KIND_AHEAD_BEHIND);
        let entries: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_str().is_some_and(|s| s.ends_with(".json")))
            .collect();
        assert_eq!(entries.len(), 1);
        fs::write(entries[0].path(), "[9,8]").unwrap();

        let repo2 = Repository::at(test.root_path()).unwrap();
        assert_eq!(
            repo2.ahead_behind_by_sha(&main_sha, &feature_sha).unwrap(),
            (9, 8)
        );
    }

    #[test]
    fn test_branch_diff_stats_reads_cache() {
        let test = TestRepo::with_initial_commit();

        test.run_git(&["checkout", "-b", "feature"]);
        fs::write(test.root_path().join("new.txt"), "content\n").unwrap();
        test.run_git(&["add", "new.txt"]);
        test.run_git(&["commit", "-m", "Feature"]);
        test.run_git(&["checkout", "main"]);

        let repo = Repository::at(test.root_path()).unwrap();

        // Real computation
        let real = repo.branch_diff_stats("main", "feature").unwrap();
        assert_eq!(real.added, 1);
        assert_eq!(real.deleted, 0);

        // Tamper with cache to return different stats
        let dir = cache::cache_dir(&repo, KIND_DIFF_STATS);
        let entries: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_str().is_some_and(|s| s.ends_with(".json")))
            .collect();
        assert_eq!(entries.len(), 1);
        let tampered = LineDiff {
            added: 999,
            deleted: 888,
        };
        fs::write(entries[0].path(), serde_json::to_string(&tampered).unwrap()).unwrap();

        let repo2 = Repository::at(test.root_path()).unwrap();
        let cached = repo2.branch_diff_stats("main", "feature").unwrap();
        assert_eq!(cached.added, 999);
        assert_eq!(cached.deleted, 888);
    }

    #[test]
    fn test_clear_all_covers_all_kinds() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        // Populate one entry in each kind
        put_merge_conflicts(&repo, "a", "b", true);
        put_merge_add_probe(
            &repo,
            "a",
            "b",
            MergeProbeResult {
                would_merge_add: true,
                is_patch_id_match: false,
            },
        );
        put_is_ancestor(&repo, "a", "b", true);
        put_has_added_changes(&repo, "a", "b", true);
        put_diff_stats(
            &repo,
            "a",
            "b",
            LineDiff {
                added: 1,
                deleted: 0,
            },
        );
        put_ahead_behind(&repo, "a", "b", (1, 0));
        put_merge_base(&repo, "a", "b", &Some("c".to_string()));

        let cleared = clear_all(&repo).unwrap();
        assert_eq!(cleared, 7, "should clear one entry per kind");

        // All kinds should be empty
        assert_eq!(merge_conflicts(&repo, "a", "b"), None);
        assert_eq!(merge_add_probe(&repo, "a", "b"), None);
        assert_eq!(is_ancestor(&repo, "a", "b"), None);
        assert_eq!(has_added_changes(&repo, "a", "b"), None);
        assert_eq!(diff_stats(&repo, "a", "b"), None);
        assert_eq!(ahead_behind(&repo, "a", "b"), None);
        assert_eq!(merge_base(&repo, "a", "b"), None);
    }

    #[test]
    fn test_merge_base_roundtrip() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        assert_eq!(merge_base(&repo, "aaaa", "bbbb"), None);

        // A common ancestor: outer Some, inner Some(sha).
        put_merge_base(&repo, "aaaa", "bbbb", &Some("cccc".to_string()));
        assert_eq!(
            merge_base(&repo, "aaaa", "bbbb"),
            Some(Some("cccc".to_string()))
        );
        // Symmetric: swapped args hit the same entry.
        assert_eq!(
            merge_base(&repo, "bbbb", "aaaa"),
            Some(Some("cccc".to_string()))
        );

        // An orphan pair: outer Some (cached), inner None (no ancestor).
        put_merge_base(&repo, "dddd", "eeee", &None);
        assert_eq!(merge_base(&repo, "dddd", "eeee"), Some(None));
    }

    #[test]
    fn test_merge_base_reads_cache() {
        let test = TestRepo::with_initial_commit();

        test.run_git(&["checkout", "-b", "feature"]);
        fs::write(test.root_path().join("new.txt"), "content\n").unwrap();
        test.run_git(&["add", "new.txt"]);
        test.run_git(&["commit", "-m", "Feature"]);

        let main_sha = test.git_output(&["rev-parse", "main"]);
        let feature_sha = test.git_output(&["rev-parse", "feature"]);

        let repo = Repository::at(test.root_path()).unwrap();

        // Real computation: feature descends from main, so the merge base is main.
        assert_eq!(
            repo.merge_base_by_sha(&main_sha, &feature_sha).unwrap(),
            Some(main_sha.clone())
        );

        // Tamper with the persisted entry; a fresh repo (cold in-memory front)
        // reads the disk back.
        let dir = cache::cache_dir(&repo, KIND_MERGE_BASE);
        let entries: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_str().is_some_and(|s| s.ends_with(".json")))
            .collect();
        assert_eq!(entries.len(), 1);
        fs::write(entries[0].path(), "\"deadbeef\"").unwrap();

        let repo2 = Repository::at(test.root_path()).unwrap();
        assert_eq!(
            repo2.merge_base_by_sha(&main_sha, &feature_sha).unwrap(),
            Some("deadbeef".to_string())
        );
    }
}
