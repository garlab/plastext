//! Embedded Material artwork and its VS Code-compatible association data.
use gpui::{AssetSource, SharedString};
use std::borrow::Cow;

pub struct Assets;
pub const MATERIAL_THEME: &str = include_str!("../assets/file-icons/material-icon-theme.json");

macro_rules! assets {
    ($($file:literal),+ $(,)?) => {
        impl AssetSource for Assets {
            fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
                Ok(match path {
                    $(concat!("file-icons/", $file, ".svg") => Some(Cow::Borrowed(
                        include_bytes!(concat!("../assets/file-icons/", $file, ".svg")).as_slice()
                    ))),+,
                    _ => None,
                })
            }
            fn list(&self, prefix: &str) -> gpui::Result<Vec<SharedString>> {
                Ok([$(concat!("file-icons/", $file, ".svg")),+].into_iter()
                    .filter(|path| path.starts_with(prefix)).map(Into::into).collect())
            }
        }
    };
}
assets! {
    "document",
    "folder-base",
    "rust",
    "markdown",
    "readme",
    "javascript",
    "typescript",
    "typescript-def",
    "react",
    "react_ts",
    "python",
    "json",
    "toml",
    "toml_light",
    "yaml",
    "xml",
    "c",
    "cpp",
    "csharp",
    "go",
    "html",
    "css",
    "sass",
    "swift",
    "kotlin",
    "java",
    "console",
    "database",
    "table",
    "image",
    "pdf",
    "zip",
    "git",
    "docker",
    "settings",
    "license",
    "lock",
    "font",
    "video",
    "audio",
    "ruby",
    "php",
    "lua",
    "vue",
    "svelte",
    "scala",
    "elixir",
    "haskell",
    "clojure",
    "zig",
    "dart",
    "r",
    "graphql",
    "tex",
    "terraform",
    "cmake",
    "makefile",
}

#[cfg(test)]
mod tests {
    use super::*;
    use plastext_settings::icons::{Appearance, IconTheme};
    use std::path::Path;

    #[test]
    fn bundled_theme_uses_the_same_resolver_as_imports() {
        let theme = IconTheme::parse(MATERIAL_THEME).unwrap();
        for (name, icon) in [
            ("Cargo.toml", "rust"),
            ("Cargo.lock", "lock"),
            ("README.md", "readme"),
            ("LICENSE-MIT", "license"),
            (".env.local", "settings"),
            ("Dockerfile.dev", "docker"),
            ("index.d.ts", "typescript-def"),
            ("App.TSX", "react_ts"),
            ("PHOTO.PNG", "image"),
            ("naïve.rs", "rust"),
            ("unknown", "document"),
        ] {
            assert_eq!(
                theme
                    .resolve(Path::new(name), false, false, false, None, Appearance::Dark)
                    .as_ref()
                    .and_then(|icon| icon.image_path()),
                Some(format!("{icon}.svg").as_str())
            );
        }
        assert_eq!(
            theme
                .resolve(
                    Path::new("config.toml"),
                    false,
                    false,
                    false,
                    None,
                    Appearance::Light
                )
                .as_ref()
                .and_then(|icon| icon.image_path()),
            Some("toml_light.svg")
        );
    }

    #[test]
    fn every_bundled_asset_decodes_within_icon_limits() {
        for path in Assets.list("file-icons/").unwrap() {
            let bytes = Assets.load(&path).unwrap().unwrap();
            let image = plastext_assets::icons::decode(&bytes, 32).unwrap();
            assert!(image.width <= 32 && image.height <= 32, "{path}");
            assert!(
                image.rgba.as_chunks::<4>().0.iter().any(|p| p[3] > 0),
                "{path}"
            );
        }
    }
}
