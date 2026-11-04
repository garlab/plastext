use std::{
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom},
    path::Path,
    sync::atomic::AtomicBool,
    time::Instant,
};

use plastext_buffer::{
    ByteSource, DiskVersion, EditBuffer, EncodingHint, OpenOptions, SaveOptions, TextEncoding,
    TextFormat, UndoGroup, save_snapshot,
};
use serde_json::json;

/// Never writes to the input: all save targets and recovery files are temporary.
pub fn run(input: &Path, encoding: Option<&str>, output: Option<&Path>) -> io::Result<()> {
    fs::create_dir_all("target/benchmarks")?;
    let work = tempfile::Builder::new()
        .prefix("m3-save-fixture-")
        .tempdir_in("target/benchmarks")?;
    let target = work.path().join("document");
    let input_version = DiskVersion::read(input)?;
    let started = Instant::now();
    fs::copy(input, &target)?;
    let fixture_copy_ms = elapsed(started);
    let memory = crate::memory::Sampler::start()?;
    let cancel = AtomicBool::new(false);
    let source_cache_budget = 8 * 1024 * 1024;
    let started = Instant::now();
    let options = OpenOptions {
        block_cache: source_cache_budget,
        ..OpenOptions::default()
    };
    let mut buffer = if let Some(encoding) = encoding {
        let hint = if encoding == "auto" {
            EncodingHint::Auto
        } else {
            EncodingHint::Explicit(
                TextEncoding::for_label(encoding)
                    .ok_or_else(|| io::Error::other("unsupported encoding label"))?,
            )
        };
        EditBuffer::open_text(&target, options, hint, &cancel, |_, _| {})?
    } else {
        EditBuffer::open(&target, options, &cancel, |_, _| {})?
    };
    let prepare_ms = elapsed(started);
    const PREFIX: &[u8] = b"// plastext save benchmark\n";
    buffer
        .replace(0..0, PREFIX, UndoGroup::New)
        .map_err(|e| io::Error::other(format!("{e:?}")))?;
    let snapshot = buffer.snapshot();
    let (prefix, bom) = encoded_prefix(snapshot.format(), PREFIX);
    let input_bytes = fs::metadata(input)?.len();
    let mut results = Vec::new();
    for atomic in [true, false] {
        let expected = DiskVersion::read(&target)?;
        let started = Instant::now();
        let receipt = save_snapshot(
            &snapshot,
            &expected,
            SaveOptions { atomic },
            &cancel,
            |_| {},
        )
        .map_err(io::Error::other)?;
        let save_ms = elapsed(started);
        let started = Instant::now();
        verify(input, &target, &prefix, bom)?;
        results.push(json!({"in_place": receipt.in_place, "save_ms": save_ms,
            "byte_verification_ms": elapsed(started), "passed": true}));
        if !receipt.version.is_current()? {
            return Err(io::Error::other("save checkpoint is stale"));
        }
    }
    if !input_version.is_current()? {
        return Err(io::Error::other("input changed during the diagnostic"));
    }
    if fs::read_dir(work.path())?.count() != 1 {
        return Err(io::Error::other("save left temporary artifacts"));
    }
    let result = json!({
        "file": input, "bytes": input_bytes, "encoding": encoding,
        "working_utf8_bytes": snapshot.len(),
        "saved_bytes": fs::metadata(&target)?.len(), "scope": "headless streaming saves of a private copy; atomic replacement and in-place recovery backup; excludes GUI, cold-cache and crash-durability certification",
        "fixture_copy_ms": fixture_copy_ms, "private_original_preparation_ms": prepare_ms,
        "source_cache_budget_bytes": source_cache_budget, "stream_buffer_bytes": 65536, "output_buffer_bytes": 65536,
        "runs": results, "complete": true, "passed": true, "sampled_rss": memory.finish()?,
    });
    let summary = crate::report::LocalRun::new("m3-save")?.save(&result, output)?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

// The diagnostic inserts ASCII only. Preserve the input BOM and encode that
// edit independently, then compare every remaining original byte after save.
fn encoded_prefix(format: TextFormat, prefix: &[u8]) -> (Vec<u8>, u64) {
    let encoding = format.encoding();
    let mut result = if format.has_bom() {
        if encoding == TextEncoding::UTF16_LE {
            vec![0xff, 0xfe]
        } else if encoding == TextEncoding::UTF16_BE {
            vec![0xfe, 0xff]
        } else {
            vec![0xef, 0xbb, 0xbf]
        }
    } else {
        Vec::new()
    };
    let bom = result.len() as u64;
    for &byte in prefix {
        assert!(byte.is_ascii());
        if encoding == TextEncoding::UTF16_LE {
            result.extend([byte, 0]);
        } else if encoding == TextEncoding::UTF16_BE {
            result.extend([0, byte]);
        } else {
            result.push(byte);
        }
    }
    (result, bom)
}

fn verify(input: &Path, saved: &Path, prefix: &[u8], bom: u64) -> io::Result<()> {
    let mut input = File::open(input)?;
    let mut saved = File::open(saved)?;
    if saved.metadata()?.len() != input.metadata()?.len() - bom + prefix.len() as u64 {
        return Err(io::Error::other("saved length differs from expected"));
    }
    let mut head = vec![0; prefix.len()];
    saved.read_exact(&mut head)?;
    if head != prefix {
        return Err(io::Error::other("saved edit is incorrect"));
    }
    input.seek(SeekFrom::Start(bom))?;
    let mut original = vec![0; 65536];
    let mut actual = vec![0; 65536];
    loop {
        let len = input.read(&mut original)?;
        if len == 0 {
            return Ok(());
        }
        saved.read_exact(&mut actual[..len])?;
        if original[..len] != actual[..len] {
            return Err(io::Error::other("unchanged bytes differ after save"));
        }
    }
}

fn elapsed(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.
}
