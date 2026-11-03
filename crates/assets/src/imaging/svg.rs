use super::{Info, Pixels, Region, validate_regions};
use resvg::{tiny_skia, usvg};

pub struct Svg {
    tree: usvg::Tree,
    info: Info,
}
impl Svg {
    pub fn open(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > crate::ENCODED_LIMIT {
            return Err("SVG exceeds the 32 MiB source limit".into());
        }
        let mut options = usvg::Options {
            image_href_resolver: usvg::ImageHrefResolver {
                resolve_string: Box::new(|_, _| None),
                resolve_data: Box::new(|_, _, _| None),
            },
            ..Default::default()
        };
        if bytes.windows(5).any(|s| s == b"<text") {
            options.fontdb_mut().load_system_fonts();
        }
        let tree = usvg::Tree::from_data(bytes, &options).map_err(|e| e.to_string())?;
        let info = Info {
            width: tree.size().width().ceil() as u32,
            height: tree.size().height().ceil() as u32,
            frames: 1,
            plays: Some(1),
            format: "SVG".into(),
            color: "sRGB".into(),
        };
        info.validate()?;
        Ok(Self { tree, info })
    }
    pub fn info(&self) -> Info {
        self.info.clone()
    }
    pub fn render(&self, regions: &[Region]) -> Result<Vec<Pixels>, String> {
        validate_regions(&self.info, regions)?;
        regions
            .iter()
            .map(|&region| {
                let mut pixels = tiny_skia::Pixmap::new(region.width, region.height)
                    .ok_or("Invalid SVG region")?;
                let scale = region.scale as f32 / super::SCALE_UNIT as f32;
                resvg::render(
                    &self.tree,
                    tiny_skia::Transform::from_row(
                        scale,
                        0.,
                        0.,
                        scale,
                        -(region.x as f32),
                        -(region.y as f32),
                    ),
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
                Ok(Pixels { region, rgba })
            })
            .collect()
    }
}
