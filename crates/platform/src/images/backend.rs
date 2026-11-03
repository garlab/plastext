use super::{Frame, Info, Region};
use plastext_assets::imaging::{Portable, Svg, render_png};
use std::{
    fs::File,
    io::{Read, Seek},
    path::{Path, PathBuf},
};
pub(super) enum Decoder {
    Svg(Box<Svg>),
    #[cfg(target_os = "macos")]
    Native {
        native: super::macos::Native,
        path: PathBuf,
        png: bool,
    },
    Portable {
        decoder: Portable,
        path: PathBuf,
        png: bool,
    },
}
impl Decoder {
    pub fn open(path: &Path) -> Result<Self, String> {
        let mut head = [0; 4096];
        let n = File::open(path)
            .and_then(|mut f| {
                f.rewind()?;
                f.read(&mut head)
            })
            .map_err(|e| e.to_string())?;
        let png = head[..n].starts_with(b"\x89PNG\r\n\x1a\n");
        let text = String::from_utf8_lossy(&head[..n]);
        let text = text.trim_start_matches('\u{feff}').trim_start();
        if text.starts_with("<svg") || text.starts_with("<?xml") || text.starts_with("<!--") {
            let mut bytes = Vec::new();
            File::open(path)
                .and_then(|mut f| {
                    f.rewind()?;
                    f.take(plastext_assets::ENCODED_LIMIT as u64 + 1)
                        .read_to_end(&mut bytes)
                })
                .map_err(|e| e.to_string())?;
            return Svg::open(&bytes).map(|svg| Self::Svg(Box::new(svg)));
        }
        #[cfg(target_os = "macos")]
        if let Ok(native) = super::macos::Native::open(path) {
            return Ok(Self::Native {
                native,
                path: path.into(),
                png,
            });
        }
        Portable::open(path).map(|decoder| Self::Portable {
            decoder,
            path: path.into(),
            png,
        })
    }
    pub fn info(&self) -> Info {
        match self {
            Self::Svg(svg) => svg.info(),
            #[cfg(target_os = "macos")]
            Self::Native { native, .. } => native.info(),
            Self::Portable { decoder, .. } => decoder.info(),
        }
    }
    pub fn render(&self, regions: &[Region]) -> Result<Frame, String> {
        let info = self.info();
        plastext_assets::imaging::validate_regions(&info, regions)?;
        match self {
            Self::Svg(svg) => Ok(Frame {
                pixels: svg.render(regions)?,
                delay_ms: 100,
            }),
            #[cfg(target_os = "macos")]
            Self::Native { native, path, png } => {
                let overview = regions.len() == 1
                    && regions[0].x == 0
                    && regions[0].y == 0
                    && (regions[0].width, regions[0].height) == info.scaled_size(regions[0].scale);
                if *png && info.frames == 1 && native.orientation == 1 && !overview {
                    return Ok(Frame {
                        pixels: native.colorize_png(render_png(
                            File::open(path).map_err(|e| e.to_string())?,
                            &info,
                            regions,
                        )?)?,
                        delay_ms: 100,
                    });
                }
                native.render(regions)
            }
            Self::Portable { decoder, path, png } => {
                if *png {
                    Ok(Frame {
                        pixels: render_png(
                            File::open(path).map_err(|e| e.to_string())?,
                            &info,
                            regions,
                        )?,
                        delay_ms: 100,
                    })
                } else {
                    decoder.render(regions)
                }
            }
        }
    }
}
