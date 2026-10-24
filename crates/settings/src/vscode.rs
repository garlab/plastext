//! Local VS Code themes: workbench colors and common TextMate token scopes.
//! See https://code.visualstudio.com/api/extension-guides/color-theme .

use std::{
    collections::BTreeMap,
    fmt,
    fs::File,
    io::Read as _,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::{Color, ThemeOverrides};

const MAX_THEME_BYTES: u64 = 2 * 1024 * 1024;
const MAX_INCLUDES: usize = 16;

#[derive(Debug)]
pub struct ThemeImportError {
    pub path: PathBuf,
    pub message: String,
}

impl fmt::Display for ThemeImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Couldn't import theme {}: {}",
            self.path.display(),
            self.message
        )
    }
}

impl std::error::Error for ThemeImportError {}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct VscodeTheme {
    include: Option<PathBuf>,
    colors: BTreeMap<String, String>,
    token_colors: Vec<TokenRule>,
}

#[derive(Deserialize)]
struct TokenRule {
    #[serde(default)]
    scope: Option<Scopes>,
    settings: TokenSettings,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Scopes {
    One(String),
    Many(Vec<String>),
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct TokenSettings {
    foreground: Option<String>,
    background: Option<String>,
}

/// Imports a JSON/JSONC theme. Local `include` paths resolve against each
/// including file; cycles and excessive nesting fail with a useful error.
/// Unknown workbench keys and semantic/font-style rules are ignored because
/// plastext has a smaller palette and no semantic token provider.
pub fn import_vscode_theme(path: &Path) -> Result<ThemeOverrides, ThemeImportError> {
    let theme = read_theme(path, &mut Vec::new())?;
    let error = |message| ThemeImportError {
        path: path.to_owned(),
        message,
    };
    let color = |value: &str| value.parse::<Color>().map_err(|err| error(err.to_string()));
    let get = |keys: &[&str]| -> Result<Option<Color>, ThemeImportError> {
        keys.iter()
            .find_map(|key| theme.colors.get(*key))
            .map(|value| color(value))
            .transpose()
    };
    let mut result = ThemeOverrides {
        background: get(&["editor.background"])?,
        surface: get(&["sideBar.background", "editorGutter.background"])?,
        border: get(&["panel.border", "sideBar.border", "widget.border"])?,
        text: get(&["editor.foreground", "foreground"])?,
        text_muted: get(&["editorLineNumber.foreground", "descriptionForeground"])?,
        selection: get(&["editor.selectionBackground"])?,
        find_match: get(&[
            "editor.findMatchHighlightBackground",
            "editor.findMatchBackground",
        ])?,
        scrollbar_thumb: get(&["scrollbarSlider.background"])?,
        ..Default::default()
    };
    let rules: Vec<_> = theme
        .token_colors
        .iter()
        .map(|rule| {
            let scopes: Vec<&str> = match &rule.scope {
                Some(Scopes::One(scope)) => vec![scope.as_str()],
                Some(Scopes::Many(scopes)) => scopes.iter().map(String::as_str).collect(),
                None => Vec::new(),
            };
            let scopes: Vec<_> = scopes
                .into_iter()
                .flat_map(|scope| scope.split(','))
                .map(str::trim)
                .filter(|scope| !scope.is_empty())
                .collect();
            Ok((
                scopes,
                rule.settings.foreground.as_deref().map(color).transpose()?,
                rule.settings.background.as_deref().map(color).transpose()?,
            ))
        })
        .collect::<Result<_, ThemeImportError>>()?;
    // An unscoped TextMate rule supplies defaults; explicit UI colors win.
    for (scopes, foreground, background) in &rules {
        if scopes.is_empty() {
            if !theme.colors.contains_key("editor.foreground")
                && !theme.colors.contains_key("foreground")
            {
                result.text = foreground.or(result.text);
            }
            if !theme.colors.contains_key("editor.background") {
                result.background = background.or(result.background);
            }
        }
    }
    let syntax = &mut result.syntax;
    let slots: [(&mut Option<Color>, &[&str]); 16] = [
        (
            &mut syntax.keyword,
            &["keyword.control", "storage.type", "storage.modifier"],
        ),
        (
            &mut syntax.function,
            &[
                "entity.name.function",
                "support.function",
                "variable.function",
            ],
        ),
        (
            &mut syntax.r#type,
            &[
                "entity.name.type",
                "entity.name.class",
                "support.type",
                "support.class",
            ],
        ),
        (&mut syntax.string, &["string.quoted", "string.unquoted"]),
        (
            &mut syntax.escape,
            &["constant.character.escape", "string.regexp"],
        ),
        (&mut syntax.number, &["constant.numeric"]),
        (
            &mut syntax.constant,
            &["constant.language", "variable.other.constant"],
        ),
        (&mut syntax.comment, &["comment.line", "comment.block"]),
        (
            &mut syntax.property,
            &[
                "variable.other.property",
                "support.type.property-name",
                "meta.object-literal.key",
            ],
        ),
        (&mut syntax.tag, &["entity.name.tag"]),
        (&mut syntax.attribute, &["entity.other.attribute-name"]),
        (&mut syntax.label, &["entity.name.label"]),
        (&mut syntax.operator, &["keyword.operator"]),
        (&mut syntax.punctuation, &["punctuation"]),
        (
            &mut syntax.heading,
            &["markup.heading", "entity.name.section"],
        ),
        (
            &mut syntax.link,
            &["markup.underline.link", "string.other.link"],
        ),
    ];
    for (slot, targets) in slots {
        let mut best = 0;
        for (scopes, foreground, _) in &rules {
            let Some(foreground) = foreground else {
                continue;
            };
            for scope in scopes {
                // Contextual selectors cannot be faithfully represented by a
                // single global capture color. Don't apply them globally.
                if scope.contains(char::is_whitespace) {
                    continue;
                }
                if scope.len() >= best
                    && targets.iter().any(|target| {
                        *target == *scope
                            || target
                                .strip_prefix(scope)
                                .is_some_and(|rest| rest.starts_with('.'))
                    })
                {
                    best = scope.len();
                    *slot = Some(*foreground);
                }
            }
        }
    }
    Ok(result)
}

fn read_theme(path: &Path, stack: &mut Vec<PathBuf>) -> Result<VscodeTheme, ThemeImportError> {
    let error = |message| ThemeImportError {
        path: path.to_owned(),
        message,
    };
    let canonical = path.canonicalize().map_err(|err| error(err.to_string()))?;
    if stack.len() >= MAX_INCLUDES || stack.contains(&canonical) {
        return Err(error(
            "cyclic include or more than 16 included themes".into(),
        ));
    }
    stack.push(canonical);
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(MAX_THEME_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|err| error(err.to_string()))?;
    if bytes.len() as u64 > MAX_THEME_BYTES {
        return Err(error("theme exceeds 2 MiB".into()));
    }
    let text = std::str::from_utf8(&bytes).map_err(|err| error(err.to_string()))?;
    let json = jsonc(text.trim_start_matches('\u{feff}')).map_err(error)?;
    let mut theme: VscodeTheme =
        serde_json::from_slice(&json).map_err(|err| error(err.to_string()))?;
    if let Some(include) = theme.include.take() {
        let mut base = read_theme(
            &path.parent().unwrap_or(Path::new(".")).join(include),
            stack,
        )?;
        base.colors.extend(theme.colors);
        base.token_colors.extend(theme.token_colors);
        theme = base;
    }
    stack.pop();
    Ok(theme)
}

/// Remove comments and trailing commas without touching strings or line
/// positions. JSON's own parser still validates strings and structure.
fn jsonc(text: &str) -> Result<Vec<u8>, String> {
    let mut out = text.as_bytes().to_vec();
    let mut i = 0;
    while i < out.len() {
        match out[i] {
            b'"' => skip_string(&out, &mut i),
            b'/' if out.get(i + 1) == Some(&b'/') => {
                while i < out.len() && out[i] != b'\n' {
                    out[i] = b' ';
                    i += 1;
                }
            }
            b'/' if out.get(i + 1) == Some(&b'*') => {
                out[i..i + 2].fill(b' ');
                i += 2;
                while i + 1 < out.len() && &out[i..i + 2] != b"*/" {
                    if out[i] != b'\n' && out[i] != b'\r' {
                        out[i] = b' ';
                    }
                    i += 1;
                }
                if i + 1 >= out.len() {
                    return Err("unterminated block comment".into());
                }
                out[i..i + 2].fill(b' ');
                i += 2;
            }
            _ => i += 1,
        }
    }
    i = 0;
    while i < out.len() {
        if out[i] == b'"' {
            skip_string(&out, &mut i);
            continue;
        }
        if out[i] == b','
            && matches!(
                out[i + 1..].iter().find(|byte| !byte.is_ascii_whitespace()),
                Some(b'}' | b']')
            )
        {
            out[i] = b' ';
        }
        i += 1;
    }
    Ok(out)
}

fn skip_string(bytes: &[u8], index: &mut usize) {
    *index += 1;
    while *index < bytes.len() {
        match bytes[*index] {
            b'\\' => *index += 2,
            b'"' => {
                *index += 1;
                break;
            }
            _ => *index += 1,
        }
    }
}

impl ThemeOverrides {
    pub(crate) fn overlay(&mut self, other: &Self) {
        macro_rules! overlay {
            ($target:expr, $source:expr, $($field:ident),+ $(,)?) => {
                $(if $source.$field.is_some() { $target.$field = $source.$field.clone(); })+
            };
        }
        overlay!(
            self,
            other,
            import,
            background,
            surface,
            border,
            text,
            text_muted,
            selection,
            find_match,
            scrollbar_thumb
        );
        overlay!(
            self.syntax,
            other.syntax,
            keyword,
            function,
            r#type,
            string,
            escape,
            number,
            constant,
            comment,
            property,
            tag,
            attribute,
            label,
            operator,
            punctuation,
            heading,
            link
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn imports_jsonc_includes_scopes_and_transparency_with_toml_overrides() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("themes")).unwrap();
        fs::write(
            dir.path().join("themes/base.json"),
            r##"{
            "colors": { "editor.background": "#123", "editor.foreground": "#abc" },
            "tokenColors": [
                { "scope": "comment", "settings": { "foreground": "#456" } },
                { "scope": "keyword.operator", "settings": { "foreground": "#f00" } }
            ]
        }"##,
        )
        .unwrap();
        fs::write(dir.path().join("themes/theme.json"), r##"{
            // A URL and comment delimiters inside a string must survive.
            "name": "https://example.test/*héllo*/\"",
            "include": "base.json",
            "colors": { "editor.selectionBackground": "#12345678", },
            "tokenColors": [
                { "scope": ["entity.name.function", "string, constant.numeric"], "settings": { "foreground": "#0f0" } },
                { "scope": "keyword", "settings": { "foreground": "#00f" } },
                { "scope": "source.rust comment", "settings": { "foreground": "#f00" } },
            ], /* trailing comment */
        }"##).unwrap();
        fs::write(dir.path().join("config.toml"), "[theme.dark]\nimport = 'themes/theme.json'\nbackground = '#000'\n[theme.dark.syntax]\ncomment = '#fff'\n").unwrap();
        let theme = crate::Settings::load_from(&dir.path().join("config.toml"))
            .unwrap()
            .theme
            .dark;
        assert_eq!(theme.background, Some(Color(0x000000ff)));
        assert_eq!(theme.text, Some(Color(0xaabbccff)));
        assert_eq!(theme.selection, Some(Color(0x12345678)));
        assert_eq!(theme.syntax.comment, Some(Color(0xffffffff)));
        assert_eq!(theme.syntax.function, Some(Color(0x00ff00ff)));
        assert_eq!(theme.syntax.number, Some(Color(0x00ff00ff)));
        assert_eq!(theme.syntax.string, Some(Color(0x00ff00ff)));
        assert_eq!(theme.syntax.keyword, Some(Color(0x0000ffff)));
        assert_eq!(
            theme.syntax.operator,
            Some(Color(0xff0000ff)),
            "specific scopes win over generic ones"
        );
    }

    #[test]
    fn reports_missing_cycles_and_invalid_input() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("theme.json");
        assert!(import_vscode_theme(&path).is_err());
        for text in [
            r#"{"include":"theme.json"}"#,
            "{/* unterminated",
            "{broken}",
            r##"{"colors":{"editor.background":"nope"}}"##,
        ] {
            fs::write(&path, text).unwrap();
            let error = import_vscode_theme(&path).unwrap_err();
            assert_eq!(error.path, path);
            assert!(!error.message.is_empty());
        }
    }
}
