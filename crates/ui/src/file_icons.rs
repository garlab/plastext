//! Bundled Material Icon Theme artwork, selected by name without reading files.
//! GPUI decodes these embedded SVGs asynchronously and caches them by asset path.

use std::borrow::Cow;

use gpui::{AssetSource, SharedString};

/// Embedded artwork for the application's GPUI asset source.
pub struct Assets;

macro_rules! icons {
    ($($variant:ident => $file:literal),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub(crate) enum FileIcon { $($variant),+ }

        impl FileIcon {
            pub fn path(self) -> &'static str {
                match self { $(Self::$variant => concat!("file-icons/", $file, ".svg")),+ }
            }
        }

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
                Ok([$(concat!("file-icons/", $file, ".svg")),+]
                    .into_iter().filter(|path| path.starts_with(prefix))
                    .map(Into::into).collect())
            }
        }
    };
}

icons! {
    File => "document",
    Folder => "folder-base",
    Rust => "rust",
    Markdown => "markdown",
    Readme => "readme",
    JavaScript => "javascript",
    TypeScript => "typescript",
    TypeScriptDefinition => "typescript-def",
    React => "react",
    ReactTypeScript => "react_ts",
    Python => "python",
    Json => "json",
    Toml => "toml",
    TomlLight => "toml_light",
    Yaml => "yaml",
    Xml => "xml",
    C => "c",
    Cpp => "cpp",
    CSharp => "csharp",
    Go => "go",
    Html => "html",
    Css => "css",
    Sass => "sass",
    Swift => "swift",
    Kotlin => "kotlin",
    Java => "java",
    Shell => "console",
    Database => "database",
    Table => "table",
    Image => "image",
    Pdf => "pdf",
    Archive => "zip",
    Git => "git",
    Docker => "docker",
    Settings => "settings",
    License => "license",
    Lock => "lock",
    Font => "font",
    Video => "video",
    Audio => "audio",
    Ruby => "ruby",
    Php => "php",
    Lua => "lua",
    Vue => "vue",
    Svelte => "svelte",
    Scala => "scala",
    Elixir => "elixir",
    Haskell => "haskell",
    Clojure => "clojure",
    Zig => "zig",
    Dart => "dart",
    R => "r",
    Graphql => "graphql",
    Tex => "tex",
    Terraform => "terraform",
    Cmake => "cmake",
    Make => "makefile",
}

impl FileIcon {
    pub fn for_name(name: &str, light: bool) -> Self {
        let name = name.to_ascii_lowercase();
        let icon = match name.as_str() {
            "cargo.toml" | "rust-toolchain" | "rust-toolchain.toml" => Self::Rust,
            "dockerfile"
            | "containerfile"
            | "compose.yml"
            | "compose.yaml"
            | "docker-compose.yml"
            | "docker-compose.yaml"
            | ".dockerignore" => Self::Docker,
            ".gitignore" | ".gitattributes" | ".gitmodules" | ".gitkeep" => Self::Git,
            "cmakelists.txt" => Self::Cmake,
            "makefile" | "gnumakefile" => Self::Make,
            ".bashrc" | ".zshrc" | ".profile" | ".bash_profile" | "justfile" => Self::Shell,
            ".editorconfig" | ".clang-format" | ".clang-tidy" => Self::Settings,
            _ if name == "readme" || name.starts_with("readme.") => Self::Readme,
            _ if ["license", "licence", "copying", "copyright"]
                .iter()
                .any(|prefix| {
                    name == *prefix
                        || name
                            .strip_prefix(prefix)
                            .is_some_and(|rest| rest.starts_with(['.', '-']))
                }) =>
            {
                Self::License
            }
            _ if name == ".env" || name.starts_with(".env.") => Self::Settings,
            _ if name.starts_with("dockerfile.") || name.starts_with("containerfile.") => {
                Self::Docker
            }
            _ if name.ends_with(".d.ts")
                || name.ends_with(".d.mts")
                || name.ends_with(".d.cts") =>
            {
                Self::TypeScriptDefinition
            }
            _ => match name.rsplit_once('.').map(|(_, extension)| extension) {
                Some("rs") => Self::Rust,
                Some("md" | "markdown" | "mdown" | "mdx") => Self::Markdown,
                Some("js" | "mjs" | "cjs") => Self::JavaScript,
                Some("ts" | "mts" | "cts") => Self::TypeScript,
                Some("jsx") => Self::React,
                Some("tsx") => Self::ReactTypeScript,
                Some("py" | "pyi" | "pyw") => Self::Python,
                Some("json" | "jsonc" | "json5" | "jsonl" | "ipynb") => Self::Json,
                Some("toml") => Self::Toml,
                Some("yaml" | "yml") => Self::Yaml,
                Some("xml" | "xsd" | "xsl" | "plist" | "csproj" | "fsproj") => Self::Xml,
                Some("c" | "h") => Self::C,
                Some("cc" | "cpp" | "cxx" | "hpp" | "hxx" | "hh") => Self::Cpp,
                Some("cs" | "csx") => Self::CSharp,
                Some("go") => Self::Go,
                Some("html" | "htm") => Self::Html,
                Some("css") => Self::Css,
                Some("sass" | "scss" | "less") => Self::Sass,
                Some("swift") => Self::Swift,
                Some("kt" | "kts") => Self::Kotlin,
                Some("java" | "jar" | "class") => Self::Java,
                Some("sh" | "bash" | "zsh" | "fish" | "ps1" | "bat" | "cmd") => Self::Shell,
                Some("sql" | "sqlite" | "sqlite3" | "db") => Self::Database,
                Some("csv" | "tsv" | "xls" | "xlsx" | "ods") => Self::Table,
                Some(
                    "svg" | "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "ico" | "tif"
                    | "tiff" | "avif" | "heic",
                ) => Self::Image,
                Some("pdf") => Self::Pdf,
                Some("zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "7z" | "rar") => {
                    Self::Archive
                }
                Some("lock") => Self::Lock,
                Some("ini" | "conf" | "cfg" | "properties") => Self::Settings,
                Some("woff" | "woff2" | "ttf" | "otf") => Self::Font,
                Some("mp4" | "mov" | "mkv" | "webm" | "avi") => Self::Video,
                Some("mp3" | "wav" | "ogg" | "flac" | "m4a" | "aac") => Self::Audio,
                Some("rb" | "rake" | "gemspec") => Self::Ruby,
                Some("php") => Self::Php,
                Some("lua") => Self::Lua,
                Some("vue") => Self::Vue,
                Some("svelte") => Self::Svelte,
                Some("scala" | "sc") => Self::Scala,
                Some("ex" | "exs") => Self::Elixir,
                Some("hs" | "lhs") => Self::Haskell,
                Some("clj" | "cljs" | "cljc" | "edn") => Self::Clojure,
                Some("zig" | "zon") => Self::Zig,
                Some("dart") => Self::Dart,
                Some("r" | "rmd") => Self::R,
                Some("graphql" | "gql") => Self::Graphql,
                Some("tex" | "bib" | "sty") => Self::Tex,
                Some("tf" | "tfvars") => Self::Terraform,
                Some("cmake") => Self::Cmake,
                Some("mk") => Self::Make,
                _ => Self::File,
            },
        };
        if light && icon == Self::Toml {
            Self::TomlLight
        } else {
            icon
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_compound_extensions_and_unknown_files() {
        for (name, expected) in [
            ("Cargo.toml", FileIcon::Rust),
            ("Cargo.lock", FileIcon::Lock),
            ("README.md", FileIcon::Readme),
            ("LICENSE-MIT", FileIcon::License),
            ("Dockerfile.dev", FileIcon::Docker),
            (".gitignore", FileIcon::Git),
            (".env.local", FileIcon::Settings),
            ("CMakeLists.txt", FileIcon::Cmake),
            ("index.d.ts", FileIcon::TypeScriptDefinition),
            ("view.tsx", FileIcon::ReactTypeScript),
            ("PHOTO.PNG", FileIcon::Image),
            ("data.csv", FileIcon::Table),
            ("naïve.rs", FileIcon::Rust),
            ("something.unknown", FileIcon::File),
            ("no-extension", FileIcon::File),
            ("", FileIcon::File),
        ] {
            assert_eq!(FileIcon::for_name(name, false), expected, "{name}");
        }
        assert_eq!(FileIcon::for_name("config.toml", true), FileIcon::TomlLight);
        assert_eq!(FileIcon::for_name("config.toml", false), FileIcon::Toml);
    }

    #[test]
    fn every_bundled_asset_decodes_and_has_visible_pixels() {
        for path in Assets.list("file-icons/").unwrap() {
            let asset_path: &str = path.as_ref();
            assert!(
                matches!(
                    gpui::ImageSource::from(asset_path),
                    gpui::ImageSource::Resource(gpui::Resource::Embedded(_))
                ),
                "{path} must resolve to an embedded asset"
            );
            let bytes = Assets.load(&path).unwrap().expect("embedded icon");
            let image =
                plastext_assets::decode(&bytes).unwrap_or_else(|error| panic!("{path}: {error}"));
            assert!(
                image
                    .rgba
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|pixel| pixel[3] > 0),
                "{path}"
            );
        }
        assert!(Assets.load("file-icons/unknown.svg").unwrap().is_none());
        assert!(Assets.list("unrelated/").unwrap().is_empty());
    }
}
