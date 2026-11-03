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
    render_with_macros(
        tex,
        display,
        first_number,
        px_per_em,
        rgba,
        std::iter::empty(),
    )
}

/// Each render owns its macro namespace. Replay earlier definitions in document
/// order; malformed formulas cannot leave partially applied definitions behind.
pub fn render_with_macros<'a>(
    tex: &str,
    display: bool,
    first_number: u32,
    px_per_em: f32,
    rgba: [u8; 4],
    definitions: impl Iterator<Item = (&'a str, bool)>,
) -> Result<Rendered, String> {
    let settings = macro_settings(display, definitions);
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
        body: raster(&layout)
            .or_else(|| {
                (layout.width == 0.).then_some(Raster {
                    width: 1,
                    height: 1,
                    baseline: 0.,
                    rgba: vec![0; 4],
                })
            })
            .ok_or("The formula could not be rasterized.")?,
        tag: layout.tag.as_deref().and_then(raster),
    })
}

/// How many automatic equation numbers display math `tex` uses, to number
/// the next formula's from. Formulas that can't number equations are not
/// laid out.
pub fn equation_numbers(tex: &str) -> u32 {
    equation_numbers_with_macros(tex, std::iter::empty())
}

pub fn equation_numbers_with_macros<'a>(
    tex: &str,
    definitions: impl Iterator<Item = (&'a str, bool)>,
) -> u32 {
    let mut definitions = definitions.peekable();
    if !may_number(tex) && definitions.peek().is_none() {
        return 0;
    }
    let settings = macro_settings(true, definitions);
    katex_native::layout_with(context(), tex, &settings).map_or(0, |layout| layout.equation_numbers)
}

fn macro_settings<'a>(
    display: bool,
    definitions: impl Iterator<Item = (&'a str, bool)>,
) -> Settings {
    let mut macros = Default::default();
    for (tex, display) in definitions {
        let settings = Settings::builder()
            .display_mode(display)
            .global_group(true)
            .max_expand(1000)
            .macros(macros)
            .build();
        let previous = settings.macros.borrow().clone();
        macros = if katex_native::katex::parse_tree::parse_tree(context(), tex, &settings).is_ok() {
            settings.macros.into_inner()
        } else {
            previous
        };
    }
    Settings::builder()
        .display_mode(display)
        .global_group(true)
        .max_expand(1000)
        .macros(macros)
        .build()
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
    fn document_macros_are_ordered_isolated_and_bounded() {
        let definitions = [(r"\newcommand{\sq}[1]{#1^2}\def\N{\mathbb{N}}", true)];
        let render =
            |tex: &str| render_with_macros(tex, false, 1, 20., [255; 4], definitions.into_iter());
        assert!(render(r"f: \N \to \N,\quad \sq{x}").is_ok());
        assert!(super::render(r"\sq{x}", false, 1, 20., [255; 4]).is_err());
        assert!(
            render_with_macros(
                r"\loop",
                false,
                1,
                20.,
                [255; 4],
                [(r"\def\loop{\loop}", true)].into_iter()
            )
            .is_err()
        );
        assert!(
            render_with_macros(
                r"\leaked",
                false,
                1,
                20.,
                [255; 4],
                [(r"\def\leaked{1}\badcommand", true)].into_iter()
            )
            .is_err()
        );
        let first = render_with_macros(
            r"\x",
            false,
            1,
            20.,
            [255; 4],
            [(r"\def\x{1}", true)].into_iter(),
        )
        .unwrap();
        let second = render_with_macros(
            r"\x",
            false,
            1,
            20.,
            [255; 4],
            [(r"\def\x{1}", true), (r"\def\x{222}", true)].into_iter(),
        )
        .unwrap();
        assert!(second.body.width > first.body.width);
        assert!(super::render(r"\def\x{1}", true, 1, 20., [255; 4]).is_ok());
        assert_eq!(
            equation_numbers_with_macros(
                r"\begin{equation}\sq{x}\end{equation}",
                definitions.into_iter()
            ),
            1
        );
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

    #[test]
    fn renders_blackboard_aliases_and_aligned_math_with_blank_lines() {
        assert!(render(r"f: \Bbb N \to \Bbb N", false, 1, 29., [255; 4]).is_ok());
        let tex = "\\begin{aligned}\nf(0) &= 0 \\\\\n\nf(1) &= 1\n\\end{aligned}";
        assert!(render(tex, true, 1, 20., [255; 4]).is_ok());
    }
}
