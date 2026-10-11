//! Attribute-preserving publication of prepared copied files on Windows.
//!
//! Exclusive rename uses verbatim parents for long paths, preserving the final
//! leaf rather than canonicalizing through a symlink. Forced replacement uses
//! std's rename first, then requests Windows' atomic read-only replacement when
//! permissions block it. No destination is deleted or made writable beforehand.

use std::fs::{self, OpenOptions};
use std::io;
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};

use windows_sys::Wdk::Storage::FileSystem::{
    FILE_RENAME_IGNORE_READONLY_ATTRIBUTE, FILE_RENAME_POSIX_SEMANTICS,
    FILE_RENAME_REPLACE_IF_EXISTS,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_RENAME_INFO,
    FileRenameInfoEx, SetFileInformationByHandle,
};

pub(super) fn publish_leaf(src: &Path, dest: &Path, force: bool) -> io::Result<()> {
    if !force {
        return renamore::rename_exclusive(verbatim_leaf(src)?, verbatim_leaf(dest)?);
    }
    match fs::rename(src, dest) {
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            replace_readonly(&verbatim_leaf(src)?, &verbatim_leaf(dest)?).or(Err(error))
        }
        result => result,
    }
}

fn verbatim_leaf(path: &Path) -> io::Result<PathBuf> {
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "copy destination has no filename",
        )
    })?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    Ok(fs::canonicalize(parent)?.join(name))
}

fn replace_readonly(src: &Path, dest: &Path) -> io::Result<()> {
    let file = OpenOptions::new()
        .access_mode(DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(src)?;
    let mut name: Vec<_> = dest.as_os_str().encode_wide().collect();
    let name_bytes = u32::try_from(name.len() * size_of::<u16>()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "copy destination path is too long",
        )
    })?;
    name.push(0);
    let offset = offset_of!(FILE_RENAME_INFO, FileName);
    let bytes = offset + name.len() * size_of::<u16>();
    let buffer_bytes = u32::try_from(bytes).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "copy destination path is too long",
        )
    })?;
    // FILE_RENAME_INFO has a variable-length trailing filename. Allocating in
    // units of the struct retains its required alignment for the whole buffer.
    let mut buffer =
        vec![FILE_RENAME_INFO::default(); bytes.div_ceil(size_of::<FILE_RENAME_INFO>())];
    let info = buffer.as_mut_ptr();
    #[expect(
        unsafe_code,
        reason = "Windows atomic read-only replacement requires a native handle and rename buffer"
    )]
    // SAFETY: the aligned buffer includes the header and complete UTF-16 name;
    // the owned source handle and buffer remain live until the synchronous API
    // returns. OPEN_REPARSE_POINT ensures a symlink's target is never opened.
    let succeeded = unsafe {
        (*info).Anonymous.Flags = FILE_RENAME_REPLACE_IF_EXISTS
            | FILE_RENAME_POSIX_SEMANTICS
            | FILE_RENAME_IGNORE_READONLY_ATTRIBUTE;
        (*info).FileNameLength = name_bytes;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            info.cast::<u8>().add(offset).cast::<u16>(),
            name.len(),
        );
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileRenameInfoEx,
            info.cast(),
            buffer_bytes,
        )
    };
    if succeeded == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
