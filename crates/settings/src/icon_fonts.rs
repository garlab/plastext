//! Font metadata in VS Code file-icon themes; no font I/O happens here.
use serde::Deserialize;
use std::path::Path;

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum Icon {
    Image(String),
    Glyph {
        font: String,
        character: char,
        /// RGBA; absent means the navigation pane's foreground colour.
        color: Option<u32>,
        /// Hundredths of a percent: 15000 means 150%.
        size: u16,
    },
}

impl Icon {
    pub fn image_path(&self) -> Option<&str> {
        match self {
            Self::Image(path) => Some(path),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct IconFont {
    pub id: String,
    /// Supported alternatives, in the order declared by the theme.
    pub sources: Vec<String>,
    pub size: u16,
}

#[derive(Deserialize)]
pub(crate) struct RawFont {
    id: String,
    src: Vec<Source>,
    size: Option<String>,
}

#[derive(Deserialize)]
struct Source {
    path: String,
    format: Option<String>,
}

pub(crate) fn fonts(raw: Vec<RawFont>) -> Result<Vec<IconFont>, String> {
    if raw.len() > 8 {
        return Err("Icon themes may declare at most 8 fonts".into());
    }
    let mut fonts = Vec::<IconFont>::new();
    for font in raw {
        if font.id.is_empty() || fonts.iter().any(|f| f.id == font.id) {
            return Err("Icon font IDs must be nonempty and unique".into());
        }
        let sources: Vec<_> = font
            .src
            .into_iter()
            .filter_map(|source| {
                let extension = Path::new(&source.path)
                    .extension()
                    .and_then(|p| p.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let format = source
                    .format
                    .as_deref()
                    .unwrap_or(&extension)
                    .to_ascii_lowercase();
                matches!(
                    format.as_str(),
                    "woff" | "truetype" | "opentype" | "ttf" | "otf"
                )
                .then_some(source.path)
            })
            .collect();
        if sources.is_empty() {
            return Err(format!(
                "Font {} has no supported source; use WOFF, TTF or OTF (WOFF2 is not supported)",
                font.id
            ));
        }
        fonts.push(IconFont {
            id: font.id,
            sources,
            size: font.size.as_deref().map(size).transpose()?.unwrap_or(10000),
        });
    }
    Ok(fonts)
}

pub(crate) fn size(text: &str) -> Result<u16, String> {
    let percent = text
        .trim()
        .strip_suffix('%')
        .and_then(|s| s.parse::<f32>().ok())
        .filter(|p| p.is_finite() && (1.0..=400.0).contains(p))
        .ok_or_else(|| {
            format!("Invalid icon font size {text}; use a percentage from 1% to 400%")
        })?;
    Ok((percent * 100.0).round() as u16)
}

pub(crate) fn deserialize_size<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<u16>, D::Error> {
    Option::<String>::deserialize(deserializer)?
        .as_deref()
        .map(size)
        .transpose()
        .map_err(serde::de::Error::custom)
}

pub(crate) fn deserialize_character<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<char>, D::Error> {
    Option::<String>::deserialize(deserializer)?
        .as_deref()
        .map(character)
        .transpose()
        .map_err(serde::de::Error::custom)
}

pub(crate) fn character(text: &str) -> Result<char, String> {
    if let Some(hex) = text.strip_prefix('\\') {
        let hex = hex.trim_end_matches(char::is_whitespace);
        if !hex.is_empty()
            && hex.len() <= 6
            && hex.chars().all(|c| c.is_ascii_hexdigit())
            && let Ok(code) = u32::from_str_radix(hex, 16)
            && let Some(character) = char::from_u32(code).filter(|c| !c.is_control())
        {
            return Ok(character);
        }
    } else {
        let mut chars = text.chars();
        if let Some(character) = chars.next().filter(|c| !c.is_control())
            && chars.next().is_none()
        {
            return Ok(character);
        }
    }
    Err("fontCharacter must be one Unicode character or a CSS escape such as \\E001".into())
}
