//! Recursive directory removal with progress reporting.
//!
//! Walks the tree iteratively (no recursion), unlinks files in parallel, then
//! removes the now-empty directories deepest-first. Each unlinked leaf bumps
//! a [`Progress`] counter so a TTY spinner can render live updates; the
//! returned `(files, bytes)` tuple drives the matching post-op summary.
//!
//! Parallel unlink uses rayon's global pool — file removal is filesystem
//! latency, not CPU, so the I/O-tuned 2× CPU cores work well. Errors on
//! individual entries propagate up via `try_for_each`; "already gone"
//! (`NotFound`) is treated as success so the function is idempotent.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use anyhow::Context;
use rayon::prelude::*;

use crate::progress::Progress;

/// Remove a directory tree, reporting per-file progress.
///
/// Returns `(files_removed, bytes_removed)` — counts exclude entries that
/// were already missing. Bytes use `symlink_metadata` (so symlinks count as
/// their own size, not the target's).
///
/// On `NotFound` for the root path, returns `(0, 0)` rather than erroring —
/// callers commonly invoke this on a path that may have been cleaned up by a
/// prior best-effort step.
pub fn remove_dir_with_progress(path: &Path, progress: &Progress) -> anyhow::Result<(usize, u64)> {
    let mut leaves: Vec<PathBuf> = Vec::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut stack = vec![path.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            // Tree (or this branch of it) is already gone — nothing to remove.
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(
                    anyhow::Error::from(e).context(format!("reading directory {}", dir.display()))
                );
            }
        };
        // Push parents first; we'll reverse before rmdir so children unlink first.
        dirs.push(dir.clone());
        for entry in entries {
            let entry = entry.with_context(|| format!("reading entry under {}", dir.display()))?;
            let file_type = entry
                .file_type()
                .with_context(|| format!("reading file type for {}", entry.path().display()))?;
            let entry_path = entry.path();
            // is_dir() is false for symlinks on Unix (lstat semantics) — they
            // fall through to the leaf branch and get removed via remove_file.
            if file_type.is_dir() {
                stack.push(entry_path);
            } else {
                leaves.push(entry_path);
            }
        }
    }

    let removed_files = AtomicUsize::new(0);
    let removed_bytes = AtomicU64::new(0);
    leaves
        .par_iter()
        .try_for_each(|leaf| -> anyhow::Result<()> {
            // Capture size before unlinking. Best-effort: symlink_metadata may
            // fail on a racy delete, in which case we still try to unlink and
            // count the leaf with zero bytes.
            let bytes = leaf.symlink_metadata().map(|m| m.len()).unwrap_or(0);
            match fs::remove_file(leaf) {
                Ok(()) => {
                    removed_files.fetch_add(1, Ordering::Relaxed);
                    removed_bytes.fetch_add(bytes, Ordering::Relaxed);
                    progress.record(bytes);
                    Ok(())
                }
                Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
                Err(e) => {
                    Err(anyhow::Error::from(e).context(format!("removing {}", leaf.display())))
                }
            }
        })?;

    // Pop order pushed parents before children, so reversing gives
    // deepest-first — exactly what `rmdir` needs.
    for dir in dirs.iter().rev() {
        match fs::remove_dir(dir) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => {
                return Err(
                    anyhow::Error::from(e).context(format!("removing directory {}", dir.display()))
                );
            }
        }
    }

    Ok((removed_files.into_inner(), removed_bytes.into_inner()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remove_dir_with_progress_empty_dir() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("empty");
        std::fs::create_dir(&dir).unwrap();

        let (files, bytes) = remove_dir_with_progress(&dir, &Progress::disabled()).unwrap();

        assert_eq!(files, 0);
        assert_eq!(bytes, 0);
        assert!(!dir.exists());
    }

    #[test]
    fn test_remove_dir_with_progress_counts_files_and_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("tree");
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("a/file1.txt"), b"hello").unwrap(); // 5 bytes
        std::fs::write(root.join("a/b/file2.txt"), b"world!").unwrap(); // 6 bytes
        std::fs::write(root.join("top.txt"), b"x").unwrap(); // 1 byte

        let (files, bytes) = remove_dir_with_progress(&root, &Progress::disabled()).unwrap();

        assert_eq!(files, 3);
        assert_eq!(bytes, 12);
        assert!(!root.exists());
    }

    #[test]
    fn test_remove_dir_with_progress_missing_root_is_ok() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("does-not-exist");

        let (files, bytes) = remove_dir_with_progress(&missing, &Progress::disabled()).unwrap();

        assert_eq!(files, 0);
        assert_eq!(bytes, 0);
    }

    #[cfg(unix)]
    #[test]
    fn test_remove_dir_with_progress_handles_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("tree");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("real.txt"), b"abc").unwrap();
        std::os::unix::fs::symlink(root.join("real.txt"), root.join("link")).unwrap();

        let (files, _bytes) = remove_dir_with_progress(&root, &Progress::disabled()).unwrap();

        // 1 file + 1 symlink = 2 leaves removed; the symlink is unlinked
        // without following.
        assert_eq!(files, 2);
        assert!(!root.exists());
    }
}
