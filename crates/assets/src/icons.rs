//! Small, inert SVG/PNG artwork for file-icon themes.
use crate::Preview;
use image::{GenericImageView, ImageReader};
use resvg::{tiny_skia, usvg};
use std::{
    fs::File,
    io::{Cursor, Read},
    path::Path,
};

pub const MAX_ICON_BYTES: usize = 256 * 1024;

pub fn read(path: &Path, root: &Path) -> Result<Vec<u8>, String> {
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    if !path.starts_with(root) {
        return Err("Icon asset leaves its theme directory".into());
    }
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(MAX_ICON_BYTES as u64 + 1).read_to_end(&mut bytes))
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_ICON_BYTES {
        return Err("Icon exceeds 256 KiB".into());
    }
    Ok(bytes)
}

/// Rasterize into at most 64×64 pixels. External SVG resources and fonts are
/// disabled; PNG dimensions are checked before allocating the decoded image.
pub fn decode(bytes: &[u8], edge: u32) -> Result<Preview, String> {
    if bytes.len() > MAX_ICON_BYTES {
        return Err("Icon exceeds 256 KiB".into());
    }
    let edge = edge.clamp(16, 64);
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        let make_reader = || ImageReader::with_format(Cursor::new(bytes), image::ImageFormat::Png);
        let (w, h) = make_reader().into_dimensions().map_err(|e| e.to_string())?;
        if w > 1024 || h > 1024 {
            return Err("PNG icon exceeds 1024×1024 pixels".into());
        }
        let mut reader = make_reader();
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(8 * 1024 * 1024);
        limits.max_image_width = Some(1024);
        limits.max_image_height = Some(1024);
        reader.limits(limits);
        let image = reader
            .decode()
            .map_err(|e| e.to_string())?
            .thumbnail(edge, edge);
        let (width, height) = image.dimensions();
        return Ok(Preview {
            rgba: image.into_rgba8().into_raw(),
            width,
            height,
            original_width: w,
            original_height: h,
        });
    }
    let options = usvg::Options {
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_string: Box::new(|_, _| None),
            resolve_data: Box::new(|_, _, _| None),
        },
        ..Default::default()
    };
    let tree = usvg::Tree::from_data(bytes, &options).map_err(|e| e.to_string())?;
    let size = tree.size();
    let scale = edge as f32 / size.width().max(size.height());
    let width = (size.width() * scale).ceil().max(1.) as u32;
    let height = (size.height() * scale).ceil().max(1.) as u32;
    let mut pixels = tiny_skia::Pixmap::new(width, height).ok_or("Invalid SVG dimensions")?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixels.as_mut(),
    );
    let rgba = pixels
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let p = pixel.demultiply();
            [p.red(), p.green(), p.blue(), p.alpha()]
        })
        .collect();
    Ok(Preview {
        rgba,
        width,
        height,
        original_width: width,
        original_height: height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn svg_output_and_encoded_input_are_bounded() {
        let svg=br#"<svg xmlns="http://www.w3.org/2000/svg" width="40000" height="20000"><rect width="40000" height="20000" fill="red"/><image href="https://example.test/image.png" width="10" height="10"/></svg>"#;
        let image = decode(svg, 32).unwrap();
        assert_eq!((image.width, image.height), (32, 16));
        assert_eq!(image.rgba.len(), 32 * 16 * 4);
        assert!(decode(&vec![b' '; MAX_ICON_BYTES + 1], 32).is_err());
        assert!(decode(b"not an image", 32).is_err());
    }
    #[test]
    fn png_dimensions_are_checked_before_decoding() {
        let mut bytes = Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(48, 24, image::Rgba([0, 255, 0, 255]))
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let icon = decode(bytes.get_ref(), 32).unwrap();
        assert_eq!((icon.width, icon.height), (32, 16));
        let mut large = Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(1025, 1, image::Rgba([0, 0, 0, 255]))
            .write_to(&mut large, image::ImageFormat::Png)
            .unwrap();
        assert!(decode(large.get_ref(), 32).err().unwrap().contains("1024"));
    }
    #[cfg(unix)]
    #[test]
    fn symlinks_cannot_read_outside_the_imported_directory() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("f.svg"), b"outside").unwrap();
        let path = root.path().join("f.svg");
        std::os::unix::fs::symlink(outside.path().join("f.svg"), &path).unwrap();
        assert!(read(&path, root.path()).unwrap_err().contains("leaves"));
    }
}
