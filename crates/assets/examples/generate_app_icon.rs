//! Regenerate the checked-in app PNG/ICNS from the SVG on macOS.
//! Run: cargo run -p plastext-assets --example generate_app_icon
use resvg::{tiny_skia, usvg};
use std::{error::Error, fs, path::Path, process::Command};

fn main() -> Result<(), Box<dyn Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let assets = root.join("crates/app/assets");
    let output = root.join("target/app-icon/plastext.iconset");
    fs::create_dir_all(&output)?;
    let tree = usvg::Tree::from_data(
        &fs::read(assets.join("plastext.svg"))?,
        &usvg::Options::default(),
    )?;
    for size in [16, 32, 128, 256, 512] {
        for scale in [1, 2] {
            let pixels = size * scale;
            let mut pixmap =
                tiny_skia::Pixmap::new(pixels, pixels).ok_or("Invalid icon dimensions")?;
            resvg::render(
                &tree,
                tiny_skia::Transform::from_scale(
                    pixels as f32 / tree.size().width(),
                    pixels as f32 / tree.size().height(),
                ),
                &mut pixmap.as_mut(),
            );
            let suffix = if scale == 2 { "@2x" } else { "" };
            let path = output.join(format!("icon_{size}x{size}{suffix}.png"));
            pixmap.save_png(&path)?;
        }
    }
    let status = Command::new("iconutil")
        .args(["-c", "icns"])
        .arg(&output)
        .arg("-o")
        .arg(output.with_extension("icns"))
        .status()?;
    if !status.success() {
        return Err("iconutil failed to create the macOS icon".into());
    }
    fs::copy(
        output.join("icon_512x512@2x.png"),
        assets.join("plastext.png"),
    )?;
    fs::copy(output.with_extension("icns"), assets.join("plastext.icns"))?;
    println!("Generated crates/app/assets/plastext.png and plastext.icns from plastext.svg");
    Ok(())
}
