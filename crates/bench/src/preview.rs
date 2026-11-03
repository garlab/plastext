use crate::{memory::Sampler, report::LocalRun};
use plastext_platform::{
    pdf::{Document, SCALE_UNIT, Tile},
    tex,
};
use serde_json::{Value, json};
use std::{
    io,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

fn timing(samples: &mut [f64]) -> Value {
    samples.sort_by(f64::total_cmp);
    json!({"p50":samples.get(samples.len()/2), "p99":samples.get((samples.len()*99).div_ceil(100).saturating_sub(1)), "max":samples.last(), "samples":samples})
}

pub fn pdf(file: &Path, rounds: u32, output: Option<&Path>) -> io::Result<()> {
    let run = LocalRun::new("pdf")?;
    let memory = Sampler::start()?;
    let started = Instant::now();
    let cancel = Arc::new(AtomicBool::new(false));
    let mut page_times = Vec::new();
    let mut tile_times = Vec::new();
    let mut open_ms = None;
    let mut pages = 0;
    let mut max_tile = 0;
    let result = (|| -> Result<(), String> {
        let (document, info) = Document::open(file, &cancel)?;
        open_ms = Some(started.elapsed().as_secs_f64() * 1000.);
        pages = info.pages;
        for _ in 0..rounds {
            for page in [1, pages.div_ceil(2), pages, pages.div_ceil(2), 1] {
                let start = Instant::now();
                let info = document.page(page, &cancel)?;
                page_times.push(start.elapsed().as_secs_f64() * 1000.);
                for scale in [SCALE_UNIT / 8, SCALE_UNIT, SCALE_UNIT * 2, SCALE_UNIT * 8] {
                    let w = (info.width * scale as f64 / SCALE_UNIT as f64).ceil() as u32;
                    let h = (info.height * scale as f64 / SCALE_UNIT as f64).ceil() as u32;
                    for (x, y) in [
                        (0, 0),
                        (
                            w.saturating_sub(1) / 512 * 512,
                            h.saturating_sub(1) / 512 * 512,
                        ),
                    ] {
                        let start = Instant::now();
                        let pixels = document.tile(Tile { page, scale, x, y }, &cancel)?;
                        tile_times.push(start.elapsed().as_secs_f64() * 1000.);
                        max_tile = max_tile.max(pixels.bgra.len());
                        if pixels.bgra.len() != pixels.width as usize * pixels.height as usize * 4
                            || pixels.bgra.len() > 512 * 512 * 4
                        {
                            return Err("PDF tile exceeded its raster bound".into());
                        }
                    }
                }
            }
        }
        Ok(())
    })();
    let memory = memory.finish()?;
    let report = json!({"file":file,"bytes":std::fs::metadata(file)?.len(),"pages":pages,"rounds":rounds,
        "open_ms":open_ms,"page_ms":timing(&mut page_times),"tile_ms":timing(&mut tile_times),"max_tile_bytes":max_tile,
        "memory":memory,"complete":result.is_ok(),"passed":result.is_ok(),
        "warnings":result.as_ref().err().into_iter().collect::<Vec<_>>(),
        "scope":"isolated PDF worker: snapshot, first/middle/last pages at 0.125x/1x/2x/8x; no UI or GPU cache; sampled helper RSS can miss short peaks; latency is informational"});
    println!(
        "{}",
        serde_json::to_string_pretty(&run.save(&report, output)?)?
    );
    result.map_err(io::Error::other)
}

pub fn tex(file: &Path, runs: u32, output: Option<&Path>) -> io::Result<()> {
    let run = LocalRun::new("tex")?;
    let memory = Sampler::start()?;
    let mut build = Vec::new();
    let mut pages = 0;
    let mut cancellation_ms = None;
    let mut live_updates = Value::Null;
    let result = (|| -> Result<(), String> {
        let engine = tex::Engine::discover()?;
        let config = tex::Config::automatic(file, None, engine.clone())?;
        for _ in 0..runs {
            let start = Instant::now();
            let compiled = tex::compile(&config, &Arc::new(AtomicBool::new(false)))?;
            pages = compiled.info.pages;
            if pages == 0 {
                return Err("Compiled PDF has no pages".into());
            }
            build.push(start.elapsed().as_secs_f64() * 1000.);
            compiled.document.tile(
                Tile {
                    page: pages,
                    scale: SCALE_UNIT * 2,
                    x: 0,
                    y: 0,
                },
                &Arc::new(AtomicBool::new(false)),
            )?;
        }
        // Cancellation uses a disposable fixture, never modifies user input.
        let fixture = tempfile::tempdir().map_err(|e| e.to_string())?;
        let stuck = fixture.path().join("stuck.tex");
        std::fs::write(
            &stuck,
            "\\documentclass{article}\\begin{document}\\loop\\iftrue\\repeat\\end{document}",
        )
        .map_err(|e| e.to_string())?;
        let config = tex::Config::new(&stuck, engine)?;
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let compiling = thread::spawn(move || tex::compile(&config, &flag));
        thread::sleep(Duration::from_millis(500));
        let start = Instant::now();
        cancel.store(true, Ordering::Relaxed);
        let cancelled = compiling.join().map_err(|_| "Compiler thread panicked")?;
        cancellation_ms = Some(start.elapsed().as_secs_f64() * 1000.);
        if cancelled.err().as_deref() != Some(tex::CANCELLED)
            || start.elapsed() > Duration::from_secs(2)
        {
            return Err("TeX cancellation failed its two-second bound".into());
        }
        live_updates = exercise_saves()?;
        Ok(())
    })();
    let report = json!({"file":file,"pages":pages,"runs":runs,"build_ms":timing(&mut build),"cancellation_ms":cancellation_ms,"live_updates":live_updates,
        "memory":memory.finish()?,"complete":result.is_ok(),"passed":result.is_ok(),"warnings":result.as_ref().err().into_iter().collect::<Vec<_>>(),
        "scope":"offline private snapshots, repeated TeX builds, last-page PDF render, disposable stuck-build cancellation; no native UI; sampled helper RSS can miss short peaks; build latency is informational"});
    println!(
        "{}",
        serde_json::to_string_pretty(&run.save(&report, output)?)?
    );
    result.map_err(io::Error::other)
}

/// Exercise the real watcher/scheduler on a disposable 100-page project. The
/// caller's file and its neighboring inputs are never changed.
fn exercise_saves() -> Result<Value, String> {
    let fixture = tempfile::tempdir().map_err(|e| e.to_string())?;
    let input = fixture.path().join("main.tex");
    let body = fixture.path().join("body.tex");
    let write = |path: &Path, text: &str| std::fs::write(path, text).map_err(|e| e.to_string());
    write(
        &input,
        "\\documentclass{article}\\begin{document}\\input{body}\\end{document}",
    )?;
    let baseline = "A page of a large live preview.\\newpage\n".repeat(100);
    write(&body, &baseline)?;
    let config = tex::Config::new(&input, tex::Engine::discover()?)?;
    let (session, _events) = tex::Session::start(config)?;
    let wait = |predicate: &dyn Fn(&tex::State) -> bool| -> Result<tex::State, String> {
        let started = Instant::now();
        loop {
            let state = session.state();
            if predicate(&state) {
                return Ok(state);
            }
            if started.elapsed() > Duration::from_secs(75) {
                return Err(format!("Live preview timed out: {}", state.status));
            }
            thread::sleep(Duration::from_millis(25));
        }
    };
    let first = wait(&|s| s.output.is_some() || s.error.is_some())?;
    if let Some(error) = first.error {
        return Err(error);
    }
    let first_id = first.output.as_ref().unwrap().document.id;
    if first.output.as_ref().unwrap().info.pages != 100 {
        return Err("Unexpected live fixture page count".into());
    }
    // Let an obsolete build get into the compiler, then save twenty revisions.
    write(&body, "\\loop\\iftrue\\repeat")?;
    wait(&|s| s.started.is_some())?;
    thread::sleep(Duration::from_millis(200));
    for revision in 0..20 {
        write(&body, &format!("Revision {revision}.\\par\n{baseline}"))?;
        thread::sleep(Duration::from_millis(10));
    }
    let started = Instant::now();
    write(&body, &format!("{baseline}Latest revision."))?;
    let waiting = session.state();
    if waiting.output.as_ref().map(|o| o.document.id) != Some(first_id) {
        return Err("An intermediate save replaced the retained PDF".into());
    }
    let latest =
        wait(&|s| s.output.as_ref().is_some_and(|o| o.info.pages == 101) || s.error.is_some())?;
    if let Some(error) = latest.error {
        return Err(error);
    }
    let latest_id = latest.output.as_ref().unwrap().document.id;
    let save_ms = started.elapsed().as_secs_f64() * 1000.;
    write(&body, "\\undefinedcontrolsequence")?;
    let failed = wait(&|s| s.error.is_some())?;
    if failed.output.as_ref().map(|o| o.document.id) != Some(latest_id) {
        return Err("A failed build discarded the last successful PDF".into());
    }
    write(&body, &baseline)?;
    wait(&|s| {
        s.error.is_none()
            && s.output
                .as_ref()
                .is_some_and(|o| o.info.pages == 100 && o.document.id != first_id)
    })?;
    Ok(
        json!({"initial_pages":100,"latest_pages":101,"burst_saves":21,"save_to_pdf_ms":save_ms,
        "obsolete_build_cancelled":true,"last_pdf_retained":true,"recovered_after_error":true}),
    )
}
