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
use std::os::windows::fs::{FileTypeExt, OpenOptionsExt};
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

/// Remove only an owned prepared leaf, retaining symlink targets and permitting
/// cleanup of source read-only attributes without changing an existing target.
pub(super) fn remove_leaf(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.file_type().is_symlink_dir() {
        fs::remove_dir(path)
    } else {
        fs::remove_file(path)
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW,
        ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
        SE_FILE_OBJECT, SetNamedSecurityInfoW,
    };
    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, OWNER_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION,
    };
    use windows_sys::Win32::Storage::FileSystem::READ_CONTROL;

    fn security(path: &Path) -> String {
        let file = OpenOptions::new()
            .access_mode(READ_CONTROL)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)
            .unwrap();
        let mut descriptor = std::ptr::null_mut();
        let mut text = std::ptr::null_mut();
        let mut length = 0;
        #[expect(
            unsafe_code,
            reason = "Read real Windows file permissions for filesystem regression coverage"
        )]
        // SAFETY: handles and outputs stay live; successful API allocations
        // remain valid through conversion and are freed with LocalFree.
        unsafe {
            assert_eq!(
                GetSecurityInfo(
                    file.as_raw_handle(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | OWNER_SECURITY_INFORMATION,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut descriptor
                ),
                0
            );
            assert_ne!(
                ConvertSecurityDescriptorToStringSecurityDescriptorW(
                    descriptor,
                    SDDL_REVISION_1,
                    DACL_SECURITY_INFORMATION | OWNER_SECURITY_INFORMATION,
                    &mut text,
                    &mut length
                ),
                0,
                "{}",
                io::Error::last_os_error()
            );
            let result =
                String::from_utf16(std::slice::from_raw_parts(text, length as usize - 1)).unwrap();
            LocalFree(text.cast());
            LocalFree(descriptor);
            result
        }
    }

    fn direct_child_permissions(path: &Path) {
        // OWNER RIGHTS full control propagates normally. Everyone's read ACE
        // reaches direct files only, exposing the old extra-directory staging.
        let sddl: Vec<u16> = "D:P(A;OICI;FA;;;OW)(A;OINP;FR;;;WD)"
            .encode_utf16()
            .chain([0])
            .collect();
        let name: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let mut descriptor = std::ptr::null_mut();
        let mut present = 0;
        let mut defaulted = 0;
        let mut acl = std::ptr::null_mut();
        #[expect(
            unsafe_code,
            reason = "Establish real Windows direct-child ACL inheritance in a disposable fixture"
        )]
        // SAFETY: descriptor is owned until its ACL has been applied; all output
        // pointers are valid and the filename stays null-terminated during the call.
        unsafe {
            assert_ne!(
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    SDDL_REVISION_1,
                    &mut descriptor,
                    std::ptr::null_mut()
                ),
                0,
                "{}",
                io::Error::last_os_error()
            );
            assert_ne!(
                GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted),
                0
            );
            assert_ne!(present, 0);
            let status = SetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                acl,
                std::ptr::null_mut(),
            );
            LocalFree(descriptor);
            assert_eq!(
                status,
                0,
                "setting protected fixture DACL @ {}: {}",
                path.display(),
                io::Error::from_raw_os_error(status as i32)
            );
        }
    }

    /// Replacing a directory symlink operates on the leaf; neither old nor
    /// new directory contents may be deleted or rewritten.
    #[test]
    fn forced_copy_replaces_directory_symlink_leaf() {
        use std::os::windows::fs::symlink_dir;
        let dir = crate::testing::test_tempdir();
        let old_target = dir.path().join("old-target");
        let new_target = dir.path().join("new-target");
        fs::create_dir(&old_target).unwrap();
        fs::create_dir(&new_target).unwrap();
        fs::write(old_target.join("data"), b"old target bytes").unwrap();
        fs::write(new_target.join("data"), b"new target bytes").unwrap();
        let source = dir.path().join("source");
        let destination = dir.path().join("destination");
        symlink_dir(&new_target, &source).unwrap();
        symlink_dir(&old_target, &destination).unwrap();
        assert!(
            crate::copy::copy_leaf(&source, &destination, None, true, None)
                .unwrap()
                .is_some()
        );
        assert_eq!(fs::read_link(&destination).unwrap(), new_target);
        assert_eq!(
            fs::read(old_target.join("data")).unwrap(),
            b"old target bytes"
        );
        assert_eq!(
            fs::read(new_target.join("data")).unwrap(),
            b"new target bytes"
        );
        assert!(!fs::read_dir(dir.path()).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".wt-copy-")
        }));
    }

    /// Preparation must inherit as a normal direct copy, without retaining old
    /// destination security. A native control reproduces the extra-generation
    /// loss before comparing the actual producer and published files.
    #[test]
    fn copied_leaf_inherits_from_final_parent() {
        let dir = crate::testing::test_tempdir();
        let source = dir.path().join("source");
        let parent = dir.path().join("destination");
        fs::write(&source, b"source bytes").unwrap();
        fs::create_dir(&parent).unwrap();
        direct_child_permissions(&parent);
        let reference = parent.join("direct-copy");
        fs::copy(&source, &reference).unwrap();
        let expected = security(&reference);
        assert!(expected.contains("(A;ID;FR;;;WD)"), "{expected}");

        let old_staging = tempfile::tempdir_in(&parent).unwrap();
        let old_leaf = old_staging.path().join("leaf");
        fs::copy(&source, &old_leaf).unwrap();
        let old_published = parent.join("old-copy");
        fs::rename(&old_leaf, &old_published).unwrap();
        let old_security = security(&old_published);
        assert!(!old_security.contains("(A;ID;FR;;;WD)"), "{old_security}");
        assert_ne!(old_security, expected);

        for force in [false, true] {
            let destination = parent.join(if force { "forced" } else { "new" });
            if force {
                fs::write(&destination, b"old bytes").unwrap();
                // A protected old ACL must be replaced with the normal fresh
                // copy rules rather than retained as a new permission policy.
                direct_child_permissions(&destination);
                assert_ne!(security(&destination), expected);
                let mut permissions = fs::metadata(&destination).unwrap().permissions();
                permissions.set_readonly(true);
                fs::set_permissions(&destination, permissions).unwrap();
            }
            let prepared = crate::copy::PreparedLeaf::new(
                &source,
                &destination,
                &source.symlink_metadata().unwrap(),
                None,
            )
            .unwrap();
            assert_eq!(prepared.path.parent(), Some(parent.as_path()));
            assert_eq!(security(&prepared.path), expected);
            assert!(prepared.persist(&destination, force).unwrap().is_some());
            assert_eq!(security(&destination), expected);
            assert_eq!(fs::read(&destination).unwrap(), b"source bytes");
        }
    }
}
