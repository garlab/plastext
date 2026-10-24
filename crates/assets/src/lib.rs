//! Bounded, UI-independent image decoding. No network access is performed here.
pub mod csv;
use base64::Engine as _;
use image::{GenericImageView, ImageReader};
use std::{
    fs::File,
    io::{Cursor, Read},
    path::Path,
};

pub const ENCODED_LIMIT: usize = 32 * 1024 * 1024;
pub const PIXEL_LIMIT: u64 = 16 * 1024 * 1024;
const PREVIEW_EDGE: u32 = 1600;

pub struct Preview {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub original_width: u32,
    pub original_height: u32,
}

pub fn is_image(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()).is_some_and(|s| {
        matches!(
            s.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "tif" | "tiff" | "svg"
        )
    })
}

/// Bound the encoded allocation too, including files that grow while read.
pub fn load_local(path: &Path) -> Result<Preview, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| e.to_string())?
        .take(ENCODED_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    decode(&bytes)
}

pub fn load_data_uri(uri: &str) -> Result<Preview, String> {
    if uri.len() > ENCODED_LIMIT * 2 {
        return Err("Image data URI exceeds the preview limit.".into());
    }
    let (header, data) = uri.split_once(',').ok_or("Invalid image data URI")?;
    if !header.starts_with("data:image/") {
        return Err("Only image data URIs are supported.".into());
    }
    let bytes = if header.ends_with(";base64") {
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| e.to_string())?
    } else {
        percent_encoding::percent_decode_str(data).collect()
    };
    decode(&bytes)
}

pub fn decode(bytes: &[u8]) -> Result<Preview, String> {
    if bytes.len() > ENCODED_LIMIT {
        return Err("Image exceeds the 32 MiB encoded preview limit.".into());
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]);
    if head.contains("<svg") {
        return svg(bytes);
    }
    let make_reader = || {
        ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|e| e.to_string())
    };
    let (original_width, original_height) = make_reader()?
        .into_dimensions()
        .map_err(|e| e.to_string())?;
    if u64::from(original_width) * u64::from(original_height) > PIXEL_LIMIT {
        return Err(
            "Image exceeds the 16-megapixel preview limit; tiled decoding is not available yet."
                .into(),
        );
    }
    let mut reader = make_reader()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(128 * 1024 * 1024);
    limits.max_image_width = Some(32768);
    limits.max_image_height = Some(32768);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|e| e.to_string())?;
    let decoded = if original_width > PREVIEW_EDGE || original_height > PREVIEW_EDGE {
        decoded.thumbnail(PREVIEW_EDGE, PREVIEW_EDGE)
    } else {
        decoded
    };
    let (width, height) = decoded.dimensions();
    Ok(Preview {
        rgba: decoded.into_rgba8().into_raw(),
        width,
        height,
        original_width,
        original_height,
    })
}

fn svg(bytes: &[u8]) -> Result<Preview, String> {
    use resvg::{tiny_skia, usvg};
    // Neither external hrefs nor nested data images may bypass our limits.
    let mut options = usvg::Options {
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_string: Box::new(|_, _| None),
            resolve_data: Box::new(|_, _, _| None),
        },
        ..Default::default()
    };
    if bytes.windows(5).any(|s| s == b"<text") {
        static FONTS: std::sync::OnceLock<std::sync::Arc<usvg::fontdb::Database>> =
            std::sync::OnceLock::new();
        options.fontdb = FONTS
            .get_or_init(|| {
                let mut fonts = usvg::fontdb::Database::new();
                fonts.load_system_fonts();
                std::sync::Arc::new(fonts)
            })
            .clone();
    }
    let tree = usvg::Tree::from_data(bytes, &options).map_err(|e| e.to_string())?;
    let size = tree.size();
    let scale = (PREVIEW_EDGE as f32 / size.width().max(size.height())).min(1.);
    let width = (size.width() * scale).ceil().max(1.) as u32;
    let height = (size.height() * scale).ceil().max(1.) as u32;
    let mut pixels = tiny_skia::Pixmap::new(width, height).ok_or("Invalid SVG dimensions")?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixels.as_mut(),
    );
    // tiny-skia premultiplies alpha; the UI image adapter expects straight RGBA.
    let rgba = pixels
        .pixels()
        .iter()
        .flat_map(|p| {
            let p = p.demultiply();
            [p.red(), p.green(), p.blue(), p.alpha()]
        })
        .collect();
    Ok(Preview {
        rgba,
        width,
        height,
        original_width: size.width().ceil() as u32,
        original_height: size.height().ceil() as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn svg_is_bounded_and_external_resources_are_not_loaded() {
        let svg=br#"<svg xmlns="http://www.w3.org/2000/svg" width="40000" height="40000"><image href="https://example.com/x.png" width="40" height="40"/><rect width="40000" height="40000" fill="red"/></svg>"#;
        let preview = decode(svg).unwrap();
        assert_eq!((preview.width, preview.height), (1600, 1600));
        assert_eq!(preview.original_width, 40000);
        assert_eq!(&preview.rgba[..4], &[255, 0, 0, 255]);
    }
    #[test]
    fn raster_dimensions_and_data_uris() {
        let mut bytes = Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(2, 3, image::Rgba([255, 0, 0, 255]))
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let data = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes.get_ref())
        );
        let preview = load_data_uri(&data).unwrap();
        assert_eq!((preview.width, preview.height), (2, 3));
        assert!(load_data_uri("data:text/html,<script>").is_err());
        assert!(decode(&[0, 1, 2]).is_err());
    }
}
