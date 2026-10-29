//! The declarative SVG/PNG subset of VS Code file-icon themes.
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

const MAX_JSON_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct IconSettings {
    /// `material`, `none`, or the path of a registered theme JSON file.
    pub theme: String,
    pub imports: Vec<Registration>,
    pub high_contrast: bool,
}

impl Default for IconSettings {
    fn default() -> Self {
        Self {
            theme: "material".into(),
            imports: Vec::new(),
            high_contrast: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Registration {
    pub name: String,
    pub path: PathBuf,
    /// The directory that owns the theme and its assets.
    pub root: PathBuf,
}

impl Registration {
    pub fn id(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}

impl IconSettings {
    pub(crate) fn resolve_paths(&mut self, base: &Path) {
        if self.theme != "material" && self.theme != "none" {
            self.theme = base.join(&self.theme).to_string_lossy().into_owned();
        }
        for entry in &mut self.imports {
            entry.path = base.join(&entry.path);
            entry.root = base.join(&entry.root);
        }
    }

    /// Save only the icons section, preserving the rest of the user's TOML and
    /// comments. Call on a worker, with writes serialized by the caller.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};
        // Keep dotfile-manager symlinks intact when atomically replacing the file.
        let target = if fs::symlink_metadata(path).is_ok_and(|meta| meta.is_symlink()) {
            Some(path.canonicalize().map_err(|e| e.to_string())?)
        } else {
            None
        };
        let path = target.as_deref().unwrap_or(path);
        let old = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e.to_string()),
        };
        let mut document = old.parse::<DocumentMut>().map_err(|e| e.to_string())?;
        if !document.contains_key("icons") {
            document["icons"] = Item::Table(Table::new());
        }
        let table = document["icons"]
            .as_table_mut()
            .ok_or("[icons] must be a table")?;
        table["theme"] = value(&self.theme);
        table["high_contrast"] = value(self.high_contrast);
        let mut entries = ArrayOfTables::new();
        for entry in &self.imports {
            let mut table = Table::new();
            table["name"] = value(&entry.name);
            table["path"] = value(entry.path.to_string_lossy().as_ref());
            table["root"] = value(entry.root.to_string_lossy().as_ref());
            entries.push(table);
        }
        table["imports"] = if self.imports.is_empty() {
            value(toml_edit::Array::new())
        } else {
            Item::ArrayOfTables(entries)
        };
        let parent = path.parent().ok_or("Settings path has no parent")?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        if let Ok(meta) = fs::metadata(path) {
            temp.as_file()
                .set_permissions(meta.permissions())
                .map_err(|e| e.to_string())?;
        }
        temp.write_all(document.to_string().as_bytes())
            .map_err(|e| e.to_string())?;
        temp.as_file().sync_all().map_err(|e| e.to_string())?;
        temp.persist(path).map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub enum Appearance {
    Dark,
    Light,
    HighContrast,
}

#[derive(Clone, Default, Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Associations {
    file: Option<String>,
    folder: Option<String>,
    folder_expanded: Option<String>,
    root_folder: Option<String>,
    root_folder_expanded: Option<String>,
    file_names: HashMap<String, String>,
    file_extensions: HashMap<String, String>,
    language_ids: HashMap<String, String>,
    folder_names: HashMap<String, String>,
    folder_names_expanded: HashMap<String, String>,
    root_folder_names: HashMap<String, String>,
    root_folder_names_expanded: HashMap<String, String>,
}

impl Associations {
    fn normalize(&mut self) {
        for map in [
            &mut self.file_names,
            &mut self.file_extensions,
            &mut self.folder_names,
            &mut self.folder_names_expanded,
            &mut self.root_folder_names,
            &mut self.root_folder_names_expanded,
        ] {
            *map = map.drain().map(|(k, v)| (k.to_lowercase(), v)).collect();
        }
    }
    fn overlay(&mut self, other: Self) {
        macro_rules! scalars { ($($field:ident),+) => { $(if other.$field.is_some() { self.$field = other.$field; })+ }; }
        scalars!(
            file,
            folder,
            folder_expanded,
            root_folder,
            root_folder_expanded
        );
        macro_rules! maps { ($($field:ident),+) => { $(self.$field.extend(other.$field);)+ }; }
        maps!(
            file_names,
            file_extensions,
            language_ids,
            folder_names,
            folder_names_expanded,
            root_folder_names,
            root_folder_names_expanded
        );
    }
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Definition {
    icon_path: Option<String>,
    font_character: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct RawTheme {
    icon_definitions: HashMap<String, Definition>,
    #[serde(flatten)]
    associations: Associations,
    light: Associations,
    high_contrast: Associations,
    hides_explorer_arrows: bool,
}

#[derive(Debug)]
pub struct IconTheme {
    definitions: HashMap<String, String>,
    dark: Associations,
    light: Associations,
    high_contrast: Associations,
    pub hides_explorer_arrows: bool,
}

impl IconTheme {
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() as u64 > MAX_JSON_BYTES {
            return Err("Icon theme exceeds 2 MiB".into());
        }
        let json = crate::vscode::jsonc(text.trim_start_matches('\u{feff}'))?;
        let mut raw: RawTheme = serde_json::from_slice(&json).map_err(|e| e.to_string())?;
        if raw.icon_definitions.is_empty() {
            return Err("No iconDefinitions found in this file".into());
        }
        if raw
            .icon_definitions
            .values()
            .any(|d| d.font_character.is_some())
        {
            return Err(
                "This theme uses icon fonts. Plastext currently imports SVG/PNG icon themes only."
                    .into(),
            );
        }
        let definitions = raw
            .icon_definitions
            .into_iter()
            .filter_map(|(id, d)| d.icon_path.map(|p| (id, p)))
            .map(|(id, path)| {
                let extension = Path::new(&path)
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if !matches!(extension.as_str(), "svg" | "png") {
                    return Err(format!("Unsupported icon asset {path}; use SVG or PNG"));
                }
                Ok((id, path))
            })
            .collect::<Result<HashMap<_, _>, String>>()?;
        raw.associations.normalize();
        raw.light.normalize();
        raw.high_contrast.normalize();
        // Appearance overrides replace associations before matching. A light
        // generic file icon must not override a more specific dark filename rule.
        let dark = raw.associations;
        let mut light = dark.clone();
        light.overlay(raw.light);
        let mut high_contrast = dark.clone();
        high_contrast.overlay(raw.high_contrast);
        let theme = Self {
            definitions,
            dark,
            light,
            high_contrast,
            hides_explorer_arrows: raw.hides_explorer_arrows,
        };
        for rules in [&theme.dark, &theme.light, &theme.high_contrast] {
            let ids = [
                &rules.file,
                &rules.folder,
                &rules.folder_expanded,
                &rules.root_folder,
                &rules.root_folder_expanded,
            ]
            .into_iter()
            .filter_map(Option::as_ref)
            .chain(
                [
                    &rules.file_names,
                    &rules.file_extensions,
                    &rules.language_ids,
                    &rules.folder_names,
                    &rules.folder_names_expanded,
                    &rules.root_folder_names,
                    &rules.root_folder_names_expanded,
                ]
                .into_iter()
                .flat_map(|m| m.values()),
            );
            for id in ids {
                if !id.is_empty() && !theme.definitions.contains_key(id) {
                    return Err(format!("Undefined icon: {id}"));
                }
            }
        }
        Ok(theme)
    }

    pub fn resolve(
        &self,
        path: &Path,
        directory: bool,
        expanded: bool,
        root: bool,
        language: Option<&str>,
        appearance: Appearance,
    ) -> Option<&str> {
        let rules = match appearance {
            Appearance::Dark => &self.dark,
            Appearance::Light => &self.light,
            Appearance::HighContrast => &self.high_contrast,
        };
        let name = path.file_name()?.to_string_lossy().to_lowercase();
        let parent = path
            .parent()
            .and_then(Path::file_name)
            .map(|p| p.to_string_lossy().to_lowercase());
        let named = |map: &HashMap<String, String>| -> Option<String> {
            parent
                .as_ref()
                .and_then(|p| map.get(&format!("{p}/{name}")))
                .or_else(|| map.get(&name))
                .cloned()
        };
        let id = if directory {
            if root {
                expanded
                    .then(|| rules.root_folder_names_expanded.get(&name).cloned())
                    .flatten()
                    .or_else(|| rules.root_folder_names.get(&name).cloned())
                    .or_else(|| {
                        expanded
                            .then(|| rules.root_folder_expanded.clone())
                            .flatten()
                    })
                    .or_else(|| rules.root_folder.clone())
            } else {
                None
            }
            .or_else(|| {
                expanded
                    .then(|| named(&rules.folder_names_expanded))
                    .flatten()
            })
            .or_else(|| named(&rules.folder_names))
            .or_else(|| expanded.then(|| rules.folder_expanded.clone()).flatten())
            .or_else(|| rules.folder.clone())
        } else {
            // Parent-qualified extensions win over all unqualified extensions;
            // within either group, the longest suffix wins (d.ts before ts).
            let extensions = |qualified: bool| {
                name.match_indices('.').find_map(|(i, _)| {
                    let suffix = &name[i + 1..];
                    if qualified {
                        parent
                            .as_ref()
                            .and_then(|p| rules.file_extensions.get(&format!("{p}/{suffix}")))
                            .cloned()
                    } else {
                        rules.file_extensions.get(suffix).cloned()
                    }
                })
            };
            named(&rules.file_names)
                .or_else(|| extensions(true))
                .or_else(|| extensions(false))
                .or_else(|| language.and_then(|id| rules.language_ids.get(id)).cloned())
                .or_else(|| rules.file.clone())
        }?;
        self.definitions.get(&id).map(String::as_str)
    }
}

fn read_json(path: &Path) -> Result<String, String> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|f| f.take(MAX_JSON_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() as u64 > MAX_JSON_BYTES {
        return Err("Theme JSON exceeds 2 MiB".into());
    }
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

/// Resolve a local asset while allowing ../icons inside an extension directory.
/// The loader also checks canonical paths when opening it, to contain symlinks.
pub fn asset_path(entry: &Registration, relative: &str) -> Result<PathBuf, String> {
    let relative = relative.replace('\\', "/");
    if relative.contains(':') || Path::new(&relative).is_absolute() {
        return Err("Icon paths must be relative local paths".into());
    }
    let mut path = entry
        .path
        .parent()
        .ok_or("Theme has no directory")?
        .to_owned();
    for component in Path::new(&relative).components() {
        match component {
            Component::Normal(part) => path.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                path.pop();
            }
            _ => return Err("Invalid icon path".into()),
        }
    }
    if !path.starts_with(&entry.root) {
        return Err(
            "Icon asset leaves its theme directory; import the extension folder instead".into(),
        );
    }
    Ok(path)
}

pub fn load(entry: &Registration) -> Result<IconTheme, String> {
    let path = entry.path.canonicalize().map_err(|e| e.to_string())?;
    let root = entry.root.canonicalize().map_err(|e| e.to_string())?;
    if !path.starts_with(&root) {
        return Err("Theme file leaves its extension directory".into());
    }
    let theme = IconTheme::parse(&read_json(&path)?)?;
    for relative in theme.definitions.values() {
        asset_path(entry, relative)?;
    }
    Ok(theme)
}

#[derive(Deserialize)]
struct Manifest {
    #[serde(default)]
    contributes: Contributions,
}
#[derive(Default, Deserialize)]
struct Contributions {
    #[serde(default, rename = "iconThemes")]
    themes: Vec<Contribution>,
}
#[derive(Deserialize)]
struct Contribution {
    id: String,
    label: Option<String>,
    path: String,
}

/// Discover a selected JSON file or themes contributed by an extracted extension.
pub fn discover(path: &Path) -> Result<Vec<Registration>, String> {
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    if path.is_dir() || path.file_name().is_some_and(|p| p == "package.json") {
        let mut root = if path.is_dir() {
            path
        } else {
            path.parent().unwrap().to_owned()
        };
        if !root.join("package.json").is_file() && root.join("extension/package.json").is_file() {
            root = root
                .join("extension")
                .canonicalize()
                .map_err(|e| e.to_string())?;
        }
        return manifest_themes(&root);
    }
    // A theme picked directly from an extension can use sibling ../icons assets.
    for parent in path.ancestors().skip(1).take(4) {
        if parent.join("package.json").is_file()
            && let Ok(entries) = manifest_themes(parent)
            && let Some(entry) = entries.into_iter().find(|entry| entry.path == path)
        {
            return Ok(vec![entry]);
        }
    }
    Ok(vec![Registration {
        name: path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        root: path.parent().ok_or("Theme has no directory")?.to_owned(),
        path,
    }])
}

fn manifest_themes(root: &Path) -> Result<Vec<Registration>, String> {
    let text = read_json(&root.join("package.json"))?;
    let manifest: Manifest =
        serde_json::from_slice(&crate::vscode::jsonc(&text)?).map_err(|e| e.to_string())?;
    if manifest.contributes.themes.is_empty() {
        return Err("This folder has no contributed file-icon themes".into());
    }
    if manifest.contributes.themes.len() > 64 {
        return Err("Extension contributes more than 64 icon themes".into());
    }
    let translations: HashMap<String, String> = read_json(&root.join("package.nls.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    manifest
        .contributes
        .themes
        .into_iter()
        .map(|theme| {
            let placeholder = Registration {
                name: String::new(),
                path: root.join("package.json"),
                root: root.to_owned(),
            };
            let path = asset_path(&placeholder, &theme.path)?
                .canonicalize()
                .map_err(|e| e.to_string())?;
            if !path.starts_with(root) {
                return Err("Theme file leaves the extension directory".into());
            }
            let mut name = theme.label.unwrap_or(theme.id);
            if let Some(key) = name.strip_prefix('%').and_then(|s| s.strip_suffix('%')) {
                name = translations.get(key).cloned().unwrap_or(name);
            }
            Ok(Registration {
                name,
                path,
                root: root.to_owned(),
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "icons_tests.rs"]
mod tests;
