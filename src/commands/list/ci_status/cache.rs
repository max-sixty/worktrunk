//! CI status caching.
//!
//! Caches CI status in `.git/wt-cache/ci-status/<branch>.json` to avoid
//! hitting API rate limits.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use worktrunk::git::Repository;
use worktrunk::path::sanitize_for_filename;

use super::PrStatus;

/// Cached CI status stored in `.git/wt-cache/ci-status/<branch>.json`
///
/// Uses file-based caching instead of git config to avoid file locking issues.
/// On Windows, concurrent `git config` writes can temporarily lock `.git/config`,
/// causing other git operations to fail with "Permission denied".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CachedCiStatus {
    /// The cached CI status (None means no CI found for this branch)
    pub status: Option<PrStatus>,
    /// Unix timestamp when the status was fetched
    pub checked_at: u64,
    /// The HEAD commit SHA when the status was fetched
    pub head: String,
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
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        // Hash the path bytes directly for consistent TTL across string representations
        repo_root.as_os_str().hash(&mut hasher);
        let hash = hasher.finish();

        // Map hash to jitter range [0, TTL_JITTER_SECS)
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

    /// Get the cache directory path: `.git/wt-cache/ci-status/`
    fn cache_dir(repo: &Repository) -> PathBuf {
        repo.git_common_dir().join("wt-cache").join("ci-status")
    }

    /// Get the cache file path for a branch.
    fn cache_file(repo: &Repository, branch: &str) -> PathBuf {
        let dir = Self::cache_dir(repo);
        let safe_branch = sanitize_for_filename(branch);
        dir.join(format!("{safe_branch}.json"))
    }

    /// Read cached CI status from file.
    pub(super) fn read(repo: &Repository, branch: &str) -> Option<Self> {
        let path = Self::cache_file(repo, branch);
        let json = fs::read_to_string(&path).ok()?;
        serde_json::from_str(&json).ok()
    }

    /// Write CI status to cache file.
    ///
    /// Uses atomic write (write to temp file, then rename) to avoid corruption
    /// and minimize lock contention on Windows.
    pub(super) fn write(&self, repo: &Repository, branch: &str) {
        let path = Self::cache_file(repo, branch);

        // Create cache directory if needed
        if let Some(parent) = path.parent()
            && let Err(e) = fs::create_dir_all(parent)
        {
            log::debug!("Failed to create cache dir for {}: {}", branch, e);
            return;
        }

        let Ok(json) = serde_json::to_string(self) else {
            log::debug!("Failed to serialize CI cache for {}", branch);
            return;
        };

        // Write to temp file first, then rename for atomic update
        let temp_path = path.with_extension("json.tmp");
        if let Err(e) = fs::write(&temp_path, &json) {
            log::debug!("Failed to write CI cache temp file for {}: {}", branch, e);
            return;
        }

        if let Err(e) = fs::rename(&temp_path, &path) {
            log::debug!("Failed to rename CI cache file for {}: {}", branch, e);
            // Clean up temp file on failure
            let _ = fs::remove_file(&temp_path);
        }
    }

    /// List all cached CI statuses as (branch_name, cached_status) pairs.
    pub(crate) fn list_all(repo: &Repository) -> Vec<(String, Self)> {
        let cache_dir = Self::cache_dir(repo);

        let entries = match fs::read_dir(&cache_dir) {
            Ok(entries) => entries,
            Err(_) => return Vec::new(),
        };

        entries
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let path = entry.path();

                // Only process .json files (skip .json.tmp)
                if path.extension()?.to_str()? != "json" {
                    return None;
                }

                let branch = path.file_stem()?.to_str()?.to_string();
                let json = fs::read_to_string(&path).ok()?;
                let cached: Self = serde_json::from_str(&json).ok()?;
                Some((branch, cached))
            })
            .collect()
    }

    /// Clear all cached CI statuses, returns count cleared.
    pub(crate) fn clear_all(repo: &Repository) -> usize {
        let cache_dir = Self::cache_dir(repo);

        let entries = match fs::read_dir(&cache_dir) {
            Ok(entries) => entries,
            Err(_) => return 0,
        };

        let mut cleared = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            // Only remove .json files
            if path.extension().is_some_and(|ext| ext == "json") && fs::remove_file(&path).is_ok() {
                cleared += 1;
            }
        }
        cleared
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
