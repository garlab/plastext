use gpui::{Rgba, WindowAppearance, rgb};

/// Colors for the window's light or dark appearance (spec CFG-2).
///
/// Hard-coded until themes are loaded by `plastext-settings`.
#[derive(Clone, Copy)]
pub struct Theme {
    pub background: Rgba,
    pub surface: Rgba,
    pub border: Rgba,
    pub text: Rgba,
    pub text_muted: Rgba,
    pub scrollbar_thumb: Rgba,
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
            },
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self {
                background: rgb(0x1c1d21),
                surface: rgb(0x232429),
                border: rgb(0x34363d),
                text: rgb(0xe6e6e9),
                text_muted: rgb(0x8d919b),
                scrollbar_thumb: rgb(0x4a4d56),
            },
        }
    }
}
