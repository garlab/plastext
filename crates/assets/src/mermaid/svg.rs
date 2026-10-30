//! Restricted generated SVG rasterization, adapted from mdviewer (MIT).
use crate::Preview;
use resvg::{tiny_skia, usvg};
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
};
const SVG_BYTES: usize = 256 * 1024;
static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
#[derive(Debug)]
pub(super) enum Error {
    Invalid,
    Unsupported,
    Limit,
}

pub(super) fn rasterize(source: &str) -> Result<Preview, Error> {
    let has_text = svg_preflight_inner(source, true)?;
    let missing_font = Arc::new(AtomicBool::new(false));
    let mut options = usvg::Options {
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_string: Box::new(|_, _| None),
            resolve_data: Box::new(|_, _, _| None),
        },
        font_family: "KaTeX_Main".into(),
        ..Default::default()
    };
    if has_text {
        options.fontdb = FONTS
            .get_or_init(|| {
                let mut database = usvg::fontdb::Database::new();
                for font in katex_native::FONTS.iter().filter(|font| {
                    matches!(
                        font.font.family,
                        katex_native::Family::Main
                            | katex_native::Family::SansSerif
                            | katex_native::Family::Typewriter
                    )
                }) {
                    let ids = database
                        .load_font_source(usvg::fontdb::Source::Binary(Arc::new(font.data)));
                    for id in ids {
                        if let Some(mut face) = database.face(id).cloned() {
                            face.style = if font.font.italic {
                                usvg::fontdb::Style::Italic
                            } else {
                                usvg::fontdb::Style::Normal
                            };
                            database.remove_face(id);
                            database.push_face_info(face);
                        }
                    }
                }
                database.set_serif_family("KaTeX_Main");
                database.set_sans_serif_family("KaTeX_SansSerif");
                database.set_monospace_family("KaTeX_Typewriter");
                Arc::new(database)
            })
            .clone();
        let select = usvg::FontResolver::default_font_selector();
        let missing = missing_font.clone();
        options.font_resolver.select_font = Box::new(move |font, database| {
            let selected = select(font, database);
            if selected.is_none() {
                missing.store(true, Ordering::Relaxed);
            }
            selected
        });
        let select = usvg::FontResolver::default_fallback_selector();
        let missing = missing_font.clone();
        options.font_resolver.select_fallback = Box::new(move |character, excluded, database| {
            let selected = select(character, excluded, database);
            if selected.is_none() {
                missing.store(true, Ordering::Relaxed);
            }
            selected
        });
    }
    let tree = usvg::Tree::from_str(source, &options).map_err(|_| Error::Invalid)?;
    if missing_font.load(Ordering::Relaxed) {
        return Err(Error::Unsupported);
    }
    let size = tree.size();
    let original = (size.width().ceil() as u32, size.height().ceil() as u32);
    if original.0 == 0
        || original.1 == 0
        || original.0 > 8192
        || original.1 > 8192
        || u64::from(original.0) * u64::from(original.1) > 4_000_000
    {
        return Err(Error::Limit);
    }
    // Raster at 2x for Retina. The logical size stays independent of pixel
    // caps, window resizing and zoom, so cached diagrams never jump in size.
    let scale = 2_f64
        .min(8192. / f64::from(original.0))
        .min(8192. / f64::from(original.1))
        .min((super::MAX_PIXELS as f64 / (f64::from(original.0) * f64::from(original.1))).sqrt());
    let (width, height) = (
        (f64::from(original.0) * scale).floor().max(1.) as u32,
        (f64::from(original.1) * scale).floor().max(1.) as u32,
    );
    let mut pixels = tiny_skia::Pixmap::new(width, height).ok_or(Error::Limit)?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(
            width as f32 / size.width(),
            height as f32 / size.height(),
        ),
        &mut pixels.as_mut(),
    );
    // GPUI uses straight alpha; tiny-skia produces premultiplied pixels.
    let rgba = pixels
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let pixel = pixel.demultiply();
            [pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()]
        })
        .collect();
    Ok(Preview {
        original_width: original.0,
        original_height: original.1,
        width,
        height,
        rgba,
    })
}
fn svg_preflight_inner(source: &str, markers: bool) -> Result<bool, Error> {
    if source.len() > SVG_BYTES {
        return Err(Error::Limit);
    }
    let document = roxmltree::Document::parse_with_options(
        source,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 4096,
        },
    )
    .map_err(|_| Error::Invalid)?;
    if document.root_element().tag_name().name() != "svg" {
        return Err(Error::Invalid);
    }
    let (mut geometry, mut text_bytes, mut has_text, mut marker_count) = (0, 0, false, 0);
    for node in document.descendants() {
        if node.ancestors().take(34).count() > 33 {
            return Err(Error::Limit);
        }
        if node.is_text() {
            text_bytes += node.text().unwrap_or_default().len();
        }
        if text_bytes > 32 * 1024 {
            return Err(Error::Limit);
        }
        if !node.is_element() {
            continue;
        }
        let name = node.tag_name().name();
        if name == "marker" && markers {
            marker_count += 1;
            // Arrow markers cannot recursively reference markers or paint servers.
            if marker_count > 16 || node.descendants().count() > 8 || node.range().len() > 2048 {
                return Err(Error::Limit);
            }
            for child in node.descendants() {
                for attr in child.attributes() {
                    if attr.value().to_ascii_lowercase().contains("url(") {
                        return Err(Error::Unsupported);
                    }
                }
            }
        } else if !matches!(
            name,
            "svg"
                | "g"
                | "defs"
                | "title"
                | "desc"
                | "path"
                | "rect"
                | "circle"
                | "ellipse"
                | "line"
                | "polyline"
                | "polygon"
                | "linearGradient"
                | "radialGradient"
                | "stop"
                | "clipPath"
                | "text"
                | "tspan"
                | "textPath"
                | "style"
        ) {
            return Err(Error::Unsupported);
        }
        has_text |= name == "text";
        if name == "style" {
            local_css(node.text().unwrap_or_default())?;
        }
        for attr in node.attributes() {
            if matches!(attr.name(), "d" | "points") {
                geometry += attr.value().len();
            }
            if attr.name() == "href" && !attr.value().starts_with('#') {
                return Err(Error::Unsupported);
            }
            local_css(attr.value())?;
        }
        if geometry > 64 * 1024 {
            return Err(Error::Limit);
        }
    }
    Ok(has_text)
}

fn local_css(value: &str) -> Result<(), Error> {
    let value = value.to_ascii_lowercase();
    if value.contains('\\') || value.contains("@import") || value.contains("@font-face") {
        return Err(Error::Unsupported);
    }
    for suffix in value.split("url(").skip(1) {
        let reference = suffix
            .split(')')
            .next()
            .unwrap_or_default()
            .trim()
            .trim_matches(['\'', '"']);
        if !reference.starts_with('#') {
            return Err(Error::Unsupported);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generated_svg_cannot_load_resources_or_expand_references() {
        for body in [
            "<image href='file:///tmp/private.png'/>",
            "<use href='#a'/>",
            "<script>alert(1)</script>",
            "<foreignObject/>",
            "<filter id='f'/>",
            "<style>@import 'https://example.com/a.css'</style>",
            "<path fill='url(https://example.com/x)'/>",
            "<text href='file:///tmp/private.txt'>label</text>",
            "<marker id='m'><path marker-end='url(#m)'/></marker>",
        ] {
            let source = format!(
                "<svg xmlns='http://www.w3.org/2000/svg' width='20' height='20'>{body}</svg>"
            );
            assert!(rasterize(&source).is_err(), "Accepted: {body}");
        }
        assert!(svg_preflight_inner(&format!("<svg>{}</svg>", "<g/>".repeat(5000)), true).is_err());
        assert!(
            svg_preflight_inner(
                &format!("<svg>{}{}</svg>", "<g>".repeat(40), "</g>".repeat(40)),
                true
            )
            .is_err()
        );
    }
    #[test]
    fn pixel_cap_preserves_logical_dimensions() {
        let p = rasterize("<svg xmlns='http://www.w3.org/2000/svg' width='2000' height='1000'><rect width='2000' height='1000' fill='red'/></svg>").unwrap();
        assert_eq!((p.original_width, p.original_height), (2000, 1000));
        assert!(p.width * p.height <= super::super::MAX_PIXELS as u32);
        assert!(p.width < p.original_width);
        assert!(
            rasterize("<svg xmlns='http://www.w3.org/2000/svg' width='9000' height='1000'/>")
                .is_err()
        );
    }
}
