//! End-to-end decoder protocol tests, including the actual child sandbox.
use plastext_platform::images::{self, Document, Region, SCALE_UNIT};
use std::{
    io::Write,
    path::Path,
    sync::{Arc, atomic::AtomicBool},
    time::Instant,
};

fn main() {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--image-worker")) {
        images::worker_main().unwrap();
        return;
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("corners.png");
    let image = image::RgbaImage::from_fn(67, 53, |x, y| {
        image::Rgba([x as u8 * 3, y as u8 * 4, 100, 255])
    });
    image.save(&path).unwrap();
    let (document, info) = Document::open(&path, &cancel).unwrap();
    let overview = document
        .render(&info, vec![info.overview(0)], &cancel)
        .unwrap();
    assert_pixels(&overview.pixels[0].rgba, image.as_raw(), 1);
    for scale in [SCALE_UNIT / 2, SCALE_UNIT, SCALE_UNIT * 3] {
        let (w, h) = info.scaled_size(scale);
        let r = Region {
            frame: 0,
            scale,
            x: w - 13,
            y: h - 9,
            width: 13,
            height: 9,
        };
        let frame = document.render(&info, vec![r], &cancel).unwrap();
        for y in 0..r.height {
            for x in 0..r.width {
                let expected =
                    image.get_pixel(r.source_x(x, info.width), r.source_y(y, info.height));
                let i = (y * r.width + x) as usize * 4;
                assert_pixels(&frame.pixels[0].rgba[i..i + 4], &expected.0, 1);
            }
        }
    }
    println!("PNG overview, cropped regions and nearest-neighbor zoom passed");
    for format in [
        image::ImageFormat::Jpeg,
        image::ImageFormat::Bmp,
        image::ImageFormat::Ico,
        image::ImageFormat::Tiff,
        image::ImageFormat::WebP,
        image::ImageFormat::Tga,
        image::ImageFormat::Qoi,
        image::ImageFormat::Pnm,
    ] {
        let path = directory
            .path()
            .join(format!("static.{}", format.extensions_str()[0]));
        let rgb = image::DynamicImage::ImageRgba8(image.clone()).into_rgb8();
        image::DynamicImage::ImageRgb8(rgb)
            .save_with_format(&path, format)
            .unwrap();
        let (doc, info) =
            Document::open(&path, &cancel).unwrap_or_else(|e| panic!("{format:?}: {e}"));
        let result = doc.render(&info, vec![info.overview(0)], &cancel).unwrap();
        assert_eq!((info.width, info.height), (67, 53));
        assert!(
            result.pixels[0]
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| p[3] == 255)
        );
    }
    println!("JPEG, BMP, ICO, TIFF, WebP, TGA, QOI and PNM overviews passed");
    let icon = directory.path().join("sizes.ico");
    let frames: Vec<_> = [16, 32, 64]
        .into_iter()
        .map(|side| {
            image::codecs::ico::IcoFrame::as_png(
                &image::RgbaImage::from_pixel(side, side, image::Rgba([255, 0, 0, 255])),
                side,
                side,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap()
        })
        .collect();
    image::codecs::ico::IcoEncoder::new(std::fs::File::create(&icon).unwrap())
        .encode_images(&frames)
        .unwrap();
    let (icon, icon_info) = Document::open(&icon, &cancel).unwrap();
    assert_eq!((icon_info.width, icon_info.height), (64, 64));
    assert!(
        icon.render(&icon_info, vec![icon_info.overview(0)], &cancel)
            .is_ok()
    );
    let svg = directory.path().join("regions.svg");
    std::fs::write(&svg,r##"<svg xmlns="http://www.w3.org/2000/svg" width="10000" height="10000"><rect x="9000" y="9000" width="1000" height="1000" fill="#ff0000"/><image href="https://example.invalid/no-network.png" width="10" height="10"/></svg>"##).unwrap();
    let (vector, metadata) = Document::open(&svg, &cancel).unwrap();
    let r = Region {
        frame: 0,
        scale: SCALE_UNIT * 2,
        x: 19000,
        y: 19000,
        width: 512,
        height: 512,
    };
    let frame = vector.render(&metadata, vec![r], &cancel).unwrap();
    assert!(
        frame.pixels[0]
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [255, 0, 0, 255])
    );
    // Switching documents recycles the child, including metadata initialization.
    let again = document
        .render(&info, vec![info.overview(0)], &cancel)
        .unwrap();
    assert_pixels(&again.pixels[0].rgba, image.as_raw(), 1);
    // The helper always reads the immutable snapshot, even after truncation.
    std::fs::write(&path, b"changed").unwrap();
    assert!(
        document
            .render(&info, vec![info.overview(0)], &cancel)
            .is_ok()
    );
    println!("SVG tiles, worker recycling and immutable snapshots passed");
    #[cfg(target_os = "macos")]
    {
        animations(directory.path(), &cancel);
        disposal(directory.path(), &cancel);
        webp_disposal(directory.path(), &cancel);
    }
    if let Some(path) = std::env::var_os("PLASTEXT_TEST_HUGE_IMAGE") {
        let start = Instant::now();
        let (large, info) = Document::open(Path::new(&path), &cancel).unwrap();
        assert_eq!((info.width, info.height), (40_000, 40_000));
        let frame = large
            .render(&info, vec![info.overview(0)], &cancel)
            .unwrap();
        let overview_ms = start.elapsed().as_secs_f64() * 1000.;
        let start = Instant::now();
        let r = Region {
            frame: 0,
            scale: SCALE_UNIT,
            x: 39_488,
            y: 39_488,
            width: 512,
            height: 512,
        };
        let tile = large.render(&info, vec![r], &cancel).unwrap();
        for y in 0..512 {
            assert_eq!(tile.pixels[0].rgba[y * 512 * 4], ((39_488 + y) % 256) as u8);
        }
        println!(
            "40k PNG: overview {overview_ms:.1} ms, bottom tile {:.1} ms; retained output {} bytes",
            start.elapsed().as_secs_f64() * 1000.,
            frame.pixels[0].rgba.len() + tile.pixels[0].rgba.len()
        );
    }
    #[cfg(target_os = "macos")]
    {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        for name in ["sample.avif", "sample.heic", "oriented.jpg", "profile.png"] {
            println!("Checking {name}");
            let path = Path::new(&dir).join(name);
            let (doc, info) = Document::open(&path, &cancel).unwrap();
            let image = doc.render(&info, vec![info.overview(0)], &cancel).unwrap();
            assert!(!image.pixels[0].rgba.is_empty());
            if name == "oriented.jpg" {
                assert_eq!((info.width, info.height), (24, 32));
            }
            let region = Region {
                frame: 0,
                scale: SCALE_UNIT,
                x: 5,
                y: 5,
                width: 8,
                height: 8,
            };
            let tile = doc.render(&info, vec![region], &cancel).unwrap();
            for y in 0..8 {
                for x in 0..8 {
                    let index = ((y + 5) * info.width + x + 5) as usize * 4;
                    assert_pixels(
                        &tile.pixels[0].rgba
                            [((y * 8 + x) * 4) as usize..((y * 8 + x + 1) * 4) as usize],
                        &image.pixels[0].rgba[index..index + 4],
                        2,
                    );
                }
            }
            println!("{name}: {} × {}, {}", info.width, info.height, info.color);
        }
    }
    println!("Image worker integration passed");
}

#[cfg(target_os = "macos")]
fn disposal(dir: &Path, cancel: &images::Cancel) {
    use image::AnimationDecoder;
    let path = dir.join("disposal.gif");
    let mut file = std::fs::File::create(&path).unwrap();
    let mut encoder = gif::Encoder::new(&mut file, 8, 8, &[]).unwrap();
    encoder.set_repeat(gif::Repeat::Finite(2)).unwrap();
    for (i, dispose) in [
        gif::DisposalMethod::Keep,
        gif::DisposalMethod::Background,
        gif::DisposalMethod::Previous,
        gif::DisposalMethod::Keep,
    ]
    .into_iter()
    .enumerate()
    {
        let side = if i == 0 { 8 } else { 4 };
        let mut pixels = Vec::new();
        for y in 0..side {
            for x in 0..side {
                pixels.extend_from_slice(&[
                    if i % 2 == 0 { 255 } else { 0 },
                    if i == 2 { 255 } else { 0 },
                    if i % 2 == 1 { 255 } else { 0 },
                    if i > 0 && x == y { 0 } else { 255 },
                ]);
            }
        }
        let mut frame = gif::Frame::from_rgba_speed(side, side, &mut pixels, 10);
        frame.dispose = dispose;
        frame.delay = 5;
        if i > 0 {
            frame.left = 2;
            frame.top = 2;
        }
        encoder.write_frame(&frame).unwrap();
    }
    drop(encoder);
    drop(file);
    let expected = image::codecs::gif::GifDecoder::new(std::io::BufReader::new(
        std::fs::File::open(&path).unwrap(),
    ))
    .unwrap()
    .into_frames()
    .collect_frames()
    .unwrap();
    let (doc, info) = Document::open(&path, cancel).unwrap();
    assert_eq!(info.plays, Some(3));
    for frame in [0, 1, 2, 3, 0, 3, 1] {
        let result = doc
            .render(&info, vec![info.overview(frame)], cancel)
            .unwrap();
        // Fully transparent pixels have no meaningful RGB color.
        for (a, b) in result.pixels[0].rgba.as_chunks::<4>().0.iter().zip(
            expected[frame as usize]
                .buffer()
                .as_raw()
                .as_chunks::<4>()
                .0
                .iter(),
        ) {
            assert_eq!(a[3], b[3], "alpha in GIF frame {frame}");
            if b[3] != 0 {
                assert_pixels(a, b, 1);
            }
        }
    }
    println!("GIF offsets, transparency, disposal and finite loops passed");
}

fn assert_pixels(actual: &[u8], expected: &[u8], tolerance: i16) {
    assert_eq!(actual.len(), expected.len());
    for (i, (&a, &b)) in actual.iter().zip(expected).enumerate() {
        assert!(
            (i16::from(a) - i16::from(b)).abs() <= tolerance,
            "pixel byte {i}: {a} != {b}"
        );
    }
}

#[cfg(target_os = "macos")]
fn webp_disposal(dir: &Path, cancel: &images::Cancel) {
    use image::AnimationDecoder;
    let path = dir.join("composited.webp");
    let mut chunks = Vec::new();
    chunk(&mut chunks, b"VP8X", &[0x12, 0, 0, 0, 7, 0, 0, 7, 0, 0]);
    chunk(&mut chunks, b"ANIM", &[0, 0, 0, 0, 0, 0]);
    for (side, x, y, color, flags) in [
        (8, 0, 0, [255, 0, 0, 255], 2),
        (4, 1, 1, [0, 0, 255, 128], 1),
        (4, 2, 2, [0, 255, 0, 255], 2),
    ] {
        let frame = image::RgbaImage::from_pixel(side, side, image::Rgba(color));
        let mut encoded = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut encoded)
            .encode(frame.as_raw(), side, side, image::ExtendedColorType::Rgba8)
            .unwrap();
        let mut body = vec![
            x,
            0,
            0,
            y,
            0,
            0,
            (side - 1) as u8,
            0,
            0,
            (side - 1) as u8,
            0,
            0,
            80,
            0,
            0,
            flags,
        ];
        body.extend_from_slice(&encoded[12..]);
        chunk(&mut chunks, b"ANMF", &body);
    }
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&(chunks.len() as u32 + 4).to_le_bytes());
    bytes.extend_from_slice(b"WEBP");
    bytes.extend_from_slice(&chunks);
    std::fs::write(&path, &bytes).unwrap();
    let mut reference = image::codecs::webp::WebPDecoder::new(std::io::Cursor::new(bytes)).unwrap();
    // image-webp leaves disposal disabled unless a background is selected.
    // Match this fixture's transparent ANIM background and libwebp/Pillow.
    reference
        .set_background_color(image::Rgba([0, 0, 0, 0]))
        .unwrap();
    let expected = reference.into_frames().collect_frames().unwrap();
    let (doc, info) = Document::open(&path, cancel).unwrap();
    assert_eq!(info.frames, 3);
    for frame in [0, 1, 2, 0, 2, 1] {
        let result = doc
            .render(&info, vec![info.overview(frame)], cancel)
            .unwrap();
        for (a, b) in result.pixels[0].rgba.as_chunks::<4>().0.iter().zip(
            expected[frame as usize]
                .buffer()
                .as_raw()
                .as_chunks::<4>()
                .0,
        ) {
            assert_eq!(a[3], b[3], "alpha in WebP frame {frame}");
            if b[3] != 0 {
                assert_pixels(a, b, 2);
            }
        }
    }
    println!("WebP offsets, alpha blending, disposal and random access passed");
}

#[cfg(target_os = "macos")]
fn animations(dir: &Path, cancel: &images::Cancel) {
    let gif = dir.join("animation.gif");
    let red = image::RgbaImage::from_pixel(4, 4, image::Rgba([255, 0, 0, 255]));
    let blue = image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 255, 255]));
    let mut encoder = image::codecs::gif::GifEncoder::new(std::fs::File::create(&gif).unwrap());
    encoder
        .set_repeat(image::codecs::gif::Repeat::Infinite)
        .unwrap();
    for frame in [&red, &blue] {
        encoder
            .encode_frame(image::Frame::from_parts(
                frame.clone(),
                0,
                0,
                image::Delay::from_numer_denom_ms(80, 1),
            ))
            .unwrap();
    }
    drop(encoder);
    let webp = dir.join("animation.webp");
    let mut chunks = Vec::new();
    // VP8X: animation flag, reserved bytes, canvas width-1 / height-1.
    chunk(&mut chunks, b"VP8X", &[2, 0, 0, 0, 3, 0, 0, 3, 0, 0]);
    chunk(&mut chunks, b"ANIM", &[0, 0, 0, 0, 0, 0]);
    for frame in [&red, &blue] {
        let mut encoded = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut encoded)
            .encode(frame.as_raw(), 4, 4, image::ExtendedColorType::Rgba8)
            .unwrap();
        let mut body = vec![0, 0, 0, 0, 0, 0, 3, 0, 0, 3, 0, 0, 80, 0, 0, 2]; // full frame; replace, keep
        body.extend_from_slice(&encoded[12..]);
        chunk(&mut chunks, b"ANMF", &body);
    }
    let mut file = std::fs::File::create(&webp).unwrap();
    file.write_all(b"RIFF").unwrap();
    file.write_all(&(chunks.len() as u32 + 4).to_le_bytes())
        .unwrap();
    file.write_all(b"WEBP").unwrap();
    file.write_all(&chunks).unwrap();
    drop(file);
    for path in [gif, webp] {
        let (doc, info) = Document::open(&path, cancel).unwrap();
        assert_eq!(info.frames, 2);
        assert_eq!(info.plays, None);
        // Sequential playback, loop rewind and random access produce composited pixels.
        for frame in [0, 1, 0, 1] {
            let result = doc
                .render(&info, vec![info.overview(frame)], cancel)
                .unwrap();
            assert_eq!(result.delay_ms, 80);
            assert_pixels(
                &result.pixels[0].rgba,
                if frame == 0 {
                    red.as_raw()
                } else {
                    blue.as_raw()
                },
                1,
            );
        }
        println!("{} animation frames and delays passed", info.format);
    }
}
fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(kind);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    if !body.len().is_multiple_of(2) {
        out.push(0);
    }
}
