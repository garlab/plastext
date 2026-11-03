use super::{Info, Pixels, Region, validate_regions};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufReader, Seek},
};

/// Scan filter rows, retaining only requested output pixels. Neither the complete
/// source image nor a full-width strip of RGBA rows is ever allocated.
pub fn render_png(mut file: File, info: &Info, regions: &[Region]) -> Result<Vec<Pixels>, String> {
    validate_regions(info, regions)?;
    if regions[0].frame != 0 || info.frames != 1 {
        return Err("Animated PNG requires the native decoder".into());
    }
    file.rewind().map_err(|e| e.to_string())?;
    let mut decoder = png::Decoder::new_with_limits(
        BufReader::new(file),
        png::Limits {
            bytes: 8 * 1024 * 1024,
        },
    );
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    if (reader.info().width, reader.info().height) != (info.width, info.height) {
        return Err("PNG dimensions changed".into());
    }
    let interlaced = reader.info().interlaced;
    let (color, depth) = reader.output_color_type();
    if depth != png::BitDepth::Eight {
        return Err("Unsupported PNG output depth".into());
    }
    let channels = color.samples();
    let mut outputs: Vec<_> = regions
        .iter()
        .map(|&region| Pixels {
            region,
            rgba: vec![0; region.bytes()],
        })
        .collect();
    let mut rows: BTreeMap<u32, Vec<(usize, u32)>> = BTreeMap::new();
    for (i, region) in regions.iter().enumerate() {
        for y in 0..region.height {
            rows.entry(region.source_y(y, info.height))
                .or_default()
                .push((i, y));
        }
    }
    let last_y = *rows.keys().next_back().unwrap();
    // Adam7 visits these seven sparse rectangles in order. Computing their
    // coordinates avoids expanding an interlaced image into a full-size buffer.
    let passes: &[(u32, u32, u32, u32)] = if interlaced {
        &[
            (0, 0, 8, 8),
            (4, 0, 8, 8),
            (0, 4, 4, 8),
            (2, 0, 4, 4),
            (0, 2, 2, 4),
            (1, 0, 2, 2),
            (0, 1, 1, 2),
        ]
    } else {
        &[(0, 0, 1, 1)]
    };
    for &(start_x, start_y, step_x, step_y) in passes {
        if start_x >= info.width || start_y >= info.height {
            continue;
        }
        let columns = (info.width - start_x).div_ceil(step_x) as usize;
        for source_y in (start_y..info.height).step_by(step_y as usize) {
            if !interlaced && source_y > last_y {
                return Ok(outputs);
            }
            let row = reader
                .next_row()
                .map_err(|e| e.to_string())?
                .ok_or("Incomplete PNG rows")?;
            let data = row.data();
            if data.len() != columns * channels {
                return Err("Invalid PNG row size".into());
            }
            let Some(wanted) = rows.get(&source_y) else {
                continue;
            };
            for &(index, y) in wanted {
                let region = regions[index];
                for x in 0..region.width {
                    let source_x = region.source_x(x, info.width);
                    if source_x < start_x || !(source_x - start_x).is_multiple_of(step_x) {
                        continue;
                    }
                    let offset = ((source_x - start_x) / step_x) as usize * channels;
                    let pixel = &data[offset..offset + channels];
                    let rgba = match color {
                        png::ColorType::Grayscale => [pixel[0], pixel[0], pixel[0], 255],
                        png::ColorType::GrayscaleAlpha => [pixel[0], pixel[0], pixel[0], pixel[1]],
                        png::ColorType::Rgb => [pixel[0], pixel[1], pixel[2], 255],
                        png::ColorType::Rgba => [pixel[0], pixel[1], pixel[2], pixel[3]],
                        png::ColorType::Indexed => {
                            return Err("PNG palette was not expanded".into());
                        }
                    };
                    let out = (y as usize * region.width as usize + x as usize) * 4;
                    outputs[index].rgba[out..out + 4].copy_from_slice(&rgba);
                }
            }
        }
    }
    Ok(outputs)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adam7_palette_rows_match_an_independent_full_decode() {
        let bytes = include_bytes!("../../tests/fixtures/interlaced.png");
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), bytes).unwrap();
        let expected = image::load_from_memory(bytes).unwrap().into_rgba8();
        let info = Info {
            width: expected.width(),
            height: expected.height(),
            frames: 1,
            plays: Some(1),
            format: "PNG".into(),
            color: "RGB".into(),
        };
        for scale in [super::super::SCALE_UNIT, super::super::SCALE_UNIT * 3] {
            let (w, h) = info.scaled_size(scale);
            let region = Region {
                frame: 0,
                scale,
                x: 0,
                y: 0,
                width: w,
                height: h,
            };
            let result = render_png(File::open(file.path()).unwrap(), &info, &[region]).unwrap();
            for y in 0..h {
                for x in 0..w {
                    let i = (y * w + x) as usize * 4;
                    assert_eq!(
                        &result[0].rgba[i..i + 4],
                        &expected
                            .get_pixel(
                                region.source_x(x, info.width),
                                region.source_y(y, info.height)
                            )
                            .0
                    );
                }
            }
        }
    }
    #[test]
    fn scanline_regions_match_full_decode_at_edges_and_with_transparency() {
        let path = tempfile::NamedTempFile::new().unwrap();
        let mut image = image::RgbaImage::new(67, 53);
        for (x, y, p) in image.enumerate_pixels_mut() {
            *p = image::Rgba([x as u8, y as u8, 99, (x + y) as u8]);
        }
        image
            .save_with_format(path.path(), image::ImageFormat::Png)
            .unwrap();
        let info = Info {
            width: 67,
            height: 53,
            frames: 1,
            plays: Some(1),
            format: "PNG".into(),
            color: "RGBA".into(),
        };
        for scale in [
            super::super::SCALE_UNIT / 2,
            super::super::SCALE_UNIT,
            super::super::SCALE_UNIT * 3,
        ] {
            let (w, h) = info.scaled_size(scale);
            let region = Region {
                frame: 0,
                scale,
                x: w - 13,
                y: h - 9,
                width: 13,
                height: 9,
            };
            let output = render_png(File::open(path.path()).unwrap(), &info, &[region]).unwrap();
            for y in 0..region.height {
                for x in 0..region.width {
                    let expected = image.get_pixel(
                        region.source_x(x, info.width),
                        region.source_y(y, info.height),
                    );
                    let i = (y * region.width + x) as usize * 4;
                    assert_eq!(&output[0].rgba[i..i + 4], &expected.0);
                }
            }
        }
    }
}
