//! Recursive directory copying with reflink (COW) and rayon parallelism.
//!
//! Copies directory trees file-by-file using `reflink_or_copy` which uses
//! copy-on-write clones where the filesystem supports them (APFS, btrfs, XFS),
//! falling back to regular copies otherwise.
//!
//! Parallelism is achieved with rayon's `par_iter` at each directory level,
//! so sibling entries are copied concurrently while the tree is walked
//! depth-first.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use anyhow::Context;
use rayon::prelude::*;

/// Maximum threads for filesystem copy operations. Beyond this, SSD I/O
/// contention causes performance to regress (benchmarked on APFS and ext4).
/// The global rayon pool is sized for network I/O (2x cores) which is too
/// many for local filesystem work.
pub const MAX_COPY_THREADS: usize = 4;

/// Copy a directory tree recursively using reflink (COW) per file.
///
/// Handles regular files, directories, and symlinks. Non-regular files (sockets,
/// FIFOs) are silently skipped. Existing entries at the destination are skipped
/// for idempotent usage.
///
/// When `force` is true, existing files and symlinks at the destination are
/// removed before copying.
///
/// Uses a dedicated rayon thread pool capped at `MAX_COPY_THREADS` to avoid
/// SSD I/O contention from the larger global pool. When called from within an
/// existing rayon pool context (e.g. via `pool.install()`), that pool is reused
/// rather than creating a new one — this prevents concurrent callers from each
/// spawning their own pool and exhausting OS file-descriptor limits on large
/// trees (EMFILE / "too many open files").
pub fn copy_dir_recursive(src: &Path, dest: &Path, force: bool) -> anyhow::Result<()> {
    // If we are already executing inside a rayon worker thread, reuse that
    // pool rather than creating a new one. This avoids concurrent callers
    // (e.g. from an outer par_iter) each allocating their own pool.
    if rayon::current_thread_index().is_some() {
        return copy_dir_recursive_inner(src, dest, force);
    }

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(MAX_COPY_THREADS)
        .build()
        .context("building copy thread pool")?;

    pool.install(|| copy_dir_recursive_inner(src, dest, force))
}

fn copy_dir_recursive_inner(src: &Path, dest: &Path, force: bool) -> anyhow::Result<()> {
    fs::create_dir_all(dest).with_context(|| format!("creating directory {}", dest.display()))?;

    let entries: Vec<_> = fs::read_dir(src)?.collect::<Result<Vec<_>, _>>()?;

    entries.into_par_iter().try_for_each(|entry| {
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());

        if file_type.is_symlink() {
            if force {
                remove_if_exists(&dest_path)?;
            }
            // Use symlink_metadata to detect broken symlinks (exists() follows symlinks
            // and returns false for broken ones, causing EEXIST on the next symlink call)
            if dest_path.symlink_metadata().is_err() {
                let target = fs::read_link(&src_path)
                    .with_context(|| format!("reading symlink {}", src_path.display()))?;
                create_symlink(&target, &src_path, &dest_path)?;
            }
        } else if file_type.is_dir() {
            copy_dir_recursive_inner(&src_path, &dest_path, force)?;
        } else if !file_type.is_file() {
            log::debug!("skipping non-regular file: {}", src_path.display());
        } else {
            if force {
                remove_if_exists(&dest_path)?;
            }
            // Check symlink_metadata (not exists()) because exists() follows symlinks
            // and returns false for broken ones, which would cause reflink_or_copy to
            // fail with ENOENT on some platforms when copying through the broken symlink.
            if dest_path.symlink_metadata().is_err() {
                match reflink_copy::reflink_or_copy(&src_path, &dest_path) {
                    Ok(_) => {}
                    Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
                    Err(e) => {
                        return Err(anyhow::Error::from(e)
                            .context(format!("copying {}", src_path.display())));
                    }
                }
            }
        }
        Ok(())
    })?;

    // Preserve source directory permissions AFTER copying contents.
    // Must be done after the loop — if the source lacks write permission (e.g., 0o555),
    // setting it before copying would make the destination read-only and fail the copies.
    #[cfg(unix)]
    {
        let src_perms = fs::metadata(src)
            .with_context(|| format!("reading permissions for {}", src.display()))?
            .permissions();
        fs::set_permissions(dest, src_perms)
            .with_context(|| format!("setting permissions on {}", dest.display()))?;
    }

    Ok(())
}

/// Remove a file, ignoring "not found" errors.
pub fn remove_if_exists(path: &Path) -> anyhow::Result<()> {
    if let Err(e) = fs::remove_file(path) {
        anyhow::ensure!(e.kind() == ErrorKind::NotFound, e);
    }
    Ok(())
}

/// Create a symlink, handling platform differences.
pub fn create_symlink(target: &Path, src_path: &Path, dest_path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        let _ = src_path; // Used on Windows to determine symlink type
        std::os::unix::fs::symlink(target, dest_path)
            .with_context(|| format!("creating symlink {}", dest_path.display()))?;
    }
    #[cfg(windows)]
    {
        let is_dir = src_path.metadata().map(|m| m.is_dir()).unwrap_or(false);
        if is_dir {
            std::os::windows::fs::symlink_dir(target, dest_path)
                .with_context(|| format!("creating symlink {}", dest_path.display()))?;
        } else {
            std::os::windows::fs::symlink_file(target, dest_path)
                .with_context(|| format!("creating symlink {}", dest_path.display()))?;
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, src_path, dest_path);
        anyhow::bail!("symlink creation not supported on this platform");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use rayon::prelude::*;
    use tempfile::TempDir;

    use super::*;

    /// Regression test: copy_dir_recursive must not create a new thread pool when
    /// called from within an existing rayon pool context.
    #[test]
    fn test_copy_dir_recursive_reuses_pool_when_called_from_par_iter() {
        let src_root = TempDir::new().unwrap();
        let dst_root = TempDir::new().unwrap();

        // Create many source directories, each with several files.
        // This has to be big enough to overflow the available file descriptor limit
        // 900*20 = 18,000 seems to be about enough.
        const DIR_COUNT: usize = 900;
        const FILES_PER_DIR: usize = 20;
        for i in 0..DIR_COUNT {
            let dir = src_root.path().join(format!("dir-{i}"));
            fs::create_dir_all(&dir).unwrap();
            for j in 0..FILES_PER_DIR {
                fs::write(
                    dir.join(format!("file-{j}.txt")),
                    format!("content {i}-{j}"),
                )
                .unwrap();
            }
        }

        // Mirror what step_copy_ignored does: one shared pool wrapping a par_iter
        // that calls copy_dir_recursive for each directory entry.
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .build()
            .unwrap();
        pool.install(|| {
            (0..DIR_COUNT)
                .into_par_iter()
                .try_for_each(|i| -> anyhow::Result<()> {
                    let src = src_root.path().join(format!("dir-{i}"));
                    let dst = dst_root.path().join(format!("dir-{i}"));
                    copy_dir_recursive(&src, &dst, false)
                })
        })
        .unwrap();

        // All files must have been copied correctly.
        for i in 0..DIR_COUNT {
            for j in 0..FILES_PER_DIR {
                let dst_file = dst_root
                    .path()
                    .join(format!("dir-{i}"))
                    .join(format!("file-{j}.txt"));
                assert!(dst_file.exists(), "dir-{i}/file-{j}.txt should be copied");
                assert_eq!(
                    fs::read_to_string(&dst_file).unwrap(),
                    format!("content {i}-{j}"),
                    "dir-{i}/file-{j}.txt content should match"
                );
            }
        }
    }
}
