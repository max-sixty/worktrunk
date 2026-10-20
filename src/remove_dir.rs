//! Recursive directory removal with progress reporting.
//!
//! Each unlinked leaf bumps a [`Progress`] counter so a TTY spinner can render
//! live updates; the caller reads `progress.totals()` to drive the matching
//! post-op summary.
//!
//! Best-effort by design: this is the trash-cleanup phase that runs *after*
//! the worktree has already been pruned from git's metadata, so a partial
//! cleanup leaves the user with a stale directory under `.git/wt/trash/` —
//! recoverable, not catastrophic. read/unlink/rmdir errors are swallowed so
//! the caller can always report the count of leaves we did manage to remove.
//!
//! The removal never follows a symlink or junction, at the root or below it,
//! so nothing outside the tree is deleted, even while another process changes
//! the tree. On Unix each directory is opened relative to its parent's file
//! descriptor with `O_NOFOLLOW`, and entries are unlinked relative to the
//! directory that holds them; a directory swapped for a symlink mid-walk is
//! unlinked as a link. Elsewhere `std::fs::remove_dir_all` provides the same
//! guarantee. The Unix walk also gives the owner read, write, and search
//! permission on each directory it enters: listing a directory and unlinking
//! its entries need them, and installers leave read-only directories
//! (`dr-xr-xr-x`) that would otherwise strand the whole tree. A directory
//! owned by another user stays as it is.
//!
//! Unix unlinks run on a dedicated 4-thread pool, each directory's files in
//! parallel — file removal is filesystem latency, not CPU, so a small pool
//! gives most of the speedup without the per-process thread cost of rayon's
//! default `2× CPU cores` global pool (which adds up across many subprocesses
//! in test runs). On a 199k-entry pnpm `node_modules` under `taskpolicy -b`,
//! this deleted about twice as fast as `rm -rf`.

use std::path::Path;

use crate::progress::{DataCopy, Progress};

/// Remove a directory tree, reporting per-file progress.
///
/// Each successful unlink is recorded on `progress`, so a `progress` dedicated
/// to this call reports `(files_removed, bytes_removed)` in `totals()`
/// afterwards. Symlinks count as their own size, not the target's. A symlink
/// or file at `path` is unlinked, as `rm -rf` does. A path without a final
/// name (`/`, `.`, `x/..`) is left alone, as `rm -rf` refuses it. Any I/O
/// errors are silently skipped — a leaf that can't be unlinked is simply not
/// counted.
///
/// Returns whether `path` is gone afterwards. A failure to inspect it counts
/// as still present.
pub fn remove_dir_with_progress(path: &Path, progress: &Progress) -> bool {
    if path.file_name().is_some() {
        tree::remove(path, progress);
    }
    matches!(path.symlink_metadata(), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
}

#[cfg(unix)]
mod tree {
    use std::os::fd::{AsFd, BorrowedFd};
    use std::path::Path;
    use std::sync::LazyLock;

    use rayon::prelude::*;
    use rustix::fs::{
        AtFlags, CWD, Dir, FileType, Mode, OFlags, RawMode, chmodat, fchmod, fstat, openat, statat,
        unlinkat,
    };
    use rustix::io::Errno;
    use rustix::path::Arg;

    use super::{DataCopy, Progress};

    /// Capped at 4 threads — same reasoning as `copy::COPY_POOL` (filesystem I/O,
    /// don't oversubscribe CPU when many wt subprocesses run in parallel).
    static REMOVE_POOL: LazyLock<rayon::ThreadPool> = LazyLock::new(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .build()
            .expect("failed to build remove thread pool")
    });

    const DIR_FLAGS: OFlags = OFlags::RDONLY
        .union(OFlags::DIRECTORY)
        .union(OFlags::NOFOLLOW)
        .union(OFlags::CLOEXEC);

    pub(super) fn remove(path: &Path, progress: &Progress) {
        REMOVE_POOL.install(|| remove_entry(CWD, path, progress));
    }

    /// Remove `name` inside `parent`, recursing into it only when it is a real
    /// directory.
    fn remove_entry(parent: BorrowedFd<'_>, name: impl Arg + Copy, progress: &Progress) {
        let dir = match openat(parent, name, DIR_FLAGS, Mode::empty()) {
            Ok(dir) => dir,
            // `O_NOFOLLOW` on a symlink fails with ELOOP (ENOTDIR on some
            // systems); either way the entry is a leaf to unlink.
            Err(Errno::NOTDIR | Errno::LOOP) => return remove_leaf(parent, name, progress),
            // A directory its owner can't read or search.
            Err(Errno::ACCESS) => {
                let Some(dir) = grant_owner_access_at(parent, name)
                    .and_then(|()| openat(parent, name, DIR_FLAGS, Mode::empty()).ok())
                else {
                    return;
                };
                dir
            }
            Err(_) => return,
        };
        empty_dir(dir.as_fd(), progress);
        let _ = unlinkat(parent, name, AtFlags::REMOVEDIR);
    }

    fn empty_dir(dir: BorrowedFd<'_>, progress: &Progress) {
        // Opening it needed read and search; unlinking entries needs write.
        if let Some(mode) = fstat(dir)
            .ok()
            .and_then(|stat| with_owner_access(stat.st_mode))
        {
            let _ = fchmod(dir, mode);
        }
        let mut dirs = Vec::new();
        let mut leaves = Vec::new();
        for entry in Dir::read_from(dir).into_iter().flatten().flatten() {
            let name = entry.file_name();
            if name == c"." || name == c".." {
                continue;
            }
            match entry.file_type() {
                // `remove_entry` opens without following symlinks and unlinks
                // whatever turns out not to be a directory.
                FileType::Directory | FileType::Unknown => dirs.push(name.to_owned()),
                _ => leaves.push(name.to_owned()),
            }
        }
        leaves
            .par_iter()
            .for_each(|name| remove_leaf(dir, name.as_c_str(), progress));
        // Subdirectories go one at a time, so the open descriptors number the
        // tree's depth.
        for name in &dirs {
            remove_entry(dir, name.as_c_str(), progress);
        }
    }

    fn remove_leaf(parent: BorrowedFd<'_>, name: impl Arg + Copy, progress: &Progress) {
        // Capture size before unlinking. Best-effort: a racy delete counts
        // the leaf with zero bytes.
        let bytes = statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)
            .map(|stat| u64::try_from(stat.st_size).unwrap_or(0))
            .unwrap_or(0);
        if unlinkat(parent, name, AtFlags::empty()).is_ok() {
            // A removal frees extents rather than moving them, so it has no
            // side of the reflink split to land on.
            progress.record(bytes, DataCopy::Neither);
        }
    }

    /// Add the owner's `rwx` bits to the directory `name`, never through a
    /// symlink.
    #[cfg(not(target_os = "linux"))]
    fn grant_owner_access_at(parent: BorrowedFd<'_>, name: impl Arg + Copy) -> Option<()> {
        let stat = statat(parent, name, AtFlags::SYMLINK_NOFOLLOW).ok()?;
        let mode = with_owner_access(stat.st_mode)?;
        chmodat(parent, name, mode, AtFlags::SYMLINK_NOFOLLOW).ok()
    }

    /// Add the owner's `rwx` bits to the directory `name`, never through a
    /// symlink. Linux's `fchmodat` ignores `AT_SYMLINK_NOFOLLOW`, so this opens
    /// the directory with `O_PATH`, which needs no permission on it, and
    /// changes the mode through that descriptor's `/proc` link.
    #[cfg(target_os = "linux")]
    fn grant_owner_access_at(parent: BorrowedFd<'_>, name: impl Arg + Copy) -> Option<()> {
        use std::os::fd::AsRawFd;

        let dir = openat(
            parent,
            name,
            OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .ok()?;
        let mode = with_owner_access(fstat(&dir).ok()?.st_mode)?;
        let link = format!("/proc/self/fd/{}", dir.as_raw_fd());
        chmodat(CWD, link.as_str(), mode, AtFlags::empty()).ok()
    }

    /// `st_mode`'s permissions with the owner's `rwx` added, or `None` when
    /// the owner already has them.
    fn with_owner_access(st_mode: RawMode) -> Option<Mode> {
        let mode = Mode::from_raw_mode(st_mode);
        (!mode.contains(Mode::RWXU)).then_some(mode | Mode::RWXU)
    }
}

#[cfg(not(unix))]
mod tree {
    use std::fs;
    use std::path::Path;

    use super::{DataCopy, Progress};

    /// Measure the tree, then delete it with `fs::remove_dir_all`, which
    /// opens each directory relative to its parent's handle and removes links
    /// and junctions without following them, even while the tree changes.
    /// The leaves are recorded once the whole tree is gone.
    pub(super) fn remove(path: &Path, progress: &Progress) {
        let Ok(metadata) = path.symlink_metadata() else {
            return;
        };
        if !metadata.is_dir() {
            // A directory symlink or junction is removed as a directory; that
            // removes the link, not its target.
            if fs::remove_file(path)
                .or_else(|_| fs::remove_dir(path))
                .is_ok()
            {
                progress.record(metadata.len(), DataCopy::Neither);
            }
            return;
        }
        let mut leaf_sizes = Vec::new();
        let mut stack = vec![path.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                // Symlinks and junctions report `is_dir() == false` and count
                // as leaves.
                let Ok(metadata) = entry.metadata() else {
                    continue;
                };
                if metadata.is_dir() {
                    stack.push(entry.path());
                } else {
                    leaf_sizes.push(metadata.len());
                }
            }
        }
        if fs::remove_dir_all(path).is_ok() {
            for bytes in leaf_sizes {
                // A removal frees extents rather than moving them, so it has
                // no side of the reflink split to land on.
                progress.record(bytes, DataCopy::Neither);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remove_dir_with_progress_empty_dir() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("empty");
        std::fs::create_dir(&dir).unwrap();

        let progress = Progress::disabled();
        remove_dir_with_progress(&dir, &progress);

        assert_eq!(progress.totals(), (0, 0));
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

        let progress = Progress::disabled();
        remove_dir_with_progress(&root, &progress);

        assert_eq!(progress.totals(), (3, 12));
        assert!(!root.exists());
    }

    #[test]
    fn test_remove_dir_with_progress_missing_root_is_ok() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("does-not-exist");

        let progress = Progress::disabled();
        remove_dir_with_progress(&missing, &progress);

        assert_eq!(progress.totals(), (0, 0));
    }

    #[test]
    fn test_remove_dir_with_progress_refuses_paths_without_a_name() {
        let temp = tempfile::tempdir().unwrap();
        let kept = temp.path().join("kept");
        std::fs::create_dir_all(kept.join("child")).unwrap();
        std::fs::write(kept.join("file.txt"), b"x").unwrap();

        let progress = Progress::disabled();
        assert!(!remove_dir_with_progress(&kept.join("child/.."), &progress));

        assert_eq!(progress.totals(), (0, 0));
        assert!(kept.join("file.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn test_remove_dir_with_progress_handles_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("tree");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("real.txt"), b"abc").unwrap();
        std::os::unix::fs::symlink(root.join("real.txt"), root.join("link")).unwrap();

        let progress = Progress::disabled();
        remove_dir_with_progress(&root, &progress);

        // 1 file + 1 symlink = 2 leaves removed; the symlink is unlinked
        // without following.
        let (files, _bytes) = progress.totals();
        assert_eq!(files, 2);
        assert!(!root.exists());
    }

    /// A directory symlink inside the tree is unlinked; the directory it
    /// points to keeps its files.
    #[cfg(unix)]
    #[test]
    fn test_remove_dir_with_progress_keeps_directory_symlink_targets() {
        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("keep.txt"), b"abc").unwrap();
        let root = temp.path().join("tree");
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("nested/link")).unwrap();

        let progress = Progress::disabled();
        remove_dir_with_progress(&root, &progress);

        assert!(!root.exists());
        assert!(outside.join("keep.txt").exists());
        assert_eq!(progress.totals().0, 1);
    }

    /// A symlink passed as the root is unlinked; its target keeps its files.
    #[cfg(unix)]
    #[test]
    fn test_remove_dir_with_progress_unlinks_symlink_root() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("keep.txt"), b"abc").unwrap();
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let progress = Progress::disabled();
        remove_dir_with_progress(&link, &progress);

        assert!(link.symlink_metadata().is_err());
        assert!(target.join("keep.txt").exists());
        assert_eq!(progress.totals().0, 1);
    }

    /// Read-only directories, as installers leave them, are deleted along
    /// with their contents.
    #[cfg(unix)]
    #[test]
    fn test_remove_dir_with_progress_removes_read_only_directories() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("tree");
        let read_only = root.join("read-only");
        std::fs::create_dir_all(read_only.join("nested")).unwrap();
        std::fs::write(read_only.join("nested/a.txt"), b"x").unwrap();
        std::fs::set_permissions(
            read_only.join("nested"),
            std::fs::Permissions::from_mode(0o555),
        )
        .unwrap();
        std::fs::set_permissions(&read_only, std::fs::Permissions::from_mode(0o555)).unwrap();

        let progress = Progress::disabled();
        remove_dir_with_progress(&root, &progress);

        assert_eq!(progress.totals(), (1, 1));
        assert!(!root.exists());
    }

    /// A directory its owner can't read is made readable and deleted.
    #[cfg(unix)]
    #[test]
    fn test_remove_dir_with_progress_removes_unreadable_directories() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("tree");
        let unreadable = root.join("unreadable");
        std::fs::create_dir_all(&unreadable).unwrap();
        std::fs::write(unreadable.join("b.txt"), b"y").unwrap();
        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000)).unwrap();

        let progress = Progress::disabled();
        remove_dir_with_progress(&root, &progress);

        assert_eq!(progress.totals(), (1, 1));
        assert!(!root.exists());
    }
}
