//! Persistent cache for SHA-keyed git command results.
//!
//! Caches the results of expensive git operations keyed on pairs of
//! content-addressed SHAs. Most entries use commit SHA pairs — the result
//! of diffing commit A against commit B is the same today as last week.
//! One variant (working-tree conflict checks) uses a composite key that
//! includes a tree SHA; see [`Repository::has_merge_conflicts_by_tree`].
//! No TTL, no invalidation logic, only a size bound to prevent unbounded
//! growth.
//!
//! # Layout
//!
//! One file per cached entry under `.git/wt/cache/{kind}/{key}.json`,
//! where `kind` identifies the operation (`merge-tree-conflicts`,
//! `merge-add-probe`, `is-ancestor`, `has-added-changes`, `diff-stats`)
//! and `key` is the SHA-pair filename. The sibling `ci-status/` and
//! `summaries/` caches use different key schemes; see
//! `commands::list::collect` for the cross-cutting design across all
//! `.git/wt/cache/` kinds.
//!
//! Symmetric kinds (merge-tree-conflicts) sort the SHA pair so
//! `merge-tree(A, B)` and `merge-tree(B, A)` hit the same entry.
//! Asymmetric kinds (merge-add-probe) preserve ordering because the
//! merge-tree result is compared against `target`'s tree, so swapping
//! arguments changes the answer.
//!
//! # Concurrency
//!
//! Two concurrent writers racing on the same key produce the same value
//! (same SHA inputs), so a torn write is benign — `read()` treats corrupt
//! JSON as a miss. The LRU sweep may also race — `fs::remove_file`
//! failures are ignored because the goal is best-effort size bounding.
//!
//! # Failure handling
//!
//! Read and write paths degrade silently — corrupt JSON, I/O errors, and
//! permission denied are logged at `debug` level; reads return `None`,
//! writes are no-ops. Callers must never observe cache failures there
//! because the cache is always an optimization over re-running git.
//!
//! The user-initiated clear path ([`clear_all`], reached via
//! `wt config state clear`) propagates I/O errors instead. A failed clear
//! that reports "cleared N entries" would be a lie to the user, mirroring
//! the same bug the ci-status file cache has already fixed.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Serialize, de::DeserializeOwned};

use super::Repository;
use super::integration::MergeProbeResult;
use crate::cache_dir::clear_json_files_in;
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

/// All cache kind identifiers, used by [`clear_all`].
const ALL_KINDS: &[&str] = &[
    KIND_MERGE_TREE_CONFLICTS,
    KIND_MERGE_ADD_PROBE,
    KIND_IS_ANCESTOR,
    KIND_HAS_ADDED_CHANGES,
    KIND_DIFF_STATS,
];

/// Get the cache directory for a given task kind.
///
/// Each kind is a top-level directory under `.git/wt/cache/`, consistent
/// with `ci-status/` and `summaries/`.
fn cache_dir(repo: &Repository, kind: &str) -> PathBuf {
    repo.wt_dir().join("cache").join(kind)
}

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

/// Read and deserialize a cache entry. Returns `None` on any failure.
fn read<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let json = fs::read_to_string(path).ok()?;
    match serde_json::from_str::<T>(&json) {
        Ok(value) => Some(value),
        Err(e) => {
            log::debug!("sha_cache: corrupt entry at {}: {}", path.display(), e);
            None
        }
    }
}

/// Serialize and write a cache entry. A half-written file is handled by
/// `read()` (corrupt JSON → `None`), so plain `fs::write` is sufficient.
fn write<T: Serialize>(path: &Path, value: &T) {
    if let Some(parent) = path.parent()
        && let Err(e) = fs::create_dir_all(parent)
    {
        log::debug!(
            "sha_cache: failed to create dir {}: {}",
            parent.display(),
            e
        );
        return;
    }

    let Ok(json) = serde_json::to_string(value) else {
        log::debug!(
            "sha_cache: failed to serialize entry for {}",
            path.display()
        );
        return;
    };

    if let Err(e) = fs::write(path, &json) {
        log::debug!("sha_cache: failed to write {}: {}", path.display(), e);
    }
}

/// Enforce a size bound on `dir`. If it holds more than `max` JSON
/// entries, delete the oldest-mtime files until the count is back at
/// `max`.
///
/// Runs after every `put`. The cheap path (`count <= max`) avoids any
/// per-file stat and is a single directory listing.
///
/// `max` is a parameter rather than hard-coded to `MAX_ENTRIES_PER_KIND`
/// so tests can exercise the trim logic without creating thousands of
/// files.
fn sweep_lru(dir: &Path, max: usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    // Fast path: collect DirEntry objects without statting. We only check
    // filename suffix to skip `.json.tmp` and other unrelated files.
    let json_entries: Vec<_> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_str().is_some_and(|s| s.ends_with(".json")))
        .collect();

    if json_entries.len() <= max {
        return;
    }

    // Over the bound — stat each entry to sort by mtime and trim the
    // oldest.
    let mut with_mtime: Vec<(PathBuf, SystemTime)> = json_entries
        .into_iter()
        .filter_map(|e| {
            let mtime = e.metadata().ok()?.modified().ok()?;
            Some((e.path(), mtime))
        })
        .collect();
    with_mtime.sort_by_key(|(_, mtime)| *mtime);

    let excess = with_mtime.len().saturating_sub(max);
    for (path, _) in with_mtime.iter().take(excess) {
        let _ = fs::remove_file(path);
    }
    log::debug!("sha_cache: swept {} entries from {}", excess, dir.display());
}

// ============================================================================
// Public API — merge-tree conflicts (symmetric)
// ============================================================================

/// Look up a cached `has_merge_conflicts(sha1, sha2)` result.
///
/// The key is order-independent: `(A, B)` and `(B, A)` hit the same entry.
pub(super) fn merge_conflicts(repo: &Repository, sha1: &str, sha2: &str) -> Option<bool> {
    let path = cache_dir(repo, KIND_MERGE_TREE_CONFLICTS).join(symmetric_key(sha1, sha2));
    read::<bool>(&path)
}

/// Store a `has_merge_conflicts(sha1, sha2)` result, triggering an LRU
/// sweep if the per-kind entry bound is exceeded.
pub(super) fn put_merge_conflicts(repo: &Repository, sha1: &str, sha2: &str, value: bool) {
    let dir = cache_dir(repo, KIND_MERGE_TREE_CONFLICTS);
    let path = dir.join(symmetric_key(sha1, sha2));
    write(&path, &value);
    sweep_lru(&dir, MAX_ENTRIES_PER_KIND);
}

// ============================================================================
// Public API — merge-add probe (asymmetric)
// ============================================================================

/// Look up a cached `merge_integration_probe(branch, target)` result.
///
/// The key is order-dependent: the merge result is compared against
/// `target`'s tree, so swapping arguments changes the semantics.
pub(super) fn merge_add_probe(
    repo: &Repository,
    branch_sha: &str,
    target_sha: &str,
) -> Option<MergeProbeResult> {
    let path = cache_dir(repo, KIND_MERGE_ADD_PROBE).join(asymmetric_key(branch_sha, target_sha));
    read::<MergeProbeResult>(&path)
}

/// Store a `merge_integration_probe(branch, target)` result, triggering
/// an LRU sweep if the per-kind entry bound is exceeded.
pub(super) fn put_merge_add_probe(
    repo: &Repository,
    branch_sha: &str,
    target_sha: &str,
    value: MergeProbeResult,
) {
    let dir = cache_dir(repo, KIND_MERGE_ADD_PROBE);
    let path = dir.join(asymmetric_key(branch_sha, target_sha));
    write(&path, &value);
    sweep_lru(&dir, MAX_ENTRIES_PER_KIND);
}

// ============================================================================
// Public API — is-ancestor (asymmetric)
// ============================================================================

/// Look up a cached `is_ancestor(base_sha, head_sha)` result.
///
/// Asymmetric: "is base ancestor of head?" differs from "is head ancestor of base?".
pub(super) fn is_ancestor(repo: &Repository, base_sha: &str, head_sha: &str) -> Option<bool> {
    let path = cache_dir(repo, KIND_IS_ANCESTOR).join(asymmetric_key(base_sha, head_sha));
    read::<bool>(&path)
}

/// Store an `is_ancestor(base_sha, head_sha)` result.
pub(super) fn put_is_ancestor(repo: &Repository, base_sha: &str, head_sha: &str, value: bool) {
    let dir = cache_dir(repo, KIND_IS_ANCESTOR);
    let path = dir.join(asymmetric_key(base_sha, head_sha));
    write(&path, &value);
    sweep_lru(&dir, MAX_ENTRIES_PER_KIND);
}

// ============================================================================
// Public API — has-added-changes (asymmetric)
// ============================================================================

/// Look up a cached `has_added_changes(branch_sha, target_sha)` result.
///
/// Asymmetric: diff from merge-base to branch is directional.
pub(super) fn has_added_changes(
    repo: &Repository,
    branch_sha: &str,
    target_sha: &str,
) -> Option<bool> {
    let path = cache_dir(repo, KIND_HAS_ADDED_CHANGES).join(asymmetric_key(branch_sha, target_sha));
    read::<bool>(&path)
}

/// Store a `has_added_changes(branch_sha, target_sha)` result.
pub(super) fn put_has_added_changes(
    repo: &Repository,
    branch_sha: &str,
    target_sha: &str,
    value: bool,
) {
    let dir = cache_dir(repo, KIND_HAS_ADDED_CHANGES);
    let path = dir.join(asymmetric_key(branch_sha, target_sha));
    write(&path, &value);
    sweep_lru(&dir, MAX_ENTRIES_PER_KIND);
}

// ============================================================================
// Public API — diff-stats (asymmetric)
// ============================================================================

/// Look up cached `branch_diff_stats(base_sha, head_sha)` result.
///
/// Asymmetric: diff from merge-base(base,head)..head is directional.
pub(super) fn diff_stats(repo: &Repository, base_sha: &str, head_sha: &str) -> Option<LineDiff> {
    let path = cache_dir(repo, KIND_DIFF_STATS).join(asymmetric_key(base_sha, head_sha));
    read::<LineDiff>(&path)
}

/// Store a `branch_diff_stats(base_sha, head_sha)` result.
pub(super) fn put_diff_stats(repo: &Repository, base_sha: &str, head_sha: &str, value: LineDiff) {
    let dir = cache_dir(repo, KIND_DIFF_STATS);
    let path = dir.join(asymmetric_key(base_sha, head_sha));
    write(&path, &value);
    sweep_lru(&dir, MAX_ENTRIES_PER_KIND);
}

// ============================================================================
// Maintenance
// ============================================================================

/// Clear all cached SHA-keyed entries, returning the count removed.
///
/// Called by `wt config state clear`. Delegates the per-kind-directory
/// work to [`clear_json_files_in`], which documents the
/// missing-dir / concurrent-removal / error-propagation semantics that
/// the module-level "Failure handling" section enshrines.
pub(crate) fn clear_all(repo: &Repository) -> anyhow::Result<usize> {
    let mut cleared = 0;
    for kind in ALL_KINDS {
        cleared += clear_json_files_in(&cache_dir(repo, kind))?;
    }
    Ok(cleared)
}

/// Count all cached SHA-keyed entries across every kind.
///
/// Called by `wt config state get` to surface the same state that
/// `clear_all` would sweep, preserving get ↔ clear parity.
pub(crate) fn count_all(repo: &Repository) -> usize {
    let mut count = 0;
    for kind in ALL_KINDS {
        let dir = cache_dir(repo, kind);
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.path().extension().is_some_and(|ext| ext == "json") {
                count += 1;
            }
        }
    }
    count
}

#[cfg(test)]
mod tests {
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

        let path = cache_dir(&repo, KIND_MERGE_TREE_CONFLICTS).join(symmetric_key("aaaa", "bbbb"));
        fs::write(&path, "not valid json {{{").unwrap();

        assert_eq!(merge_conflicts(&repo, "aaaa", "bbbb"), None);
    }

    // LRU sweep

    #[test]
    fn test_sweep_lru_trims_oldest_entries() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();
        let dir = cache_dir(&repo, KIND_MERGE_TREE_CONFLICTS);
        fs::create_dir_all(&dir).unwrap();

        // Create 5 entries with monotonically increasing mtimes
        for i in 0..5 {
            let path = dir.join(format!("entry{i}.json"));
            fs::write(&path, "true").unwrap();
            // Brief sleep to ensure distinct mtimes
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        sweep_lru(&dir, 3);

        let mut remaining: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        remaining.sort();
        // The 2 oldest (entry0, entry1) should have been deleted
        assert_eq!(remaining, ["entry2.json", "entry3.json", "entry4.json"]);
    }

    #[test]
    fn test_sweep_lru_no_op_under_bound() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();
        let dir = cache_dir(&repo, KIND_MERGE_TREE_CONFLICTS);
        fs::create_dir_all(&dir).unwrap();

        for i in 0..3 {
            fs::write(dir.join(format!("entry{i}.json")), "true").unwrap();
        }

        sweep_lru(&dir, 5);

        let count = fs::read_dir(&dir).unwrap().count();
        assert_eq!(count, 3, "should not delete anything when under bound");
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

        let repo = Repository::at(test.root_path()).unwrap();

        // First call: real computation — clean merge → false
        assert!(!repo.has_merge_conflicts("main", "feature").unwrap());

        // Tamper with the cache file to return the wrong answer
        let dir = cache_dir(&repo, KIND_MERGE_TREE_CONFLICTS);
        let entries: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_str().is_some_and(|s| s.ends_with(".json")))
            .collect();
        assert_eq!(entries.len(), 1, "exactly one cache entry expected");
        fs::write(entries[0].path(), "true").unwrap();

        // Second call: reads the tampered value from cache
        // Note: we need a fresh Repository so the in-memory RepoCache doesn't
        // interfere (resolved_refs, merge_base, etc. are all keyed by ref name
        // and would bypass the cache for the same invocation — but
        // rev_parse_commit is cached per command, and on a fresh Repo the SHAs
        // will resolve identically to the first run).
        let repo2 = Repository::at(test.root_path()).unwrap();
        assert!(repo2.has_merge_conflicts("main", "feature").unwrap());
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

        let repo = Repository::at(test.root_path()).unwrap();

        // Call with composite keying — computes and caches
        let result = repo
            .has_merge_conflicts_by_tree("main", &branch_head, &tree_sha)
            .unwrap();

        // Verify the cache entry uses the composite key
        let dir = cache_dir(&repo, KIND_MERGE_TREE_CONFLICTS);
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
            .has_merge_conflicts_by_tree("main", &branch_head, &tree_sha)
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

        let repo = Repository::at(test.root_path()).unwrap();

        // Before rebase: feature conflicts with main (both modified shared.txt)
        let result_before = repo
            .has_merge_conflicts_by_tree("main", &head_before, &tree1)
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
            .has_merge_conflicts_by_tree("main", &head_after, &tree2)
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

        let repo = Repository::at(test.root_path()).unwrap();

        // First call: feature is fully integrated → would_merge_add=false
        let real = repo.merge_integration_probe("feature", "main").unwrap();
        assert!(!real.would_merge_add);

        // Tamper with the cache to flip the answer
        let dir = cache_dir(&repo, KIND_MERGE_ADD_PROBE);
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
        let cached = repo2.merge_integration_probe("feature", "main").unwrap();
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

    // Cache consultation: is_ancestor, has_added_changes, branch_diff_stats

    #[test]
    fn test_is_ancestor_reads_cache() {
        let test = TestRepo::with_initial_commit();

        test.run_git(&["checkout", "-b", "feature"]);
        fs::write(test.root_path().join("new.txt"), "content\n").unwrap();
        test.run_git(&["add", "new.txt"]);
        test.run_git(&["commit", "-m", "Feature"]);
        test.run_git(&["checkout", "main"]);

        let repo = Repository::at(test.root_path()).unwrap();

        // feature is NOT an ancestor of main (main didn't merge feature)
        assert!(!repo.is_ancestor("feature", "main").unwrap());

        // Tamper with cache to flip the answer
        let dir = cache_dir(&repo, KIND_IS_ANCESTOR);
        let entries: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_str().is_some_and(|s| s.ends_with(".json")))
            .collect();
        assert_eq!(entries.len(), 1);
        fs::write(entries[0].path(), "true").unwrap();

        let repo2 = Repository::at(test.root_path()).unwrap();
        assert!(repo2.is_ancestor("feature", "main").unwrap());
    }

    #[test]
    fn test_has_added_changes_reads_cache() {
        let test = TestRepo::with_initial_commit();

        test.run_git(&["checkout", "-b", "feature"]);
        fs::write(test.root_path().join("new.txt"), "content\n").unwrap();
        test.run_git(&["add", "new.txt"]);
        test.run_git(&["commit", "-m", "Feature"]);
        test.run_git(&["checkout", "main"]);

        let repo = Repository::at(test.root_path()).unwrap();

        // feature has added changes compared to main
        assert!(repo.has_added_changes("feature", "main").unwrap());

        // Tamper with cache to flip the answer
        let dir = cache_dir(&repo, KIND_HAS_ADDED_CHANGES);
        let entries: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_str().is_some_and(|s| s.ends_with(".json")))
            .collect();
        assert_eq!(entries.len(), 1);
        fs::write(entries[0].path(), "false").unwrap();

        let repo2 = Repository::at(test.root_path()).unwrap();
        assert!(!repo2.has_added_changes("feature", "main").unwrap());
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
        let dir = cache_dir(&repo, KIND_DIFF_STATS);
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

        let cleared = clear_all(&repo).unwrap();
        assert_eq!(cleared, 5, "should clear one entry per kind");

        // All kinds should be empty
        assert_eq!(merge_conflicts(&repo, "a", "b"), None);
        assert_eq!(merge_add_probe(&repo, "a", "b"), None);
        assert_eq!(is_ancestor(&repo, "a", "b"), None);
        assert_eq!(has_added_changes(&repo, "a", "b"), None);
        assert_eq!(diff_stats(&repo, "a", "b"), None);
    }
}
