//! Persistent cache for picker preview content, keyed by SHA + dimensions.
//!
//! Three of the picker's preview modes are deterministic functions of git
//! object SHAs at a given terminal width: Log on `(branch_head_sha)`,
//! BranchDiff on `(default_head_sha, branch_head_sha)`, and UpstreamDiff on
//! `(branch_head_sha, upstream_head_sha)`. Identical inputs produce identical
//! output, so a disk cache hit short-circuits the git subprocess on
//! subsequent `wt switch` invocations. WorkingTree is intentionally not
//! cached — its inputs include the mutable working tree, which has no cheap
//! stable hash. Summary has its own cache (`crate::summary`).
//!
//! Layout: `.git/wt/cache/picker-preview/{mode}-{sha}[-{sha}]-{w}[-{h}].json`,
//! where the value is the pre-pager string the matching `compute_*_preview`
//! returns. The pager step in `compute_and_page_preview` runs on every read,
//! so changing the configured pager invalidates nothing — the cache is
//! pager-agnostic.
//!
//! No explicit invalidation: SHAs are content-addressed, so a `git fetch`
//! that moves the default branch or upstream produces fresh keys; the LRU
//! sweep prunes stale entries.
//!
//! Per-kind LRU bound is intentionally small (rendered diffs can be tens to
//! hundreds of KB, much larger than the 80-byte SHA-pair entries in
//! `git/repository/sha_cache.rs`). See [`worktrunk::cache`] for read/write/LRU
//! mechanics, torn-write semantics, and the user-initiated clear error
//! policy.

use worktrunk::cache;
use worktrunk::git::Repository;

const KIND: &str = "picker-preview";

/// 500 entries × tens-of-KB rendered diffs ≈ tens of MB. Tunable; the
/// user-visible knob is `wt config state clear`.
const MAX_ENTRIES: usize = 500;

fn log_key(sha: &str, w: usize, h: usize) -> String {
    format!("log-{sha}-{w}-{h}.json")
}

fn branch_diff_key(base_sha: &str, branch_sha: &str, w: usize) -> String {
    format!("branch-diff-{base_sha}-{branch_sha}-{w}.json")
}

fn upstream_diff_key(branch_sha: &str, upstream_sha: &str, w: usize) -> String {
    format!("upstream-diff-{branch_sha}-{upstream_sha}-{w}.json")
}

pub(super) fn read_log(repo: &Repository, sha: &str, w: usize, h: usize) -> Option<String> {
    cache::read(repo, KIND, &log_key(sha, w, h))
}

pub(super) fn write_log(repo: &Repository, sha: &str, w: usize, h: usize, value: &str) {
    cache::write_with_lru(repo, KIND, &log_key(sha, w, h), &value, MAX_ENTRIES);
}

pub(super) fn read_branch_diff(
    repo: &Repository,
    base_sha: &str,
    branch_sha: &str,
    w: usize,
) -> Option<String> {
    cache::read(repo, KIND, &branch_diff_key(base_sha, branch_sha, w))
}

pub(super) fn write_branch_diff(
    repo: &Repository,
    base_sha: &str,
    branch_sha: &str,
    w: usize,
    value: &str,
) {
    cache::write_with_lru(
        repo,
        KIND,
        &branch_diff_key(base_sha, branch_sha, w),
        &value,
        MAX_ENTRIES,
    );
}

pub(super) fn read_upstream_diff(
    repo: &Repository,
    branch_sha: &str,
    upstream_sha: &str,
    w: usize,
) -> Option<String> {
    cache::read(repo, KIND, &upstream_diff_key(branch_sha, upstream_sha, w))
}

pub(super) fn write_upstream_diff(
    repo: &Repository,
    branch_sha: &str,
    upstream_sha: &str,
    w: usize,
    value: &str,
) {
    cache::write_with_lru(
        repo,
        KIND,
        &upstream_diff_key(branch_sha, upstream_sha, w),
        &value,
        MAX_ENTRIES,
    );
}

/// Clear all cached preview entries, returning the count of `.json` files
/// removed. Called by `wt config state clear`; see
/// [`worktrunk::cache::clear_json_files`] for the missing-dir /
/// concurrent-removal / error-propagation semantics.
pub(crate) fn clear_all(repo: &Repository) -> anyhow::Result<usize> {
    cache::clear_json_files(&cache::cache_dir(repo, KIND))
}

/// Count cached preview entries for `wt config state get`.
pub(crate) fn count_all(repo: &Repository) -> usize {
    cache::count_json_files(&cache::cache_dir(repo, KIND))
}

#[cfg(test)]
mod tests {
    use super::*;
    use worktrunk::testing::TestRepo;

    #[test]
    fn log_roundtrip() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        assert_eq!(read_log(&repo, "deadbeef", 80, 24), None);
        write_log(&repo, "deadbeef", 80, 24, "rendered log");
        assert_eq!(
            read_log(&repo, "deadbeef", 80, 24),
            Some("rendered log".to_string())
        );
    }

    #[test]
    fn log_width_invalidates() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        write_log(&repo, "deadbeef", 80, 24, "rendered log");
        // Different width misses — render width affects line wrapping and
        // timestamp visibility, so cached entries cannot be reused.
        assert_eq!(read_log(&repo, "deadbeef", 100, 24), None);
        assert_eq!(read_log(&repo, "deadbeef", 80, 30), None);
    }

    #[test]
    fn log_sha_invalidates() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        write_log(&repo, "deadbeef", 80, 24, "rendered log");
        assert_eq!(read_log(&repo, "cafe", 80, 24), None);
    }

    #[test]
    fn branch_diff_roundtrip_and_asymmetric() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        write_branch_diff(&repo, "base", "tip", 80, "rendered diff");
        assert_eq!(
            read_branch_diff(&repo, "base", "tip", 80),
            Some("rendered diff".to_string())
        );
        // Asymmetric: swapping is a different key.
        assert_eq!(read_branch_diff(&repo, "tip", "base", 80), None);
    }

    #[test]
    fn upstream_diff_roundtrip() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        write_upstream_diff(&repo, "branch", "upstream", 80, "rendered upstream diff");
        assert_eq!(
            read_upstream_diff(&repo, "branch", "upstream", 80),
            Some("rendered upstream diff".to_string())
        );
    }

    #[test]
    fn modes_share_kind_but_distinct_keys() {
        // Same SHA + width across modes must not collide — the mode prefix
        // in the filename is what keeps Log, BranchDiff, and UpstreamDiff
        // separated under a single cache kind.
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        write_log(&repo, "x", 80, 24, "log-value");
        write_branch_diff(&repo, "x", "x", 80, "branch-diff-value");
        write_upstream_diff(&repo, "x", "x", 80, "upstream-diff-value");

        assert_eq!(read_log(&repo, "x", 80, 24).unwrap(), "log-value");
        assert_eq!(
            read_branch_diff(&repo, "x", "x", 80).unwrap(),
            "branch-diff-value"
        );
        assert_eq!(
            read_upstream_diff(&repo, "x", "x", 80).unwrap(),
            "upstream-diff-value"
        );
        assert_eq!(count_all(&repo), 3);
    }

    #[test]
    fn clear_all_removes_entries() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        write_log(&repo, "a", 80, 24, "x");
        write_log(&repo, "b", 80, 24, "y");
        write_branch_diff(&repo, "base", "tip", 80, "z");

        assert_eq!(count_all(&repo), 3);
        let removed = clear_all(&repo).unwrap();
        assert_eq!(removed, 3);
        assert_eq!(count_all(&repo), 0);
        assert_eq!(read_log(&repo, "a", 80, 24), None);
    }
}
