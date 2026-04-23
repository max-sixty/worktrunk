//! Shared helpers for the `.git/wt/cache/` subdirectories.
//!
//! Worktrunk keeps several disk-backed caches under `.git/wt/cache/`:
//! `ci-status/<branch>.json` and `{kind}/<sha-pair>.json` for `sha_cache`
//! kinds. Their user-initiated clear semantics coincide: enumerate
//! `.json` files, delete them, tolerate missing dirs and concurrent
//! removal as success, surface other I/O errors so `wt config state
//! clear` cannot report a count it didn't actually achieve.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

/// Delete all `.json` files directly inside `dir`, returning the count.
///
/// - Missing `dir` is `Ok(0)` (cache never populated).
/// - A per-file `NotFound` is skipped (another process cleared it
///   between listing and removal).
/// - Any other I/O error — `read_dir`, per-entry iteration, or
///   `remove_file` — is returned with the relevant path in context.
///
/// Only entries with exactly the `.json` extension are removed;
/// siblings like `.json.tmp` or `README` survive.
pub fn clear_json_files_in(dir: &Path) -> Result<usize> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => {
            return Err(anyhow::Error::new(e).context(format!("failed to read {}", dir.display())));
        }
    };
    let mut cleared = 0;
    for entry in entries {
        let entry = entry.with_context(|| format!("failed to read entry in {}", dir.display()))?;
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => cleared += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(
                    anyhow::Error::new(e).context(format!("failed to remove {}", path.display()))
                );
            }
        }
    }
    Ok(cleared)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestRepo;

    #[test]
    fn test_missing_dir_returns_zero() {
        let test = TestRepo::with_initial_commit();
        let missing = test.root_path().join("never-existed");

        assert_eq!(clear_json_files_in(&missing).unwrap(), 0);
    }

    #[test]
    fn test_propagates_non_not_found_read_dir_error() {
        let test = TestRepo::with_initial_commit();
        // Regular file where a directory is expected → ENOTDIR.
        let path = test.root_path().join("not-a-dir");
        fs::write(&path, "stray").unwrap();

        let err = clear_json_files_in(&path).unwrap_err();
        assert!(
            err.to_string().contains("failed to read"),
            "expected read-failure context, got: {err}"
        );
    }

    #[test]
    fn test_propagates_per_file_remove_error() {
        let test = TestRepo::with_initial_commit();
        let dir = test.root_path().join("cache");
        fs::create_dir(&dir).unwrap();
        // A directory named `*.json` makes remove_file return EISDIR.
        fs::create_dir(dir.join("bad.json")).unwrap();

        let err = clear_json_files_in(&dir).unwrap_err();
        assert!(
            err.to_string().contains("failed to remove"),
            "expected remove-failure context, got: {err}"
        );
    }

    #[test]
    fn test_skips_non_json_extensions() {
        let test = TestRepo::with_initial_commit();
        let dir = test.root_path().join("cache");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("a.json"), "{}").unwrap();
        fs::write(dir.join("a.json.tmp"), "leftover").unwrap();
        fs::write(dir.join("README"), "stray").unwrap();

        let count = clear_json_files_in(&dir).unwrap();
        assert_eq!(count, 1);
        assert!(!dir.join("a.json").exists());
        assert!(dir.join("a.json.tmp").exists());
        assert!(dir.join("README").exists());
    }

    #[test]
    fn test_empty_dir_returns_zero() {
        let test = TestRepo::with_initial_commit();
        let dir = test.root_path().join("cache");
        fs::create_dir(&dir).unwrap();

        assert_eq!(clear_json_files_in(&dir).unwrap(), 0);
    }
}
