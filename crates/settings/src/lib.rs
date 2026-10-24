//! Configuration, themes, keymaps and hot reload (spec §5.10).
//!
//! So far: [`Settings`] loaded from `config.toml`, with defaults for whatever
//! the file leaves out.

mod byte_size;

use std::{fmt, fs, io, path::PathBuf};

use serde::Deserialize;

pub use byte_size::ByteSize;

/// Everything configurable, as read from `config.toml` (spec CFG-1).
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub editor: Editor,
    pub files: Files,
    pub limits: Limits,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct Editor {
    pub font: String,
    pub font_size: f32,
    /// A multiple of the font size.
    pub line_height: f32,
    pub tab_size: usize,
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            font: "Menlo".into(),
            font_size: 13.,
            line_height: 1.5,
            tab_size: 4,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct Files {
    pub show_hidden: bool,
    pub show_ignored: ShowIgnored,
    /// Globs, relative to the project root, of paths never shown.
    pub exclude: Vec<String>,
}

impl Default for Files {
    fn default() -> Self {
        Self {
            show_hidden: false,
            show_ignored: ShowIgnored::Dim,
            exclude: vec!["**/.git".into(), "**/.DS_Store".into()],
        }
    }
}

/// How the file tree lists entries matched by ignore files.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ShowIgnored {
    Hide,
    #[default]
    Dim,
    Show,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct Limits {
    /// Files up to this size are read whole; larger ones in blocks on demand.
    pub large_file_threshold: ByteSize,
    /// Memory for the cached blocks of each large file.
    pub block_cache: ByteSize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            large_file_threshold: ByteSize(64_000_000),
            block_cache: ByteSize(256_000_000),
        }
    }
}

/// Why `config.toml` couldn't be used.
#[derive(Debug)]
pub enum LoadError {
    Read(PathBuf, io::Error),
    Parse(PathBuf, Box<toml::de::Error>),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Read(path, err) => write!(f, "Couldn't read {}: {err}", path.display()),
            Self::Parse(path, err) => write!(f, "{} is invalid: {}", path.display(), err.message()),
        }
    }
}

impl std::error::Error for LoadError {}

impl Settings {
    /// Where the settings file lives: `$XDG_CONFIG_HOME/plastext/config.toml`,
    /// by default under `~/.config`, or `%APPDATA%\plastext\config.toml` on Windows.
    pub fn path() -> Option<PathBuf> {
        let dir = if cfg!(windows) {
            PathBuf::from(std::env::var_os("APPDATA")?)
        } else if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").filter(|dir| !dir.is_empty())
        {
            PathBuf::from(dir)
        } else {
            PathBuf::from(std::env::var_os("HOME")?).join(".config")
        };
        Some(dir.join("plastext").join("config.toml"))
    }

    /// Reads the settings file. A missing file means the defaults. Blocks:
    /// call it off the main thread.
    pub fn load() -> Result<Self, LoadError> {
        let Some(path) = Self::path() else {
            return Ok(Self::default());
        };
        match fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text).map_err(|err| LoadError::Parse(path, Box::new(err))),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(LoadError::Read(path, err)),
        }
    }

    pub fn parse(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_settings_take_their_defaults() {
        let settings = Settings::parse(
            r#"
            [editor]
            font_size = 15

            [files]
            show_ignored = "hide"

            [limits]
            large_file_threshold = "16MB"

            [markdown]
            default_view = "split"
            "#,
        )
        .unwrap();
        assert_eq!(settings.editor.font_size, 15.);
        assert_eq!(settings.editor.font, "Menlo");
        assert_eq!(settings.files.show_ignored, ShowIgnored::Hide);
        assert_eq!(settings.files.exclude, Files::default().exclude);
        assert_eq!(settings.limits.large_file_threshold, ByteSize(16_000_000));
        assert_eq!(settings.limits.block_cache, Limits::default().block_cache);
        assert_eq!(Settings::parse("").unwrap(), Settings::default());
    }

    #[test]
    fn invalid_settings_are_rejected() {
        assert!(Settings::parse("[editor]\nfont_size = \"big\"").is_err());
        assert!(Settings::parse("[files]\nshow_ignored = \"sometimes\"").is_err());
        assert!(Settings::parse("[limits]\nblock_cache = \"lots\"").is_err());
    }
}
