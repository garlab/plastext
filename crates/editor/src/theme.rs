use gpui::{App, Global, Rgba, Window, WindowAppearance, rgb, rgba};
use plastext_settings::{Color, ThemeOverrides};
use plastext_syntax::Highlight;

/// Colors for the light or dark appearance (spec CFG-2). The built-in ones
/// follow GitHub's light and dark themes; settings can override any color.
#[derive(Clone, Copy)]
pub struct Theme {
    pub background: Rgba,
    pub surface: Rgba,
    pub border: Rgba,
    pub text: Rgba,
    pub text_muted: Rgba,
    pub selection: Rgba,
    pub find_match: Rgba,
    pub scrollbar_thumb: Rgba,
    /// One color per [`Highlight`], in declaration order.
    syntax: [Rgba; HIGHLIGHTS.len()],
}

const HIGHLIGHTS: [Highlight; 16] = [
    Highlight::Keyword,
    Highlight::Function,
    Highlight::Type,
    Highlight::String,
    Highlight::Escape,
    Highlight::Number,
    Highlight::Constant,
    Highlight::Comment,
    Highlight::Property,
    Highlight::Tag,
    Highlight::Attribute,
    Highlight::Label,
    Highlight::Operator,
    Highlight::Punctuation,
    Highlight::Heading,
    Highlight::Link,
];

/// The themes for both appearances, as configured.
pub struct Themes {
    pub light: Theme,
    pub dark: Theme,
}

impl Global for Themes {}

impl Themes {
    /// The built-in themes with `light` and `dark` overrides applied.
    pub fn new(light: &ThemeOverrides, dark: &ThemeOverrides) -> Self {
        Self {
            light: Theme::light().with(light),
            dark: Theme::dark().with(dark),
        }
    }
}

impl Theme {
    /// The theme for `window`'s appearance, from the [`Themes`] global if set.
    pub fn get(window: &Window, cx: &App) -> Self {
        let dark = matches!(
            window.appearance(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        );
        match cx.try_global::<Themes>() {
            Some(themes) if dark => themes.dark,
            Some(themes) => themes.light,
            None if dark => Self::dark(),
            None => Self::light(),
        }
    }

    pub fn syntax(&self, highlight: Highlight) -> Rgba {
        self.syntax[highlight as usize]
    }

    fn light() -> Self {
        let text = rgb(0x1f2328);
        Self {
            background: rgb(0xffffff),
            surface: rgb(0xf6f6f7),
            border: rgb(0xe1e1e4),
            text,
            text_muted: rgb(0x6b7079),
            selection: rgb(0xd6e4fa),
            find_match: rgb(0xfff1a8),
            scrollbar_thumb: rgb(0xc9cbd1),
            syntax: HIGHLIGHTS.map(|highlight| builtin_syntax(highlight, false).map_or(text, rgb)),
        }
    }

    fn dark() -> Self {
        let text = rgb(0xe6e6e9);
        Self {
            background: rgb(0x1c1d21),
            surface: rgb(0x232429),
            border: rgb(0x34363d),
            text,
            text_muted: rgb(0x8d919b),
            selection: rgb(0x2c3d58),
            find_match: rgb(0x5b4b1c),
            scrollbar_thumb: rgb(0x4a4d56),
            syntax: HIGHLIGHTS.map(|highlight| builtin_syntax(highlight, true).map_or(text, rgb)),
        }
    }

    fn with(mut self, overrides: &ThemeOverrides) -> Self {
        let set = |color: &mut Rgba, value: Option<Color>| {
            if let Some(Color(value)) = value {
                *color = rgba(value);
            }
        };
        set(&mut self.background, overrides.background);
        set(&mut self.surface, overrides.surface);
        set(&mut self.border, overrides.border);
        set(&mut self.text, overrides.text);
        set(&mut self.text_muted, overrides.text_muted);
        set(&mut self.selection, overrides.selection);
        set(&mut self.find_match, overrides.find_match);
        set(&mut self.scrollbar_thumb, overrides.scrollbar_thumb);
        let syntax = &overrides.syntax;
        let syntax_overrides = [
            syntax.keyword,
            syntax.function,
            syntax.r#type,
            syntax.string,
            syntax.escape,
            syntax.number,
            syntax.constant,
            syntax.comment,
            syntax.property,
            syntax.tag,
            syntax.attribute,
            syntax.label,
            syntax.operator,
            syntax.punctuation,
            syntax.heading,
            syntax.link,
        ];
        for (color, value) in self.syntax.iter_mut().zip(syntax_overrides) {
            set(color, value);
        }
        self
    }
}

/// GitHub's syntax colors; `None` means the text color.
fn builtin_syntax(highlight: Highlight, dark: bool) -> Option<u32> {
    use Highlight::*;
    let (light, dark_color) = match highlight {
        Keyword => (0xcf222e, 0xff7b72),
        Function => (0x8250df, 0xd2a8ff),
        Type => (0x953800, 0xffa657),
        String | Link => (0x0a3069, 0xa5d6ff),
        Escape | Tag => (0x116329, 0x7ee787),
        Number | Constant | Property | Attribute | Label | Heading => (0x0550ae, 0x79c0ff),
        Comment => (0x6e7781, 0x8b949e),
        Operator | Punctuation => return None,
    };
    Some(if dark { dark_color } else { light })
}

#[cfg(test)]
mod tests {
    use plastext_settings::SyntaxOverrides;

    use super::*;

    #[test]
    fn highlights_index_their_colors() {
        for (index, highlight) in HIGHLIGHTS.iter().enumerate() {
            assert_eq!(*highlight as usize, index);
        }
    }

    #[test]
    fn overrides_replace_only_their_colors() {
        let overrides = ThemeOverrides {
            background: Some(Color(0x101014ff)),
            syntax: SyntaxOverrides {
                comment: Some(Color(0x00ff00ff)),
                ..Default::default()
            },
            ..Default::default()
        };
        let theme = Theme::dark().with(&overrides);
        assert_eq!(theme.background, rgb(0x101014));
        assert_eq!(theme.syntax(Highlight::Comment), rgb(0x00ff00));
        assert_eq!(theme.syntax(Highlight::Keyword), rgb(0xff7b72));
        assert_eq!(theme.text, Theme::dark().text);
    }
}
