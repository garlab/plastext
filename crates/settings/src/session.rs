//! Bounded folder sessions. Disk access is explicit and belongs on a worker.
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

const VERSION: u32 = 1;
const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_FOLDERS: usize = 20;
const MAX_PANES: usize = 64;
const MAX_TABS: usize = 4096;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub layout: SessionLayout,
    pub panes: Vec<SessionPane>,
    pub active_pane: usize,
    pub sidebar_width: f32,
    pub zoom: u16,
    pub sync_scroll: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SessionLayout {
    Pane(usize),
    Split {
        vertical: bool,
        children: Vec<SessionChild>,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionChild {
    pub layout: SessionLayout,
    pub weight: f32,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionPane {
    pub tabs: Vec<SessionTab>,
    pub active: Option<usize>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionTab {
    pub path: PathBuf,
    /// The transient, italic tab rather than its document viewing mode.
    pub preview: bool,
    pub view: ViewState,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewMode {
    #[default]
    Source,
    Rendered,
    Pdf,
    Binary,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ViewState {
    pub mode: ViewMode,
    pub offset: u64,
    pub scroll: Option<SessionScroll>,
    pub pdf: Option<PdfPosition>,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourcePosition {
    pub start: u64,
    pub end: u64,
    pub fraction: f32,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionScroll {
    pub source: SourcePosition,
    pub rendered: Option<(usize, SourcePosition)>,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PdfPosition {
    pub page: u32,
    pub fit_page: bool,
    pub pan: [f64; 2],
}

impl Session {
    pub fn validate(&self) -> io::Result<()> {
        fn layout(node: &SessionLayout, depth: usize, seen: &mut [bool]) -> bool {
            if depth > 32 {
                return false;
            }
            match node {
                SessionLayout::Pane(id) => match seen.get_mut(*id) {
                    Some(found) if !*found => {
                        *found = true;
                        true
                    }
                    _ => false,
                },
                SessionLayout::Split { children, .. } => {
                    children.len() >= 2
                        && children.len() <= MAX_PANES
                        && children.iter().all(|c| {
                            c.weight.is_finite()
                                && c.weight > 0.
                                && c.weight <= 1_000_000.
                                && layout(&c.layout, depth + 1, seen)
                        })
                }
            }
        }
        let mut seen = vec![false; self.panes.len().min(MAX_PANES)];
        if self.panes.is_empty()
            || self.panes.len() > MAX_PANES
            || self.active_pane >= self.panes.len()
            || !self.sidebar_width.is_finite()
            || !(0. ..=10000.).contains(&self.sidebar_width)
            || !(50..=200).contains(&self.zoom)
            || !layout(&self.layout, 0, &mut seen)
            || seen.contains(&false)
            || self.panes.iter().map(|p| p.tabs.len()).sum::<usize>() > MAX_TABS
        {
            return Err(invalid());
        }
        for pane in &self.panes {
            if pane.active.is_some_and(|i| i >= pane.tabs.len())
                || (!pane.tabs.is_empty() && pane.active.is_none())
                || pane.tabs.iter().filter(|t| t.preview).count() > 1
            {
                return Err(invalid());
            }
            for (i, tab) in pane.tabs.iter().enumerate() {
                if !tab.path.is_absolute()
                    || tab.path.as_os_str().len() > 8192
                    || pane.tabs[..i].iter().any(|t| t.path == tab.path)
                    || !tab.view.valid()
                {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }
}
impl ViewState {
    fn valid(&self) -> bool {
        let position = |p: SourcePosition| {
            p.end >= p.start && p.fraction.is_finite() && (0. ..=1.).contains(&p.fraction)
        };
        self.scroll
            .is_none_or(|s| position(s.source) && s.rendered.is_none_or(|(_, p)| position(p)))
            && self
                .pdf
                .is_none_or(|p| p.page > 0 && p.pan.iter().all(|n| n.is_finite() && n.abs() <= 1e9))
    }
}
fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Invalid or oversized folder session",
    )
}

#[derive(Serialize, Deserialize)]
struct Saved {
    root: PathBuf,
    session: Session,
}
#[derive(Serialize, Deserialize)]
struct Store {
    version: u32,
    folders: Vec<Saved>,
}
impl Default for Store {
    fn default() -> Self {
        Self {
            version: VERSION,
            folders: Vec::new(),
        }
    }
}
fn root_key(root: &Path) -> PathBuf {
    fs::canonicalize(root).unwrap_or_else(|_| root.to_owned())
}
fn read(path: &Path) -> io::Result<Store> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Store::default()),
        Err(e) => return Err(e),
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BYTES {
        return Err(invalid());
    }
    let store: Store = serde_json::from_slice(&bytes)?;
    if store.version != VERSION || store.folders.len() > MAX_FOLDERS {
        return Err(invalid());
    }
    for folder in &store.folders {
        if !folder.root.is_absolute() {
            return Err(invalid());
        }
        folder.session.validate()?;
    }
    Ok(store)
}

/// Small metadata only; never contains document contents or unsaved edits.
pub struct Sessions;
impl Sessions {
    pub fn path() -> Option<PathBuf> {
        Some(crate::Settings::path()?.with_file_name("sessions.json"))
    }
    pub fn load(path: &Path, root: &Path) -> io::Result<Option<Session>> {
        let key = root_key(root);
        Ok(read(path)?
            .folders
            .into_iter()
            .find(|f| f.root == key)
            .map(|f| f.session))
    }
    /// One atomic write for a batch of windows; identical records do not write.
    /// Returns whether storage changed. Call only after a close is approved.
    pub fn save(path: &Path, sessions: Vec<(PathBuf, Session)>) -> io::Result<bool> {
        if sessions.is_empty() {
            return Ok(false);
        }
        let mut store = read(path)?;
        let mut changed = false;
        for (root, session) in sessions {
            session.validate()?;
            let root = root_key(&root);
            if !root.is_absolute() {
                return Err(invalid());
            }
            if store
                .folders
                .iter()
                .any(|f| f.root == root && f.session == session)
            {
                continue;
            }
            store.folders.retain(|f| f.root != root);
            store.folders.insert(0, Saved { root, session });
            store.folders.truncate(MAX_FOLDERS);
            changed = true;
        }
        if !changed {
            return Ok(false);
        }
        let bytes = loop {
            let bytes = serde_json::to_vec(&store)?;
            if bytes.len() < MAX_BYTES {
                break bytes;
            }
            if store.folders.len() <= 1 {
                return Err(invalid());
            }
            store.folders.pop();
        };
        let parent = path.parent().ok_or_else(invalid)?;
        fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(&bytes)?;
        file.write_all(b"\n")?;
        file.persist(path).map_err(|e| e.error)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn session(root: &Path) -> Session {
        Session {
            layout: SessionLayout::Pane(0),
            active_pane: 0,
            sidebar_width: 240.,
            zoom: 100,
            sync_scroll: false,
            panes: vec![SessionPane {
                active: Some(0),
                tabs: vec![SessionTab {
                    path: root.join("notes.tex"),
                    preview: false,
                    view: ViewState {
                        mode: ViewMode::Rendered,
                        offset: 400,
                        scroll: Some(SessionScroll {
                            source: SourcePosition {
                                start: 400,
                                end: 420,
                                fraction: 0.5,
                            },
                            rendered: None,
                        }),
                        pdf: None,
                    },
                }],
            }],
        }
    }
    #[test]
    fn roundtrip_skip_unchanged_and_bound_folder_history() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("state/sessions.json");
        for n in 0..25 {
            let root = dir.path().join(format!("folder-{n}"));
            Sessions::save(&file, vec![(root.clone(), session(&root))]).unwrap();
        }
        assert_eq!(read(&file).unwrap().folders.len(), MAX_FOLDERS);
        let root = dir.path().join("folder-24");
        let state = session(&root);
        assert_eq!(Sessions::load(&file, &root).unwrap(), Some(state.clone()));
        let before = fs::metadata(&file).unwrap().modified().unwrap();
        assert!(!Sessions::save(&file, vec![(root.clone(), state)]).unwrap());
        assert_eq!(fs::metadata(&file).unwrap().modified().unwrap(), before);
        assert!(
            Sessions::load(&file, &dir.path().join("folder-0"))
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn validates_layout_indices_weights_and_untrusted_state() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = session(dir.path());
        state.layout = SessionLayout::Split {
            vertical: false,
            children: vec![
                SessionChild {
                    layout: SessionLayout::Pane(0),
                    weight: 1.,
                },
                SessionChild {
                    layout: SessionLayout::Pane(0),
                    weight: 1.,
                },
            ],
        };
        assert!(state.validate().is_err());
        state.layout = SessionLayout::Pane(0);
        state.panes[0].active = Some(4);
        assert!(state.validate().is_err());
        let file = dir.path().join("sessions.json");
        fs::write(&file, "{\"version\":999,\"folders\":[]}").unwrap();
        assert!(Sessions::load(&file, dir.path()).is_err());
        fs::write(&file, vec![b' '; MAX_BYTES + 1]).unwrap();
        assert!(Sessions::load(&file, dir.path()).is_err());
    }
}
