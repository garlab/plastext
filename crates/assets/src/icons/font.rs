//! Local outline fonts, including bounded WOFF 1 decompression. Fonts stay private
//! to an icon theme: no registration with the OS or application text system.
use crate::Preview;
use resvg::tiny_skia::{self, PathBuilder};
use std::{io::Read, path::Path};
use ttf_parser::{Face, OutlineBuilder};

const MAX_SOURCE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_FONT_BYTES: usize = 4 * 1024 * 1024;
const MAX_OUTLINE_COMMANDS: usize = 32768;

pub struct Font {
    data: Vec<u8>,
}

impl Font {
    pub fn read(path: &Path, root: &Path) -> Result<Self, String> {
        Self::decode(super::read_limited(path, root, MAX_SOURCE_BYTES)?)
    }

    pub fn decode(data: Vec<u8>) -> Result<Self, String> {
        if data.len() > MAX_SOURCE_BYTES {
            return Err("Icon font exceeds 2 MiB".into());
        }
        let data = match data.get(..4) {
            Some(b"wOFF") => sfnt(&data)?,
            Some(b"wOF2") => {
                return Err("WOFF2 icon fonts are not supported; use WOFF, TTF or OTF".into());
            }
            Some(b"ttcf") => {
                return Err("Font collections are not supported; use a single font".into());
            }
            _ => data,
        };
        Face::parse(&data, 0).map_err(|e| format!("Invalid icon font: {e:?}"))?;
        Ok(Self { data })
    }

    pub fn retained_bytes(&self) -> usize {
        self.data.capacity()
    }

    /// Scale using the theme's font size, centring the outline in a fixed icon
    /// slot. Oversized outlines are contained rather than spilling into filenames.
    pub fn render(
        &self,
        character: char,
        color: u32,
        size: u16,
        edge: u32,
    ) -> Result<Preview, String> {
        let edge = edge.clamp(16, 64);
        let face = Face::parse(&self.data, 0).map_err(|e| format!("Invalid icon font: {e:?}"))?;
        let glyph = face
            .glyph_index(character)
            .ok_or_else(|| format!("Icon font has no glyph for U+{:04X}", character as u32))?;
        let mut outline = Outline {
            path: PathBuilder::new(),
            commands: 0,
        };
        let bounds = face
            .outline_glyph(glyph, &mut outline)
            .ok_or("Icon glyph has no supported outline")?;
        if outline.commands > MAX_OUTLINE_COMMANDS {
            return Err("Icon glyph outline is too complex".into());
        }
        let path = outline.path.finish().ok_or("Empty icon glyph outline")?;
        let width = f32::from(bounds.x_max) - f32::from(bounds.x_min);
        let height = f32::from(bounds.y_max) - f32::from(bounds.y_min);
        if width <= 0. || height <= 0. || size == 0 || size > 40000 {
            return Err("Invalid icon glyph dimensions or font size".into());
        }
        let scale = (edge as f32 * f32::from(size) / 10000. / f32::from(face.units_per_em()))
            .min(edge as f32 / width.max(height));
        let x = (edge as f32 - width * scale) / 2. - f32::from(bounds.x_min) * scale;
        let y = (edge as f32 - height * scale) / 2. + f32::from(bounds.y_max) * scale;
        let mut pixels = tiny_skia::Pixmap::new(edge, edge).ok_or("Invalid icon size")?;
        let mut paint = tiny_skia::Paint::default();
        let [r, g, b, a] = color.to_be_bytes();
        paint.set_color_rgba8(r, g, b, a);
        pixels.fill_path(
            &path,
            &paint,
            tiny_skia::FillRule::Winding,
            tiny_skia::Transform::from_row(scale, 0., 0., -scale, x, y),
            None,
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
            width: edge,
            height: edge,
            original_width: edge,
            original_height: edge,
        })
    }
}

struct Outline {
    path: PathBuilder,
    commands: usize,
}
impl Outline {
    fn push(&mut self, f: impl FnOnce(&mut PathBuilder)) {
        self.commands += 1;
        if self.commands <= MAX_OUTLINE_COMMANDS {
            f(&mut self.path);
        }
    }
}
impl OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        self.push(|p| p.move_to(x, y));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.push(|p| p.line_to(x, y));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.push(|p| p.quad_to(x1, y1, x, y));
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.push(|p| p.cubic_to(x1, y1, x2, y2, x, y));
    }
    fn close(&mut self) {
        self.push(|p| p.close());
    }
}

fn u16_at(bytes: &[u8], at: usize) -> Result<u16, String> {
    Ok(u16::from_be_bytes(
        bytes
            .get(at..at + 2)
            .ok_or("Truncated WOFF")?
            .try_into()
            .unwrap(),
    ))
}
fn u32_at(bytes: &[u8], at: usize) -> Result<u32, String> {
    Ok(u32::from_be_bytes(
        bytes
            .get(at..at + 4)
            .ok_or("Truncated WOFF")?
            .try_into()
            .unwrap(),
    ))
}
fn padded(size: usize) -> usize {
    (size + 3) & !3
}

/// Reconstruct the sfnt directory and tables from WOFF 1. Offsets, expanded
/// lengths and zlib output are checked before allocating table buffers.
/// Format: https://www.w3.org/TR/WOFF/
fn sfnt(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let count = usize::from(u16_at(bytes, 12)?);
    let total = u32_at(bytes, 16)? as usize;
    if bytes.len() < 44
        || u32_at(bytes, 8)? as usize != bytes.len()
        || u16_at(bytes, 14)? != 0
        || !(1..=256).contains(&count)
        || total > MAX_FONT_BYTES
        || total < 12 + count * 16
        || !total.is_multiple_of(4)
    {
        return Err("Invalid or oversized WOFF header (decoded fonts are limited to 4 MiB)".into());
    }
    let mut output = Vec::with_capacity(total);
    output.resize(12 + count * 16, 0);
    output[..4].copy_from_slice(&bytes[4..8]);
    output[4..6].copy_from_slice(&(count as u16).to_be_bytes());
    let power = 1usize << count.ilog2();
    output[6..8].copy_from_slice(&((power * 16) as u16).to_be_bytes());
    output[8..10].copy_from_slice(&(count.ilog2() as u16).to_be_bytes());
    output[10..12].copy_from_slice(&((count * 16 - power * 16) as u16).to_be_bytes());
    let mut ranges = vec![(0, 44 + count * 20)];
    let mut previous_tag = None;
    for i in 0..count {
        let at = 44 + i * 20;
        let tag = u32_at(bytes, at)?;
        let offset = u32_at(bytes, at + 4)? as usize;
        let compressed = u32_at(bytes, at + 8)? as usize;
        let original = u32_at(bytes, at + 12)? as usize;
        if previous_tag.is_some_and(|previous| previous >= tag)
            || !offset.is_multiple_of(4)
            || original == 0
            || compressed == 0
            || compressed > original
            || original > total
            || output.len() + padded(original) > total
        {
            return Err("Invalid WOFF table directory".into());
        }
        previous_tag = Some(tag);
        let source = bytes
            .get(
                offset
                    ..offset
                        .checked_add(compressed)
                        .ok_or("Invalid WOFF offset")?,
            )
            .ok_or("Truncated WOFF table")?;
        ranges.push((offset, padded(offset + compressed)));
        let dest = output.len();
        let record = 12 + i * 16;
        output[record..record + 4].copy_from_slice(&tag.to_be_bytes());
        output[record + 4..record + 8].copy_from_slice(&u32_at(bytes, at + 16)?.to_be_bytes());
        output[record + 8..record + 12].copy_from_slice(&(dest as u32).to_be_bytes());
        output[record + 12..record + 16].copy_from_slice(&(original as u32).to_be_bytes());
        if compressed == original {
            output.extend_from_slice(source);
        } else {
            let mut decoder = flate2::read::ZlibDecoder::new(source);
            (&mut decoder)
                .take(original as u64 + 1)
                .read_to_end(&mut output)
                .map_err(|e| format!("Invalid WOFF compression: {e}"))?;
            if output.len() != dest + original || decoder.total_in() != compressed as u64 {
                return Err("WOFF expanded length does not match its directory".into());
            }
        }
        output.resize(dest + padded(original), 0);
    }
    if output.len() != total {
        return Err("WOFF decoded size does not match its header".into());
    }
    // Metadata/private payloads are never decoded, but must not overlap tables.
    for (offset_at, length_at) in [(24, 28), (36, 40)] {
        let offset = u32_at(bytes, offset_at)? as usize;
        let length = u32_at(bytes, length_at)? as usize;
        if offset != 0 || length != 0 {
            if offset == 0
                || length == 0
                || !offset.is_multiple_of(4)
                || offset + length > bytes.len()
            {
                return Err("Invalid WOFF metadata range".into());
            }
            ranges.push((offset, padded(offset + length)));
        }
    }
    ranges.sort_unstable();
    if ranges.windows(2).any(|pair| pair[0].1 != pair[1].0)
        || ranges
            .last()
            .is_none_or(|last| last.1 < bytes.len() || last.1 > padded(bytes.len()))
    {
        return Err("Overlapping or non-contiguous WOFF data".into());
    }
    Ok(output)
}

#[cfg(test)]
#[path = "font_tests.rs"]
mod tests;
