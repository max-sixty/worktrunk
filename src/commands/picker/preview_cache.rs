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
//! Layout: `.git/wt/cache/picker-preview/{mode}-{sha}[-{sha}]-{w}[-{h}].json`.
//! The diff modes cache the pre-pager rendered string; the pager step in
//! `compute_and_page_preview` runs on every read, so changing the
//! configured pager invalidates nothing — the cache is pager-agnostic.
//! The Log mode caches a small struct (raw `git log` output + per-commit
//! stats) and recomputes the dim/bright split and relative-time formatting
//! on every render — see [`LogCacheEntry`] for why.
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

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use worktrunk::cache;
use worktrunk::git::Repository;

const KIND: &str = "picker-preview";

/// Cached payload for the Log preview.
///
/// The Log render has two time-varying inputs that must be recomputed on
/// every call rather than baked into the cache: the dim/bright split (from
/// `merge-base(default_branch, head)` + `rev-list --right-only`, which
/// shifts as `main` advances) and the relative-time strings ("5m", "2h",
/// "3d", computed against `epoch_now()`). To keep the cache key simple
/// (just `(branch_head_sha, w, h)` — main's SHA stays out so a `git fetch`
/// doesn't invalidate every entry), we cache the SHA-deterministic
/// artifacts only: the raw `git log --graph` output (with `%ct` timestamps
/// embedded) and the per-commit `(insertions, deletions)` map from
/// `batch_fetch_stats`. The render path re-runs `process_log_with_dimming`
/// against fresh `unique_commits` and `format_log_output` against
/// `epoch_now()`, so output stays correct as `main` and wall-clock advance.
///
/// `raw_log` also embeds `git log --format=%C(auto)%d` ref decorations
/// (e.g. `HEAD -> feature, main`), which are *not* SHA-deterministic —
/// another ref starting or stopping pointing at the same commit (most
/// commonly: a squash merge landing `main` at the cached SHA) would
/// leave the decoration text stale even though the cache key is still
/// valid. The orchestrator mitigates this by enqueuing a background
/// refresh task on the global rayon pool whenever a Log preview hit
/// the disk cache; the refresh re-runs `compute_log_raw_and_stats`,
/// overwrites this entry, and updates the in-memory `PreviewCache`.
/// The cached entry served on the *current* render is still potentially
/// stale — refresh is async — but the next visit to the same row reads
/// fresh content. See `commands::picker::preview_orchestrator::spawn_preview`.
#[derive(Serialize, Deserialize, Default)]
pub(super) struct LogCacheEntry {
    pub raw_log: String,
    /// Empty when `width < TIMESTAMP_WIDTH_THRESHOLD` (the no-timestamp
    /// path doesn't fetch stats). Keys are full commit SHAs.
    pub stats: HashMap<String, (usize, usize)>,
}

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

pub(super) fn read_log(repo: &Repository, sha: &str, w: usize, h: usize) -> Option<LogCacheEntry> {
    cache::read(repo, KIND, &log_key(sha, w, h))
}

pub(super) fn write_log(repo: &Repository, sha: &str, w: usize, h: usize, value: &LogCacheEntry) {
    cache::write_with_lru(repo, KIND, &log_key(sha, w, h), value, MAX_ENTRIES);
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

    fn sample_log_entry() -> LogCacheEntry {
        let mut stats = HashMap::new();
        stats.insert("abc123".to_string(), (5, 2));
        LogCacheEntry {
            raw_log: "raw log content".to_string(),
            stats,
        }
    }

    #[test]
    fn log_roundtrip() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        assert!(read_log(&repo, "deadbeef", 80, 24).is_none());
        write_log(&repo, "deadbeef", 80, 24, &sample_log_entry());

        let read = read_log(&repo, "deadbeef", 80, 24).expect("entry exists");
        assert_eq!(read.raw_log, "raw log content");
        assert_eq!(read.stats.get("abc123"), Some(&(5, 2)));
    }

    #[test]
    fn log_width_invalidates() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        write_log(&repo, "deadbeef", 80, 24, &sample_log_entry());
        // Different width misses — render width changes the requested log
        // format (with vs without timestamps), so cached entries cannot be
        // reused. Different height misses for the same reason via log_limit.
        assert!(read_log(&repo, "deadbeef", 100, 24).is_none());
        assert!(read_log(&repo, "deadbeef", 80, 30).is_none());
    }

    #[test]
    fn log_sha_invalidates() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        write_log(&repo, "deadbeef", 80, 24, &sample_log_entry());
        assert!(read_log(&repo, "cafe", 80, 24).is_none());
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

        write_log(&repo, "x", 80, 24, &sample_log_entry());
        write_branch_diff(&repo, "x", "x", 80, "branch-diff-value");
        write_upstream_diff(&repo, "x", "x", 80, "upstream-diff-value");

        assert_eq!(
            read_log(&repo, "x", 80, 24).unwrap().raw_log,
            "raw log content"
        );
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

        write_log(&repo, "a", 80, 24, &sample_log_entry());
        write_log(&repo, "b", 80, 24, &sample_log_entry());
        write_branch_diff(&repo, "base", "tip", 80, "z");

        assert_eq!(count_all(&repo), 3);
        let removed = clear_all(&repo).unwrap();
        assert_eq!(removed, 3);
        assert_eq!(count_all(&repo), 0);
        assert!(read_log(&repo, "a", 80, 24).is_none());
    }
}
