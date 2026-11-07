use super::{Cancel, check};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub(super) const INPUT_LIMIT: u64 = 512 * 1024 * 1024;
const FILE_LIMIT: usize = 10_000;
#[derive(Default)]
pub(super) struct Budget {
    bytes: u64,
    entries: usize,
}

pub(super) fn excluded(path: &Path) -> bool {
    path.components()
        .any(|part| part.as_os_str().to_string_lossy().starts_with('.'))
        || matches!(
            path.extension().and_then(|s| s.to_str()),
            Some("aux" | "log" | "fls" | "fdb_latexmk" | "synctex" | "toc" | "out" | "bcf" | "blg")
        )
}

/// Content fingerprints include inputs first discovered by TeX during this build:
/// every regular input in the authorized folder was captured before execution.
pub(super) struct Snapshot {
    root: PathBuf,
    hashes: BTreeMap<PathBuf, [u8; 32]>,
}
impl Snapshot {
    pub fn capture(
        root: &Path,
        destination: &Path,
        cancel: &Cancel,
        budget: &mut Budget,
    ) -> Result<Self, String> {
        fs::create_dir_all(destination).map_err(|e| e.to_string())?;
        let mut result = Self {
            root: root.to_owned(),
            hashes: BTreeMap::new(),
        };
        let mut entries = budget.entries;
        walk(root, cancel, &mut entries, &mut |relative, mut file| {
            check(cancel)?;
            if result.hashes.len() >= FILE_LIMIT {
                return Err("TeX input folder exceeds 10,000 files".into());
            }
            let len = file.metadata().map_err(|e| e.to_string())?.len();
            budget.bytes += len;
            if budget.bytes > INPUT_LIMIT {
                return Err(
                    "TeX input folder exceeds 512 MiB; use a smaller document folder".into(),
                );
            }
            let target = destination.join(relative);
            fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
            let mut out = File::create(target).map_err(|e| e.to_string())?;
            let hash = hash_copy(&mut file, &mut out, cancel, len)?;
            result.hashes.insert(relative.to_owned(), hash);
            Ok(())
        })?;
        budget.entries = entries;
        Ok(result)
    }
    pub fn validate(&self, cancel: &Cancel) -> Result<(), String> {
        let mut seen = 0;
        walk(&self.root, cancel, &mut 0, &mut |relative, mut file| {
            check(cancel)?;
            let expected = self
                .hashes
                .get(relative)
                .ok_or("TeX inputs changed during compilation")?;
            let len = file.metadata().map_err(|e| e.to_string())?.len();
            if len > INPUT_LIMIT {
                return Err("TeX inputs changed during compilation".into());
            }
            let hash = hash_copy(&mut file, &mut std::io::sink(), cancel, len)?;
            if &hash != expected {
                return Err("TeX inputs changed during compilation".into());
            }
            seen += 1;
            Ok(())
        })?;
        if seen != self.hashes.len() {
            return Err("TeX inputs changed during compilation".into());
        }
        Ok(())
    }
}

fn hash_copy(
    input: &mut File,
    output: &mut impl Write,
    cancel: &Cancel,
    len: u64,
) -> Result<[u8; 32], String> {
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    let mut read = 0;
    loop {
        check(cancel)?;
        let count = input.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        read += count as u64;
        if read > len {
            return Err("TeX input changed while snapshotting".into());
        }
        output
            .write_all(&buffer[..count])
            .map_err(|e| e.to_string())?;
        hash.update(&buffer[..count]);
    }
    if read != len {
        return Err("TeX input changed while snapshotting".into());
    }
    Ok(hash.finalize().into())
}

#[cfg(unix)]
fn walk(
    root: &Path,
    cancel: &Cancel,
    entries: &mut usize,
    visit: &mut impl FnMut(&Path, File) -> Result<(), String>,
) -> Result<(), String> {
    use std::{
        ffi::{CStr, CString},
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        },
    };
    struct Directory(*mut libc::DIR);
    impl Drop for Directory {
        fn drop(&mut self) {
            unsafe {
                libc::closedir(self.0);
            }
        }
    }
    fn descend(
        dir: File,
        relative: &Path,
        visit: &mut impl FnMut(&Path, File) -> Result<(), String>,
        depth: usize,
        entries: &mut usize,
        cancel: &Cancel,
    ) -> Result<(), String> {
        if depth > 32 {
            return Err("TeX input folder is nested too deeply".into());
        }
        // Directory descriptors anchor traversal even if another process renames
        // a directory. No symlink is followed, including in ancestor components.
        let descriptor = unsafe { libc::fcntl(dir.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let ptr = unsafe { libc::fdopendir(descriptor) };
        if ptr.is_null() {
            let error = std::io::Error::last_os_error().to_string();
            unsafe {
                libc::close(descriptor);
            }
            return Err(error);
        }
        let stream = Directory(ptr);
        loop {
            check(cancel)?;
            let entry = unsafe { libc::readdir(stream.0) };
            if entry.is_null() {
                break;
            }
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
            if name.to_bytes().starts_with(b".") {
                continue;
            }
            *entries += 1;
            if *entries > FILE_LIMIT {
                return Err("TeX input folder exceeds 10,000 entries".into());
            }
            let path = relative.join(std::ffi::OsStr::from_bytes(name.to_bytes()));
            if excluded(&path) {
                continue;
            }
            let name = CString::new(name.to_bytes()).unwrap();
            let fd = unsafe {
                libc::openat(
                    dir.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err(format!(
                    "Cannot snapshot {} (symlinks are not supported): {}",
                    path.display(),
                    std::io::Error::last_os_error()
                ));
            }
            let file = unsafe { File::from_raw_fd(fd) };
            let meta = file.metadata().map_err(|e| e.to_string())?;
            if meta.is_dir() {
                descend(file, &path, visit, depth + 1, entries, cancel)?;
            } else if meta.is_file() {
                visit(&path, file)?;
            } else {
                return Err(format!("Unsupported TeX input: {}", path.display()));
            }
        }
        Ok(())
    }
    descend(
        File::open(root).map_err(|e| e.to_string())?,
        Path::new(""),
        visit,
        0,
        entries,
        cancel,
    )
}
#[cfg(not(unix))]
fn walk(
    _: &Path,
    _: &Cancel,
    _: &mut usize,
    _: &mut impl FnMut(&Path, File) -> Result<(), String>,
) -> Result<(), String> {
    Err("TeX preview is currently available on macOS".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, atomic::AtomicBool};
    #[cfg(unix)]
    #[test]
    fn snapshots_are_private_and_new_or_changed_dependencies_reject_publication() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        fs::create_dir(source.path().join("chapters")).unwrap();
        let input = source.path().join("chapters/intro.tex");
        fs::write(&input, "first").unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let snapshot = Snapshot::capture(
            source.path(),
            target.path(),
            &cancel,
            &mut Budget::default(),
        )
        .unwrap();
        snapshot.validate(&cancel).unwrap();
        fs::write(&input, "other").unwrap(); // Same length still invalidates.
        assert!(snapshot.validate(&cancel).is_err());
        assert_eq!(
            fs::read_to_string(target.path().join("chapters/intro.tex")).unwrap(),
            "first"
        );
        fs::write(&input, "first").unwrap();
        fs::write(source.path().join("new.sty"), "new").unwrap();
        assert!(snapshot.validate(&cancel).is_err());
    }
    #[cfg(not(unix))]
    #[test]
    fn unsupported_platform_rejects_snapshotting() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let result = Snapshot::capture(
            source.path(),
            target.path(),
            &Arc::new(AtomicBool::new(false)),
            &mut Budget::default(),
        );
        assert_eq!(
            result.err().unwrap(),
            "TeX preview is currently available on macOS"
        );
    }
    #[cfg(unix)]
    #[test]
    fn snapshot_never_follows_input_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("escape.tex")).unwrap();
        assert!(
            Snapshot::capture(
                root.path(),
                out.path(),
                &Arc::new(AtomicBool::new(false)),
                &mut Budget::default()
            )
            .is_err()
        );
    }
}
