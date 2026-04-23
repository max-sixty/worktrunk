//! Shared primitives for the on-disk caches under `.git/wt/cache/`.
//!
//! Three callers use these primitives: `sha_cache` (content-addressed SHA-pair
//! results), `ci_status::cache` (branch → CI status with TTL), and `summary`
//! (branch → LLM summary with content-addressed filenames). Each owns its
//! layout, struct shape, and freshness rules — this module only owns the
//! filesystem mechanics so those rules have one implementation instead of
//! three.
//!
//! # Torn-write semantics
//!
//! Writes use a plain [`fs::write`], not temp-file-plus-rename. A crash in the
//! middle of a write produces a truncated file at the expected path, which
//! [`read_json`] rejects as corrupt JSON — indistinguishable from a cache miss
//! from the caller's perspective. Two concurrent writers for the same key
//! produce the same value for content-addressed caches (benign) and the last
//! writer wins for TTL-based ones (benign — the next read re-fetches if
//! stale). Neither case justifies the rename dance.
//!
//! # Error policy
//!
//! - [`read_json`] returns `None` on any failure (missing file, I/O error,
//!   corrupt JSON) — callers treat all three as a cache miss. Corrupt JSON
//!   is logged at debug.
//! - [`write_json`] degrades silently. Callers never observe cache write
//!   failures because a failed write just means the next access re-computes.
//! - [`clear_one`] and [`clear_json_files`] propagate non-`NotFound` I/O
//!   errors so `wt config state clear` can report truthfully when it can't
//!   delete a file (e.g. permission denied). `NotFound` is counted as "already
//!   gone" so concurrent clearers don't fight each other.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Serialize, de::DeserializeOwned};

use crate::git::Repository;

/// The root directory for a named cache kind.
///
/// Returns `<git-common-dir>/wt/cache/<kind>/`. All worktrunk caches live
/// here; the `kind` is the subdirectory name (e.g. `"ci-status"`,
/// `"summaries"`, `"is-ancestor"`).
pub fn cache_dir(repo: &Repository, kind: &str) -> PathBuf {
    repo.wt_dir().join("cache").join(kind)
}

/// Read and deserialize a JSON cache entry.
///
/// Returns `None` on any failure. Corrupt JSON is logged at debug — a torn
/// write is indistinguishable from a cache miss at this layer.
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let json = fs::read_to_string(path).ok()?;
    match serde_json::from_str::<T>(&json) {
        Ok(value) => Some(value),
        Err(e) => {
            log::debug!("cache: corrupt entry at {}: {}", path.display(), e);
            None
        }
    }
}

/// Serialize and write a JSON cache entry, creating parent directories as
/// needed.
///
/// Degrades silently on any failure — parent dir creation, serialization,
/// or the write itself. A failed write just means the next access
/// re-computes; callers must never observe the error.
pub fn write_json<T: Serialize>(path: &Path, value: &T) {
    if let Some(parent) = path.parent()
        && let Err(e) = fs::create_dir_all(parent)
    {
        log::debug!("cache: failed to create dir {}: {}", parent.display(), e);
        return;
    }

    let Ok(json) = serde_json::to_string(value) else {
        log::debug!("cache: failed to serialize entry for {}", path.display());
        return;
    };

    if let Err(e) = fs::write(path, &json) {
        log::debug!("cache: failed to write {}: {}", path.display(), e);
    }
}

/// Remove a single cache entry.
///
/// Returns `Ok(true)` if a file was removed, `Ok(false)` if it was already
/// gone (a concurrent clearer, or the caller being paranoid). Propagates
/// other I/O errors with the path attached, so `wt config state clear`
/// reports "Cleared"/"No cache" truthfully instead of swallowing a
/// permission-denied failure.
pub fn clear_one(path: &Path) -> anyhow::Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => {
            Err(anyhow::Error::new(e).context(format!("failed to remove {}", path.display())))
        }
    }
}

/// Remove every top-level `.json` file in `dir`, returning the count
/// removed.
///
/// Missing directory is `Ok(0)` — the caller's cache is already empty.
/// Concurrent removal of individual entries is counted as "already gone".
/// Non-`.json` siblings (e.g. leftover `.json.tmp` from old code, or a
/// stray `README`) are left in place.
pub fn clear_json_files(dir: &Path) -> anyhow::Result<usize> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => {
            return Err(anyhow::Error::new(e).context(format!("failed to read {}", dir.display())));
        }
    };

    let mut cleared = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        if clear_one(&path)? {
            cleared += 1;
        }
    }
    Ok(cleared)
}

/// Count top-level `.json` files in `dir`, returning `0` when the directory
/// is missing. Used by `wt config state get` for the `get ↔ clear` parity
/// view.
pub fn count_json_files(dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
    struct V {
        x: u32,
    }

    #[test]
    fn test_read_write_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("sub/entry.json");

        // Missing file is a miss.
        assert!(read_json::<V>(&path).is_none());

        // Write creates parent dirs and round-trips.
        write_json(&path, &V { x: 42 });
        assert_eq!(read_json::<V>(&path), Some(V { x: 42 }));
    }

    #[test]
    fn test_read_corrupt_json_returns_none() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("bad.json");
        fs::write(&path, "not json {{").unwrap();
        assert!(read_json::<V>(&path).is_none());
    }

    #[test]
    fn test_clear_one_missing_returns_false() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nope.json");
        assert!(!clear_one(&path).unwrap());
    }

    #[test]
    fn test_clear_one_propagates_non_not_found() {
        let tmp = TempDir::new().unwrap();
        // Put a directory where a file is expected so remove_file returns
        // EISDIR (or similar), not NotFound.
        let path = tmp.path().join("dir.json");
        fs::create_dir(&path).unwrap();
        let err = clear_one(&path).unwrap_err();
        assert!(err.to_string().contains("failed to remove"), "got: {err}");
    }

    #[test]
    fn test_clear_json_files_counts_and_skips() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("c");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.json"), "{}").unwrap();
        fs::write(dir.join("b.json"), "{}").unwrap();
        // Non-.json siblings must be skipped and left in place.
        fs::write(dir.join("README"), "stray").unwrap();
        fs::write(dir.join("a.json.tmp"), "leftover").unwrap();

        assert_eq!(clear_json_files(&dir).unwrap(), 2);
        assert!(!dir.join("a.json").exists());
        assert!(!dir.join("b.json").exists());
        assert!(dir.join("README").exists());
        assert!(dir.join("a.json.tmp").exists());
    }

    #[test]
    fn test_clear_json_files_missing_dir_is_zero() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(clear_json_files(&tmp.path().join("nope")).unwrap(), 0);
    }

    #[test]
    fn test_clear_json_files_propagates_read_dir_error() {
        let tmp = TempDir::new().unwrap();
        // Put a file where a directory is expected — read_dir returns
        // NotADirectory (not NotFound).
        let path = tmp.path().join("not-a-dir");
        fs::write(&path, "file").unwrap();
        let err = clear_json_files(&path).unwrap_err();
        assert!(err.to_string().contains("failed to read"), "got: {err}");
    }

    #[test]
    fn test_count_json_files() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("c");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.json"), "{}").unwrap();
        fs::write(dir.join("README"), "stray").unwrap();

        assert_eq!(count_json_files(&dir), 1);
        assert_eq!(count_json_files(&tmp.path().join("nope")), 0);
    }
}
