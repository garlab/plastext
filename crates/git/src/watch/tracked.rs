use super::*;
use std::hash::{DefaultHasher, Hasher};

// One MiB per repository, including parent-directory fingerprints. Beyond
// this cap, the caller retains the conservative directory-based filter.
const MAX_PATHS: usize = 128 * 1024;

/// Compact, sorted fingerprints. Collisions can only cause an extra scan;
/// they never suppress a tracked change. No index or filename copies survive.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Paths(Vec<u64>);

impl Paths {
    pub fn from_index(
        index: &gix::index::State,
        directories: &HashSet<PathBuf>,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, String> {
        let count = index.entries().len().saturating_add(directories.len());
        if count > MAX_PATHS {
            return Ok(None);
        }
        let mut hashes = Vec::with_capacity(count);
        for entry in index.entries() {
            if cancel.load(Ordering::Relaxed) {
                return Err("Git status cancelled".into());
            }
            let path = gix::path::from_bstr(entry.path(index)).map_err(|e| e.to_string())?;
            hashes.extend(fingerprint(&path));
        }
        hashes.extend(directories.iter().filter_map(|path| fingerprint(path)));
        hashes.sort_unstable();
        hashes.dedup();
        hashes.shrink_to_fit();
        Ok(Some(Self(hashes)))
    }

    pub fn may_contain(&self, path: &Path) -> bool {
        // Filesystem Unicode normalization/case folding varies by volume.
        // Keep non-ASCII events rather than risk dropping a tracked edit.
        fingerprint(path).is_none_or(|hash| self.0.binary_search(&hash).is_ok())
    }
}

fn fingerprint(path: &Path) -> Option<u64> {
    if !path.as_os_str().as_encoded_bytes().is_ascii() {
        return None;
    }
    let mut hash = DefaultHasher::new();
    for component in path.components() {
        for byte in component.as_os_str().as_encoded_bytes() {
            hash.write_u8(byte.to_ascii_lowercase());
        }
        // Components normalize native separators. Case folding is conservative
        // even on case-sensitive volumes and supports ordinary macOS/Windows.
        hash.write_u8(b'/');
    }
    Some(hash.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn membership_cap_falls_back_without_losing_tracked_events() {
        let mut index = gix::index::State::new(gix::hash::Kind::Sha1);
        for i in 0..MAX_PATHS {
            if i == MAX_PATHS - 1 {
                let plan =
                    Plan::from_index(&index, Path::new("project"), &[], &AtomicBool::new(false))
                        .unwrap();
                let tracked = plan.tracked.as_ref().unwrap();
                assert!(tracked.0.capacity() * size_of::<u64>() <= 1024 * 1024);
                assert!(plan.relevant(Path::new("project/f000000")));
                assert!(!plan.relevant(Path::new("project/generated.part")));
            }
            index.dangerously_push_entry(
                Default::default(),
                gix::hash::ObjectId::null(gix::hash::Kind::Sha1),
                gix::index::entry::Flags::empty(),
                gix::index::entry::Mode::FILE,
                format!("f{i:06}").as_str().into(),
            );
        }
        let plan =
            Plan::from_index(&index, Path::new("project"), &[], &AtomicBool::new(false)).unwrap();
        assert!(plan.tracked.is_none());
        assert!(plan.relevant(Path::new("project/f131071")));
        assert!(plan.relevant(Path::new("project/generated.part")));
    }

    #[test]
    fn fingerprints_preserve_component_boundaries_and_native_separators() {
        assert_eq!(
            fingerprint(Path::new("SRC/FILE")),
            fingerprint(Path::new("src/file"))
        );
        assert_ne!(
            fingerprint(Path::new("srcfile")),
            fingerprint(Path::new("src/file"))
        );
        assert!(fingerprint(Path::new("src/é.txt")).is_none());
        #[cfg(windows)]
        assert_eq!(
            fingerprint(Path::new("src\\file")),
            fingerprint(Path::new("src/file"))
        );
    }

    #[test]
    fn sparse_directory_entries_keep_descendant_events_and_polling_fallback() {
        let mut index = gix::index::State::new(gix::hash::Kind::Sha1);
        index.dangerously_push_entry(
            Default::default(),
            gix::hash::ObjectId::null(gix::hash::Kind::Sha1),
            gix::index::entry::Flags::empty(),
            gix::index::entry::Mode::DIR,
            "sparse/".into(),
        );
        let plan =
            Plan::from_index(&index, Path::new("project"), &[], &AtomicBool::new(false)).unwrap();
        assert!(plan.tracked.is_none());
        assert!(plan.directories.is_none());
        assert!(!plan.paths().1);
        assert!(plan.relevant(Path::new("project/sparse/src/file.rs")));
    }
}
