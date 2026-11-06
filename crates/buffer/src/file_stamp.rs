//! Handle-based file versions shared by edit preparation and save checks.
use std::{fs::File, io, path::Path, time::SystemTime};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileStamp {
    pub(crate) len: u64,
    pub(crate) links: u64,
    pub(crate) identity: [u64; 2],
    changed: [i64; 2],
    pub(crate) modified: SystemTime,
    permissions: [u32; 3],
}

pub(crate) fn read(file: &File) -> io::Result<FileStamp> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "file versions require a regular file",
        ));
    }
    #[cfg(unix)]
    {
        from_metadata(metadata)
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
        Ok(FileStamp {
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

/// On Unix, stat already includes identity and change time, without opening a
/// path that may have been replaced with a FIFO. Windows needs a file handle.
pub(crate) fn read_path(path: &Path) -> io::Result<FileStamp> {
    #[cfg(unix)]
    {
        from_metadata(std::fs::metadata(path)?)
    }
    #[cfg(windows)]
    {
        read(&File::open(path)?)
    }
}

#[cfg(unix)]
fn from_metadata(metadata: std::fs::Metadata) -> io::Result<FileStamp> {
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "file versions require a regular file",
        ));
    }
    use std::os::unix::fs::MetadataExt;
    Ok(FileStamp {
        len: metadata.len(),
        links: metadata.nlink(),
        identity: [metadata.dev(), metadata.ino()],
        changed: [metadata.ctime(), metadata.ctime_nsec()],
        modified: metadata.modified()?,
        permissions: [metadata.mode(), metadata.uid(), metadata.gid()],
    })
}
