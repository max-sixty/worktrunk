//! CI status caching.
//!
//! Caches CI status in `.git/wt/cache/ci-status/<branch>.json` to avoid
//! hitting API rate limits. Built on the shared `worktrunk::cache`
//! primitives for read/write/clear mechanics.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use worktrunk::cache;
use worktrunk::git::Repository;
use worktrunk::path::sanitize_for_filename;

use super::PrStatus;

/// Subdirectory of `.git/wt/cache/` holding cached CI statuses.
const KIND: &str = "ci-status";

/// Cached CI status stored in `.git/wt/cache/ci-status/<branch>.json`.
///
/// Uses file-based caching instead of git config to avoid file locking
/// issues on Windows where concurrent `git config` writes can lock
/// `.git/config` and cause other git operations to fail.
///
/// Old cache entries without the `branch` field fail deserialization and
/// are treated as cache misses — they get re-fetched with the new format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CachedCiStatus {
    /// The cached CI status (None means no CI found for this branch)
    pub status: Option<PrStatus>,
    /// Unix timestamp when the status was fetched
    pub checked_at: u64,
    /// The HEAD commit SHA when the status was fetched
    pub head: String,
    /// The original branch name (for display in `wt config state show`)
    pub branch: String,
}

impl CachedCiStatus {
    /// Base cache TTL in seconds.
    const TTL_BASE_SECS: u64 = 30;

    /// Maximum jitter added to TTL in seconds.
    /// Actual TTL will be BASE + (0..JITTER) based on repo path hash.
    const TTL_JITTER_SECS: u64 = 30;

    /// Compute TTL with jitter based on repo path.
    ///
    /// Different directories get different TTLs [30, 60) seconds, which spreads
    /// out cache expirations when multiple statuslines run concurrently.
    pub(crate) fn ttl_for_repo(repo_root: &Path) -> u64 {
        // `DefaultHasher` is fine here — the output is ephemeral (used only
        // to pick a TTL for this process), never persisted.
        let mut hasher = DefaultHasher::new();
        repo_root.as_os_str().hash(&mut hasher);
        let hash = hasher.finish();

        let jitter = hash % Self::TTL_JITTER_SECS;
        Self::TTL_BASE_SECS + jitter
    }

    /// Check if the cache is still valid
    pub(super) fn is_valid(&self, current_head: &str, now_secs: u64, repo_root: &Path) -> bool {
        // Cache is valid if:
        // 1. HEAD hasn't changed (same commit)
        // 2. TTL hasn't expired (with deterministic jitter based on repo path)
        let ttl = Self::ttl_for_repo(repo_root);
        self.head == current_head && now_secs.saturating_sub(self.checked_at) < ttl
    }

    /// Get the cache directory path: `.git/wt/cache/ci-status/`
    fn cache_dir(repo: &Repository) -> PathBuf {
        cache::cache_dir(repo, KIND)
    }

    /// Get the cache file path for a branch.
    fn cache_file(repo: &Repository, branch: &str) -> PathBuf {
        let safe_branch = sanitize_for_filename(branch);
        Self::cache_dir(repo).join(format!("{safe_branch}.json"))
    }

    /// Read cached CI status from file.
    pub(super) fn read(repo: &Repository, branch: &str) -> Option<Self> {
        cache::read_json(&Self::cache_file(repo, branch))
    }

    /// Write CI status to cache file.
    ///
    /// A torn write under a concurrent reader produces unparsable bytes
    /// at the expected path, which `read()` treats as a miss — the next
    /// read just re-fetches. See `worktrunk::cache` for the shared
    /// torn-write semantics.
    pub(super) fn write(&self, repo: &Repository, branch: &str) {
        cache::write_json(&Self::cache_file(repo, branch), self);
    }

    /// List all cached CI statuses. The branch name lives in each entry's
    /// `branch` field, matching `CachedSummary::list_all`.
    pub(crate) fn list_all(repo: &Repository) -> Vec<Self> {
        let dir = Self::cache_dir(repo);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };

        entries
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                if path.extension()?.to_str()? != "json" {
                    return None;
                }
                cache::read_json(&path)
            })
            .collect()
    }

    /// Clear the cached CI status for a single branch.
    ///
    /// Returns `Ok(true)` if a cache file was removed, `Ok(false)` if
    /// none existed. Propagates non-`NotFound` I/O errors so the caller
    /// can report truthfully to the user.
    pub(crate) fn clear_one(repo: &Repository, branch: &str) -> anyhow::Result<bool> {
        cache::clear_one(&Self::cache_file(repo, branch))
    }

    /// Clear all cached CI statuses, returning the count cleared.
    ///
    /// Delegates to [`cache::clear_json_files`], which documents the
    /// missing-dir / concurrent-removal / error-propagation semantics.
    pub(crate) fn clear_all(repo: &Repository) -> anyhow::Result<usize> {
        cache::clear_json_files(&Self::cache_dir(repo))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ttl_jitter_range_and_determinism() {
        // Check range: TTL should be in [30, 60)
        let paths = [
            "/tmp/repo1",
            "/tmp/repo2",
            "/workspace/project",
            "/home/user/code",
        ];
        for path in paths {
            let ttl = CachedCiStatus::ttl_for_repo(Path::new(path));
            assert!(
                (30..60).contains(&ttl),
                "TTL {} for path {} should be in [30, 60)",
                ttl,
                path
            );
        }

        // Check determinism: same path should always produce same TTL
        let path = Path::new("/some/consistent/path");
        let ttl1 = CachedCiStatus::ttl_for_repo(path);
        let ttl2 = CachedCiStatus::ttl_for_repo(path);
        assert_eq!(ttl1, ttl2, "Same path should produce same TTL");

        // Check diversity: different paths should likely produce different TTLs
        let diverse_paths: Vec<_> = (0..20).map(|i| format!("/repo/path{}", i)).collect();
        let ttls: std::collections::HashSet<_> = diverse_paths
            .iter()
            .map(|p| CachedCiStatus::ttl_for_repo(Path::new(p)))
            .collect();
        // With 20 paths mapping to 30 possible values, we expect good diversity
        assert!(
            ttls.len() >= 10,
            "Expected diverse TTLs across paths, got {} unique values",
            ttls.len()
        );
    }
}
