//! Asset classification, data URI handling and view models. Decoders live in document-graphics.
pub mod csv;
pub mod icons;
use base64::Engine as _;
use std::path::Path;

pub const ENCODED_LIMIT: usize = 32 * 1024 * 1024;

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
            "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "webp"
                | "bmp"
                | "ico"
                | "tif"
                | "tiff"
                | "svg"
                | "avif"
                | "heic"
                | "heif"
                | "jxl"
                | "icns"
                | "psd"
                | "tga"
                | "qoi"
                | "pnm"
                | "ppm"
                | "pgm"
                | "pbm"
        )
    })
}

pub fn data_uri_bytes(uri: &str) -> Result<Vec<u8>, String> {
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
    if bytes.len() > ENCODED_LIMIT {
        return Err("Image exceeds the 32 MiB data URI limit".into());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn data_uris_preserve_bytes_and_reject_non_images() {
        assert_eq!(
            data_uri_bytes("data:image/png;base64,AQID").unwrap(),
            [1, 2, 3]
        );
        assert_eq!(
            data_uri_bytes("data:image/svg+xml,%3Csvg%2F%3E").unwrap(),
            b"<svg/>"
        );
        assert!(data_uri_bytes("data:text/html,<script>").is_err());
        assert!(data_uri_bytes("data:image/png;base64,!").is_err());
    }
}
