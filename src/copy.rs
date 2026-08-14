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
//! [`crate::priority::lower_current_process`] before starting work.
//!
//! Every successful leaf copy calls `progress.record(bytes)` on the caller's
//! [`Progress`], which both feeds the TTY spinner (when enabled) and
//! accumulates the `(files, bytes)` totals the caller reads back via
//! [`Progress::totals`]. Non-interactive callers pass [`Progress::disabled`]
//! to skip the spinner; counting still happens.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Context;
use rayon::prelude::*;

use crate::path::{canonicalize_with_parents, format_path_for_display};
use crate::progress::Progress;

/// Capped at 4 threads to avoid saturating the CPU — the global rayon pool is
/// much larger (2× CPU cores, tuned for network I/O in `wt list`).
static COPY_POOL: LazyLock<rayon::ThreadPool> = LazyLock::new(|| {
    rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .expect("failed to build copy thread pool")
});

/// Copy a single file or symlink, using reflink (COW) when possible.
///
/// Detects symlinks via `symlink_metadata` on the source. Returns `Some(bytes)`
/// when the entry was copied (reporting the source's logical byte size), or
/// `None` if skipped because the destination already exists, or because the
/// source vanished after the caller's directory walk collected it (e.g. a
/// concurrent build deleting/replacing a build artifact). When `force` is
/// true, existing entries are removed before copying.
///
/// The vanished-source skip never costs the destination a file: the source is
/// stat'd before `force` removes anything, so a source that is already gone
/// leaves the destination untouched. A source that disappears in the narrower
/// window between that stat and the copy is an error only when `force` had
/// already removed an existing destination — silently reporting a skip would
/// leave a hole where a file used to be. Where there was nothing to remove,
/// that window is a skip like any other.
///
/// The vanished-source skips log at debug, so `wt -vv` explains a short file
/// count; the idempotent destination-already-exists skip stays silent.
///
/// When `root` is `Some`, refuses destination paths whose parent resolves
/// outside `root`. The check guards the parent chain, not the final leaf, so
/// leaf-symlink behavior is preserved (without `force` the symlink is skipped;
/// with `force` the symlink itself is replaced).
pub fn copy_leaf(
    src: &Path,
    dest: &Path,
    root: Option<&Path>,
    force: bool,
) -> anyhow::Result<Option<u64>> {
    if let Some(root) = root {
        ensure_path_within_root(dest.parent().unwrap_or(dest), root)?;
    }
    // Use symlink_metadata (not exists()) because exists() follows symlinks
    // and returns false for broken ones. Checked before the source is stat'd
    // so an idempotent re-run costs one syscall per already-present leaf.
    if !force && dest.symlink_metadata().is_ok() {
        return Ok(None);
    }
    // Stat the source before `force` removes the destination: the source can
    // vanish between the caller's directory walk and this copy — e.g. a
    // concurrent build rewriting `target/`. Skip rather than fail the whole
    // batch over one file that's no longer there, and skip without having
    // deleted the destination we can no longer replace.
    let src_meta = match src.symlink_metadata() {
        Ok(meta) => meta,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            tracing::debug!(path = %src.display(), "skipping vanished source: {}", src.display());
            return Ok(None);
        }
        Err(e) => {
            return Err(
                anyhow::Error::from(e).context(format!("reading metadata for {}", src.display()))
            );
        }
    };

    // Only a destination we actually removed can be left as a hole, so that —
    // not `force` — is what the late-window arms below guard on. A destination
    // re-created by a racing writer lands on the `AlreadyExists` arm (regular
    // files); for a symlink source `create_symlink` reports it as an error.
    let dest_removed = force && remove_if_exists(dest)?;

    let is_symlink = src_meta.file_type().is_symlink();
    let bytes = src_meta.len();

    if is_symlink {
        let target = match fs::read_link(src) {
            Ok(target) => target,
            // The source vanished after the stat above. If `force` removed a
            // destination, it is already gone, so this has to stay loud.
            Err(e) if e.kind() == ErrorKind::NotFound && !dest_removed => {
                tracing::debug!(path = %src.display(), "skipping vanished symlink source: {}", src.display());
                return Ok(None);
            }
            Err(e) => {
                return Err(
                    anyhow::Error::from(e).context(format!("reading symlink {}", src.display()))
                );
            }
        };
        create_symlink(&target, src, dest)?;
    } else {
        match reflink_copy::reflink_or_copy(src, dest) {
            Ok(_) => {
                // Preserve file permissions (especially the execute bit) —
                // needed on Linux, skipped on macOS.
                //
                // On btrfs/XFS, reflink (FICLONE ioctl) clones data extents
                // only — the destination gets umask-based permissions, losing
                // execute bits. std::fs::copy's fallback preserves permissions
                // via fchmod, creating an asymmetry in reflink_or_copy.
                //
                // On macOS/APFS, clonefile() already preserves the source's
                // mode bits, so this chmod is redundant — skip it to save a
                // syscall per file.
                //
                // Refs: ioctl_ficlonerange(2), LWN Articles/331808
                #[cfg(all(unix, not(target_os = "macos")))]
                {
                    fs::set_permissions(dest, src_meta.permissions())
                        .context("setting destination file permissions")?;
                }
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                tracing::debug!(path = %dest.display(), "skipping existing destination: {}", dest.display());
                return Ok(None);
            }
            // Same window as the `read_link` arm — and broader than a vanished
            // source, since `reflink_or_copy` also reports `NotFound` when the
            // destination's parent is missing. Both callers create the parent
            // first, so that case is latent; once `force` has removed a
            // destination it errors either way.
            Err(e) if e.kind() == ErrorKind::NotFound && !dest_removed => {
                tracing::debug!(path = %src.display(), "skipping vanished source: {}", src.display());
                return Ok(None);
            }
            Err(e) => {
                return Err(anyhow::Error::from(e).context(format!("copying {}", src.display())));
            }
        }
    }
    Ok(Some(bytes))
}

fn ensure_path_within_root(path: &Path, root: &Path) -> anyhow::Result<()> {
    let canonical_root = canonicalize_with_parents(root);
    let canonical_path = canonicalize_with_parents(path);

    anyhow::ensure!(
        canonical_path.starts_with(&canonical_root),
        "refusing to copy outside destination worktree: {} resolves outside {}",
        format_path_for_display(path),
        format_path_for_display(root)
    );

    Ok(())
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
/// (sockets, FIFOs) are skipped. Existing entries at the destination
/// are skipped for idempotent usage.
///
/// When `force` is true, existing files and symlinks at the destination are
/// removed before copying.
///
/// Sources that disappear mid-walk — a concurrent build rewriting the tree —
/// are skipped rather than failing the batch: a subdirectory that vanishes
/// before it is read is dropped along with its contents, an entry that vanishes
/// between listing and `file_type` is dropped, and a directory that vanishes
/// after its contents copy keeps the destination's default permissions instead
/// of the source's. Every such skip logs at debug (`wt -vv`). The tree's own
/// root is the exception: the caller named it, so its absence is an error.
///
/// Returns how many of the entries the walk collected to copy were not copied.
/// A pure copy ignores it; a caller that deletes the source afterwards must
/// refuse on a non-zero count, since the source still holds content the
/// destination never received. A non-regular file is dropped at classification
/// rather than counted: it carries no content the destination can be short of,
/// so refusing over one would cost a caller the whole operation to protect
/// nothing. Zero does not prove the destination holds everything the source
/// does — a file created after `read_dir` snapshots its directory is never seen,
/// and copy-then-delete cannot close that without a rename.
///
/// Each copied leaf is recorded on `progress` —
/// skipped entries are not counted — so a `progress` dedicated to this call
/// reports `(files_copied, bytes_copied)` in `totals()` afterwards; a shared
/// one accumulates across calls.
///
/// When `root` is `Some`, refuses destination directory ancestry that resolves
/// outside `root`. Leaves inherit the guarantee because `entry.file_name()` is
/// a single basename and cannot escape the validated parent directory.
#[must_use = "a caller that deletes the source must refuse on a non-zero skip count"]
pub fn copy_dir_recursive(
    src: &Path,
    dest: &Path,
    root: Option<&Path>,
    force: bool,
    progress: &Progress,
) -> anyhow::Result<usize> {
    // Phase 1: Walk directories iteratively, creating dest dirs and collecting leaves.
    let mut skipped = 0usize;
    let mut leaves = Vec::new();
    // The bool marks the tree's own root: the caller named it, so its absence
    // is a real error, while a subdirectory that disappears mid-walk is the
    // same concurrent-build race `copy_leaf` skips over.
    let mut dir_stack = vec![(src.to_path_buf(), dest.to_path_buf(), true)];
    #[cfg(unix)]
    let mut dirs_for_perms: Vec<(PathBuf, PathBuf)> = Vec::new();

    while let Some((src_dir, dest_dir, is_root)) = dir_stack.pop() {
        if let Some(root) = root {
            ensure_path_within_root(&dest_dir, root)?;
        }

        // Read the source before creating the destination directory, so a
        // subtree that vanished leaves no empty directory behind.
        let entries = match fs::read_dir(&src_dir).and_then(|it| it.collect::<Result<Vec<_>, _>>())
        {
            Ok(entries) => entries,
            Err(e) if e.kind() == ErrorKind::NotFound && !is_root => {
                tracing::debug!(path = %src_dir.display(), "skipping vanished directory: {}", src_dir.display());
                skipped += 1;
                continue;
            }
            Err(e) => {
                return Err(anyhow::Error::from(e)
                    .context(format!("reading directory {}", src_dir.display())));
            }
        };

        fs::create_dir_all(&dest_dir)
            .with_context(|| format!("creating directory {}", dest_dir.display()))?;
        #[cfg(unix)]
        dirs_for_perms.push((src_dir.clone(), dest_dir.clone()));

        for entry in entries {
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                // The entry was listed but is gone already — same race.
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    tracing::debug!(path = %entry.path().display(), "skipping vanished entry: {}", entry.path().display());
                    skipped += 1;
                    continue;
                }
                Err(e) => {
                    return Err(anyhow::Error::from(e)
                        .context(format!("reading metadata for {}", entry.path().display())));
                }
            };
            let src_path = entry.path();
            let dest_path = dest_dir.join(entry.file_name());

            if file_type.is_dir() {
                dir_stack.push((src_path, dest_path, false));
            } else if file_type.is_file() || file_type.is_symlink() {
                leaves.push(CopyLeaf {
                    src: src_path,
                    dest: dest_path,
                });
            } else {
                tracing::debug!(path = %src_path.display(), "skipping non-regular file: {}", src_path.display());
            }
        }
    }

    // Phase 2: Copy all leaves in parallel.
    let skipped_leaves = AtomicUsize::new(0);
    COPY_POOL.install(|| {
        leaves
            .par_iter()
            .try_for_each(|leaf| -> anyhow::Result<()> {
                match copy_leaf(&leaf.src, &leaf.dest, None, force)? {
                    Some(bytes) => progress.record(bytes),
                    None => {
                        skipped_leaves.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Ok(())
            })
    })?;
    skipped += skipped_leaves.into_inner();

    // Phase 3: Preserve source directory permissions AFTER copying contents.
    // Must be done after copying — if the source lacks write permission (e.g., 0o555),
    // setting it before copying would make the destination read-only and fail the copies.
    #[cfg(unix)]
    for (src_dir, dest_dir) in &dirs_for_perms {
        let src_perms = match fs::metadata(src_dir) {
            Ok(meta) => meta.permissions(),
            // The source directory went away after its contents were copied.
            // The destination is already written; leave it with the default
            // permissions rather than failing a batch that otherwise succeeded.
            Err(e) if e.kind() == ErrorKind::NotFound => {
                tracing::debug!(path = %src_dir.display(), "skipping permissions for vanished directory: {}", src_dir.display());
                skipped += 1;
                continue;
            }
            Err(e) => {
                return Err(anyhow::Error::from(e)
                    .context(format!("reading permissions for {}", src_dir.display())));
            }
        };
        fs::set_permissions(dest_dir, src_perms)
            .with_context(|| format!("setting permissions on {}", dest_dir.display()))?;
    }

    Ok(skipped)
}

/// Remove a file, ignoring "not found" errors. Reports whether one was removed,
/// which is what tells `copy_leaf` whether a later skip would leave a hole.
fn remove_if_exists(path: &Path) -> anyhow::Result<bool> {
    if let Err(e) = fs::remove_file(path) {
        anyhow::ensure!(e.kind() == ErrorKind::NotFound, e);
        return Ok(false);
    }
    Ok(true)
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
        // NotFound is silently ignored, and reported as "nothing removed"
        assert!(!remove_if_exists(Path::new("/nonexistent/file")).unwrap());
    }

    #[test]
    fn test_remove_if_exists_reports_removal() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file");
        fs::write(&file, b"contents").unwrap();

        assert!(remove_if_exists(&file).unwrap());
        assert!(!file.exists());
    }

    #[test]
    fn test_remove_if_exists_not_a_file() {
        // Trying to remove a directory with remove_file produces a non-NotFound error
        let dir = std::env::temp_dir();
        assert!(remove_if_exists(&dir).is_err());
    }

    #[test]
    fn test_copy_leaf_skips_vanished_source() {
        // Simulates a source file that existed during the caller's directory
        // walk but is gone by copy time (e.g. a concurrent build rewriting
        // `target/`). Should be skipped, not treated as a fatal error.
        let dest_dir = tempfile::tempdir().unwrap();
        let src = dest_dir.path().join("does-not-exist");
        let dest = dest_dir.path().join("dest");

        let result = copy_leaf(&src, &dest, None, false).unwrap();

        assert_eq!(result, None);
        assert!(!dest.exists());
    }

    #[test]
    fn test_copy_leaf_force_keeps_destination_when_source_vanished() {
        // Under `force` the skip must not cost the destination its contents:
        // the source is stat'd before the destination is removed, so a source
        // that is already gone leaves the existing destination in place.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("does-not-exist");
        let dest = dir.path().join("dest");
        fs::write(&dest, b"pre-existing content").unwrap();

        let result = copy_leaf(&src, &dest, None, true).unwrap();

        assert_eq!(result, None);
        assert_eq!(fs::read(&dest).unwrap(), b"pre-existing content");
    }

    #[test]
    fn test_copy_leaf_skips_existing_destination() {
        // The destination check moved ahead of the source stat; without
        // `force` an existing destination still wins and keeps its contents.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dest = dir.path().join("dest");
        fs::write(&src, b"source content").unwrap();
        fs::write(&dest, b"pre-existing content").unwrap();

        let result = copy_leaf(&src, &dest, None, false).unwrap();

        assert_eq!(result, None);
        assert_eq!(fs::read(&dest).unwrap(), b"pre-existing content");
    }

    #[test]
    fn test_copy_leaf_force_replaces_existing_destination() {
        // The counterpart: with `force` the destination is removed and the
        // source copied over it, reporting the source's byte count.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dest = dir.path().join("dest");
        fs::write(&src, b"source content").unwrap();
        fs::write(&dest, b"pre-existing content").unwrap();

        let result = copy_leaf(&src, &dest, None, true).unwrap();

        assert_eq!(result, Some(b"source content".len() as u64));
        assert_eq!(fs::read(&dest).unwrap(), b"source content");
    }

    #[test]
    fn test_copy_dir_recursive_counts_skipped_existing_destination() {
        // The skipped count is what tells a caller that deletes the source
        // whether the destination received everything. An idempotent re-run
        // skips a leaf that is already there, and says so.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dest = dir.path().join("dest");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dest).unwrap();
        fs::write(src.join("a"), b"a").unwrap();
        fs::write(src.join("b"), b"b").unwrap();
        fs::write(dest.join("a"), b"pre-existing").unwrap();

        let skipped = copy_dir_recursive(&src, &dest, None, false, &Progress::disabled()).unwrap();

        assert_eq!(skipped, 1);
        assert_eq!(fs::read(dest.join("a")).unwrap(), b"pre-existing");
        assert_eq!(fs::read(dest.join("b")).unwrap(), b"b");
    }

    #[cfg(unix)]
    #[test]
    fn test_copy_dir_recursive_does_not_count_a_non_regular_file() {
        // A socket is dropped at classification, not counted: the count exists
        // so a caller that deletes the source can refuse, and a socket carries
        // no content that refusal would protect.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dest = dir.path().join("dest");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("regular"), b"content").unwrap();
        let listener = std::os::unix::net::UnixListener::bind(src.join("sock")).unwrap();
        drop(listener);
        // Closing the fd doesn't unlink, so the socket is still there to
        // classify. Asserted, so a zero count can't come from an empty fixture.
        assert!(src.join("sock").exists());

        let skipped = copy_dir_recursive(&src, &dest, None, true, &Progress::disabled()).unwrap();

        assert_eq!(skipped, 0);
        assert_eq!(fs::read(dest.join("regular")).unwrap(), b"content");
        assert!(!dest.join("sock").exists());
    }

    #[test]
    fn test_copy_dir_recursive_reports_no_skips_for_a_complete_copy() {
        // The control: a clean copy reports zero, which is what lets a move
        // treat any non-zero count as a reason to keep the source.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dest = dir.path().join("dest");
        fs::create_dir_all(src.join("nested")).unwrap();
        fs::write(src.join("a"), b"a").unwrap();
        fs::write(src.join("nested").join("b"), b"b").unwrap();

        let skipped = copy_dir_recursive(&src, &dest, None, true, &Progress::disabled()).unwrap();

        assert_eq!(skipped, 0);
        assert_eq!(fs::read(dest.join("nested").join("b")).unwrap(), b"b");
    }

    #[test]
    fn test_copy_dir_recursive_missing_source_root_errors() {
        // A subdirectory that vanishes mid-walk is skipped, but the root the
        // caller named is not — its absence is a real error, and nothing is
        // created at the destination.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("does-not-exist");
        let dest = dir.path().join("dest");

        let err = copy_dir_recursive(&src, &dest, None, false, &Progress::disabled()).unwrap_err();

        assert!(
            err.to_string().contains("reading directory"),
            "error should name the failing operation and path, got: {err:#}"
        );
        assert!(!dest.exists());
    }
}
