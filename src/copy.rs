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
//! A leaf is prepared at an exclusively created sibling path, then published
//! by rename. No-force publication refuses an occupied name; filesystems
//! without exclusive rename use exclusive creation instead. Force replaces
//! only a completed leaf atomically. Failed copies never remove existing destination data.
//! Windows uses the final parent so files inherit ordinary direct-child ACLs.
//!
//! Callers that want low-priority I/O (e.g. `step_copy_ignored`) should call
//! [`crate::priority::lower_current_process`] before starting work.
//!
//! Every successful leaf copy calls `progress.record(..)` on the caller's
//! [`Progress`], which both feeds the TTY spinner (when enabled) and
//! accumulates the `(files, bytes)` totals the caller reads back via
//! [`Progress::totals`]. Non-interactive callers pass [`Progress::disabled`]
//! to skip the spinner; counting still happens.
//!
//! Each record classifies the explicit copy API: a successful clone is
//! [`DataCopy::Reflinked`], a regular copy is [`DataCopy::Written`]. Filesystems
//! may accelerate ordinary copies internally; their physical I/O is opaque.
//! Windows uses the standard CopyFile API, including its native ReFS acceleration.
//!
//! A caller can pass a `stamp` to give every copied regular file the same
//! modification time. Without one, a file keeps whatever time its copy left it:
//! APFS clones and Windows' `CopyFileEx` carry the source's, while Linux's
//! `FICLONE` and `fs::copy` fallback write the time of the copy — and since
//! leaves copy in parallel, that time orders files by thread scheduling.
//! Build tools read that order: cargo marks a crate dirty when a dependency's
//! output is newer than its own, so a `target/` copied that way recompiles a
//! scattering of crates (#4248). Stamping with a time taken before the copy
//! makes files within the tree compare equal and keeps them no older than a
//! checkout that preceded it; preserving source times instead would make
//! outputs older than freshly checked-out sources.

use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

use anyhow::Context;
use rayon::prelude::*;

use crate::path::{canonicalize_with_parents, format_path_for_display};
use crate::progress::{DataCopy, Progress};

#[cfg(windows)]
mod windows;

/// Pins the reflink classification so snapshot tests can assert the summary
/// line: `1` reports every copied file as reflinked, `0` as an ordinary copy.
///
/// Whether a clone succeeds is a property of the filesystem under the test's
/// temp directory, and CI spans APFS, ext4, and NTFS — so without this seam no
/// single snapshot could hold on all three, and the branch a given run took
/// would be invisible. It overrides only the classification, leaving each
/// platform's copy behavior unchanged.
static FORCED_DATA_COPY: LazyLock<Option<DataCopy>> =
    LazyLock::new(
        || match std::env::var("WORKTRUNK_TEST_REFLINK").as_deref() {
            Ok("1") => Some(DataCopy::Reflinked),
            Ok("0") => Some(DataCopy::Written),
            _ => None,
        },
    );

/// Read `reflink_or_copy`'s success value, which reports the *fallback*: `None`
/// is the platform's clone syscall having succeeded, `Some(bytes)` the byte copy
/// that stood in for it.
///
/// A named function rather than an inline `if`, because no filesystem exercises
/// both arms — a run on APFS never sees the fallback, one on ext4 never sees the
/// clone — so the mapping is pinned by a unit test rather than by whichever
/// runner happens to execute it.
fn classify_copy(fallback: Option<u64>) -> DataCopy {
    match fallback {
        None => DataCopy::Reflinked,
        Some(_) => DataCopy::Written,
    }
}

/// Each provider leaves creation and cleanup with an owner that can account
/// for partial output. macOS clonefile is atomic; Linux FICLONE works on our
/// owned FD. Windows' ordinary CopyFile API owns its native ReFS acceleration.
fn reflink_or_copy(src: &Path, dest: &Path) -> io::Result<Option<u64>> {
    #[cfg(target_os = "macos")]
    {
        fall_back_on_rejected_reflink(reflink_copy::reflink(src, dest).map(|()| None), || {
            copy_to_new_file(src, dest)
        })
    }
    #[cfg(target_os = "linux")]
    {
        let source = fs::File::open(src)?;
        let permissions = source.metadata()?.permissions();
        with_new_file(dest, |file| {
            match rustix::fs::ioctl_ficlone(&file, &source) {
                Ok(()) => {
                    file.set_permissions(permissions)?;
                    Ok(None)
                }
                Err(error) => {
                    tracing::debug!("reflink rejected ({error}), falling back to a byte copy");
                    drop(file);
                    fs::copy(src, dest).map(Some)
                }
            }
        })
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        copy_to_new_file(src, dest).map(Some)
    }
}

/// Exclusively claim a new leaf before the platform's truncating copy.
#[cfg(any(not(target_os = "linux"), test))]
fn copy_to_new_file(src: &Path, dest: &Path) -> io::Result<u64> {
    with_new_file(dest, |file| {
        drop(file);
        fs::copy(src, dest)
    })
}

/// Own an exclusively created path through preparation, reporting cleanup errors.
fn with_new_file<T>(dest: &Path, fill: impl FnOnce(fs::File) -> io::Result<T>) -> io::Result<T> {
    let file = fs::File::create_new(dest)?;
    let mut path = tempfile::TempPath::try_from_path(dest)?;
    match fill(file) {
        Ok(value) => {
            path.disable_cleanup(true);
            Ok(value)
        }
        Err(error) => Err(discard_path(path, error)),
    }
}

/// Remove only the owned leaf, reporting any failure rather than hiding it in Drop.
fn remove_owned_path(mut path: tempfile::TempPath) -> io::Result<()> {
    #[cfg(windows)]
    let result = windows::remove_leaf(&path);
    #[cfg(not(windows))]
    let result = fs::remove_file(&path);
    path.disable_cleanup(true);
    result
}

/// Cleanup failures remain errors even when publication lost to another writer.
fn discard_path(path: tempfile::TempPath, error: io::Error) -> io::Error {
    let display_path = format_path_for_display(&path);
    match remove_owned_path(path) {
        Ok(()) => error,
        Err(cleanup) => io::Error::other(format!(
            "{error}; also failed to remove created file @ {display_path}: {cleanup}",
        )),
    }
}

/// A missing source or occupied output is definitive; other clone failures
/// (including EPERM, #4433) can still allow an ordinary copy.
#[cfg(any(target_os = "macos", test))]
fn fall_back_on_rejected_reflink(
    result: io::Result<Option<u64>>,
    copy: impl FnOnce() -> io::Result<u64>,
) -> io::Result<Option<u64>> {
    match result {
        Err(error) if !matches!(error.kind(), ErrorKind::NotFound | ErrorKind::AlreadyExists) => {
            tracing::debug!("reflink rejected ({error}), falling back to a byte copy");
            copy().map(Some)
        }
        result => result,
    }
}

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
/// Detects symlinks via `symlink_metadata` on the source. Returns
/// `Some((bytes, data))` when the entry was copied — the source's logical byte
/// size, and whether the filesystem shared its extents — or `None` if skipped
/// because the destination already exists, or because the source vanished after
/// the caller's directory walk collected it (e.g. a concurrent build
/// deleting/replacing a build artifact). A symlink reports
/// [`DataCopy::Neither`]: its content is a path, so there are no extents either
/// to share or to write, and counting it on either side would make a tree full
/// of symlinks read as a partial reflink failure. When `force` is true,
/// existing entries are replaced only after the copy is complete.
///
/// Each leaf is prepared beside its destination. Publishing it
/// atomically replaces the destination with `force`, or refuses a destination
/// that another writer created without it. If exclusive rename is unsupported,
/// a new destination is created exclusively and can be visible during copying. Failed or vanished sources leave
/// the destination untouched; temporary contents are removed on return.
///
/// The vanished-source skips log at debug, so `wt -vv` explains a short file
/// count; the idempotent destination-already-exists skip stays silent.
///
/// When `root` is `Some`, refuses destination paths whose parent resolves
/// outside `root`. The check guards the parent chain, not the final leaf, so
/// leaf-symlink behavior is preserved (without `force` the symlink is skipped;
/// with `force` the symlink itself is replaced).
///
/// When `stamp` is `Some`, a copied regular file gets it as its modification
/// time (see the module docs). Symlinks keep theirs: `std` can't set a link's
/// own times without following it.
pub fn copy_leaf(
    src: &Path,
    dest: &Path,
    root: Option<&Path>,
    force: bool,
    stamp: Option<SystemTime>,
) -> anyhow::Result<Option<(u64, DataCopy)>> {
    if let Some(root) = root {
        ensure_path_within_root(dest.parent().unwrap_or(dest), root)?;
    }
    // Use symlink_metadata (not exists()) because exists() follows symlinks
    // and returns false for broken ones. Checked before the source is stat'd
    // so an idempotent re-run costs one syscall per already-present leaf.
    if !force && dest.symlink_metadata().is_ok() {
        return Ok(None);
    }
    // The source can vanish between the caller's walk and this copy, e.g. a
    // concurrent build rewriting target/. No destination is removed on a skip.
    let src_meta = match src.symlink_metadata() {
        Ok(meta) => meta,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            tracing::debug!(path = %src.display(), "skipping vanished source: {}", src.display());
            return Ok(None);
        }
        Err(e) => {
            return Err(anyhow::Error::from(e).context(format!(
                "reading metadata for {}",
                format_path_for_display(src)
            )));
        }
    };

    let result = PreparedLeaf::new(src, dest, &src_meta, stamp);
    let prepared = match result {
        Ok(prepared) => prepared,
        Err(error) => {
            // The dependency may rewrite a vanished source's error kind on
            // fallback. Inspect the source itself, rather than confusing a
            // missing destination parent with a vanished source.
            if src
                .symlink_metadata()
                .is_err_and(|error| error.kind() == ErrorKind::NotFound)
            {
                tracing::debug!(path = %src.display(), "skipping vanished source: {}", src.display());
                return Ok(None);
            }
            return Err(error);
        }
    };
    prepared.persist(dest, force)
}

/// An exclusively created sibling leaf owned until atomic publication.
struct PreparedLeaf {
    path: tempfile::TempPath,
    bytes: u64,
    data: DataCopy,
}

impl PreparedLeaf {
    fn new(
        src: &Path,
        dest: &Path,
        metadata: &fs::Metadata,
        stamp: Option<SystemTime>,
    ) -> anyhow::Result<Self> {
        let parent = dest
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        dest.file_name()
            .context("copy destination has no filename")?;
        let is_link = metadata.file_type().is_symlink();
        anyhow::ensure!(
            is_link || metadata.is_file(),
            "copy source is not a file or symlink: {}",
            format_path_for_display(src)
        );
        let target = if is_link {
            Some(
                fs::read_link(src)
                    .with_context(|| format!("reading symlink {}", format_path_for_display(src)))?,
            )
        } else {
            None
        };
        let leaf = tempfile::Builder::new()
            .prefix(".wt-copy-")
            .make_in(parent, |path| {
                if let Some(target) = &target {
                    create_symlink(target, src, path)?;
                    Ok(DataCopy::Neither)
                } else {
                    let fallback = reflink_or_copy(src, path)?;
                    Ok(FORCED_DATA_COPY.unwrap_or(classify_copy(fallback)))
                }
            })
            .with_context(|| format!("preparing copy @ {}", format_path_for_display(dest)))?;
        let (data, path) = leaf.into_parts();
        if !is_link && let Some(stamp) = stamp {
            set_modified(&path, stamp);
        }
        Ok(Self {
            path,
            bytes: metadata.len(),
            data,
        })
    }

    fn persist(self, dest: &Path, force: bool) -> anyhow::Result<Option<(u64, DataCopy)>> {
        match persist_leaf(self.path, dest, force) {
            Ok(PublishedLeaf::Renamed) => Ok(Some((self.bytes, self.data))),
            Ok(PublishedLeaf::Created { path, data }) => {
                let display_path = format_path_for_display(&path);
                remove_owned_path(path)
                    .with_context(|| format!("removing prepared copy @ {display_path}"))?;
                Ok(Some((self.bytes, data)))
            }
            Err(error) => {
                let error = discard_path(error.path, error.error);
                if !force && error.kind() == ErrorKind::AlreadyExists {
                    Ok(None)
                } else {
                    Err(anyhow::Error::from(error).context(format!(
                        "publishing copied file @ {}",
                        format_path_for_display(dest)
                    )))
                }
            }
        }
    }
}

enum PublishedLeaf {
    Renamed,
    Created {
        path: tempfile::TempPath,
        data: DataCopy,
    },
}

fn persist_leaf(
    path: tempfile::TempPath,
    dest: &Path,
    force: bool,
) -> Result<PublishedLeaf, tempfile::PathPersistError> {
    #[cfg(windows)]
    let result = {
        let mut path = path;
        match windows::publish_leaf(&path, dest, force) {
            Ok(()) => {
                path.disable_cleanup(true);
                Ok(())
            }
            Err(error) => Err(tempfile::PathPersistError { error, path }),
        }
    };
    #[cfg(not(windows))]
    let result = if force {
        path.persist(dest)
    } else {
        path.persist_noclobber(dest)
    };
    finish_publication(result, dest, force)
}

/// macOS ENOTSUP and EOPNOTSUPP are different errnos; std maps only the
/// latter to Unsupported, while exFAT exclusive rename returns ENOTSUP.
fn publication_unsupported(error: &io::Error) -> bool {
    #[cfg(unix)]
    if rustix::io::Errno::from_io_error(error) == Some(rustix::io::Errno::NOTSUP) {
        return true;
    }
    error.kind() == ErrorKind::Unsupported
}

/// exFAT and other filesystems can lack both exclusive rename and hard links.
/// Only an unsupported no-force publication falls back to exclusive creation;
/// the completed prepared source remains owned until that copy has finished.
fn finish_publication(
    result: Result<(), tempfile::PathPersistError>,
    dest: &Path,
    force: bool,
) -> Result<PublishedLeaf, tempfile::PathPersistError> {
    match result {
        Ok(()) => Ok(PublishedLeaf::Renamed),
        Err(mut error) if !force && publication_unsupported(&error.error) => {
            let created = (|| {
                let metadata = error.path.symlink_metadata()?;
                if metadata.file_type().is_symlink() {
                    create_symlink(&fs::read_link(&error.path)?, &error.path, dest)?;
                    Ok(DataCopy::Neither)
                } else {
                    let fallback = reflink_or_copy(&error.path, dest)?;
                    if let Ok(modified) = metadata.modified() {
                        set_modified(dest, modified);
                    }
                    Ok(FORCED_DATA_COPY.unwrap_or(classify_copy(fallback)))
                }
            })();
            match created {
                Ok(data) => Ok(PublishedLeaf::Created {
                    path: error.path,
                    data,
                }),
                Err(created) => {
                    error.error = created;
                    Err(error)
                }
            }
        }
        Err(error) => Err(error),
    }
}

/// Set a copied file's modification time, best-effort: the contents are already
/// in place, so a file we can't reopen keeps its copy-time mtime rather than
/// failing a batch that otherwise succeeded. Logged at debug, so `wt -vv` shows
/// it.
fn set_modified(path: &Path, stamp: SystemTime) {
    // Windows needs a handle with FILE_WRITE_ATTRIBUTES, which a read-only
    // open doesn't grant; Unix sets times on any descriptor the owner holds.
    #[cfg(windows)]
    let file = {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_WRITE_ATTRIBUTES;
        fs::OpenOptions::new()
            .access_mode(FILE_WRITE_ATTRIBUTES)
            .open(path)
    };
    #[cfg(not(windows))]
    let file = fs::File::open(path);

    if let Err(e) = file.and_then(|file| file.set_modified(stamp)) {
        tracing::debug!(path = %path.display(), "leaving copy-time mtime on {}: {e}", path.display());
    }
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
/// replaced atomically after copying.
///
/// Sources that disappear mid-walk — a concurrent build rewriting the tree —
/// are skipped rather than failing the batch: a subdirectory that vanishes
/// before it is read is dropped along with its contents, an entry that vanishes
/// between listing and `file_type` is dropped, and a directory that vanishes
/// after its contents copy keeps the destination's default permissions instead
/// of the source's. Every such skip logs at debug (`wt -vv`). The tree's own
/// root is the exception: the caller named it, so its absence is an error.
///
/// Returns how many entries were skipped during copying or permission restoration.
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
///
/// `stamp` is passed to each [`copy_leaf`].
#[must_use = "a caller that deletes the source must refuse on a non-zero skip count"]
pub fn copy_dir_recursive(
    src: &Path,
    dest: &Path,
    root: Option<&Path>,
    force: bool,
    stamp: Option<SystemTime>,
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
                return Err(anyhow::Error::from(e).context(format!(
                    "reading directory {}",
                    format_path_for_display(&src_dir)
                )));
            }
        };

        fs::create_dir_all(&dest_dir).with_context(|| {
            format!("creating directory {}", format_path_for_display(&dest_dir))
        })?;
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
                    return Err(anyhow::Error::from(e).context(format!(
                        "reading metadata for {}",
                        format_path_for_display(&entry.path())
                    )));
                }
            };
            let src_path = entry.path();
            let dest_path = dest_dir.join(entry.file_name());

            if file_type.is_dir() {
                dir_stack.push((src_path, dest_path, false));
            } else if file_type.is_file() || file_type.is_symlink() {
                // An idempotent copy needs no staging space, including in a
                // read-only destination. Publication still checks atomically
                // for a writer that arrives after this walk.
                if !force && dest_path.symlink_metadata().is_ok() {
                    skipped += 1;
                    continue;
                }
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
                match copy_leaf(&leaf.src, &leaf.dest, None, force, stamp)? {
                    Some((bytes, data)) => progress.record(bytes, data),
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
                return Err(anyhow::Error::from(e).context(format!(
                    "reading permissions for {}",
                    format_path_for_display(src_dir)
                )));
            }
        };
        fs::set_permissions(dest_dir, src_perms).with_context(|| {
            format!(
                "setting permissions on {}",
                format_path_for_display(dest_dir)
            )
        })?;
    }

    Ok(skipped)
}

/// Create a symlink, handling platform differences.
fn create_symlink(target: &Path, src_path: &Path, dest_path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let _ = src_path; // Used on Windows to determine symlink type
        std::os::unix::fs::symlink(target, dest_path)?;
    }
    #[cfg(windows)]
    {
        let is_dir = src_path.metadata().map(|m| m.is_dir()).unwrap_or(false);
        if is_dir {
            std::os::windows::fs::symlink_dir(target, dest_path)?;
        } else {
            std::os::windows::fs::symlink_file(target, dest_path)?;
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, src_path, dest_path);
        return Err(std::io::Error::new(
            ErrorKind::Unsupported,
            "symlink creation not supported on this platform",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_copy_leaf_skips_vanished_source() {
        // Simulates a source file that existed during the caller's directory
        // walk but is gone by copy time (e.g. a concurrent build rewriting
        // `target/`). Should be skipped, not treated as a fatal error.
        let dest_dir = tempfile::tempdir().unwrap();
        let src = dest_dir.path().join("does-not-exist");
        let dest = dest_dir.path().join("dest");

        let result = copy_leaf(&src, &dest, None, false, None).unwrap();

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

        let result = copy_leaf(&src, &dest, None, true, None).unwrap();

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

        let result = copy_leaf(&src, &dest, None, false, None).unwrap();

        assert_eq!(result, None);
        assert_eq!(fs::read(&dest).unwrap(), b"pre-existing content");
    }

    #[test]
    fn test_classify_copy_reads_the_fallback() {
        // `reflink_or_copy` reports what it fell back to, so the absent value
        // is the successful clone. Asserted here because a filesystem only ever
        // produces one of the two.
        assert_eq!(classify_copy(None), DataCopy::Reflinked);
        assert_eq!(classify_copy(Some(4096)), DataCopy::Written);
    }

    #[test]
    fn test_reflink_rejected_with_eperm_falls_back_to_copy() {
        // Some filesystems reject FICLONE with EPERM rather than EOPNOTSUPP
        // (#4433); a byte copy can still succeed.
        let rejected = Err(io::Error::from(ErrorKind::PermissionDenied));
        let result = fall_back_on_rejected_reflink(rejected, || Ok(42));
        assert_eq!(result.unwrap(), Some(42));

        // A real permission problem fails the retry too, and is reported.
        let rejected = Err(io::Error::from(ErrorKind::PermissionDenied));
        let result = fall_back_on_rejected_reflink(rejected, || {
            Err(io::Error::from(ErrorKind::PermissionDenied))
        });
        assert_eq!(result.unwrap_err().kind(), ErrorKind::PermissionDenied);

        // Other outcomes pass through without a retry.
        let retry = || -> io::Result<u64> { panic!("retried") };
        assert_eq!(
            fall_back_on_rejected_reflink(Ok(None), retry).unwrap(),
            None
        );
        let exists = Err(io::Error::from(ErrorKind::AlreadyExists));
        assert_eq!(
            fall_back_on_rejected_reflink(exists, retry)
                .unwrap_err()
                .kind(),
            ErrorKind::AlreadyExists
        );
    }

    #[test]
    fn test_rejected_reflink_fallback_does_not_overwrite_racing_destination() {
        // Exclusive fallback creation preserves a name occupied before the byte copy.
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("src");
        let dest = temp.path().join("dest");
        fs::write(&src, "source").unwrap();

        let rejected = Err(io::Error::from(ErrorKind::PermissionDenied));
        let result = fall_back_on_rejected_reflink(rejected, || {
            fs::write(&dest, "racer").unwrap();
            copy_to_new_file(&src, &dest)
        });
        assert_eq!(result.unwrap_err().kind(), ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(&dest).unwrap(), "racer");
        // No temporary output was created.
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2);
    }

    #[test]
    fn test_copy_to_new_file_copies_contents_and_mode() {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("script.sh");
        let dest = temp.path().join("copy.sh");
        fs::write(&src, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&src, fs::Permissions::from_mode(0o751)).unwrap();
        }

        assert_eq!(copy_to_new_file(&src, &dest).unwrap(), 10);
        assert_eq!(fs::read_to_string(&dest).unwrap(), "#!/bin/sh\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&dest).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o751);
        }

        // A missing source creates nothing.
        let missing = temp.path().join("missing");
        let dest = temp.path().join("never");
        assert_eq!(
            copy_to_new_file(&missing, &dest).unwrap_err().kind(),
            ErrorKind::NotFound
        );
        assert!(dest.symlink_metadata().is_err());

        // A failed platform copy leaves neither a destination nor its temporary file.
        #[cfg(unix)]
        {
            let src_dir = temp.path().join("dir");
            fs::create_dir(&src_dir).unwrap();
            let out = tempfile::tempdir().unwrap();
            let dest = out.path().join("partial");
            assert!(copy_to_new_file(&src_dir, &dest).is_err());
            assert_eq!(fs::read_dir(out.path()).unwrap().count(), 0);
        }
    }

    /// Cleanup after a failed copy only touches the temp file it created, so a
    /// file another process puts at the destination during the copy survives.
    #[cfg(unix)]
    #[test]
    fn test_failed_copy_cleanup_leaves_racing_destination() {
        let temp = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let dest = out.path().join("dest");
        fs::write(&dest, "racer").unwrap();

        assert!(copy_to_new_file(temp.path(), &dest).is_err());
        assert_eq!(fs::read_to_string(&dest).unwrap(), "racer");
        assert_eq!(fs::read_dir(out.path()).unwrap().count(), 1);
    }

    /// Unsupported exclusive publication preserves a file's content/time and
    /// a dangling symlink's relative target, refuses competing destinations,
    /// and leaves forced publication errors as failures.
    #[test]
    fn test_unsupported_publication_creates_exclusively() {
        let dir = crate::testing::test_tempdir();
        let dest = dir.path().join("destination");
        let stamp = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_735_776_000);
        #[cfg(unix)]
        let source_types = [false, true];
        #[cfg(not(unix))]
        let source_types = [false];
        for source_link in source_types {
            for (force, occupied) in [(false, false), (false, true), (true, false)] {
                let temp = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
                let temp = if source_link {
                    let path = temp.into_temp_path();
                    fs::remove_file(&path).unwrap();
                    create_symlink(Path::new("missing relative target"), &path, &path).unwrap();
                    path
                } else {
                    fs::write(temp.path(), b"prepared bytes").unwrap();
                    set_modified(temp.path(), stamp);
                    temp.into_temp_path()
                };
                let prepared_path = temp.to_path_buf();
                if occupied {
                    fs::write(&dest, b"racing bytes").unwrap();
                }
                #[cfg(unix)]
                let unsupported = io::Error::from(rustix::io::Errno::NOTSUP);
                #[cfg(not(unix))]
                let unsupported = io::Error::from(ErrorKind::Unsupported);
                let unsupported_kind = unsupported.kind();
                let result = finish_publication(
                    Err(tempfile::PathPersistError {
                        error: unsupported,
                        path: temp,
                    }),
                    &dest,
                    force,
                );
                match result {
                    Ok(PublishedLeaf::Created { path, data }) => {
                        assert!(!force && !occupied);
                        if source_link {
                            assert_eq!(data, DataCopy::Neither);
                            assert_eq!(
                                fs::read_link(&dest).unwrap(),
                                Path::new("missing relative target")
                            );
                        } else {
                            assert_ne!(data, DataCopy::Neither);
                            assert_eq!(fs::read(&dest).unwrap(), b"prepared bytes");
                            assert_eq!(fs::metadata(&dest).unwrap().modified().unwrap(), stamp);
                        }
                        remove_owned_path(path).unwrap();
                        fs::remove_file(&dest).unwrap();
                    }
                    Err(error) => {
                        assert_eq!(
                            error.error.kind(),
                            if force {
                                unsupported_kind
                            } else {
                                ErrorKind::AlreadyExists
                            }
                        );
                        remove_owned_path(error.path).unwrap();
                        if occupied {
                            assert_eq!(fs::read(&dest).unwrap(), b"racing bytes");
                            fs::remove_file(&dest).unwrap();
                        } else {
                            assert!(!dest.exists());
                        }
                    }
                    Ok(PublishedLeaf::Renamed) => panic!("unsupported rename succeeded"),
                }
                assert!(prepared_path.symlink_metadata().is_err());
            }
        }
    }

    /// Failed cloning or metadata preparation can leave bytes in the owned
    /// output; cleanup must cover that partial file before returning the error.
    #[test]
    fn test_partial_preparation_failure_cleans_owned_file() {
        use std::io::Write;
        let dir = crate::testing::test_tempdir();
        let dest = dir.path().join("partial");
        let result: io::Result<()> = with_new_file(&dest, |mut file| {
            file.write_all(b"partial bytes")?;
            Err(io::Error::from(ErrorKind::PermissionDenied))
        });
        assert_eq!(result.unwrap_err().kind(), ErrorKind::PermissionDenied);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn test_discard_path_reports_cleanup_failure() {
        let dir = tempfile::tempdir().unwrap();

        let temp = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
        let err = discard_path(
            temp.into_temp_path(),
            io::Error::from(ErrorKind::AlreadyExists),
        );
        assert_eq!(err.kind(), ErrorKind::AlreadyExists);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);

        // The temp file is already gone, so removing it fails. Unix only:
        // Windows can't reliably delete a file another handle holds open.
        #[cfg(unix)]
        {
            let temp = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
            fs::remove_file(temp.path()).unwrap();
            let err = discard_path(
                temp.into_temp_path(),
                io::Error::from(ErrorKind::AlreadyExists),
            );
            assert!(
                err.to_string()
                    .contains("also failed to remove created file"),
                "{err}"
            );
        }
    }

    #[test]
    fn test_copy_leaf_force_replaces_existing_destination() {
        // Force publishes the complete source even over a read-only file.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dest = dir.path().join("dest");
        fs::write(&src, b"source content").unwrap();
        fs::write(&dest, b"pre-existing content").unwrap();
        let original_permissions = fs::metadata(&dest).unwrap().permissions();
        let mut readonly = original_permissions.clone();
        readonly.set_readonly(true);
        fs::set_permissions(&dest, readonly).unwrap();

        // Preparation failure must preserve even a read-only destination.
        let directory = dir.path().join("uncopyable directory");
        fs::create_dir(&directory).unwrap();
        let failed = copy_leaf(&directory, &dest, None, true, None);
        let retained = fs::read(&dest).unwrap();
        let retained_readonly = fs::metadata(&dest).unwrap().permissions().readonly();
        let result = copy_leaf(&src, &dest, None, true, None);
        // Restore before assertions so Windows can clean up on regression.
        fs::set_permissions(&dest, original_permissions).unwrap();

        assert!(failed.is_err());
        assert_eq!(retained, b"pre-existing content");
        assert!(retained_readonly);
        let result = result.unwrap();

        // Only the byte half is asserted: whether the copy reflinked depends on
        // the filesystem under the temp dir, which varies across CI runners.
        assert_eq!(
            result.map(|(bytes, _)| bytes),
            Some(b"source content".len() as u64)
        );
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

        let skipped =
            copy_dir_recursive(&src, &dest, None, false, None, &Progress::disabled()).unwrap();

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

        let skipped =
            copy_dir_recursive(&src, &dest, None, true, None, &Progress::disabled()).unwrap();

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

        let skipped =
            copy_dir_recursive(&src, &dest, None, true, None, &Progress::disabled()).unwrap();

        assert_eq!(skipped, 0);
        assert_eq!(fs::read(dest.join("nested").join("b")).unwrap(), b"b");
    }

    #[test]
    fn test_copy_dir_recursive_copies_a_tree_deeper_than_windows_max_path() {
        // A `node_modules` tree nests deep enough to push the destination past
        // Windows' 260-character path limit. The containment check canonicalizes
        // both ends, and used to get one spelling for the long destination and
        // another for its short root — so the copy was refused as "outside" a
        // worktree it was inside (#3898). Off Windows the depth is unremarkable
        // and this just exercises a deep tree.
        let dir = tempfile::tempdir().unwrap();
        let src_root = dir.path().join("src");
        let dest_root = dir.path().join("dest");

        let mut relative = PathBuf::from("node_modules");
        while dest_root.join(&relative).as_os_str().len() < 320 {
            relative.push("nested-dependency-directory");
        }
        let deep_src = src_root.join(&relative);
        fs::create_dir_all(&deep_src).unwrap();
        // The real destination root is an existing worktree.
        fs::create_dir_all(&dest_root).unwrap();
        fs::write(deep_src.join("payload.txt"), b"deep file").unwrap();

        let skipped = copy_dir_recursive(
            &src_root.join("node_modules"),
            &dest_root.join("node_modules"),
            Some(&dest_root),
            false,
            None,
            &Progress::disabled(),
        )
        .expect("a tree inside the destination root should copy");

        assert_eq!(skipped, 0);
        assert_eq!(
            fs::read(dest_root.join(&relative).join("payload.txt")).unwrap(),
            b"deep file"
        );
    }

    #[test]
    fn test_copy_dir_recursive_stamps_every_file_with_one_mtime() {
        // A copy that leaves each file its own write time orders files by
        // thread scheduling, which cargo reads as a dependency newer than its
        // dependent (#4248). With a stamp, every copied file carries it —
        // whatever the source's mtimes were.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dest = dir.path().join("dest");
        fs::create_dir_all(src.join("nested")).unwrap();
        let old = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_500_000_000);
        for (name, offset) in [("a", 0), ("nested/b", 7), ("nested/c", 3)] {
            let path = src.join(name);
            fs::write(&path, name).unwrap();
            fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(old + std::time::Duration::from_secs(offset))
                .unwrap();
        }
        let stamp = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);

        let skipped =
            copy_dir_recursive(&src, &dest, None, false, Some(stamp), &Progress::disabled())
                .unwrap();

        assert_eq!(skipped, 0);
        for name in ["a", "nested/b", "nested/c"] {
            let mtime = fs::metadata(dest.join(name)).unwrap().modified().unwrap();
            assert_eq!(mtime, stamp, "{name} should carry the stamp");
        }
    }

    #[test]
    fn test_copy_leaf_stamps_a_read_only_file() {
        // A read-only source copies to a read-only destination; setting its
        // time must not need write access to it.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dest = dir.path().join("dest");
        fs::write(&src, b"content").unwrap();
        let original_permissions = fs::metadata(&src).unwrap().permissions();
        let mut readonly = original_permissions.clone();
        readonly.set_readonly(true);
        fs::set_permissions(&src, readonly).unwrap();
        let stamp = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);

        copy_leaf(&src, &dest, None, false, Some(stamp)).unwrap();

        let meta = fs::metadata(&dest).unwrap();
        // Restore before assertions so Windows can remove both files even if
        // the copied mode or timestamp regresses.
        fs::set_permissions(&src, original_permissions.clone()).unwrap();
        fs::set_permissions(&dest, original_permissions).unwrap();
        assert_eq!(meta.modified().unwrap(), stamp);
        assert!(meta.permissions().readonly());
    }

    #[test]
    fn test_copy_dir_recursive_missing_source_root_errors() {
        // A subdirectory that vanishes mid-walk is skipped, but the root the
        // caller named is not — its absence is a real error, and nothing is
        // created at the destination.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("does-not-exist");
        let dest = dir.path().join("dest");

        let err =
            copy_dir_recursive(&src, &dest, None, false, None, &Progress::disabled()).unwrap_err();

        assert!(
            err.to_string().contains("reading directory"),
            "error should name the failing operation and path, got: {err:#}"
        );
        assert!(!dest.exists());
    }

    /// Failed filesystem operations retain their cause and quote paths that
    /// contain spaces, including failures before any data can be copied.
    #[cfg(unix)]
    #[test]
    fn test_copy_errors_quote_paths_and_retain_io_causes() {
        let dir = crate::testing::test_tempdir();
        let blocker = dir.path().join("file as parent");
        fs::write(&blocker, b"existing data").unwrap();
        let invalid_source = blocker.join("child");
        let dest = dir.path().join("destination");
        let metadata_error = copy_leaf(&invalid_source, &dest, None, false, None).unwrap_err();
        assert!(metadata_error.downcast_ref::<std::io::Error>().is_some());
        assert_eq!(
            metadata_error.to_string(),
            format!("reading metadata for '{}'", invalid_source.display())
        );
        assert!(!dest.exists());

        let src = dir.path().join("source");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("data"), b"data").unwrap();
        let invalid_dest = blocker.join("destination");
        let directory_error = copy_dir_recursive(
            &src,
            &invalid_dest,
            None,
            false,
            None,
            &Progress::disabled(),
        )
        .unwrap_err();
        assert!(directory_error.downcast_ref::<std::io::Error>().is_some());
        assert_eq!(
            directory_error.to_string(),
            format!("creating directory '{}'", invalid_dest.display())
        );

        let source_file = src.join("source file");
        fs::write(&source_file, b"source data").unwrap();
        let copy_error = copy_leaf(&source_file, &invalid_dest, None, false, None).unwrap_err();
        assert!(copy_error.downcast_ref::<std::io::Error>().is_some());
        assert_eq!(
            copy_error.to_string(),
            format!("preparing copy @ '{}'", invalid_dest.display())
        );
        assert_eq!(fs::read(&source_file).unwrap(), b"source data");

        let source_link = src.join("source link");
        std::os::unix::fs::symlink(&source_file, &source_link).unwrap();
        let link_error = copy_leaf(&source_link, &invalid_dest, None, false, None).unwrap_err();
        assert!(link_error.downcast_ref::<std::io::Error>().is_some());
        assert_eq!(
            link_error.to_string(),
            format!("preparing copy @ '{}'", invalid_dest.display())
        );
        assert_eq!(fs::read_link(&source_link).unwrap(), source_file);
        assert_eq!(fs::read(&blocker).unwrap(), b"existing data");
    }

    /// Failed publication keeps the source and an occupied destination directory.
    #[test]
    fn test_copy_publication_failure_keeps_directory() {
        use path_slash::PathExt;

        for recursive in [false, true] {
            let dir = crate::testing::test_tempdir();
            let src = dir.path().join("source tree");
            let dest = dir.path().join("destination tree");
            let src_file = src.join("blocked file");
            let occupied = dest.join("blocked file");
            fs::create_dir(&src).unwrap();
            fs::create_dir_all(&occupied).unwrap();
            fs::write(&src_file, b"source data").unwrap();
            fs::write(occupied.join("protected"), b"existing data").unwrap();

            let error = if recursive {
                copy_dir_recursive(&src, &dest, None, true, None, &Progress::disabled())
                    .unwrap_err()
            } else {
                copy_leaf(&src_file, &occupied, None, true, None).unwrap_err()
            };

            assert!(error.downcast_ref::<std::io::Error>().is_some());
            assert_eq!(
                error.to_string(),
                format!("publishing copied file @ '{}'", occupied.to_slash_lossy())
            );
            assert_eq!(fs::read(&src_file).unwrap(), b"source data");
            assert_eq!(
                fs::read(occupied.join("protected")).unwrap(),
                b"existing data"
            );
            let names: Vec<_> = fs::read_dir(&dest)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect();
            assert_eq!(names, ["blocked file"]);
        }
    }

    /// A destination failure is an error, while a failed forced copy leaves
    /// the existing destination intact.
    #[cfg(unix)]
    #[test]
    fn test_copy_failure_keeps_existing_data() {
        let dir = crate::testing::test_tempdir();
        let src = dir.path().join("source file");
        fs::write(&src, b"source data").unwrap();
        let missing_parent = dir.path().join("missing parent/destination");
        assert!(copy_leaf(&src, &missing_parent, None, false, None).is_err());
        assert_eq!(fs::read(&src).unwrap(), b"source data");

        let dest = dir.path().join("existing destination");
        fs::write(&dest, b"existing data").unwrap();
        let uncopyable = dir.path().join("uncopyable directory");
        fs::create_dir(&uncopyable).unwrap();
        let result = copy_leaf(&uncopyable, &dest, None, true, None);
        assert!(result.is_err());
        assert_eq!(fs::read(&dest).unwrap(), b"existing data");
        assert_eq!(fs::read(&src).unwrap(), b"source data");

        let protected = dir.path().join("protected");
        fs::write(&protected, b"protected data").unwrap();
        let link = dir.path().join("existing link");
        std::os::unix::fs::symlink(&protected, &link).unwrap();
        let result = copy_leaf(&uncopyable, &link, None, true, None);
        assert!(result.is_err());
        assert_eq!(fs::read_link(&link).unwrap(), protected);
        assert_eq!(fs::read(&link).unwrap(), b"protected data");
        assert!(fs::read_dir(dir.path()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".wt-copy-")
        }));
    }

    #[cfg(windows)]
    #[test]
    fn test_copy_leaf_publishes_beyond_windows_max_path() {
        let dir = crate::testing::test_tempdir();
        let src = dir.path().join("source");
        fs::write(&src, b"source data").unwrap();
        let mut parent = dir.path().join("destination");
        for _ in 0..8 {
            parent.push("a directory with a long name");
        }
        fs::create_dir_all(&parent).unwrap();
        let dest = parent.join("copied file");
        assert!(dest.as_os_str().len() > 260);

        assert!(copy_leaf(&src, &dest, None, false, None).unwrap().is_some());
        assert_eq!(fs::read(&dest).unwrap(), b"source data");
        assert!(fs::read_dir(&parent).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".wt-copy-")
        }));
    }

    /// Losing publication preserves the later writer and cleans a read-only
    /// prepared file on every platform, including Windows.
    #[test]
    fn test_prepared_readonly_copy_preserves_competing_file() {
        let dir = crate::testing::test_tempdir();
        let src = dir.path().join("source");
        let dest = dir.path().join("destination");
        fs::write(&src, b"source data").unwrap();
        let original_permissions = fs::metadata(&src).unwrap().permissions();
        let mut readonly = original_permissions.clone();
        readonly.set_readonly(true);
        fs::set_permissions(&src, readonly).unwrap();
        let prepared =
            PreparedLeaf::new(&src, &dest, &src.symlink_metadata().unwrap(), None).unwrap();
        let staging = prepared.path.to_path_buf();
        fs::write(&dest, b"competing data").unwrap();

        let result = prepared.persist(&dest, false);
        fs::set_permissions(&src, original_permissions).unwrap();

        assert!(result.unwrap().is_none());
        assert_eq!(fs::read(&dest).unwrap(), b"competing data");
        assert_eq!(fs::read(&src).unwrap(), b"source data");
        assert!(
            !staging.exists(),
            "failed publication must clean readonly staging"
        );
    }

    /// Preparation never grants permission to overwrite a later writer's
    /// leaf, including symlinks whose targets must not be touched by fallback.
    #[cfg(unix)]
    #[test]
    fn test_prepared_copy_preserves_competing_destinations() {
        use std::os::unix::fs::symlink;

        for source_link in [false, true] {
            for destination_link in [false, true] {
                let dir = crate::testing::test_tempdir();
                let src = dir.path().join("source");
                if source_link {
                    symlink("missing relative target", &src).unwrap();
                } else {
                    fs::write(&src, b"source data").unwrap();
                }
                let dest = dir.path().join("destination");
                let protected = dir.path().join("protected");
                fs::write(&protected, b"existing data").unwrap();
                let prepared =
                    PreparedLeaf::new(&src, &dest, &src.symlink_metadata().unwrap(), None).unwrap();
                let staging = prepared.path.to_path_buf();
                // A real writer occupies the final name after preparation.
                if destination_link {
                    symlink(&protected, &dest).unwrap();
                } else {
                    fs::write(&dest, b"existing data").unwrap();
                }
                assert!(prepared.persist(&dest, false).unwrap().is_none());
                assert_eq!(fs::read(&dest).unwrap(), b"existing data");
                assert_eq!(fs::read(&protected).unwrap(), b"existing data");
                assert_eq!(
                    dest.symlink_metadata().unwrap().is_symlink(),
                    destination_link
                );
                assert!(!staging.exists());
            }
        }
    }

    /// Forced publication replaces the leaf rather than following it, and
    /// preserves a relative dangling link when that is the source.
    #[cfg(unix)]
    #[test]
    fn test_forced_copy_replaces_symlink_without_following_it() {
        use std::os::unix::fs::symlink;

        let dir = crate::testing::test_tempdir();
        let src = dir.path().join("source");
        let protected = dir.path().join("protected");
        let dest = dir.path().join("destination");
        fs::write(&src, b"source data").unwrap();
        fs::write(&protected, b"existing data").unwrap();
        symlink(&protected, &dest).unwrap();
        assert!(copy_leaf(&src, &dest, None, true, None).unwrap().is_some());
        assert!(!dest.symlink_metadata().unwrap().is_symlink());
        assert_eq!(fs::read(&dest).unwrap(), b"source data");
        assert_eq!(fs::read(&protected).unwrap(), b"existing data");

        let link = dir.path().join("broken source");
        symlink("missing relative target", &link).unwrap();
        assert!(copy_leaf(&link, &dest, None, true, None).unwrap().is_some());
        assert_eq!(
            fs::read_link(&dest).unwrap(),
            Path::new("missing relative target")
        );
        assert_eq!(fs::read(&protected).unwrap(), b"existing data");
        let mut names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        assert_eq!(
            names,
            ["broken source", "destination", "protected", "source"]
        );
    }
}
