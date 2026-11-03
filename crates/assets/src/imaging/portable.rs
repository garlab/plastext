use super::{Frame, Info, Pixels, Region, validate_regions};
use image::ImageReader;
use std::{
    fs::File,
    io::{BufReader, Seek},
    path::{Path, PathBuf},
};
fn reader(path: &Path) -> Result<ImageReader<BufReader<File>>, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    file.rewind().map_err(|e| e.to_string())?;
    ImageReader::new(BufReader::new(file))
        .with_guessed_format()
        .map_err(|e| e.to_string())
}
pub struct Portable {
    path: PathBuf,
    info: Info,
}
impl Portable {
    pub fn open(path: &Path) -> Result<Self, String> {
        let reader = reader(path)?;
        let format = format!("{:?}", reader.format().ok_or("Unsupported image format")?)
            .to_ascii_uppercase();
        let (width, height) = reader.into_dimensions().map_err(|e| e.to_string())?;
        let info = Info {
            width,
            height,
            frames: 1,
            plays: Some(1),
            format,
            color: "RGB".into(),
        };
        info.validate()?;
        Ok(Self {
            path: path.into(),
            info,
        })
    }
    pub fn info(&self) -> Info {
        self.info.clone()
    }
    pub fn render(&self, regions: &[Region]) -> Result<Frame, String> {
        validate_regions(&self.info, regions)?;
        if u64::from(self.info.width) * u64::from(self.info.height) > crate::PIXEL_LIMIT {
            return Err("This codec exceeds the portable decoder's 16-megapixel limit".into());
        }
        let mut reader = reader(&self.path)?;
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(128 * 1024 * 1024);
        reader.limits(limits);
        let image = reader.decode().map_err(|e| e.to_string())?.into_rgba8();
        let pixels = regions
            .iter()
            .map(|&region| {
                let mut rgba = vec![0; region.bytes()];
                for y in 0..region.height {
                    for x in 0..region.width {
                        let p = image.get_pixel(
                            region.source_x(x, self.info.width),
                            region.source_y(y, self.info.height),
                        );
                        let i = (y * region.width + x) as usize * 4;
                        rgba[i..i + 4].copy_from_slice(&p.0);
                    }
                }
                Pixels { region, rgba }
            })
            .collect();
        Ok(Frame {
            pixels,
            delay_ms: 100,
        })
    }
}
