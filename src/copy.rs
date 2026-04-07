//! Directory copying with reflink (COW) and rayon parallelism.
//!
//! Copies directory trees file-by-file using `reflink_or_copy` which uses
//! copy-on-write clones where the filesystem supports them (APFS, btrfs, XFS),
//! falling back to regular copies otherwise.
//!
//! All copy I/O runs on a dedicated 4-thread pool rather than the global rayon
//! pool (which is sized at 2× CPU cores for network I/O) to avoid saturating
//! the CPU on a background operation. Directory trees are walked iteratively
//! (no recursion) then copied in a single parallel pass.
//!
//! Callers that want low-priority I/O (e.g. `step_copy_ignored`) should call
//! [`lower_process_priority`] before starting work.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Context;
use rayon::prelude::*;

/// Capped at 4 threads to avoid saturating the CPU — the global rayon pool is
/// much larger (2× CPU cores, tuned for network I/O in `wt list`).
static COPY_POOL: LazyLock<rayon::ThreadPool> = LazyLock::new(|| {
    rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .expect("failed to build copy thread pool")
});

/// Lower the current process's scheduling priority so copy I/O doesn't
/// compete with interactive foreground work.
///
/// Uses `renice` rather than a direct `nice(2)` syscall to stay within the
/// `forbid(unsafe_code)` lint. Non-fatal: if `renice` is missing or fails,
/// copies proceed at normal priority.
pub fn lower_process_priority() {
    #[cfg(unix)]
    {
        use std::process::{Command, Stdio};
        let _ = Command::new("renice")
            .args(["-n", "19", "-p", &std::process::id().to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Copy a single file or symlink, using reflink (COW) when possible.
///
/// Detects symlinks via `symlink_metadata` on the source. Returns `true` if
/// the entry was copied, `false` if skipped (destination already exists).
/// When `force` is true, existing entries are removed before copying.
pub fn copy_leaf(src: &Path, dest: &Path, force: bool) -> anyhow::Result<bool> {
    if force {
        remove_if_exists(dest)?;
    }
    // Use symlink_metadata (not exists()) because exists() follows symlinks
    // and returns false for broken ones.
    if dest.symlink_metadata().is_ok() {
        return Ok(false);
    }

    let is_symlink = src
        .symlink_metadata()
        .with_context(|| format!("reading metadata for {}", src.display()))?
        .file_type()
        .is_symlink();

    if is_symlink {
        let target =
            fs::read_link(src).with_context(|| format!("reading symlink {}", src.display()))?;
        create_symlink(&target, src, dest)?;
    } else {
        match reflink_copy::reflink_or_copy(src, dest) {
            Ok(_) => {
                // Preserve file permissions (especially the execute bit).
                //
                // On btrfs/XFS, reflink (FICLONE ioctl) clones data extents
                // only — the destination gets umask-based permissions, losing
                // execute bits. std::fs::copy's fallback preserves permissions
                // via fchmod, creating an asymmetry in reflink_or_copy.
                //
                // Refs: ioctl_ficlonerange(2), LWN Articles/331808
                #[cfg(unix)]
                {
                    let perms = fs::metadata(src)
                        .context("reading source file permissions")?
                        .permissions();
                    fs::set_permissions(dest, perms)
                        .context("setting destination file permissions")?;
                }
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => return Ok(false),
            Err(e) => {
                return Err(anyhow::Error::from(e).context(format!("copying {}", src.display())));
            }
        }
    }
    Ok(true)
}

/// A leaf item (file or symlink) collected during the directory walk.
struct CopyLeaf {
    src: PathBuf,
    dest: PathBuf,
}

/// Copy a directory tree using reflink (COW) per file.
///
/// Walks the tree iteratively (no recursion), then copies all files and
/// symlinks in parallel on a dedicated 4-thread pool. Non-regular files
/// (sockets, FIFOs) are silently skipped. Existing entries at the destination
/// are skipped for idempotent usage.
///
/// When `force` is true, existing files and symlinks at the destination are
/// removed before copying.
///
/// Returns the number of files actually copied (excludes skipped entries).
pub fn copy_dir_recursive(src: &Path, dest: &Path, force: bool) -> anyhow::Result<usize> {
    // Phase 1: Walk directories iteratively, creating dest dirs and collecting leaves.
    let mut leaves = Vec::new();
    let mut dir_stack = vec![(src.to_path_buf(), dest.to_path_buf())];
    #[cfg(unix)]
    let mut dirs_for_perms: Vec<(PathBuf, PathBuf)> = Vec::new();

    while let Some((src_dir, dest_dir)) = dir_stack.pop() {
        fs::create_dir_all(&dest_dir)
            .with_context(|| format!("creating directory {}", dest_dir.display()))?;
        #[cfg(unix)]
        dirs_for_perms.push((src_dir.clone(), dest_dir.clone()));

        let entries: Vec<_> = fs::read_dir(&src_dir)?.collect::<Result<Vec<_>, _>>()?;
        for entry in entries {
            let file_type = entry.file_type()?;
            let src_path = entry.path();
            let dest_path = dest_dir.join(entry.file_name());

            if file_type.is_dir() {
                dir_stack.push((src_path, dest_path));
            } else if file_type.is_file() || file_type.is_symlink() {
                leaves.push(CopyLeaf {
                    src: src_path,
                    dest: dest_path,
                });
            } else {
                log::debug!("skipping non-regular file: {}", src_path.display());
            }
        }
    }

    // Phase 2: Copy all leaves in parallel.
    let copied = AtomicUsize::new(0);
    COPY_POOL.install(|| {
        leaves
            .par_iter()
            .try_for_each(|leaf| -> anyhow::Result<()> {
                if copy_leaf(&leaf.src, &leaf.dest, force)? {
                    copied.fetch_add(1, Ordering::Relaxed);
                }
                Ok(())
            })
    })?;

    // Phase 3: Preserve source directory permissions AFTER copying contents.
    // Must be done after copying — if the source lacks write permission (e.g., 0o555),
    // setting it before copying would make the destination read-only and fail the copies.
    #[cfg(unix)]
    for (src_dir, dest_dir) in &dirs_for_perms {
        let src_perms = fs::metadata(src_dir)
            .with_context(|| format!("reading permissions for {}", src_dir.display()))?
            .permissions();
        fs::set_permissions(dest_dir, src_perms)
            .with_context(|| format!("setting permissions on {}", dest_dir.display()))?;
    }

    Ok(copied.into_inner())
}

/// Remove a file, ignoring "not found" errors.
fn remove_if_exists(path: &Path) -> anyhow::Result<()> {
    if let Err(e) = fs::remove_file(path) {
        anyhow::ensure!(e.kind() == ErrorKind::NotFound, e);
    }
    Ok(())
}

/// Create a symlink, handling platform differences.
fn create_symlink(target: &Path, src_path: &Path, dest_path: &Path) -> anyhow::Result<()> {
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
    use super::*;

    #[test]
    fn test_remove_if_exists_nonexistent() {
        // NotFound is silently ignored
        assert!(remove_if_exists(Path::new("/nonexistent/file")).is_ok());
    }

    #[test]
    fn test_remove_if_exists_not_a_file() {
        // Trying to remove a directory with remove_file produces a non-NotFound error
        let dir = std::env::temp_dir();
        assert!(remove_if_exists(&dir).is_err());
    }
}
