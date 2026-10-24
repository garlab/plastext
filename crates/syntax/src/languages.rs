use std::{path::Path, sync::OnceLock};

use tree_sitter::Query;

use crate::highlight::CaptureKind;

/// A language plastext can highlight (spec CODE-3). Grammars and queries are
/// compiled in; nothing is downloaded at runtime.
pub struct Language {
    pub name: &'static str,
    grammar: fn() -> tree_sitter::Language,
    /// Highlight queries, concatenated in order: earlier patterns win.
    highlights: &'static [&'static str],
    extensions: &'static [&'static str],
    file_names: &'static [&'static str],
    /// Interpreters named on a `#!` line.
    interpreters: &'static [&'static str],
    query: OnceLock<Option<HighlightQuery>>,
}

pub(crate) struct HighlightQuery {
    pub query: Query,
    /// What each capture, by index, does.
    pub kinds: Vec<CaptureKind>,
}

impl Language {
    pub(crate) fn grammar(&self) -> tree_sitter::Language {
        (self.grammar)()
    }

    /// The compiled highlight query, or `None` if it doesn't compile.
    pub(crate) fn highlight_query(&self) -> Option<&HighlightQuery> {
        self.query
            .get_or_init(|| {
                let source = self.highlights.concat();
                match Query::new(&self.grammar(), &source) {
                    Ok(query) => {
                        let kinds = query
                            .capture_names()
                            .iter()
                            .map(|name| CaptureKind::from_name(name))
                            .collect();
                        Some(HighlightQuery { query, kinds })
                    }
                    Err(err) => {
                        eprintln!(
                            "plastext: the {} highlight query doesn't compile: {err}",
                            self.name
                        );
                        None
                    }
                }
            })
            .as_ref()
    }
}

macro_rules! language {
    ($name:literal, $grammar:expr, [$($highlights:expr),+ $(,)?], extensions: [$($extension:literal),* $(,)?], file_names: [$($file_name:literal),* $(,)?], interpreters: [$($interpreter:literal),* $(,)?] $(,)?) => {
        Language {
            name: $name,
            grammar: || $grammar.into(),
            highlights: &[$($highlights),+],
            extensions: &[$($extension),*],
            file_names: &[$($file_name),*],
            interpreters: &[$($interpreter),*],
            query: OnceLock::new(),
        }
    };
}

static LANGUAGES: [Language; 15] = [
    language!(
        "Bash",
        tree_sitter_bash::LANGUAGE,
        [tree_sitter_bash::HIGHLIGHT_QUERY],
        extensions: ["sh", "bash", "zsh", "ksh"],
        file_names: [".bashrc", ".bash_profile", ".profile", ".zshrc", ".zprofile", ".zshenv", "PKGBUILD"],
        interpreters: ["sh", "bash", "zsh", "ksh", "dash"],
    ),
    language!(
        "C",
        tree_sitter_c::LANGUAGE,
        [tree_sitter_c::HIGHLIGHT_QUERY],
        extensions: ["c", "h"],
        file_names: [],
        interpreters: [],
    ),
    language!(
        "C++",
        tree_sitter_cpp::LANGUAGE,
        [tree_sitter_cpp::HIGHLIGHT_QUERY, tree_sitter_c::HIGHLIGHT_QUERY],
        extensions: ["cc", "cpp", "cxx", "c++", "hh", "hpp", "hxx", "h++", "ipp", "tpp", "inl"],
        file_names: [],
        interpreters: [],
    ),
    language!(
        "CSS",
        tree_sitter_css::LANGUAGE,
        [tree_sitter_css::HIGHLIGHTS_QUERY],
        extensions: ["css"],
        file_names: [],
        interpreters: [],
    ),
    language!(
        "Go",
        tree_sitter_go::LANGUAGE,
        [tree_sitter_go::HIGHLIGHTS_QUERY],
        extensions: ["go"],
        file_names: [],
        interpreters: [],
    ),
    language!(
        "HTML",
        tree_sitter_html::LANGUAGE,
        [tree_sitter_html::HIGHLIGHTS_QUERY],
        extensions: ["html", "htm", "xhtml"],
        file_names: [],
        interpreters: [],
    ),
    language!(
        "JavaScript",
        tree_sitter_javascript::LANGUAGE,
        [tree_sitter_javascript::HIGHLIGHT_QUERY, tree_sitter_javascript::JSX_HIGHLIGHT_QUERY],
        extensions: ["js", "mjs", "cjs", "jsx"],
        file_names: [],
        interpreters: ["node", "deno", "bun"],
    ),
    language!(
        "JSON",
        tree_sitter_json::LANGUAGE,
        [tree_sitter_json::HIGHLIGHTS_QUERY],
        extensions: ["json", "jsonc", "json5", "geojson", "webmanifest"],
        file_names: [".babelrc", ".eslintrc", ".prettierrc"],
        interpreters: [],
    ),
    language!(
        "Markdown",
        tree_sitter_md::LANGUAGE,
        [tree_sitter_md::HIGHLIGHT_QUERY_BLOCK],
        extensions: ["md", "markdown", "mdown", "mkd", "mdx"],
        file_names: [],
        interpreters: [],
    ),
    language!(
        "Python",
        tree_sitter_python::LANGUAGE,
        [tree_sitter_python::HIGHLIGHTS_QUERY],
        extensions: ["py", "pyi", "pyw"],
        file_names: [],
        interpreters: ["python", "python2", "python3"],
    ),
    language!(
        "Rust",
        tree_sitter_rust::LANGUAGE,
        [tree_sitter_rust::HIGHLIGHTS_QUERY],
        extensions: ["rs"],
        file_names: [],
        interpreters: [],
    ),
    language!(
        "TOML",
        tree_sitter_toml_ng::LANGUAGE,
        [tree_sitter_toml_ng::HIGHLIGHTS_QUERY],
        extensions: ["toml"],
        file_names: ["Cargo.lock", "Pipfile", "poetry.lock", "uv.lock"],
        interpreters: [],
    ),
    language!(
        "TSX",
        tree_sitter_typescript::LANGUAGE_TSX,
        [
            tree_sitter_typescript::HIGHLIGHTS_QUERY,
            tree_sitter_javascript::HIGHLIGHT_QUERY,
            tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
        ],
        extensions: ["tsx"],
        file_names: [],
        interpreters: [],
    ),
    language!(
        "TypeScript",
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
        [tree_sitter_typescript::HIGHLIGHTS_QUERY, tree_sitter_javascript::HIGHLIGHT_QUERY],
        extensions: ["ts", "mts", "cts"],
        file_names: [],
        interpreters: [],
    ),
    language!(
        "YAML",
        tree_sitter_yaml::LANGUAGE,
        [tree_sitter_yaml::HIGHLIGHTS_QUERY],
        extensions: ["yml", "yaml"],
        file_names: [".clang-format", ".clang-tidy"],
        interpreters: [],
    ),
];

/// All built-in languages.
pub fn languages() -> &'static [Language] {
    &LANGUAGES
}

/// The language of the file at `path` whose contents start with `head`,
/// judged by file name, then extension, then `#!` line (spec CODE-4).
pub fn detect(path: &Path, head: &[u8]) -> Option<&'static Language> {
    let file_name = path.file_name().and_then(|name| name.to_str());
    if let Some(file_name) = file_name
        && let Some(language) = LANGUAGES
            .iter()
            .find(|language| language.file_names.contains(&file_name))
    {
        return Some(language);
    }
    if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
        let extension = extension.to_ascii_lowercase();
        if let Some(language) = LANGUAGES
            .iter()
            .find(|language| language.extensions.contains(&extension.as_str()))
        {
            return Some(language);
        }
    }
    let interpreter = interpreter(head)?;
    LANGUAGES.iter().find(|language| {
        language.interpreters.iter().any(|name| {
            interpreter
                .strip_prefix(name)
                .is_some_and(|version| version.chars().all(|c| c.is_ascii_digit() || c == '.'))
        })
    })
}

/// The program a `#!` line runs, looking through `env`: `python3` for
/// `#!/usr/bin/env -S python3 -u`.
fn interpreter(head: &[u8]) -> Option<&str> {
    let line = head.strip_prefix(b"#!")?;
    let line = &line[..line.iter().position(|&b| b == b'\n').unwrap_or(line.len())];
    let mut words = std::str::from_utf8(line).ok()?.split_whitespace();
    let program = words.next()?.rsplit('/').next()?;
    if program != "env" {
        return Some(program);
    }
    words.find(|word| !word.starts_with('-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(path: &str, head: &str) -> Option<&'static str> {
        detect(Path::new(path), head.as_bytes()).map(|language| language.name)
    }

    #[test]
    fn detects_by_name_extension_and_shebang() {
        assert_eq!(name("src/main.rs", ""), Some("Rust"));
        assert_eq!(name("App.TSX", ""), Some("TSX"));
        assert_eq!(name("Cargo.lock", ""), Some("TOML"));
        assert_eq!(name("bin/tool", "#!/usr/bin/env python3\n"), Some("Python"));
        assert_eq!(
            name("bin/tool", "#!/usr/bin/env -S python3.12 -u\n"),
            Some("Python")
        );
        assert_eq!(name("run", "#!/bin/bash -e\n"), Some("Bash"));
        assert_eq!(name("notes.txt", ""), None);
        assert_eq!(name("run", "#!/usr/bin/perl\n"), None);
    }

    #[test]
    fn every_highlight_query_compiles() {
        for language in languages() {
            assert!(language.highlight_query().is_some(), "{}", language.name);
        }
    }
}
