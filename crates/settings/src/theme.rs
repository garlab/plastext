use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, de};

/// Color overrides for the light and dark themes (spec CFG-2). Colors left
/// out keep the built-in ones.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Themes {
    pub light: ThemeOverrides,
    pub dark: ThemeOverrides,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeOverrides {
    pub background: Option<Color>,
    /// Sidebars, the gutter and bars.
    pub surface: Option<Color>,
    pub border: Option<Color>,
    pub text: Option<Color>,
    pub text_muted: Option<Color>,
    pub selection: Option<Color>,
    pub find_match: Option<Color>,
    pub scrollbar_thumb: Option<Color>,
    pub syntax: SyntaxOverrides,
}

/// Syntax colors, one per kind of highlight.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct SyntaxOverrides {
    pub keyword: Option<Color>,
    pub function: Option<Color>,
    pub r#type: Option<Color>,
    pub string: Option<Color>,
    pub escape: Option<Color>,
    pub number: Option<Color>,
    pub constant: Option<Color>,
    pub comment: Option<Color>,
    pub property: Option<Color>,
    pub tag: Option<Color>,
    pub attribute: Option<Color>,
    pub label: Option<Color>,
    pub operator: Option<Color>,
    pub punctuation: Option<Color>,
    pub heading: Option<Color>,
    pub link: Option<Color>,
}

/// An RGB color written `"#rrggbb"` or `"#rgb"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color(pub u32);

#[derive(Debug, PartialEq, Eq)]
pub struct ParseColorError(String);

impl fmt::Display for ParseColorError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "`{}` isn't a color such as \"#1f2328\" or \"#fff\"",
            self.0
        )
    }
}

impl std::error::Error for ParseColorError {}

impl FromStr for Color {
    type Err = ParseColorError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let error = || ParseColorError(text.to_owned());
        let hex = text.strip_prefix('#').ok_or_else(error)?;
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(error());
        }
        let value = match hex.len() {
            6 => u32::from_str_radix(hex, 16).map_err(|_| error())?,
            3 => hex
                .chars()
                .map(|c| c.to_digit(16).unwrap_or(0))
                .fold(0, |rgb, digit| (rgb << 8) | (digit * 0x11)),
            _ => return Err(error()),
        };
        Ok(Self(value))
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_colors() {
        assert_eq!("#1f2328".parse(), Ok(Color(0x1f2328)));
        assert_eq!("#FFF".parse(), Ok(Color(0xffffff)));
        assert_eq!("#a1b".parse(), Ok(Color(0xaa11bb)));
        for text in ["1f2328", "#12345", "#ggg", "#+12345"] {
            assert!(text.parse::<Color>().is_err(), "{text}");
        }
    }
}
