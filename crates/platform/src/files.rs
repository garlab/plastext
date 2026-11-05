//! Blocking file operations. Run on a worker; clipboard helpers run on the UI thread.
use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
};

/// A duplicate lives beside the original, with a collision-free name.
pub fn duplicate(source: &Path) -> io::Result<PathBuf> {
    copy_unique(
        source,
        source
            .parent()
            .ok_or_else(|| io::Error::other("No parent folder"))?,
        true,
    )
}

pub fn copy_into(source: &Path, directory: &Path) -> io::Result<PathBuf> {
    copy_unique(source, directory, false)
}

fn copy_unique(source: &Path, directory: &Path, duplicate: bool) -> io::Result<PathBuf> {
    let metadata = fs::symlink_metadata(source)?;
    let filename = source
        .file_name()
        .ok_or_else(|| io::Error::other("No filename"))?;
    let name = Path::new(filename);
    let (stem, extension) = if metadata.is_dir() {
        (filename, None)
    } else {
        (name.file_stem().unwrap_or(filename), name.extension())
    };
    for number in usize::from(duplicate)..10_000 {
        let mut candidate = OsString::from(stem);
        if number > 0 {
            candidate.push(if number == 1 {
                " copy".into()
            } else {
                format!(" copy {number}")
            });
        }
        if let Some(extension) = extension {
            candidate.push(".");
            candidate.push(extension);
        }
        let destination = directory.join(candidate);
        validate_destination(source, &destination)?;
        if fs::symlink_metadata(&destination).is_ok() {
            continue;
        }
        match native::copy(source, &destination) {
            Ok(()) => return Ok(destination),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::other("Could not find an unused copy name"))
}

/// Never replaces an existing entry. Symlinks themselves are moved.
pub fn move_to(source: &Path, destination: &Path) -> io::Result<()> {
    if source == destination {
        return Ok(());
    }
    validate_destination(source, destination)?;
    native::move_to(source, destination)
}

fn validate_destination(source: &Path, destination: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    let parent = destination
        .parent()
        .ok_or_else(|| io::Error::other("No destination folder"))?;
    if metadata.is_dir() && fs::canonicalize(parent)?.starts_with(fs::canonicalize(source)?) {
        return Err(io::Error::other(
            "A folder cannot be copied or moved inside itself",
        ));
    }
    Ok(())
}

/// Uses the system Bin. Failure never falls back to permanent deletion.
pub fn trash(path: &Path) -> io::Result<PathBuf> {
    native::trash(path)
}

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use objc2::rc::{Retained, autoreleasepool};
    use objc2_foundation::{NSFileManager, NSURL};

    fn url(path: &Path) -> io::Result<Retained<NSURL>> {
        NSURL::from_file_path(path).ok_or_else(|| io::Error::other("Invalid file path"))
    }
    pub fn copy(source: &Path, destination: &Path) -> io::Result<()> {
        autoreleasepool(|_| {
            NSFileManager::new()
                .copyItemAtURL_toURL_error(&*url(source)?, &*url(destination)?)
                .map_err(|error| {
                    let kind = if error.domain().to_string() == "NSCocoaErrorDomain"
                        && error.code() == 516
                    {
                        io::ErrorKind::AlreadyExists
                    } else {
                        io::ErrorKind::Other
                    };
                    io::Error::new(kind, error.to_string())
                })
        })
    }
    pub fn move_to(source: &Path, destination: &Path) -> io::Result<()> {
        autoreleasepool(|_| {
            NSFileManager::new()
                .moveItemAtURL_toURL_error(&*url(source)?, &*url(destination)?)
                .map_err(|error| io::Error::other(error.to_string()))
        })
    }
    pub fn trash(path: &Path) -> io::Result<PathBuf> {
        autoreleasepool(|_| {
            let mut destination = None;
            NSFileManager::new()
                .trashItemAtURL_resultingItemURL_error(&*url(path)?, Some(&mut destination))
                .map_err(|error| io::Error::other(error.to_string()))?;
            destination
                .and_then(|url| url.to_file_path())
                .ok_or_else(|| io::Error::other("The system did not return the Bin location"))
        })
    }
}

#[cfg(not(target_os = "macos"))]
mod native {
    use super::*;
    fn unsupported() -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "File operations are not available on this platform yet",
        )
    }
    pub fn copy(_: &Path, _: &Path) -> io::Result<()> {
        Err(unsupported())
    }
    pub fn move_to(_: &Path, _: &Path) -> io::Result<()> {
        Err(unsupported())
    }
    pub fn trash(_: &Path) -> io::Result<PathBuf> {
        Err(unsupported())
    }
}

/// Adds a Finder-compatible file URL to GPUI's existing clipboard contents.
#[cfg(target_os = "macos")]
pub fn write_clipboard_url(path: &Path) {
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeFileURL};
    use objc2_foundation::NSURL;
    if let Some(url) = NSURL::from_file_path(path)
        && let Some(string) = url.absoluteString()
    {
        // SAFETY: AppKit's exported type constant is immutable. Called on the UI thread.
        NSPasteboard::generalPasteboard()
            .setString_forType(&string, unsafe { NSPasteboardTypeFileURL });
    }
}

#[cfg(target_os = "macos")]
pub fn read_clipboard_url() -> Option<PathBuf> {
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeFileURL};
    use objc2_foundation::NSURL;
    // SAFETY: AppKit's exported type constant is immutable. Called on the UI thread.
    let string =
        NSPasteboard::generalPasteboard().stringForType(unsafe { NSPasteboardTypeFileURL })?;
    NSURL::URLWithString(&string)?.to_file_path()
}

#[cfg(not(target_os = "macos"))]
pub fn write_clipboard_url(_: &Path) {}
#[cfg(not(target_os = "macos"))]
pub fn read_clipboard_url() -> Option<PathBuf> {
    None
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    #[test]
    #[ignore = "moves only a temporary fixture to the system Bin, then restores it"]
    fn system_bin_keeps_file_contents_recoverable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plastext-trash-test.txt");
        fs::write(&path, "recoverable").unwrap();
        let binned = trash(&path).unwrap();
        assert!(!path.exists());
        let contents = fs::read(&binned).unwrap();
        move_to(&binned, &path).unwrap();
        assert_eq!(contents, b"recoverable");
    }
    #[test]
    fn copies_and_moves_never_overwrite_and_keep_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let source = root.join("Café.txt");
        fs::write(&source, "original").unwrap();
        fs::write(root.join("Café copy.txt"), "keep").unwrap();
        assert_eq!(duplicate(&source).unwrap(), root.join("Café copy 2.txt"));
        assert_eq!(
            fs::read_to_string(root.join("Café copy.txt")).unwrap(),
            "keep"
        );
        assert!(move_to(&source, &root.join("Café copy.txt")).is_err());
        assert_eq!(fs::read_to_string(&source).unwrap(), "original");
        std::os::unix::fs::symlink(&source, root.join("link")).unwrap();
        let copied = duplicate(&root.join("link")).unwrap();
        assert_eq!(fs::read_link(copied).unwrap(), source);
        move_to(&root.join("link"), &root.join("renamed link")).unwrap();
        assert!(source.exists());
        fs::create_dir(root.join("folder")).unwrap();
        fs::write(root.join("folder/file"), "nested").unwrap();
        assert!(copy_into(&root.join("folder"), &root.join("folder")).is_err());
        let copied = duplicate(&root.join("folder")).unwrap();
        assert_eq!(fs::read_to_string(copied.join("file")).unwrap(), "nested");
        move_to(&source, &root.join("CAFÉ.TXT")).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("CAFÉ.TXT")).unwrap(),
            "original"
        );
    }
}
