use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

pub const TILE_SIZE: u32 = 512;
pub const SCALE_UNIT: u32 = 4096;
pub(super) const MAX_MESSAGE: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct PageInfo {
    pub pages: u32,
    pub page: u32,
    pub width: f64,
    pub height: f64,
}

impl PageInfo {
    pub fn validate(self) -> Result<Self, String> {
        if self.pages == 0
            || self.page == 0
            || self.page > self.pages
            || !self.width.is_finite()
            || !self.height.is_finite()
            || self.width <= 0.
            || self.height <= 0.
            || self.width > 1_000_000.
            || self.height > 1_000_000.
        {
            return Err("Invalid or unsupported PDF page dimensions".into());
        }
        Ok(self)
    }
}

/// Coordinates are physical pixels, quantized to 1/4096 pixel per PDF point.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Tile {
    pub page: u32,
    pub scale: u32,
    pub x: u32,
    pub y: u32,
}

impl Tile {
    pub fn dimensions(self, info: PageInfo) -> Result<(u32, u32), String> {
        if self.page != info.page || !(1..=64 * SCALE_UNIT).contains(&self.scale) {
            return Err("Invalid PDF tile scale or page".into());
        }
        let scale = self.scale as f64 / SCALE_UNIT as f64;
        let width = (info.width * scale).ceil() as u32;
        let height = (info.height * scale).ceil() as u32;
        if self.x >= width
            || self.y >= height
            || !self.x.is_multiple_of(TILE_SIZE)
            || !self.y.is_multiple_of(TILE_SIZE)
        {
            return Err("PDF tile lies outside the page".into());
        }
        Ok((
            (width - self.x).min(TILE_SIZE),
            (height - self.y).min(TILE_SIZE),
        ))
    }
}

pub struct TilePixels {
    pub tile: Tile,
    pub width: u32,
    pub height: u32,
    /// Premultiplied BGRA, directly suitable for GPUI's image atlas.
    pub bgra: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct Request {
    pub operation: Operation,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
pub(super) enum Operation {
    Info { page: u32 },
    Render(Tile),
}
#[derive(Serialize, Deserialize)]
pub(super) enum Response {
    Info(PageInfo),
    Pixels { width: u32, height: u32 },
    Error(String),
}

pub(super) fn write_message(output: &mut impl Write, value: &impl Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_MESSAGE {
        return Err(io::Error::other("PDF message exceeds limit"));
    }
    output.write_all(&(bytes.len() as u32).to_le_bytes())?;
    output.write_all(&bytes)?;
    output.flush()
}

pub(super) fn read_message<T: serde::de::DeserializeOwned>(input: &mut impl Read) -> io::Result<T> {
    let mut size = [0; 4];
    input.read_exact(&mut size)?;
    let size = u32::from_le_bytes(size) as usize;
    if size > MAX_MESSAGE {
        return Err(io::Error::other("PDF message exceeds limit"));
    }
    let mut bytes = vec![0; size];
    input.read_exact(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unbounded_frames_and_tiles() {
        assert!(read_message::<Response>(&mut &u32::MAX.to_le_bytes()[..]).is_err());
        let info = PageInfo {
            pages: 1,
            page: 1,
            width: 600.,
            height: 800.,
        }
        .validate()
        .unwrap();
        let mut tile = Tile {
            page: 1,
            scale: SCALE_UNIT,
            x: 512,
            y: 512,
        };
        assert_eq!(tile.dimensions(info).unwrap(), (88, 288));
        tile.scale = u32::MAX;
        assert!(tile.dimensions(info).is_err());
        assert!(
            PageInfo {
                width: f64::NAN,
                ..info
            }
            .validate()
            .is_err()
        );
    }
}
