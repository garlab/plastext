//! Shared theme selection and a bounded cache for visible file-tree icons.
use crate::file_icons::{Assets, MATERIAL_THEME};
use gpui::{
    App, AppContext, AssetSource, Context, Entity, EventEmitter, Global, RenderImage, Task,
};
use plastext_settings::{
    Settings,
    icons::{self, Icon, IconSettings, IconTheme, Registration},
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::Path,
    sync::{Arc, LazyLock},
};

const CACHE_ENTRIES: usize = 128;
const MAX_PENDING: usize = 4;

pub struct LoadedTheme {
    pub rules: IconTheme,
    origin: Option<Registration>,
    fonts: HashMap<String, plastext_assets::icons::font::Font>,
}

impl LoadedTheme {
    fn load(entry: Registration) -> Result<Self, String> {
        let rules = icons::load(&entry)?;
        let mut fonts = HashMap::new();
        let mut bytes = 0;
        // Read fonts once, off the UI thread. Glyph images are still rasterized
        // lazily. Keep all decoded font data within a separate 8 MiB budget.
        for font in &rules.fonts {
            let mut loaded = Err(format!("No usable source for icon font {}", font.id));
            for source in &font.sources {
                loaded = icons::asset_path(&entry, source)
                    .and_then(|path| plastext_assets::icons::font::Font::read(&path, &entry.root));
                if loaded.is_ok() {
                    break;
                }
            }
            let loaded = loaded?;
            bytes += loaded.retained_bytes();
            if bytes > 8 * 1024 * 1024 {
                return Err("Icon theme fonts exceed the 8 MiB decoded font budget".into());
            }
            fonts.insert(font.id.clone(), loaded);
        }
        Ok(Self {
            rules,
            origin: Some(entry),
            fonts,
        })
    }
}

static MATERIAL: LazyLock<Arc<LoadedTheme>> = LazyLock::new(|| {
    Arc::new(LoadedTheme {
        rules: IconTheme::parse(MATERIAL_THEME).expect("bundled icon theme is valid"),
        origin: None,
        fonts: HashMap::new(),
    })
});

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct Key {
    icon: Icon,
    edge: u32,
}

struct Cache<T> {
    values: HashMap<Key, T>,
    order: VecDeque<Key>,
}
impl<T> Default for Cache<T> {
    fn default() -> Self {
        Self {
            values: HashMap::new(),
            order: VecDeque::new(),
        }
    }
}
impl<T> Cache<T> {
    fn get(&mut self, key: &Key) -> Option<&T> {
        if self.values.contains_key(key) {
            self.order.retain(|k| k != key);
            self.order.push_back(key.clone());
        }
        self.values.get(key)
    }
    fn insert(&mut self, key: Key, value: T) {
        self.order.retain(|k| k != &key);
        self.order.push_back(key.clone());
        self.values.insert(key, value);
        while self.order.len() > CACHE_ENTRIES {
            if let Some(old) = self.order.pop_front() {
                self.values.remove(&old);
            }
        }
    }
}

struct Store(Entity<IconThemes>);
impl Global for Store {}
pub struct IconNotice(pub String);

pub struct IconThemes {
    pub preferences: IconSettings,
    pub active: String,
    pub error: Option<String>,
    current: Option<Arc<LoadedTheme>>,
    generation: u64,
    selection_request: u64,
    cache: Cache<Option<Arc<RenderImage>>>,
    pending: HashSet<Key>,
    in_flight: usize,
    reported_asset_error: bool,
    save: async_channel::Sender<IconSettings>,
    _saving: Task<()>,
}
impl EventEmitter<IconNotice> for IconThemes {}

impl IconThemes {
    pub fn shared(preferences: &IconSettings, cx: &mut App) -> Entity<Self> {
        if let Some(store) = cx.try_global::<Store>() {
            return store.0.clone();
        }
        let entity = cx.new(|cx: &mut Context<Self>| {
            let (save, receive) = async_channel::unbounded::<IconSettings>();
            let saving = cx.spawn(async move |this, cx| {
                // Serialize writes, coalescing pending changes. A slow earlier
                // write can never overwrite a newer selection.
                while let Ok(mut preferences) = receive.recv().await {
                    while let Ok(newer) = receive.try_recv() {
                        preferences = newer;
                    }
                    let work = cx.background_executor().spawn(async move {
                        let path = Settings::path().ok_or("Settings directory is unavailable")?;
                        preferences.save(&path)
                    });
                    if let Err(error) = work.await {
                        this.update(cx, |this, cx| {
                            this.report(
                                format!("Icon theme changed, but couldn't save settings: {error}"),
                                cx,
                            )
                        })
                        .ok();
                    }
                }
            });
            Self {
                preferences: preferences.clone(),
                active: "material".into(),
                error: None,
                current: Some(MATERIAL.clone()),
                generation: 0,
                selection_request: 0,
                cache: Cache::default(),
                pending: HashSet::new(),
                in_flight: 0,
                reported_asset_error: false,
                save,
                _saving: saving,
            }
        });
        cx.set_global(Store(entity.clone()));
        entity.update(cx, |this, cx| {
            this.select(preferences.theme.clone(), false, cx)
        });
        entity
    }

    pub fn current(&self) -> Option<Arc<LoadedTheme>> {
        self.current.clone()
    }

    fn report(&mut self, message: String, cx: &mut Context<Self>) {
        self.error = Some(message.clone());
        cx.emit(IconNotice(message));
    }

    pub fn register(&mut self, entries: Vec<Registration>, cx: &mut Context<Self>) {
        for entry in entries {
            self.preferences
                .imports
                .retain(|old| old.path != entry.path);
            self.preferences.imports.push(entry);
        }
        self.save.try_send(self.preferences.clone()).ok();
        cx.notify();
    }

    pub fn select(&mut self, id: String, persist: bool, cx: &mut Context<Self>) {
        self.selection_request += 1;
        let request = self.selection_request;
        match id.as_str() {
            "material" => self.apply(id, Some(MATERIAL.clone()), persist, cx),
            "none" => self.apply(id, None, persist, cx),
            _ => {
                let Some(entry) = self
                    .preferences
                    .imports
                    .iter()
                    .find(|entry| entry.id() == id)
                    .cloned()
                else {
                    self.report("The selected icon theme is not registered. Use File Icon Theme to import or choose one.".into(),cx);
                    return;
                };
                let work = cx
                    .background_executor()
                    .spawn(async move { LoadedTheme::load(entry).map(Arc::new) });
                cx.spawn(async move |this, cx| {
                    let result = work.await;
                    this.update(cx, |this, cx| {
                        if request != this.selection_request {
                            return;
                        }
                        match result {
                            Ok(theme) => this.apply(id, Some(theme), persist, cx),
                            Err(error) => {
                                this.report(format!("Couldn't load icon theme: {error}"), cx)
                            }
                        }
                    })
                    .ok();
                })
                .detach();
            }
        }
    }

    fn apply(
        &mut self,
        id: String,
        theme: Option<Arc<LoadedTheme>>,
        persist: bool,
        cx: &mut Context<Self>,
    ) {
        self.active = id.clone();
        self.error = None;
        self.preferences.theme = id;
        self.current = theme;
        self.generation += 1;
        self.cache = Cache::default();
        self.pending.clear();
        self.reported_asset_error = false;
        if persist {
            self.save.try_send(self.preferences.clone()).ok();
        }
        cx.notify();
    }

    pub fn image(
        &mut self,
        mut icon: Icon,
        scale: f32,
        foreground: u32,
        cx: &mut Context<Self>,
    ) -> Option<Arc<RenderImage>> {
        if let Icon::Glyph { color, .. } = &mut icon {
            *color = Some(color.unwrap_or(foreground));
        }
        let key = Key {
            icon,
            edge: (16. * scale.ceil()).clamp(16., 64.) as u32,
        };
        if let Some(image) = self.cache.get(&key) {
            return image.clone();
        }
        if self.in_flight >= MAX_PENDING || self.pending.contains(&key) {
            return None;
        }
        let theme = self.current.clone()?;
        self.pending.insert(key.clone());
        self.in_flight += 1;
        let generation = self.generation;
        let work = cx.background_executor().spawn({
            let key = key.clone();
            async move {
                let preview = match &key.icon {
                    Icon::Image(asset) => {
                        let bytes = if let Some(entry) = &theme.origin {
                            let path = icons::asset_path(entry, asset)?;
                            std::borrow::Cow::Owned(plastext_assets::icons::read(
                                &path,
                                &entry.root,
                            )?)
                        } else {
                            Assets
                                .load(&format!("file-icons/{asset}"))
                                .map_err(|e| e.to_string())?
                                .ok_or("Bundled icon is missing")?
                        };
                        plastext_assets::icons::decode(&bytes, key.edge)?
                    }
                    Icon::Glyph {
                        font,
                        character,
                        color,
                        size,
                    } => theme
                        .fonts
                        .get(font)
                        .ok_or("Icon font was not loaded")?
                        .render(*character, color.unwrap_or(foreground), *size, key.edge)?,
                };
                Ok::<_, String>(crate::image_view::render_image(preview).0)
            }
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                this.in_flight -= 1;
                if generation == this.generation {
                    this.pending.remove(&key);
                    if let Err(error) = &result
                        && !this.reported_asset_error
                    {
                        this.reported_asset_error = true;
                        this.report(
                            format!("Couldn't load an icon from this theme: {error}"),
                            cx,
                        );
                    }
                    this.cache.insert(key, result.ok());
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        None
    }
}

/// Known editor languages, detected from names only: icon lookup never opens a file.
pub fn language_id(path: &Path) -> Option<String> {
    if path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("jsx"))
    {
        return Some("javascriptreact".into());
    }
    let language = plastext_syntax::detect(path, b"")?;
    Some(match language.name {
        "TSX" => "typescriptreact".into(),
        "JSX" => "javascriptreact".into(),
        "Bash" => "shellscript".into(),
        "C++" => "cpp".into(),
        "C#" => "csharp".into(),
        other => other.to_ascii_lowercase(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editor_language_names_map_to_vscode_ids() {
        assert_eq!(
            language_id(Path::new("App.jsx")).as_deref(),
            Some("javascriptreact")
        );
        assert_eq!(
            language_id(Path::new("App.TSX")).as_deref(),
            Some("typescriptreact")
        );
        assert_eq!(
            language_id(Path::new("run.sh")).as_deref(),
            Some("shellscript")
        );
        assert_eq!(language_id(Path::new("main.cpp")).as_deref(), Some("cpp"));
    }
    #[test]
    fn cache_evicts_old_icons_and_releases_their_memory() {
        let key = |i| Key {
            icon: Icon::Image(format!("{i}.svg")),
            edge: 32,
        };
        let mut cache = Cache::default();
        let first = Arc::new(vec![0u8; 4096]);
        let weak = Arc::downgrade(&first);
        cache.insert(key(0), first);
        for i in 1..CACHE_ENTRIES {
            cache.insert(key(i), Arc::new(Vec::new()));
        }
        cache.get(&key(1));
        cache.insert(key(CACHE_ENTRIES), Arc::new(Vec::new()));
        assert_eq!(cache.values.len(), CACHE_ENTRIES);
        assert!(weak.upgrade().is_none());
        assert!(cache.get(&key(1)).is_some());
        cache = Cache::default();
        assert!(cache.values.is_empty());
    }
}
