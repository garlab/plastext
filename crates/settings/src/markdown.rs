use serde::{Deserialize, Deserializer, de};

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct Markdown {
    pub prose_font: String,
    pub max_width: ProseWidth,
}

impl Default for Markdown {
    fn default() -> Self {
        Self {
            prose_font: "system".into(),
            max_width: ProseWidth::Chars(80.),
        }
    }
}

/// Readable text measure. `ch` is the actual zero-glyph advance of the prose font.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ProseWidth {
    Chars(f32),
    Pixels(f32),
}

impl<'de> Deserialize<'de> for ProseWidth {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        let value = value.trim();
        let (number, chars) = value
            .strip_suffix("ch")
            .map(|v| (v, true))
            .or_else(|| value.strip_suffix("px").map(|v| (v, false)))
            .ok_or_else(|| de::Error::custom("markdown.max_width requires ch or px units"))?;
        let number: f32 = number.trim().parse().map_err(de::Error::custom)?;
        if !number.is_finite() || !(1. ..=10_000.).contains(&number) {
            return Err(de::Error::custom(
                "markdown.max_width must be finite and between 1 and 10000",
            ));
        }
        Ok(if chars {
            Self::Chars(number)
        } else {
            Self::Pixels(number)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_font_and_measure_and_rejects_invalid_lengths() {
        let settings =
            crate::Settings::parse("[markdown]\nprose_font='Georgia'\nmax_width='720px'").unwrap();
        assert_eq!(settings.markdown.prose_font, "Georgia");
        assert_eq!(settings.markdown.max_width, ProseWidth::Pixels(720.));
        assert_eq!(
            crate::Settings::parse("").unwrap().markdown.max_width,
            ProseWidth::Chars(80.)
        );
        for width in ["-1ch", "0px", "NaNpx", "infch", "80", "50%", "10001px"] {
            assert!(crate::Settings::parse(&format!("[markdown]\nmax_width='{width}'")).is_err());
        }
    }
}
