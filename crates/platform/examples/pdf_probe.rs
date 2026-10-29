//! Headless smoke/profiling entry point for the same broker used by the app.
//! Usage: cargo run --release -p plastext-platform --example pdf_probe -- file.pdf
//! Optional second argument writes a bounded first-page thumbnail as a local PPM.
use plastext_platform::pdf::{Document, SCALE_UNIT, Tile};
use std::{
    io::Write,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
    time::Instant,
};

fn main() -> Result<(), String> {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--pdf-worker")) {
        return plastext_platform::pdf::worker_main();
    }
    let mut args = std::env::args_os().skip(1);
    let path = PathBuf::from(args.next().ok_or("Expected a PDF path")?);
    let output = args.next().map(PathBuf::from);
    let cancel = Arc::new(AtomicBool::new(false));
    let start = Instant::now();
    let (document, first) = Document::open(&path, &cancel)?;
    let open_ms = start.elapsed().as_secs_f64() * 1000.;
    let mut times = Vec::new();
    for page in [1, first.pages.div_ceil(2), first.pages] {
        document.page(page, &cancel)?;
        let start = Instant::now();
        let _tile = document.tile(
            Tile {
                page,
                scale: SCALE_UNIT * 2,
                x: 0,
                y: 0,
            },
            &cancel,
        )?;
        times.push(start.elapsed().as_secs_f64() * 1000.);
    }
    if let Some(output) = &output {
        let scale = (1024. / first.width.max(first.height) * SCALE_UNIT as f64)
            .floor()
            .max(1.) as u32;
        let width = (first.width * scale as f64 / SCALE_UNIT as f64).ceil() as usize;
        let height = (first.height * scale as f64 / SCALE_UNIT as f64).ceil() as usize;
        if width > 1024 || height > 1024 {
            return Err("Thumbnail dimensions exceed limit".into());
        }
        let mut rgb = vec![255; width * height * 3];
        for y in (0..height).step_by(512) {
            for x in (0..width).step_by(512) {
                let tile = document.tile(
                    Tile {
                        page: 1,
                        scale,
                        x: x as u32,
                        y: y as u32,
                    },
                    &cancel,
                )?;
                for (index, &[b, g, r, _]) in tile.bgra.as_chunks::<4>().0.iter().enumerate() {
                    let offset = ((y + index / tile.width as usize) * width
                        + x
                        + index % tile.width as usize)
                        * 3;
                    rgb[offset..offset + 3].copy_from_slice(&[r, g, b]);
                }
            }
        }
        let mut file = std::fs::File::create(output).map_err(|e| e.to_string())?;
        write!(file, "P6\n{width} {height}\n255\n").map_err(|e| e.to_string())?;
        file.write_all(&rgb).map_err(|e| e.to_string())?;
    }
    println!(
        "{}",
        serde_json::json!({"pages":first.pages, "open_ms":open_ms, "tile_ms":times,
        "tile_size":512, "scale":2, "profile":"smoke, not performance certification"})
    );
    Ok(())
}
