use plastext_buffer::{ByteSource as _, FileSource, OpenOptions};
use plastext_markdown::{DocumentIndex, PARSED_CACHE_BYTES, PageCache, parse, parsed_bytes};
use serde_json::json;
use std::{
    io,
    path::Path,
    sync::{Arc, atomic::AtomicBool},
    time::Instant,
};

pub fn run(file: &Path, output: Option<&Path>) -> io::Result<()> {
    let rss_start = rss_mb()?;
    let start = Instant::now();
    let source = FileSource::open(file, OpenOptions::default())?;
    let head = source.head(64 * 1024)?;
    let preview = parse(&String::from_utf8_lossy(&head));
    let first_ms = start.elapsed().as_secs_f64() * 1000.;
    let initial_blocks = preview.blocks.len();
    drop(preview);
    let full = Instant::now();
    // This core benchmark excludes the math engine's equation-counting pass.
    let index = DocumentIndex::build(&source, || false, |_, _| 0)?;
    let full_ms = full.elapsed().as_secs_f64() * 1000.;
    let rss_index = rss_mb()?;
    let peak_index = peak_rss_mib();
    let mut cache = PageCache::default();
    let mut max_page = 0;
    let mut max_block = 0;
    let mut samples = Vec::new();
    let mut passes = Vec::new();
    // Traversal in both directions exercises eviction and revisiting old pages.
    for reverse in [false, true] {
        let start = Instant::now();
        for n in 0..index.pages.len() {
            let page = if reverse {
                index.pages.len() - 1 - n
            } else {
                n
            };
            let started = Instant::now();
            let blocks = index.parse_page(&source, page, || false)?;
            samples.push(started.elapsed().as_secs_f64() * 1000.);
            max_page = max_page.max(parsed_bytes(&blocks));
            max_block = max_block.max(blocks.iter().map(|b| b.text.len()).max().unwrap_or(0));
            if !cache.insert(page, blocks) {
                return Err(io::Error::other("Parsed page exceeds the cache budget"));
            }
        }
        passes.push(
            json!({"direction":if reverse { "backward" } else { "forward" },
            "elapsed_ms":start.elapsed().as_secs_f64() * 1000., "rss_mib":rss_mb()?,
            "cache_bytes":cache.bytes(), "cached_pages":cache.len()}),
        );
    }
    let before_search = cache.bytes();
    let search = Instant::now();
    let mut matches = 0;
    index.search(
        &source,
        "plastext-unmatched-benchmark-query",
        10_000,
        || false,
        |hits| {
            matches += hits.len();
            true
        },
    )?;
    let search_ms = search.elapsed().as_secs_f64() * 1000.;
    samples.sort_unstable_by(f64::total_cmp);
    let quantile = |q: f64| {
        samples
            .get(((samples.len().saturating_sub(1)) as f64 * q) as usize)
            .copied()
            .unwrap_or(0.)
    };
    let passed = cache.bytes() <= PARSED_CACHE_BYTES
        && cache.bytes() == before_search
        && max_block <= plastext_markdown::BLOCK_TEXT_LIMIT;
    let result = json!({"schema":1,"file":file,"bytes":source.len(),
        "scope":"core source index, bounded parsed cache and rendered search; excludes native layout, equation counting and reference-machine certification",
        "initial_64k_blocks":initial_blocks,"initial_read_and_parse_ms":first_ms,
        "full_index_ms":full_ms,"blocks":index.blocks.len(),"pages":index.pages.len(),"headings":index.headings.len(),
        "index_bytes":index.retained_bytes(),"max_block_text_bytes":max_block,"max_parsed_page_bytes":max_page,
        "parsed_cache_budget_bytes":PARSED_CACHE_BYTES,"warnings":index.warnings,
        "memory_mib":{"before":rss_start,"index_retained":rss_index,"peak_after_index":peak_index,
            "after_search":rss_mb()?,"peak":peak_rss_mib()},
        "traversals":passes,
        "page_parse_ms":{"p50":quantile(0.5),"p99":quantile(0.99),"max":samples.last().copied().unwrap_or(0.),"samples":samples},
        "rendered_find":{"elapsed_ms":search_ms,"matches":matches,"cache_bytes_before":before_search,"cache_bytes_after":cache.bytes()},
        "complete":true,"passed":passed});
    let summary = crate::report::LocalRun::new("m2")?.save(&result, output)?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    if !passed {
        return Err(io::Error::other("Markdown memory/layout bound exceeded"));
    }
    Ok(())
}

/// This process's resident memory, in MiB. Missing measurements fail the run.
fn rss_mb() -> io::Result<f64> {
    rss_of(std::process::id())
}

pub(crate) fn rss_of(pid: u32) -> io::Result<f64> {
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("Could not measure resident memory"));
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<f64>()
        .map(|kib| kib / 1024.)
        .map_err(io::Error::other)
}

fn peak_rss_mib() -> Option<f64> {
    #[cfg(unix)]
    {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
        // SAFETY: getrusage initializes the supplied struct on success.
        if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } == 0 {
            let usage = unsafe { usage.assume_init() };
            let divisor = if cfg!(target_os = "macos") {
                1024. * 1024.
            } else {
                1024.
            };
            return Some(usage.ru_maxrss as f64 / divisor);
        }
    }
    None
}

pub fn csv(file: &Path, output: Option<&Path>) -> io::Result<()> {
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
    let passed =
        index.error().is_none() && !rows.is_empty() && rows.iter().all(|row| row.cells.is_ok());
    let result = json!({
        "schema": 1, "file": file, "bytes": source.len(), "rows": count,
        "index_ms": indexed, "final_page_ms": page_ms, "page_rows": rows.len(),
        "scope": "core indexing and page decode only; excludes native layout/paint",
        "complete": true, "passed": passed,
    });
    let summary = crate::report::LocalRun::new("csv")?.save(&result, output)?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    if let Some(error) = index.error() {
        return Err(io::Error::other(error));
    }
    if !passed {
        return Err(io::Error::other(
            "CSV page decode failed or produced no rows",
        ));
    }
    Ok(())
}
