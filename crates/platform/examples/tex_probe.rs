//! Offline, read-only input smoke test. Generated outputs stay in private tempdirs.
use plastext_platform::{
    pdf,
    tex::{self, Config, Engine},
};
use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
    time::Instant,
};
fn main() -> Result<(), String> {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--pdf-worker")) {
        return pdf::worker_main();
    }
    let mut paths = Vec::new();
    let mut inputs = Vec::new();
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--input-folder" {
            inputs.push(PathBuf::from(
                args.next().ok_or("Expected an input folder")?,
            ));
        } else {
            paths.push(PathBuf::from(arg));
        }
    }
    if paths.is_empty() {
        return Err("Expected a .tex root document".into());
    }
    let engine = Engine::discover()?;
    for path in paths {
        let config = Config::with_inputs(&path, engine.clone(), &inputs)?;
        let start = Instant::now();
        let result = tex::compile(&config, &Arc::new(AtomicBool::new(false)))?;
        println!(
            "{}",
            serde_json::json!({"file":path.file_name().unwrap_or_default().to_string_lossy(),"pages":result.info.pages,"elapsed_ms":start.elapsed().as_millis(),"profile":"smoke, not performance certification"})
        );
    }
    Ok(())
}
