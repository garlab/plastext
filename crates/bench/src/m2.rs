use plastext_buffer::{ByteSource as _, FileSource, OpenOptions};
use plastext_markdown::{parse, parse_stream};
use serde_json::json;
use std::{
    fs, io,
    ops::ControlFlow,
    path::Path,
    sync::{Arc, atomic::AtomicBool},
    time::Instant,
};

pub fn run(file: &Path, output: Option<&Path>) -> io::Result<()> {
    let start = Instant::now();
    let source = FileSource::open(file, OpenOptions::default())?;
    if source.len() > 128 * 1024 * 1024 {
        return Err(io::Error::other(
            "M2 preview benchmark accepts up to 128 MiB",
        ));
    }
    let head = source.head(64 * 1024)?;
    let preview = parse(&String::from_utf8_lossy(&head));
    let first_ms = start.elapsed().as_secs_f64() * 1000.;
    let mut bytes = Vec::with_capacity(source.len() as usize);
    source.scan(0..source.len(), |chunk| {
        bytes.extend_from_slice(chunk);
        ControlFlow::Continue(())
    })?;
    let text = std::str::from_utf8(&bytes).map_err(io::Error::other)?;
    let full = Instant::now();
    let mut blocks = 0;
    let mut max_block = 0;
    let mut max_batch = 0;
    let summary = parse_stream(
        text,
        || false,
        |batch| {
            max_batch = max_batch.max(batch.len());
            blocks += batch.len();
            max_block = max_block.max(batch.iter().map(|b| b.text.len()).max().unwrap_or(0));
            true
        },
    );
    let full_ms = full.elapsed().as_secs_f64() * 1000.;
    let result = json!({"schema":1,"file":file,"bytes":source.len(),"scope":"core parse only; excludes native layout/paint and reference-machine certification","initial_64k_blocks":preview.blocks.len(),"initial_read_and_parse_ms":first_ms,"full_parse_ms":full_ms,"blocks":blocks,"headings":summary.headings.len(),"max_block_text_bytes":max_block,"max_batch_blocks":max_batch,"warnings":summary.warnings});
    println!("{}", serde_json::to_string_pretty(&result)?);
    if let Some(output) = output {
        fs::write(output, serde_json::to_vec_pretty(&result)?)?;
    }
    if max_block > plastext_markdown::BLOCK_TEXT_LIMIT || max_batch > 256 {
        return Err(io::Error::other("Markdown layout/batch bound exceeded"));
    }
    Ok(())
}

pub fn csv(file: &Path) -> io::Result<()> {
    let start = Instant::now();
    let source = Arc::new(FileSource::open(file, OpenOptions::default())?);
    let delimiter = if file.extension().is_some_and(|s| s == "tsv") {
        b'\t'
    } else {
        b','
    };
    let (send, receive) = std::sync::mpsc::channel();
    let index = plastext_assets::csv::CsvIndex::start(
        source.clone(),
        delimiter,
        Arc::new(AtomicBool::new(false)),
        Arc::new(move || {
            send.send(()).ok();
        }),
    );
    while !index.progress().complete {
        receive
            .recv_timeout(std::time::Duration::from_secs(60))
            .map_err(io::Error::other)?;
    }
    let indexed = start.elapsed().as_secs_f64() * 1000.;
    let count = index.progress().rows;
    let page = Instant::now();
    let rows = index.page(&source, delimiter, count.saturating_sub(60), 60)?;
    let page_ms = page.elapsed().as_secs_f64() * 1000.;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema": 1, "file": file, "bytes": source.len(), "rows": count,
            "index_ms": indexed, "final_page_ms": page_ms, "page_rows": rows.len(),
            "scope": "core indexing and page decode only; excludes native layout/paint",
        }))?
    );
    if let Some(error) = index.error() {
        return Err(io::Error::other(error));
    }
    if rows.iter().any(|row| row.cells.is_err()) {
        return Err(io::Error::other("CSV page decode failed"));
    }
    Ok(())
}
