//! Recursive directory copying with reflink (COW) and rayon parallelism.
//!
//! Copies directory trees file-by-file using `reflink_or_copy` which uses
//! copy-on-write clones where the filesystem supports them (APFS, btrfs, XFS),
//! falling back to regular copies otherwise.
//!
//! All copy I/O runs in a dedicated 4-thread pool rather than the global rayon
//! pool (which is sized at 2× CPU cores for network I/O) to avoid saturating
//! the CPU on a background operation.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::sync::LazyLock;

use anyhow::Context;
use rayon::prelude::*;

/// Capped at 4 threads to avoid saturating the CPU — the global rayon pool is
/// much larger (2× CPU cores, tuned for network I/O in `wt list`).
///
/// The 8 MiB stack matches the default on Linux/macOS; Windows defaults to
/// ~2 MiB, which can overflow under concurrent reflink/copy work (observed as
/// a worker-thread stack overflow in `test_copy_ignored_many_directories_no_emfile`
/// on Windows CI).
static COPY_POOL: LazyLock<rayon::ThreadPool> = LazyLock::new(|| {
    rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .stack_size(8 * 1024 * 1024)
        .build()
        .expect("failed to build copy thread pool")
});

/// Copy a directory tree recursively using reflink (COW) per file.
///
/// Handles regular files, directories, and symlinks. Non-regular files (sockets,
/// FIFOs) are silently skipped. Existing entries at the destination are skipped
/// for idempotent usage.
///
/// When `force` is true, existing files and symlinks at the destination are
/// removed before copying.
///
/// Uses a dedicated 4-thread pool. Nested calls (recursive directories) skip
/// pool entry and run inline on the current worker thread.
pub fn copy_dir_recursive(src: &Path, dest: &Path, force: bool) -> anyhow::Result<()> {
    COPY_POOL.install(|| copy_dir_recursive_inner(src, dest, force))
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
