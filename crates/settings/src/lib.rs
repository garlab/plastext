//! Configuration, themes, keymaps and hot reload (spec §5.10).
//!
//! So far: [`Settings`] loaded from `config.toml`, with defaults for whatever
//! the file leaves out.

mod byte_size;
pub mod editing;
mod icon_fonts;
pub mod icons;
mod markdown;
mod recent_folders;
mod theme;
mod vscode;

use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
};

use serde::Deserialize;

pub use byte_size::ByteSize;
pub use markdown::{Markdown, ProseWidth};
pub use recent_folders::RecentFolders;
pub use theme::{Color, SyntaxOverrides, ThemeOverrides, Themes};
pub use vscode::{ThemeImportError, import_vscode_theme};

/// Everything configurable, as read from `config.toml` (spec CFG-1).
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub editor: Editor,
    pub files: Files,
    pub limits: Limits,
    pub theme: Themes,
    pub markdown: Markdown,
    pub icons: icons::IconSettings,
    pub tex: Tex,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Tex {
    /// Open TeX in preview; compilation still pauses when no preview is visible.
    pub preview: bool,
    /// Optional installed TeX Live pdfLaTeX executable. Never a project command.
    pub compiler: Option<PathBuf>,
}
impl Default for Tex {
    fn default() -> Self {
        Self {
            preview: true,
            compiler: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct Editor {
    pub font: String,
    pub font_size: f32,
    /// A multiple of the font size.
    pub line_height: f32,
    pub tab_size: usize,
    /// Pin the first lines of the definitions enclosing the view's top.
    pub sticky_scroll: bool,
    pub open_mode: editing::OpenMode,
    pub atomic_save: bool,
    pub auto_indent: bool,
    pub auto_close_pairs: bool,
    pub detect_indentation: bool,
    pub insert_spaces: bool,
    pub indent_size: usize,
    pub editorconfig: bool,
    pub show_whitespace: bool,
    pub trim_trailing_whitespace: bool,
    pub insert_final_newline: Option<bool>,
    pub charset: Option<String>,
    pub end_of_line: Option<String>,
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            font: "Menlo".into(),
            font_size: 13.,
            line_height: 1.5,
            tab_size: 4,
            sticky_scroll: true,
            open_mode: editing::OpenMode::Edit,
            atomic_save: true,
            auto_indent: true,
            auto_close_pairs: false,
            detect_indentation: true,
            insert_spaces: true,
            indent_size: 4,
            editorconfig: true,
            show_whitespace: false,
            trim_trailing_whitespace: false,
            insert_final_newline: None,
            charset: None,
            end_of_line: None,
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
    /// Larger files aren't parsed for syntax highlighting.
    pub syntax_max_file_size: ByteSize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            large_file_threshold: ByteSize(64_000_000),
            block_cache: ByteSize(256_000_000),
            syntax_max_file_size: ByteSize(32_000_000),
        }
    }
}

/// Why `config.toml` couldn't be used.
#[derive(Debug)]
pub enum LoadError {
    Read(PathBuf, io::Error),
    Parse(PathBuf, Box<toml::de::Error>),
    Theme(ThemeImportError),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Read(path, err) => write!(f, "Couldn't read {}: {err}", path.display()),
            Self::Parse(path, err) => write!(f, "{} is invalid: {}", path.display(), err.message()),
            Self::Theme(err) => write!(f, "{err}"),
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
        Self::load_from(&path)
    }

    /// Loads settings and local theme imports off the main thread.
    pub fn load_from(path: &Path) -> Result<Self, LoadError> {
        match fs::read_to_string(path) {
            Ok(text) => {
                let mut settings = Self::parse(&text)
                    .map_err(|err| LoadError::Parse(path.to_owned(), Box::new(err)))?;
                let base = path.parent().unwrap_or(Path::new("."));
                settings.icons.resolve_paths(base);
                if let Some(compiler) = &mut settings.tex.compiler {
                    *compiler = base.join(&*compiler);
                }
                for theme in [&mut settings.theme.light, &mut settings.theme.dark] {
                    if let Some(import) = &theme.import {
                        let mut imported =
                            import_vscode_theme(&base.join(import)).map_err(LoadError::Theme)?;
                        imported.overlay(theme);
                        *theme = imported;
                    }
                }
                Ok(settings)
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(LoadError::Read(path.to_owned(), err)),
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
    fn reads_theme_overrides() {
        let settings = Settings::parse(
            r##"
            [theme.dark]
            background = "#101014"

            [theme.dark.syntax]
            type = "#ffa657"
            "##,
        )
        .unwrap();
        assert_eq!(settings.theme.dark.background, Some(Color(0x101014ff)));
        assert_eq!(settings.theme.dark.syntax.r#type, Some(Color(0xffa657ff)));
        assert_eq!(settings.theme.light, ThemeOverrides::default());
        assert!(
            Settings::parse("[theme.dark]\nbackgroud = \"#000\"").is_err(),
            "typos are reported"
        );
    }

    #[test]
    fn invalid_settings_are_rejected() {
        assert!(Settings::parse("[editor]\nfont_size = \"big\"").is_err());
        assert!(Settings::parse("[files]\nshow_ignored = \"sometimes\"").is_err());
        assert!(Settings::parse("[limits]\nblock_cache = \"lots\"").is_err());
        assert!(Settings::default().tex.preview);
        let settings =
            Settings::parse("[tex]\npreview = false\ncompiler = '/opt/tex/bin/pdflatex'").unwrap();
        assert!(!settings.tex.preview);
        assert_eq!(
            settings.tex.compiler,
            Some(PathBuf::from("/opt/tex/bin/pdflatex"))
        );
        assert!(Settings::parse("[tex]\ncommand = 'make'").is_err());
    }
}
