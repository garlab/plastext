use super::*;
use std::io::Write;

fn put16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + 2].copy_from_slice(&value.to_be_bytes());
}
fn put32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_be_bytes());
}

/// An original triangle glyph at U+E001. Generated in-memory, with no dependency
/// on installed fonts or third-party artwork in the test suite.
fn tables() -> Vec<([u8; 4], Vec<u8>)> {
    let mut head = vec![0; 54];
    put32(&mut head, 0, 0x00010000);
    put32(&mut head, 12, 0x5f0f3cf5);
    put16(&mut head, 18, 1000);
    let mut hhea = vec![0; 36];
    put32(&mut hhea, 0, 0x00010000);
    put16(&mut hhea, 4, 800);
    put16(&mut hhea, 6, (-200i16) as u16);
    put16(&mut hhea, 34, 2);
    let mut maxp = vec![0; 32];
    put32(&mut maxp, 0, 0x00010000);
    put16(&mut maxp, 4, 2);
    let mut cmap = vec![0; 44];
    put16(&mut cmap, 2, 1);
    put16(&mut cmap, 4, 3);
    put16(&mut cmap, 6, 1);
    put32(&mut cmap, 8, 12);
    for (at, value) in [
        (12, 4),
        (14, 32),
        (18, 4),
        (20, 4),
        (22, 1),
        (26, 0xe001),
        (28, 0xffff),
        (32, 0xe001),
        (34, 0xffff),
        (36, 0x2000),
        (38, 1),
    ] {
        put16(&mut cmap, at, value);
    }
    let mut glyf = vec![0; 30];
    for (at, value) in [
        (0, 1),
        (6, 800),
        (8, 700),
        (10, 2),
        (19, 800),
        (21, (-800i16) as u16),
        (27, 700),
    ] {
        put16(&mut glyf, at, value);
    }
    glyf[14..17].copy_from_slice(&[1, 1, 1]);
    let mut hmtx = vec![0; 8];
    put16(&mut hmtx, 0, 1000);
    put16(&mut hmtx, 4, 1000);
    vec![
        (*b"cmap", cmap),
        (*b"glyf", glyf),
        (*b"head", head),
        (*b"hhea", hhea),
        (*b"hmtx", hmtx),
        (*b"loca", vec![0, 0, 0, 0, 0, 15]),
        (*b"maxp", maxp),
    ]
}

fn woff(compress: bool) -> Vec<u8> {
    let tables = tables();
    let mut bytes = vec![0; 44 + tables.len() * 20];
    bytes[..4].copy_from_slice(b"wOFF");
    put32(&mut bytes, 4, 0x00010000);
    put16(&mut bytes, 12, tables.len() as u16);
    put32(
        &mut bytes,
        16,
        (12 + tables.len() * 16
            + tables
                .iter()
                .map(|(_, data)| padded(data.len()))
                .sum::<usize>()) as u32,
    );
    for (i, (tag, data)) in tables.iter().enumerate() {
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(data).unwrap();
        let compressed = encoder.finish().unwrap();
        let source = if compress && compressed.len() < data.len() {
            &compressed
        } else {
            data
        };
        let at = 44 + i * 20;
        let offset = bytes.len();
        bytes[at..at + 4].copy_from_slice(tag);
        put32(&mut bytes, at + 4, offset as u32);
        put32(&mut bytes, at + 8, source.len() as u32);
        put32(&mut bytes, at + 12, data.len() as u32);
        bytes.extend_from_slice(source);
        bytes.resize(padded(bytes.len()), 0);
    }
    let len = bytes.len();
    put32(&mut bytes, 8, len as u32);
    bytes
}

#[test]
fn woff_and_ttf_render_identical_coloured_bounded_glyphs() {
    let raw = sfnt(&woff(false)).unwrap();
    let ttf = Font::decode(raw).unwrap();
    let compressed = Font::decode(woff(true)).unwrap();
    let render = |font: &Font, size, edge| font.render('\u{e001}', 0x519abaff, size, edge).unwrap();
    let image = render(&ttf, 10000, 32);
    assert_eq!(image.rgba, render(&compressed, 10000, 32).rgba);
    assert!(
        image
            .rgba
            .as_chunks::<4>()
            .0
            .contains(&[0x51, 0x9a, 0xba, 255])
    );
    let coverage = |image: &Preview| {
        image
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| u64::from(p[3]))
            .sum::<u64>()
    };
    assert!(coverage(&render(&ttf, 5000, 32)) < coverage(&image));
    assert!(coverage(&render(&ttf, 15000, 32)) > coverage(&image));
    let bounded = render(&ttf, 40000, 10000);
    assert_eq!(
        (bounded.width, bounded.height, bounded.rgba.len()),
        (64, 64, 64 * 64 * 4)
    );
    assert!(ttf.render('x', 0xffffffff, 10000, 32).is_err());
    let translucent = ttf.render('\u{e001}', 0xff000080, 10000, 32).unwrap();
    assert!(
        translucent
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| p[3] <= 128)
    );
}

#[test]
fn malformed_and_oversized_fonts_are_rejected() {
    let valid = woff(true);
    for end in 0..valid.len() {
        assert!(Font::decode(valid[..end].to_vec()).is_err());
    }
    for (at, value) in [
        (16, (MAX_FONT_BYTES + 4) as u32),
        (48, 0),
        (52, u32::MAX),
        (56, 1),
    ] {
        let mut data = valid.clone();
        put32(&mut data, at, value);
        assert!(Font::decode(data).is_err(), "offset {at}");
    }
    assert!(Font::decode(vec![0; MAX_SOURCE_BYTES + 1]).is_err());
    assert!(
        Font::decode(b"wOF2not-a-supported-font".to_vec())
            .err()
            .unwrap()
            .contains("WOFF2")
    );
}

#[cfg(unix)]
#[test]
fn font_sources_obey_the_import_root() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let source = outside.path().join("font.woff");
    std::fs::write(&source, woff(true)).unwrap();
    let link = root.path().join("font.woff");
    std::os::unix::fs::symlink(source, &link).unwrap();
    assert!(
        Font::read(&link, root.path())
            .err()
            .unwrap()
            .contains("leaves")
    );
}
