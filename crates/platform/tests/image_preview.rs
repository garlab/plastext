//! End-to-end decoder protocol tests, including the actual child sandbox.
#[cfg(unix)]
use plastext_platform::images::{self, Document, Region, SCALE_UNIT};
#[cfg(unix)]
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
    time::Instant,
};

#[cfg(unix)]
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
    println!("Image snapshot, broker and shared-worker integration passed");
}

#[cfg(unix)]
fn assert_pixels(actual: &[u8], expected: &[u8], tolerance: i16) {
    assert_eq!(actual.len(), expected.len());
    for (i, (&a, &b)) in actual.iter().zip(expected).enumerate() {
        assert!(
            (i16::from(a) - i16::from(b)).abs() <= tolerance,
            "pixel byte {i}: {a} != {b}"
        );
    }
}

#[cfg(not(unix))]
fn main() {
    use plastext_platform::images::{self, Document};
    use std::sync::{Arc, atomic::AtomicBool};

    // The shared worker currently transfers the image snapshot through a Unix
    // file descriptor. Unsupported hosts must fail explicitly, without decoding
    // the image in the app process as a fallback.
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("image.png");
    image::RgbaImage::new(1, 1).save(&path).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let expected = "Isolated image previews are unavailable on this platform";
    assert_eq!(
        Document::open(&path, &cancel).err().as_deref(),
        Some(expected)
    );
    assert_eq!(images::worker_main().err().as_deref(), Some(expected));
    println!("Image integration: unsupported-platform errors passed");
}
