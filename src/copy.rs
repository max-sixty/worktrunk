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
//! Every successful leaf copy calls `progress.record(..)` on the caller's
//! [`Progress`], which both feeds the TTY spinner (when enabled) and
//! accumulates the `(files, bytes)` totals the caller reads back via
//! [`Progress::totals`]. Non-interactive callers pass [`Progress::disabled`]
//! to skip the spinner; counting still happens.
//!
//! Each record carries a [`DataCopy`] saying whether the filesystem shared the
//! source's extents or wrote the bytes out, so a caller can report what the
//! copy actually cost on disk. `reflink_or_copy` returns that for free: `None`
//! when the platform's clone syscall succeeded, `Some(bytes)` when it fell
//! through to a byte copy. The signal is exact per file but says only that a
//! clone did not happen, never why — an unsupported filesystem and a
//! cross-device copy are indistinguishable here.
//!
//! A caller can pass a `stamp` to give every copied regular file the same
//! modification time. Without one, a file keeps whatever time its copy left it:
//! APFS clones and Apple/Windows copies carry the source's, while Linux's
//! `FICLONE` and `fs::copy` fallback write the time of the copy. Since leaves copy
//! in parallel, that time orders files by thread scheduling.
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

/// Pins the reflink classification so snapshot tests can assert the summary
/// line: `1` reports every copied file as reflinked, `0` as written in full.
///
/// Whether a clone succeeds is a property of the filesystem under the test's
/// temp directory, and CI spans APFS, ext4, and NTFS — so without this seam no
/// single snapshot could hold on all three, and the branch a given run took
/// would be invisible. It overrides only the label; the copy itself still
/// attempts a reflink and behaves identically either way.
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

/// Clone when supported, otherwise use the platform's full-file copy.
///
/// Some filesystems answer `FICLONE` with `EPERM` rather than `EOPNOTSUPP`
/// (#4433); genuine permission problems still fail the copy. Every fallback
/// goes through [`copy_to_new_file`], using the native API for file content
/// such as Apple resource forks and Windows alternate data streams.
fn reflink_or_copy(src: &Path, dest: &Path) -> io::Result<Option<u64>> {
    fall_back_on_rejected_reflink(reflink_copy::reflink(src, dest), || {
        copy_to_new_file(src, dest)
    })
}

/// Reserve an absent destination, then copy with the platform's native API.
///
/// Reading the source first avoids creating a destination for a missing or
/// unreadable file. `create_new` refuses an occupied destination, including a
/// writer arriving after clone cleanup. Native copying keeps platform copy
/// behavior, including resource forks and Windows streams, without requiring
/// rename or hard-link support.
///
/// The reservation is closed before native copying reopens the pathname. A
/// failed copy removes the incomplete destination; cleanup failures are reported.
/// Like the clone crate's cleanup, copying and removal use the pathname: external
/// unlink/replacement after reservation is outside this guarantee. The caller
/// retains the source on failure.
fn copy_to_new_file(src: &Path, dest: &Path) -> io::Result<u64> {
    let source = fs::File::open(src)?;
    source.metadata()?;
    drop(source);
    let reservation = fs::File::options()
        .write(true)
        .create_new(true)
        .open(dest)?;
    drop(reservation);
    match fs::copy(src, dest) {
        Ok(bytes) => Ok(bytes),
        Err(error) => Err(match fs::remove_file(dest) {
            Ok(()) => error,
            Err(cleanup) => io::Error::other(format!(
                "{error}; also failed to remove incomplete copy {}: {cleanup}",
                dest.display()
            )),
        }),
    }
}

/// The decision half of [`reflink_or_copy`], split out so filesystem-specific
/// clone errors can exercise the same byte-copy contract on every runner.
fn fall_back_on_rejected_reflink(
    result: io::Result<()>,
    copy: impl FnOnce() -> io::Result<u64>,
) -> io::Result<Option<u64>> {
    match result {
        Ok(()) => Ok(None),
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::AlreadyExists) => Err(e),
        Err(e) => {
            tracing::debug!("reflink rejected ({e}), falling back to a byte copy");
            copy().map(Some)
        }
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
/// existing entries are removed before copying.
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

    let data = if is_symlink {
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
        DataCopy::Neither
    } else {
        match reflink_or_copy(src, dest) {
            Ok(fallback) => {
                if let Some(stamp) = stamp {
                    set_modified(dest, stamp);
                }
                FORCED_DATA_COPY.unwrap_or(classify_copy(fallback))
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
                return Err(anyhow::Error::from(e).context(format!(
                    "copying {} to {}",
                    src.display(),
                    dest.display()
                )));
            }
        }
    };
    Ok(Some((bytes, data)))
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
        const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
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
    fn test_reflink_outcomes_preserve_failures_and_classification() {
        // Genuine permission problems still fail the byte retry.
        let rejected = Err(io::Error::from(ErrorKind::PermissionDenied));
        let result = fall_back_on_rejected_reflink(rejected, || {
            Err(io::Error::from(ErrorKind::PermissionDenied))
        });
        assert_eq!(result.unwrap_err().kind(), ErrorKind::PermissionDenied);

        let retry = || -> io::Result<u64> { panic!("retried") };
        assert_eq!(fall_back_on_rejected_reflink(Ok(()), retry).unwrap(), None);
        for kind in [ErrorKind::NotFound, ErrorKind::AlreadyExists] {
            assert_eq!(
                fall_back_on_rejected_reflink(Err(io::Error::from(kind)), retry)
                    .unwrap_err()
                    .kind(),
                kind
            );
        }

        // The public clone wrapper can map a vanished source to InvalidInput
        // on non-Apple platforms. Its native copy must recover NotFound for
        // copy_leaf's vanished-source policy.
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing");
        let dest = temp.path().join("dest");
        assert_eq!(
            reflink_or_copy(&missing, &dest).unwrap_err().kind(),
            ErrorKind::NotFound
        );
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    #[test]
    fn test_rejected_reflinks_share_native_copy() {
        // Inject the filesystem-specific clone result; copying uses the same
        // native API as production, including its permissions and error paths.
        for kind in [
            ErrorKind::PermissionDenied,
            ErrorKind::Unsupported,
            ErrorKind::CrossesDevices,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let src = dir.path().join("src");
            let dest = dir.path().join("dest");
            let missing = dir.path().join("missing");
            let absent_dest = dir.path().join("absent-dest");
            fs::write(&src, "source").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&src, fs::Permissions::from_mode(0o751)).unwrap();
            }
            let result = fall_back_on_rejected_reflink(Err(io::Error::from(kind)), || {
                copy_to_new_file(&src, &dest)
            });
            assert_eq!(result.unwrap(), Some(6));
            assert_eq!(fs::read_to_string(&dest).unwrap(), "source");
            let raced = dir.path().join("raced");
            let result = fall_back_on_rejected_reflink(Err(io::Error::from(kind)), || {
                fs::write(&raced, "racer").unwrap();
                copy_to_new_file(&src, &raced)
            });
            assert_eq!(result.unwrap_err().kind(), ErrorKind::AlreadyExists);
            assert_eq!(fs::read_to_string(&raced).unwrap(), "racer");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    fs::metadata(&dest).unwrap().permissions().mode() & 0o777,
                    0o751
                );
            }
            let result = fall_back_on_rejected_reflink(Err(io::Error::from(kind)), || {
                copy_to_new_file(&missing, &absent_dest)
            });
            assert_eq!(result.unwrap_err().kind(), ErrorKind::NotFound);
            assert_eq!(fs::read_to_string(&src).unwrap(), "source");
            assert!(!absent_dest.exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_native_copy_failure_cleans_only_its_destination() {
        // A directory can be opened and stat'd, but the native file copy rejects
        // it after reservation. Its failure must not leave a partial file or
        // delete a destination which was already occupied.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("source-directory");
        let dest = dir.path().join("dest");
        fs::create_dir(&src).unwrap();
        assert_eq!(
            copy_to_new_file(&src, &dest).unwrap_err().kind(),
            ErrorKind::InvalidInput
        );
        assert!(!dest.exists());
        assert!(src.is_dir());
        fs::write(&dest, "existing content").unwrap();
        assert_eq!(
            copy_to_new_file(&src, &dest).unwrap_err().kind(),
            ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read_to_string(&dest).unwrap(), "existing content");
        assert!(src.is_dir());
    }

    #[cfg(windows)]
    #[test]
    fn test_reflink_fallback_preserves_readonly_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dest = dir.path().join("dest");
        fs::write(&src, "readonly content").unwrap();
        let writable = fs::metadata(&src).unwrap().permissions();
        let mut readonly = writable.clone();
        readonly.set_readonly(true);
        fs::set_permissions(&src, readonly).unwrap();
        let result = fall_back_on_rejected_reflink(
            Err(io::Error::from(ErrorKind::PermissionDenied)),
            || copy_to_new_file(&src, &dest),
        );
        assert_eq!(result.unwrap(), Some(16));
        assert!(fs::metadata(&dest).unwrap().permissions().readonly());
        assert_eq!(
            copy_to_new_file(&src, &dest).unwrap_err().kind(),
            ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read_to_string(&dest).unwrap(), "readonly content");
        fs::set_permissions(&src, writable.clone()).unwrap();
        fs::set_permissions(&dest, writable).unwrap();
    }

    #[cfg(any(target_os = "macos", windows))]
    #[test]
    fn test_reflink_fallback_preserves_named_file_content() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        #[cfg(target_os = "macos")]
        let named_content = |path: &Path| path.join("..namedfork/rsrc");
        #[cfg(windows)]
        let named_content = |path: &Path| {
            let mut stream = path.as_os_str().to_owned();
            stream.push(":worktrunk-content");
            PathBuf::from(stream)
        };
        fs::write(&src, "main content").unwrap();
        fs::write(named_content(&src), "named content").unwrap();
        for (index, kind) in [
            ErrorKind::PermissionDenied,
            ErrorKind::Unsupported,
            ErrorKind::CrossesDevices,
        ]
        .into_iter()
        .enumerate()
        {
            let dest = dir.path().join(format!("dest-{index}"));
            let result = fall_back_on_rejected_reflink(Err(io::Error::from(kind)), || {
                copy_to_new_file(&src, &dest)
            });
            assert_eq!(result.unwrap(), Some(12));
            assert_eq!(fs::read_to_string(&dest).unwrap(), "main content");
            assert_eq!(
                fs::read_to_string(named_content(&dest)).unwrap(),
                "named content"
            );
        }
        assert_eq!(
            fs::read_to_string(named_content(&src)).unwrap(),
            "named content"
        );
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

        let result = copy_leaf(&src, &dest, None, true, None).unwrap();

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

    #[cfg(unix)]
    #[test]
    fn test_copy_leaf_stamps_a_read_only_file() {
        // A read-only source copies to a read-only destination; setting its
        // time must not need write access to it.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dest = dir.path().join("dest");
        fs::write(&src, b"content").unwrap();
        fs::set_permissions(&src, fs::Permissions::from_mode(0o444)).unwrap();
        let stamp = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);

        copy_leaf(&src, &dest, None, false, Some(stamp)).unwrap();

        let meta = fs::metadata(&dest).unwrap();
        assert_eq!(meta.modified().unwrap(), stamp);
        assert_eq!(meta.permissions().mode() & 0o777, 0o444);
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
}
