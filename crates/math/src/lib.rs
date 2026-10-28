//! TeX math parsing and layout (spec MD-3): katex-rs parses, katex-native
//! lays out with KaTeX's fonts and rasterizes.

use std::sync::OnceLock;

use katex_native::{
    Color,
    katex::{KatexContext, Settings},
};

#[cfg(feature = "typst-engine")]
mod typst_engine;

#[cfg(feature = "typst-engine")]
pub use typst_engine::{Fonts, Layout, TypstMath};

/// A formula rendered to pixels: its body and, in display math, its tag or
/// equation numbers, which go against the right edge of the container.
pub struct Rendered {
    pub body: Raster,
    pub tag: Option<Raster>,
}

pub struct Raster {
    pub width: u32,
    pub height: u32,
    /// The baseline, in pixels from the top.
    pub baseline: f32,
    /// Straight (not premultiplied) RGBA, row by row.
    pub rgba: Vec<u8>,
}

/// KaTeX sets math 1.21 times as large as the surrounding text.
pub const SCALE: f32 = 1.21;

fn context() -> &'static KatexContext {
    static CONTEXT: OnceLock<KatexContext> = OnceLock::new();
    CONTEXT.get_or_init(KatexContext::default)
}

/// Renders `tex` with `px_per_em` pixels per em of the formula, drawing in
/// `rgba` wherever the TeX doesn't set a color. Automatic equation numbers
/// start at `first_number`.
pub fn render(
    tex: &str,
    display: bool,
    first_number: u32,
    px_per_em: f32,
    rgba: [u8; 4],
) -> Result<Rendered, String> {
    let settings = Settings::builder().display_mode(display).build();
    let layout = katex_native::layout_numbered(context(), tex, &settings, first_number)
        .map_err(|err| err.to_string())?;
    let [r, g, b, a] = rgba;
    let raster = |layout: &katex_native::Layout| {
        katex_native::raster::rasterize(layout, f64::from(px_per_em), Color { r, g, b, a }).map(
            |raster| Raster {
                width: raster.width,
                height: raster.height,
                baseline: raster.baseline as f32,
                rgba: raster.rgba,
            },
        )
    };
    Ok(Rendered {
        body: raster(&layout).ok_or("The formula is empty.")?,
        tag: layout.tag.as_deref().and_then(raster),
    })
}

/// How many automatic equation numbers display math `tex` uses, to number
/// the next formula's from. Formulas that can't number equations are not
/// laid out.
pub fn equation_numbers(tex: &str) -> u32 {
    if !may_number(tex) {
        return 0;
    }
    let settings = Settings::builder().display_mode(true).build();
    katex_native::layout_with(context(), tex, &settings).map_or(0, |layout| layout.equation_numbers)
}

/// Whether `tex` uses an environment that numbers equations (the starred
/// forms don't).
pub fn may_number(tex: &str) -> bool {
    ["equation", "gather", "align", "alignat", "multline"]
        .iter()
        .any(|env| tex.contains(&format!("\\begin{{{env}}}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_and_reports_errors() {
        let rendered = render(r"\frac{a}{b}", true, 1, 20., [0, 0, 0, 255]).unwrap();
        let body = &rendered.body;
        assert_eq!(body.rgba.len(), (body.width * body.height * 4) as usize);
        assert!(body.baseline > 0. && body.baseline < body.height as f32);
        assert!(rendered.tag.is_none());
        assert!(render(r"\frac{a}", false, 1, 20., [0, 0, 0, 255]).is_err());
    }

    #[test]
    fn counts_and_continues_equation_numbers() {
        let gather = r"\begin{gather} a \\ b \nonumber \\ c \end{gather}";
        assert_eq!(equation_numbers(gather), 2);
        assert_eq!(equation_numbers(r"\begin{gather*} a \\ b \end{gather*}"), 0);
        assert_eq!(equation_numbers(r"\frac{a}{b}"), 0);
        let rendered = render(gather, true, 5, 20., [0, 0, 0, 255]).unwrap();
        assert!(rendered.tag.is_some());
    }
}
