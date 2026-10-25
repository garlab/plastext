//! MiTeX conversion laid out by Typst, kept only to measure what that route
//! costs (spec §7.2 math spike). One engine is reused across formulas: its
//! library, fonts and MiTeX helpers stay fixed, so Typst's memoization can
//! carry work from one formula to the next. The app never uses this unless
//! it's built with `--features math-spike`.

use std::{fmt::Write as _, path::PathBuf};

use typst::{
    Library, LibraryExt as _, World,
    diag::{FileError, FileResult},
    foundations::{Bytes, Datetime},
    layout::{Frame, FrameItem},
    syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot},
    text::{Font, FontBook},
    utils::LazyHash,
};
use typst_layout::PagedDocument;

/// Where the engine's fonts come from.
#[derive(Clone, Copy, Debug)]
pub enum Fonts {
    /// Every font Typst bundles (~9.7 MB), as the first spike did.
    Bundled,
    /// Only New Computer Modern Math (~1.3 MB), from Typst's bundle.
    BundledMath,
    /// STIX Two Math and Text, which ship with macOS. Nothing is embedded.
    System,
}

/// What a laid-out formula would hand to the renderer.
#[derive(Clone, Copy, Debug, Default)]
pub struct Layout {
    pub width: f64,
    pub height: f64,
    pub glyphs: usize,
    pub shapes: usize,
}

pub struct TypstMath {
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    fonts: Vec<Font>,
    standard: Source,
    prelude: Source,
    preamble: String,
}

impl TypstMath {
    pub fn new(fonts: Fonts) -> Result<Self, String> {
        let (fonts, text_font, math_font) = match fonts {
            Fonts::Bundled => (
                typst_assets::fonts()
                    .flat_map(|data| Font::iter(Bytes::new(data)))
                    .collect::<Vec<_>>(),
                "Libertinus Serif",
                "New Computer Modern Math",
            ),
            Fonts::BundledMath => (
                typst_assets::fonts()
                    .flat_map(|data| Font::iter(Bytes::new(data)))
                    .filter(|font| {
                        font.info().family == "New Computer Modern Math"
                            && font.info().variant == Default::default()
                    })
                    .collect(),
                "New Computer Modern Math",
                "New Computer Modern Math",
            ),
            Fonts::System => {
                let dir = "/System/Library/Fonts/Supplemental";
                let mut fonts = Vec::new();
                for file in [
                    "STIXTwoMath.otf",
                    "STIXTwoText.ttf",
                    "STIXTwoText-Italic.ttf",
                ] {
                    let data = std::fs::read(format!("{dir}/{file}"))
                        .map_err(|err| format!("{file}: {err}"))?;
                    fonts.extend(Font::iter(Bytes::new(data)));
                }
                (fonts, "STIX Two Text", "STIX Two Math")
            }
        };
        if fonts.is_empty() {
            return Err("no fonts".into());
        }
        let mut book = FontBook::new();
        for font in &fonts {
            book.push(font.info().clone());
        }
        Ok(Self {
            library: LazyHash::new(Library::builder().build()),
            book: LazyHash::new(book),
            fonts,
            standard: Source::new(
                file_id("standard.typ"),
                include_str!("../spike/mitex/standard.typ").into(),
            ),
            prelude: Source::new(
                file_id("prelude.typ"),
                include_str!("../spike/mitex/prelude.typ").into(),
            ),
            preamble: format!(
                "#import \"standard.typ\": package\n\
                 #set page(width: auto, height: auto, margin: 0pt)\n\
                 #set text(size: 16pt, font: \"{text_font}\")\n\
                 #show math.equation: set text(font: \"{math_font}\")\n"
            ),
        })
    }

    /// Converts TeX to Typst math markup, without laying it out.
    pub fn convert(tex: &str) -> Result<String, String> {
        mitex::convert_math(tex, None)
    }

    pub fn layout(&self, tex: &str, display: bool) -> Result<Layout, String> {
        Ok(self.layout_all(&[(tex, display)])?[0])
    }

    /// Lays out several formulas in one Typst document, one page each. One
    /// failing formula fails them all.
    pub fn layout_all(&self, formulas: &[(&str, bool)]) -> Result<Vec<Layout>, String> {
        let mut main = self.preamble.clone();
        for (i, &(tex, display)) in formulas.iter().enumerate() {
            let converted = Self::convert(tex)?;
            let markup = if display {
                format!("$ {converted} $")
            } else {
                format!("${converted}$")
            };
            if i > 0 {
                main.push_str("#pagebreak()\n");
            }
            writeln!(
                main,
                "#math.equation(block: {display}, eval({markup:?}, scope: package.scope))"
            )
            .ok();
        }
        let world = MathWorld {
            math: self,
            main: Source::new(file_id("main.typ"), main),
        };
        let document: PagedDocument = typst::compile(&world)
            .output
            .map_err(|errors| format!("{errors:?}"))?;
        if document.pages().len() != formulas.len() {
            return Err(format!(
                "expected {} pages, got {}",
                formulas.len(),
                document.pages().len()
            ));
        }
        Ok(document
            .pages()
            .iter()
            .map(|page| {
                let size = page.frame.size();
                let mut layout = Layout {
                    width: size.x.to_pt(),
                    height: size.y.to_pt(),
                    ..Layout::default()
                };
                count(&page.frame, &mut layout);
                layout
            })
            .collect())
    }

    /// Drops memoized results not used in the last `max_age` compilations.
    pub fn evict(max_age: usize) {
        typst::comemo::evict(max_age);
    }
}

fn count(frame: &Frame, layout: &mut Layout) {
    for (_, item) in frame.items() {
        match item {
            FrameItem::Group(group) => count(&group.frame, layout),
            FrameItem::Text(text) => layout.glyphs += text.glyphs.len(),
            FrameItem::Shape(..) => layout.shapes += 1,
            _ => {}
        }
    }
}

fn file_id(path: &str) -> FileId {
    RootedPath::new(
        VirtualRoot::Project,
        VirtualPath::new(path).expect("valid path"),
    )
    .intern()
}

struct MathWorld<'a> {
    math: &'a TypstMath,
    main: Source,
}

impl World for MathWorld<'_> {
    fn library(&self) -> &LazyHash<Library> {
        &self.math.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.math.book
    }

    fn main(&self) -> FileId {
        self.main.id()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        [&self.main, &self.math.standard, &self.math.prelude]
            .into_iter()
            .find(|source| source.id() == id)
            .cloned()
            .ok_or_else(|| FileError::NotFound(PathBuf::from(format!("{id:?}"))))
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        Err(FileError::NotFound(PathBuf::from(format!("{id:?}"))))
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.math.fonts.get(index).cloned()
    }

    fn today(&self, _: Option<typst::foundations::Duration>) -> Option<Datetime> {
        None
    }
}
