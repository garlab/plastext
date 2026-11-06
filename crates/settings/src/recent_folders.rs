//! Small local folder history, kept separately from user configuration.
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

const LIMIT: usize = 10;
const MAX_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct RecentFolders(Vec<PathBuf>);

impl RecentFolders {
    pub fn path() -> Option<PathBuf> {
        Some(crate::Settings::path()?.with_file_name("recent-folders.json"))
    }

    pub fn folders(&self) -> &[PathBuf] {
        &self.0
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// Most recently opened first. Callers resolve paths before recording them.
    pub fn record(&mut self, path: PathBuf) -> bool {
        if !path.is_absolute() || self.0.first() == Some(&path) {
            return false;
        }
        self.0.retain(|old| old != &path);
        self.0.insert(0, path);
        self.0.truncate(LIMIT);
        true
    }

    /// Blocking, bounded disk access; call on a worker.
    pub fn load(path: &Path) -> io::Result<Self> {
        let file = match fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "folder history is too large",
            ));
        }
        let mut history: Self = serde_json::from_slice(&bytes)?;
        let mut unique = Vec::new();
        for path in history.0 {
            if path.is_absolute() && !unique.contains(&path) {
                unique.push(path);
                if unique.len() == LIMIT {
                    break;
                }
            }
        }
        history.0 = unique;
        Ok(history)
    }

    /// Atomically replace history, without touching config.toml. Call on a worker.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let bytes = serde_json::to_vec(self)?;
        if bytes.len() as u64 + 1 > MAX_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "folder history is too large",
            ));
        }
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("missing history directory"))?;
        fs::create_dir_all(parent)?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        temp.write_all(&bytes)?;
        temp.write_all(b"\n")?;
        temp.persist(path).map_err(|error| error.error)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_is_bounded_deduplicated_and_persists_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("state/recent-folders.json");
        let mut history = RecentFolders::load(&file).unwrap();
        for i in 0..15 {
            history.record(dir.path().join(format!("folder-{i}")));
        }
        assert_eq!(history.folders().len(), LIMIT);
        let reused = dir.path().join("folder-8");
        assert!(history.record(reused.clone()));
        assert!(!history.record(reused.clone()));
        assert!(!history.record(PathBuf::from("relative")));
        assert_eq!(history.folders()[0], reused);
        history.save(&file).unwrap();
        assert_eq!(RecentFolders::load(&file).unwrap(), history);
        history.clear();
        history.save(&file).unwrap();
        assert!(RecentFolders::load(&file).unwrap().folders().is_empty());
    }

    #[test]
    fn malformed_and_oversized_history_is_rejected() {
        let file = tempfile::NamedTempFile::new().unwrap();
        fs::write(file.path(), "not json").unwrap();
        assert!(RecentFolders::load(file.path()).is_err());
        fs::write(file.path(), vec![b' '; MAX_BYTES as usize + 1]).unwrap();
        assert!(RecentFolders::load(file.path()).is_err());
    }
}
