use std::{hint::black_box, io, path::Path, sync::atomic::AtomicBool, time::Instant};

use plastext_buffer::{ByteSource, EditBuffer, OpenOptions, UndoGroup};
use serde_json::{Value, json};

pub fn run(file: &Path, edits: u32, output: Option<&Path>) -> io::Result<()> {
    let memory = crate::memory::Sampler::start()?;
    let cache_budget = 8 * 1024 * 1024;
    let started = Instant::now();
    let mut buffer = EditBuffer::open(
        file,
        OpenOptions {
            block_cache: cache_budget,
            ..OpenOptions::default()
        },
        &AtomicBool::new(false),
        |_, _| {},
    )?;
    let prepare_ms = elapsed(started);
    let original = buffer.snapshot();
    let mut prefetch = Vec::new();
    let mut replace = Vec::new();
    let mut snapshot = Vec::new();
    let mut undo = Vec::new();
    let mut redo = Vec::new();
    let mut retained = Vec::new();
    let mut max_history_bytes = 0;
    for i in 0..edits {
        let offset = u64::from(i).wrapping_mul(104_729) % (original.len() + 1);
        let end = (offset + 1).min(original.len());
        // Stand in for the editor's background prefetch. Disk I/O is reported
        // separately and is never silently folded into a "cached edit" sample.
        let started = Instant::now();
        original.load(offset.saturating_sub(65_536)..(end + 65_536).min(original.len()))?;
        prefetch.push(elapsed(started));
        let before = original.bytes(offset..end).map_err(debug_error)?;
        let started = Instant::now();
        buffer
            .replace(offset..end, "λ\n".as_bytes(), UndoGroup::New)
            .map_err(debug_error)?;
        replace.push(elapsed(started));
        let started = Instant::now();
        let edited = black_box(buffer.snapshot());
        snapshot.push(elapsed(started));
        if edited
            .bytes(offset..offset + 3)
            .map_err(debug_error)?
            .as_ref()
            != "λ\n".as_bytes()
        {
            return Err(io::Error::other("edit returned incorrect bytes"));
        }
        max_history_bytes = max_history_bytes.max(buffer.history_bytes());
        let started = Instant::now();
        let undone = buffer.undo();
        undo.push(elapsed(started));
        if !undone
            || buffer.snapshot().revision() != original.revision()
            || buffer.snapshot().bytes(offset..end).map_err(debug_error)? != before
        {
            return Err(io::Error::other(
                "undo did not restore the original version",
            ));
        }
        let started = Instant::now();
        let redone = buffer.redo();
        redo.push(elapsed(started));
        if !redone || buffer.snapshot().revision() != edited.revision() {
            return Err(io::Error::other("redo did not restore the edited version"));
        }
        if retained.len() < 64 {
            retained.push((edited, offset));
        }
        if !buffer.undo() {
            return Err(io::Error::other("could not restore baseline"));
        }
    }
    for (snapshot, offset) in &retained {
        if snapshot
            .bytes(*offset..*offset + 3)
            .map_err(debug_error)?
            .as_ref()
            != "λ\n".as_bytes()
        {
            return Err(io::Error::other("a retained snapshot changed"));
        }
    }
    let result = json!({
        "file": file, "bytes": original.len(), "edits": edits,
        "scope": "headless byte edits with separately timed prefetch; excludes UI, grapheme/UTF-16 mapping, save and reference-machine certification",
        "private_original_preparation_ms": prepare_ms, "chunked_original": buffer.is_chunked(),
        "source_cache_budget_bytes": cache_budget, "max_history_charge_bytes": max_history_bytes,
        "retained_snapshots": retained.len(), "complete": true, "passed": true,
        "metrics": [metric("prefetch", prefetch), metric("cached replace", replace),
            metric("snapshot", snapshot), metric("undo", undo), metric("redo", redo)],
        "sampled_rss": memory.finish()?,
    });
    let summary = crate::report::LocalRun::new("m3-edit")?.save(&result, output)?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

fn debug_error(error: impl std::fmt::Debug) -> io::Error {
    io::Error::other(format!("{error:?}"))
}

fn elapsed(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.
}

fn metric(name: &str, mut samples: Vec<f64>) -> Value {
    samples.sort_by(f64::total_cmp);
    json!({"name": name, "unit": "ms", "p50": samples[samples.len() / 2],
        "p99": samples[(samples.len() * 99).div_ceil(100) - 1], "max": samples.last(), "samples": samples})
}
