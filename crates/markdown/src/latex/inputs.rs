//! Local include policy. The shared reader owns LaTeX expansion; this layer
//! supplies bounded UTF-8 files inside the document's project.
use super::*;
use plastext_buffer::DiskVersion;
use std::{
    fs::{self, OpenOptions},
    io::Read,
    path::{Component, Path, PathBuf},
};

/// A file consulted during expansion, including missing inputs and file tests.
#[derive(Clone, Debug)]
pub struct Dependency {
    pub path: PathBuf,
    version: Option<DiskVersion>,
}
impl Dependency {
    /// Blocking metadata check; callers run this off the UI thread.
    pub fn changed(&self) -> bool {
        self.changed_version().is_some()
    }

    /// Capture a new version to avoid repeatedly rebuilding a failed revision.
    pub fn changed_version(&self) -> Option<Self> {
        let version = match DiskVersion::read(&self.path) {
            Ok(version) => Some(version),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            // Retry transient permission/sharing errors on the next event.
            Err(_) => return None,
        };
        (version != self.version).then(|| Self {
            path: self.path.clone(),
            version,
        })
    }

    /// Watch both the requested name and its resolved target, if different.
    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        std::iter::once(self.path.as_path()).chain(
            self.version
                .as_ref()
                .filter(|version| version.target() != self.path)
                .map(|version| version.target()),
        )
    }
}

pub(super) struct Inputs<'a> {
    base: PathBuf,
    root: PathBuf,
    cancel: &'a AtomicBool,
    bytes: usize,
    pub dependencies: Vec<Dependency>,
}
impl<'a> Inputs<'a> {
    pub fn new(path: &Path, cancel: &'a AtomicBool) -> io::Result<Self> {
        let base = fs::canonicalize(path.parent().unwrap_or(Path::new(".")))?;
        // Worktrees use a .git file; ordinary repositories use a directory.
        // Without a repository boundary, only the document directory is read.
        let root = base
            .ancestors()
            .take(16)
            .find(|dir| dir.join(".git").exists())
            .unwrap_or(&base)
            .to_owned();
        Ok(Self {
            base,
            root,
            cancel,
            bytes: 0,
            dependencies: Vec::new(),
        })
    }

    pub fn read(&mut self, name: &str, kind: lr::InputKind) -> Option<String> {
        if self.cancel.load(Ordering::Relaxed) || self.dependencies.len() >= 256 {
            return None;
        }
        let requested = self.base.join(name);
        let normalized = normalize(&requested);
        if !normalized.starts_with(&self.root) {
            return None;
        }
        let version = DiskVersion::read(&requested).ok();
        if version
            .as_ref()
            .is_some_and(|v| !v.target().starts_with(&self.root))
        {
            return None;
        }
        let path = version.as_ref().map_or(normalized, |v| v.path().to_owned());
        if !self.dependencies.iter().any(|d| d.path == path) {
            self.dependencies.push(Dependency {
                path: path.clone(),
                version: version.clone(),
            });
        }
        let version = version?;
        if kind == lr::InputKind::Exists {
            return Some(String::new());
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
        }
        let mut file = options.open(version.target()).ok()?;
        let metadata = file.metadata().ok()?;
        if !metadata.is_file()
            || metadata.len() > MAX_SOURCE_BYTES.saturating_sub(self.bytes) as u64
        {
            return None;
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        let mut chunk = [0; 64 * 1024];
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                return None;
            }
            let count = file.read(&mut chunk).ok()?;
            if count == 0 {
                break;
            }
            self.bytes = self.bytes.checked_add(count)?;
            if self.bytes > MAX_SOURCE_BYTES {
                return None;
            }
            bytes.extend_from_slice(&chunk[..count]);
        }
        if !version.is_current().ok()? {
            return None;
        }
        String::from_utf8(bytes).ok()
    }
}

fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            _ => result.push(component.as_os_str()),
        }
    }
    result
}
