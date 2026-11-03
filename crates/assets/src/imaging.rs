//! Bounded image regions shared by the isolated decoder and the viewer.
mod png_rows;
mod portable;
mod svg;
pub use png_rows::render_png;
pub use portable::Portable;
use serde::{Deserialize, Serialize};
pub use svg::Svg;

pub const SCALE_UNIT: u32 = 4096;
pub const TILE_EDGE: u32 = 512;
pub const OVERVIEW_EDGE: u32 = 1600;
pub const MAX_BATCH_PIXELS: u64 = 4 * 1024 * 1024;
pub const MAX_REGIONS: usize = 16;
pub const MAX_SOURCE_PIXELS: u64 = 2_000_000_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Info {
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    /// Total plays; None means repeat indefinitely. Still images have one play.
    pub plays: Option<u32>,
    pub format: String,
    pub color: String,
}
impl Info {
    pub fn validate(&self) -> Result<(), String> {
        if self.width == 0
            || self.height == 0
            || self.width > 100_000
            || self.height > 100_000
            || u64::from(self.width) * u64::from(self.height) > MAX_SOURCE_PIXELS
            || self.frames == 0
            || self.frames > 10_000
            || self.format.len() > 128
            || self.color.len() > 256
        {
            return Err("Image dimensions or frame count exceed preview limits".into());
        }
        Ok(())
    }
    pub fn overview(&self, frame: u32) -> Region {
        self.overview_at(frame, OVERVIEW_EDGE)
    }
    pub fn overview_at(&self, frame: u32, edge: u32) -> Region {
        let scale = (edge.min(OVERVIEW_EDGE) as f64 / self.width.max(self.height) as f64).min(1.);
        let scale = (scale * SCALE_UNIT as f64).floor().max(1.) as u32;
        let (width, height) = self.scaled_size(scale);
        Region {
            frame,
            scale,
            x: 0,
            y: 0,
            width,
            height,
        }
    }
    pub fn scaled_size(&self, scale: u32) -> (u32, u32) {
        let side = |n| (u64::from(n) * u64::from(scale)).div_ceil(SCALE_UNIT as u64) as u32;
        (side(self.width), side(self.height))
    }
}

/// A rectangle in the scaled image, measured in physical output pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Region {
    pub frame: u32,
    pub scale: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}
impl Region {
    pub fn validate(self, info: &Info) -> Result<(), String> {
        info.validate()?;
        let (w, h) = info.scaled_size(self.scale);
        if self.frame >= info.frames
            || !(1..=64 * SCALE_UNIT).contains(&self.scale)
            || self.width == 0
            || self.height == 0
            || self.width > OVERVIEW_EDGE
            || self.height > OVERVIEW_EDGE
            || u64::from(self.x) + u64::from(self.width) > u64::from(w)
            || u64::from(self.y) + u64::from(self.height) > u64::from(h)
        {
            return Err("Invalid image region".into());
        }
        Ok(())
    }
    pub fn bytes(self) -> usize {
        self.width as usize * self.height as usize * 4
    }
    pub fn source_x(self, x: u32, width: u32) -> u32 {
        (((u64::from(self.x) + u64::from(x)) * SCALE_UNIT as u64) / self.scale as u64)
            .min(width.saturating_sub(1) as u64) as u32
    }
    pub fn source_y(self, y: u32, height: u32) -> u32 {
        (((u64::from(self.y) + u64::from(y)) * SCALE_UNIT as u64) / self.scale as u64)
            .min(height.saturating_sub(1) as u64) as u32
    }
}

pub fn validate_regions(info: &Info, regions: &[Region]) -> Result<(), String> {
    if regions.is_empty() || regions.len() > MAX_REGIONS {
        return Err("Invalid image region count".into());
    }
    let mut pixels = 0u64;
    for &region in regions {
        region.validate(info)?;
        if region.frame != regions[0].frame {
            return Err("Mixed image frames in one request".into());
        }
        pixels += u64::from(region.width) * u64::from(region.height);
    }
    if pixels > MAX_BATCH_PIXELS {
        return Err("Image request exceeds the decoded-pixel budget".into());
    }
    Ok(())
}

pub struct Pixels {
    pub region: Region,
    pub rgba: Vec<u8>,
}
pub struct Frame {
    pub pixels: Vec<Pixels>,
    pub delay_ms: u32,
}

/// Match browser behavior for absent/very short delays; never busy-loop a zero-delay GIF.
pub fn frame_delay(seconds: f64) -> u32 {
    if !seconds.is_finite() || seconds < 0.011 {
        100
    } else {
        (seconds * 1000.).round().clamp(11., 60_000.) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oversized_metadata_regions_and_batches_are_rejected_before_allocating() {
        let info = Info {
            width: 40_000,
            height: 40_000,
            frames: 1,
            plays: Some(1),
            format: "PNG".into(),
            color: "RGB".into(),
        };
        let overview = info.overview(0);
        validate_regions(&info, &[overview]).unwrap();
        assert!(overview.bytes() <= 1600 * 1600 * 4);
        for region in [
            Region {
                scale: 0,
                ..overview
            },
            Region {
                x: u32::MAX,
                ..overview
            },
            Region {
                frame: 1,
                ..overview
            },
            Region {
                width: 5000,
                ..overview
            },
        ] {
            assert!(validate_regions(&info, &[region]).is_err());
        }
        assert!(validate_regions(&info, &[overview, overview]).is_err());
        assert!(
            Info {
                width: 100_001,
                ..info
            }
            .validate()
            .is_err()
        );
    }
    #[test]
    fn animation_delays_never_spin_or_schedule_invalid_timers() {
        for seconds in [0., -1., f64::NAN, 0.01] {
            assert_eq!(frame_delay(seconds), 100);
        }
        assert_eq!(frame_delay(0.04), 40);
        assert_eq!(frame_delay(1000.), 60_000);
    }
}
