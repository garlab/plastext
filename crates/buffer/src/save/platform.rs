use std::{fs::File, io, path::Path, time::SystemTime};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Stamp {
    pub len: u64,
    pub links: u64,
    identity: [u64; 2],
    changed: [i64; 2],
    modified: SystemTime,
    permissions: [u32; 3],
}

pub(super) fn open(path: &Path, writable: bool) -> io::Result<File> {
    #[cfg(unix)]
    let file = {
        use rustix::fs::{Mode, OFlags};
        let access = if writable {
            OFlags::RDWR
        } else {
            OFlags::RDONLY
        };
        File::from(rustix::fs::open(
            path,
            access | OFlags::CLOEXEC | OFlags::NONBLOCK | OFlags::NOFOLLOW,
            Mode::empty(),
        )?)
    };
    #[cfg(windows)]
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(writable)
        .open(path)?;
    // NONBLOCK keeps a path swapped to a FIFO from hanging preparation; no
    // special file is ever accepted as a save destination.
    stamp(&file)?;
    if writable && file.metadata()?.permissions().readonly() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "file is read-only",
        ));
    }
    Ok(file)
}

pub(super) fn stamp(file: &File) -> io::Result<Stamp> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "saving requires a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(Stamp {
            len: metadata.len(),
            links: metadata.nlink(),
            identity: [metadata.dev(), metadata.ino()],
            changed: [metadata.ctime(), metadata.ctime_nsec()],
            modified: metadata.modified()?,
            permissions: [metadata.mode(), metadata.uid(), metadata.gid()],
        })
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_BASIC_INFO, FileBasicInfo, GetFileInformationByHandle,
            GetFileInformationByHandleEx,
        };
        // SAFETY: both structures are plain Win32 output structures, and the
        // live File keeps the handle valid for the duration of these calls.
        let (info, basic) = unsafe {
            let mut info: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
            let mut basic: FILE_BASIC_INFO = std::mem::zeroed();
            if GetFileInformationByHandle(file.as_raw_handle(), &mut info) == 0
                || GetFileInformationByHandleEx(
                    file.as_raw_handle(),
                    FileBasicInfo,
                    (&mut basic as *mut FILE_BASIC_INFO).cast(),
                    size_of::<FILE_BASIC_INFO>() as u32,
                ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            (info, basic)
        };
        Ok(Stamp {
            len: metadata.len(),
            links: u64::from(info.nNumberOfLinks),
            identity: [
                u64::from(info.dwVolumeSerialNumber),
                u64::from(info.nFileIndexHigh) << 32 | u64::from(info.nFileIndexLow),
            ],
            changed: [basic.ChangeTime, 0],
            modified: metadata.modified()?,
            permissions: [info.dwFileAttributes, 0, 0],
        })
    }
}

#[cfg(unix)]
pub(super) fn copy_metadata(
    from: &File,
    to: &File,
    cancel: &std::sync::atomic::AtomicBool,
) -> io::Result<()> {
    use rustix::fs::{
        Gid, Uid, XattrFlags, fchown, fgetxattr, flistxattr, fremovexattr, fsetxattr,
    };
    use std::{ffi::CStr, os::unix::fs::MetadataExt};

    let before = from.metadata()?;
    let target = to.metadata()?;
    if before.uid() != target.uid() || before.gid() != target.gid() {
        fchown(
            to,
            Some(Uid::from_raw(before.uid())),
            Some(Gid::from_raw(before.gid())),
        )?;
    }
    to.set_permissions(before.permissions())?;

    // Fail before commit if attributes cannot be preserved within these bounds.
    // In particular, never allocate an unbounded macOS resource fork in RAM.
    fn names(file: &File) -> io::Result<Vec<u8>> {
        let len = match flistxattr(file, &mut [0_u8; 0][..]) {
            Ok(len) => len,
            Err(rustix::io::Errno::NOTSUP) => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        if len > 64 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "extended attribute names exceed 64 KiB",
            ));
        }
        let mut names = vec![0; len];
        let len = flistxattr(file, names.as_mut_slice())?;
        names.truncate(len);
        Ok(names)
    }
    let source_names = names(from)?;
    for name in names(to)?.split_inclusive(|&b| b == 0) {
        if !source_names
            .split_inclusive(|&b| b == 0)
            .any(|source| source == name)
        {
            fremovexattr(
                to,
                CStr::from_bytes_with_nul(name).map_err(io::Error::other)?,
            )?;
        }
    }
    for name in source_names.split_inclusive(|&b| b == 0) {
        crate::edit::check_cancel(cancel)?;
        let name = CStr::from_bytes_with_nul(name).map_err(io::Error::other)?;
        let len = fgetxattr(from, name, &mut [0_u8; 0][..])?;
        if len > 8 * 1024 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "an extended attribute exceeds 8 MiB",
            ));
        }
        let mut value = vec![0; len];
        let len = fgetxattr(from, name, value.as_mut_slice())?;
        fsetxattr(to, name, &value[..len], XattrFlags::empty())?;
    }
    #[cfg(target_os = "macos")]
    {
        use rustix::fs::{CopyfileFlags, copyfile_state_alloc, copyfile_state_free, fcopyfile};
        let state = copyfile_state_alloc()?;
        // SAFETY: state was allocated above and is freed exactly once after use.
        let copied = unsafe { fcopyfile(from, to, state, CopyfileFlags::ACL) };
        let freed = unsafe { copyfile_state_free(state) };
        copied?;
        freed?;
    }
    Ok(())
}

#[cfg(windows)]
pub(super) fn copy_metadata(
    from: &File,
    to: &File,
    _: &std::sync::atomic::AtomicBool,
) -> io::Result<()> {
    // ReplaceFileW merges the destination's ACLs and named streams at commit.
    to.set_permissions(from.metadata()?.permissions())
}

#[cfg(unix)]
pub(super) fn replace(
    temp: tempfile::NamedTempFile,
    path: &Path,
) -> Result<File, super::SaveFailure> {
    temp.persist(path).map_err(|error| error.error.into())
}

#[cfg(windows)]
pub(super) fn replace(
    temp: tempfile::NamedTempFile,
    path: &Path,
) -> Result<File, super::SaveFailure> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_NORMAL, ReplaceFileW, SetFileAttributesW,
    };
    let before = stamp(temp.as_file())?;
    // ReplaceFileW opens the replacement with exclusive sharing. Close our
    // handle first, retaining only the cleanup guard for its path.
    let temp = temp.into_temp_path();
    let mut backup = tempfile::Builder::new()
        .prefix(".plastext-replaced-")
        .tempfile_in(path.parent().unwrap())?
        .into_temp_path();
    std::fs::remove_file(&backup)?;
    let target: Vec<_> = path.as_os_str().encode_wide().chain([0]).collect();
    let replacement: Vec<_> = temp.as_os_str().encode_wide().chain([0]).collect();
    let backup_path: Vec<_> = backup.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: paths are NUL-terminated and live throughout the call. No merge
    // errors are ignored; failure must not be reported as a successful save.
    let result = (|| -> io::Result<File> {
        if unsafe { SetFileAttributesW(replacement.as_ptr(), FILE_ATTRIBUTE_NORMAL) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe {
            ReplaceFileW(
                target.as_ptr(),
                replacement.as_ptr(),
                backup_path.as_ptr(),
                0,
                std::ptr::null(),
                std::ptr::null(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)?;
        let after = stamp(&file)?;
        if before.identity != after.identity
            || before.len != after.len
            || before.modified != after.modified
        {
            return Err(io::Error::other(
                "replacement changed before its result could be verified",
            ));
        }
        file.sync_all()?;
        Ok(file)
    })();
    result.map_err(|error| {
        let mut failure = super::SaveFailure::from(error);
        // Some ReplaceFileW failures have already moved the original. Never
        // delete its backup merely because the operation reported failure.
        if backup.exists() {
            backup.disable_cleanup(true);
            failure.recovery_path = Some(backup.to_path_buf());
        }
        failure
    })
}

pub(super) fn sync_parent(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(path.parent().unwrap())?.sync_all()?;
    #[cfg(windows)]
    let _ = path; // Windows has no portable directory-fsync equivalent.
    Ok(())
}
