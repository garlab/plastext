//! EditorConfig is read off the UI thread with bounded local configuration files.
use super::Editor;
use ec4rs::{ConfigParser, Properties, PropertiesSource};
use serde::Deserialize;
use std::{
    fs,
    io::{self, Read},
    path::Path,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpenMode {
    Read,
    #[default]
    Edit,
}

pub fn for_file(path: &Path, sample: &str, defaults: &Editor) -> io::Result<Editor> {
    let mut editor = defaults.clone();
    editor.indent_size = editor.indent_size.clamp(1, 16);
    editor.tab_size = editor.tab_size.clamp(1, 16);
    if editor.detect_indentation {
        let mut tabs = 0;
        let mut spaces = Vec::new();
        for line in sample.lines().take(1000) {
            if line.starts_with('\t') {
                tabs += 1;
            }
            let count = line.bytes().take_while(|b| *b == b' ').count();
            if (1..=16).contains(&count) && !line.trim().is_empty() {
                spaces.push(count);
            }
        }
        if tabs > spaces.len() {
            editor.insert_spaces = false;
        } else if !spaces.is_empty() {
            editor.insert_spaces = true;
            let gcd = |mut a: usize, mut b: usize| {
                while b != 0 {
                    (a, b) = (b, a % b);
                }
                a
            };
            let width = spaces.into_iter().reduce(gcd).unwrap();
            if (2..=8).contains(&width) {
                editor.indent_size = width;
                editor.tab_size = width;
            }
        }
    }
    if !editor.editorconfig {
        return Ok(editor);
    }
    let props = properties(path)?;
    let value = |key| {
        props
            .get_raw_for_key(key)
            .filter_unset()
            .into_option()
            .map(str::to_ascii_lowercase)
    };
    match value("indent_style").as_deref() {
        Some("tab") => editor.insert_spaces = false,
        Some("space") => editor.insert_spaces = true,
        _ => {}
    }
    let width = |key| {
        value(key)
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|v| (1..=16).contains(v))
    };
    if let Some(size) = width("tab_width") {
        editor.tab_size = size;
    }
    if let Some(size) = width("indent_size") {
        editor.indent_size = size;
    } else if value("indent_size").as_deref() == Some("tab") {
        editor.indent_size = editor.tab_size;
    }
    if let Some(value) = value("trim_trailing_whitespace").and_then(|v| v.parse().ok()) {
        editor.trim_trailing_whitespace = value;
    }
    if let Some(value) = value("insert_final_newline").and_then(|v| v.parse().ok()) {
        editor.insert_final_newline = Some(value);
    }
    if let Some(value) = value("charset").filter(|v| {
        matches!(
            v.as_str(),
            "latin1" | "utf-8" | "utf-8-bom" | "utf-16le" | "utf-16be"
        )
    }) {
        editor.charset = Some(value);
    }
    if let Some(value) = value("end_of_line").filter(|v| matches!(v.as_str(), "lf" | "crlf" | "cr"))
    {
        editor.end_of_line = Some(value);
    }
    Ok(editor)
}

fn properties(path: &Path) -> io::Result<Properties> {
    let mut configs = Vec::new();
    for parent in path
        .parent()
        .into_iter()
        .flat_map(Path::ancestors)
        .take(128)
    {
        let config = parent.join(".editorconfig");
        let mut bytes = Vec::new();
        match fs::metadata(&config) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
            Ok(meta) if !meta.is_file() || meta.len() > 65536 => {
                return Err(io::Error::other(
                    "EditorConfig must be a regular file of at most 64 KiB",
                ));
            }
            _ => {}
        }
        fs::File::open(&config)?
            .take(65537)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 65536 {
            return Err(io::Error::other("EditorConfig exceeds 64 KiB"));
        }
        let mut parser = ConfigParser::new(bytes.as_slice()).map_err(io::Error::other)?;
        let root = parser.is_root;
        let mut sections = Vec::new();
        loop {
            match parser.read_section() {
                Ok(section) => sections.push(section),
                Err(ec4rs::ParseError::Eof) => break,
                Err(error) => return Err(io::Error::other(error)),
            }
        }
        configs.push((parent.to_owned(), sections));
        if root {
            break;
        }
    }
    let mut props = Properties::new();
    for (parent, sections) in configs.into_iter().rev() {
        for section in sections {
            (&section)
                .apply_to(&mut props, path.strip_prefix(&parent).unwrap())
                .map_err(io::Error::other)?;
        }
    }
    props.use_fallbacks();
    Ok(props)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inheritance_unset_globs_and_indent_detection() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("src")).unwrap();
        fs::write(root.path().join(".editorconfig"), "root=true\n[*]\nindent_style=space\nindent_size=2\nend_of_line=crlf\ntrim_trailing_whitespace=true\n[*.{rs,toml}]\ninsert_final_newline=true\n").unwrap();
        fs::write(
            root.path().join("src/.editorconfig"),
            "[*.rs]\nindent_size=4\nend_of_line=unset\n",
        )
        .unwrap();
        let settings = for_file(
            &root.path().join("src/main.rs"),
            "\told\n",
            &Editor::default(),
        )
        .unwrap();
        assert_eq!(settings.indent_size, 4);
        assert!(settings.insert_spaces);
        assert_eq!(settings.end_of_line, None);
        assert_eq!(settings.insert_final_newline, Some(true));
        assert!(settings.trim_trailing_whitespace);
    }
}
