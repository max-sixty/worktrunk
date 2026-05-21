//! Atomic, no-overwrite filesystem rename.
//!
//! [`std::fs::rename`] silently overwrites an existing destination on every
//! platform. That makes the "compute a fresh backup path, then move the old
//! path onto it" pattern a time-of-check/time-of-use race: anything that
//! appears at the destination between the existence check and the rename is
//! destroyed without warning. [`rename_no_replace`] closes that gap by asking
//! the OS to perform the rename only when the destination does not exist, as a
//! single atomic step — so a colliding destination fails the call instead of
//! being overwritten.
//!
//! It lives in its own module so every `--clobber`-style backup can route
//! through one atomic rename rather than each call site reimplementing it.

use std::io;
use std::path::Path;

/// Rename `from` to `to`, failing if `to` already exists.
///
/// Unlike [`std::fs::rename`], an existing destination is never overwritten:
/// the call fails with [`io::ErrorKind::AlreadyExists`] and leaves both paths
/// untouched. The existence check and the move are one atomic operation, so
/// there is no time-of-check/time-of-use window for data loss.
///
/// Linux and macOS use `renameat2(RENAME_NOREPLACE)` / `renameatx_np(RENAME_EXCL)`;
/// Windows uses `MoveFileExW` without `MOVEFILE_REPLACE_EXISTING`. Other
/// platforms fail with [`io::ErrorKind::Unsupported`].
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};

    renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE).map_err(io::Error::from)
}

/// Rename `from` to `to`, failing if `to` already exists.
///
/// Unlike [`std::fs::rename`], an existing destination is never overwritten:
/// the call fails with [`io::ErrorKind::AlreadyExists`] and leaves both paths
/// untouched. The existence check and the move are one atomic operation, so
/// there is no time-of-check/time-of-use window for data loss.
///
/// Linux and macOS use `renameat2(RENAME_NOREPLACE)` / `renameatx_np(RENAME_EXCL)`;
/// Windows uses `MoveFileExW` without `MOVEFILE_REPLACE_EXISTING`. Other
/// platforms fail with [`io::ErrorKind::Unsupported`].
#[cfg(windows)]
pub fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

    fn to_wide(path: &Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    let from_wide = to_wide(from);
    let to_wide = to_wide(to);

    // SAFETY: both arguments are null-terminated UTF-16 buffers that outlive
    // the call. `dwflags` is 0 — `MOVEFILE_REPLACE_EXISTING` is deliberately
    // omitted, so an existing destination fails the call (ERROR_ALREADY_EXISTS,
    // which maps to `ErrorKind::AlreadyExists`) instead of being overwritten.
    let moved = unsafe { MoveFileExW(from_wide.as_ptr(), to_wide.as_ptr(), 0) };
    if moved == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Rename `from` to `to`, failing if `to` already exists.
///
/// Unlike [`std::fs::rename`], an existing destination is never overwritten:
/// the call fails with [`io::ErrorKind::AlreadyExists`] and leaves both paths
/// untouched. The existence check and the move are one atomic operation, so
/// there is no time-of-check/time-of-use window for data loss.
///
/// Linux and macOS use `renameat2(RENAME_NOREPLACE)` / `renameatx_np(RENAME_EXCL)`;
/// Windows uses `MoveFileExW` without `MOVEFILE_REPLACE_EXISTING`. Other
/// platforms fail with [`io::ErrorKind::Unsupported`].
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    let _ = (from, to);
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic no-overwrite rename is not supported on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rename_no_replace_moves_to_fresh_path() {
        let temp = tempfile::tempdir().unwrap();
        let from = temp.path().join("src");
        let to = temp.path().join("dst");
        std::fs::write(&from, b"payload").unwrap();

        rename_no_replace(&from, &to).unwrap();

        assert!(!from.exists(), "source should be moved away");
        assert_eq!(std::fs::read(&to).unwrap(), b"payload");
    }

    #[test]
    fn test_rename_no_replace_refuses_existing_file() {
        let temp = tempfile::tempdir().unwrap();
        let from = temp.path().join("src");
        let to = temp.path().join("dst");
        std::fs::write(&from, b"new").unwrap();
        std::fs::write(&to, b"original").unwrap();

        let err = rename_no_replace(&from, &to).unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        // Both paths are untouched — no data destroyed.
        assert_eq!(std::fs::read(&from).unwrap(), b"new");
        assert_eq!(std::fs::read(&to).unwrap(), b"original");
    }

    #[test]
    fn test_rename_no_replace_refuses_existing_directory() {
        // The `--clobber` backup moves directories, so an existing directory at
        // the destination must also fail closed rather than be replaced.
        let temp = tempfile::tempdir().unwrap();
        let from = temp.path().join("src");
        let to = temp.path().join("dst");
        std::fs::create_dir(&from).unwrap();
        std::fs::write(from.join("payload"), b"moved").unwrap();
        std::fs::create_dir(&to).unwrap();
        std::fs::write(to.join("existing"), b"keep").unwrap();

        let err = rename_no_replace(&from, &to).unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert!(from.join("payload").exists(), "source dir must be intact");
        assert!(
            to.join("existing").exists(),
            "destination dir must be intact"
        );
    }
}
