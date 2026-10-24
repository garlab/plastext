use gpui::{Rgba, WindowAppearance, rgb};
use plastext_syntax::Highlight;

/// Colors for the window's light or dark appearance (spec CFG-2).
///
/// Hard-coded until themes are loaded by `plastext-settings`. Syntax colors
/// follow GitHub's light and dark themes.
#[derive(Clone, Copy)]
pub struct Theme {
    pub background: Rgba,
    pub surface: Rgba,
    pub border: Rgba,
    pub text: Rgba,
    pub text_muted: Rgba,
    pub scrollbar_thumb: Rgba,
    dark: bool,
}

impl Theme {
    pub fn for_appearance(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self {
                background: rgb(0xffffff),
                surface: rgb(0xf6f6f7),
                border: rgb(0xe1e1e4),
                text: rgb(0x1f2328),
                text_muted: rgb(0x6b7079),
                scrollbar_thumb: rgb(0xc9cbd1),
                dark: false,
            },
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self {
                background: rgb(0x1c1d21),
                surface: rgb(0x232429),
                border: rgb(0x34363d),
                text: rgb(0xe6e6e9),
                text_muted: rgb(0x8d919b),
                scrollbar_thumb: rgb(0x4a4d56),
                dark: true,
            },
        }
    }

    pub fn syntax(&self, highlight: Highlight) -> Rgba {
        use Highlight::*;
        let (light, dark) = match highlight {
            Keyword => (0xcf222e, 0xff7b72),
            Function => (0x8250df, 0xd2a8ff),
            Type => (0x953800, 0xffa657),
            String | Link => (0x0a3069, 0xa5d6ff),
            Escape => (0x116329, 0x7ee787),
            Number | Constant | Property | Attribute => (0x0550ae, 0x79c0ff),
            Comment => (0x6e7781, 0x8b949e),
            Tag => (0x116329, 0x7ee787),
            Label | Heading => (0x0550ae, 0x79c0ff),
            Operator | Punctuation => return self.text,
        };
        rgb(if self.dark { dark } else { light })
    }
}
