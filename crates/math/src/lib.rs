//! TeX math parsing and layout (spec MD-3): katex-rs parses, katex-native
//! lays out with KaTeX's fonts and rasterizes.

use std::sync::OnceLock;

use katex_native::{Color, katex::KatexContext};

#[cfg(feature = "typst-engine")]
mod typst_engine;

#[cfg(feature = "typst-engine")]
pub use typst_engine::{Fonts, Layout, TypstMath};

/// A formula rendered to pixels.
pub struct Rendered {
    pub width: u32,
    pub height: u32,
    /// The baseline, in pixels from the top.
    pub baseline: f32,
    /// Straight (not premultiplied) RGBA, row by row.
    pub rgba: Vec<u8>,
}

/// KaTeX sets math 1.21 times as large as the surrounding text.
pub const SCALE: f32 = 1.21;

/// Renders `tex` with `px_per_em` pixels per em of the formula, drawing in
/// `rgba` wherever the TeX doesn't set a color.
pub fn render(tex: &str, display: bool, px_per_em: f32, rgba: [u8; 4]) -> Result<Rendered, String> {
    static CONTEXT: OnceLock<KatexContext> = OnceLock::new();
    let ctx = CONTEXT.get_or_init(KatexContext::default);
    let layout = katex_native::layout(ctx, tex, display).map_err(|err| err.to_string())?;
    let [r, g, b, a] = rgba;
    let raster =
        katex_native::raster::rasterize(&layout, f64::from(px_per_em), Color { r, g, b, a })
            .ok_or("The formula is empty.")?;
    Ok(Rendered {
        width: raster.width,
        height: raster.height,
        baseline: raster.baseline as f32,
        rgba: raster.rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_and_reports_errors() {
        let rendered = render(r"\frac{a}{b}", true, 20., [0, 0, 0, 255]).unwrap();
        assert_eq!(
            rendered.rgba.len(),
            (rendered.width * rendered.height * 4) as usize
        );
        assert!(rendered.baseline > 0. && rendered.baseline < rendered.height as f32);
        assert!(render(r"\frac{a}", false, 20., [0, 0, 0, 255]).is_err());
    }
}
