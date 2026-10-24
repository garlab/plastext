//! Text engine micro-benchmarks: `cargo bench -p plastext-bench`.

use std::{hint::black_box, path::Path, sync::atomic::AtomicBool};

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use plastext_buffer::{ByteSource, FileSource, LineIndex, rows};
use plastext_syntax::SyntaxTree;

const SIZE: u64 = 64_000_000;

fn line_index(c: &mut Criterion) {
    let source = FileSource::from_bytes(plastext_bench::log_bytes(SIZE));
    let mut group = c.benchmark_group("line_index");
    group.throughput(Throughput::Bytes(source.len()));
    group.sample_size(10);
    group.bench_function("build 64 MB of log lines", |b| {
        b.iter(|| LineIndex::build(black_box(&source)).unwrap())
    });
    group.finish();
}

fn display_rows(c: &mut Criterion) {
    let source = FileSource::from_bytes(plastext_bench::log_bytes(SIZE));
    let offsets: Vec<u64> = (0..1000).map(|i| i * (source.len() / 1000)).collect();
    let mut group = c.benchmark_group("rows");
    group.bench_function("row_start_at 1000 random offsets", |b| {
        b.iter(|| {
            for &offset in &offsets {
                black_box(rows::row_start_at(&source, offset).unwrap());
            }
        })
    });
    group.bench_function("a 60-row page", |b| {
        b.iter(|| rows::rows(&source, black_box(SIZE / 2 - SIZE / 2 % 4096), 60))
    });

    let mut line = Vec::new();
    plastext_bench::write_long_line(&mut line, SIZE).unwrap();
    let source = FileSource::from_bytes(line);
    group.bench_function("a 60-row page of a 64 MB single line", |b| {
        b.iter(|| {
            let start = rows::row_start_at(&source, black_box(SIZE / 2)).unwrap();
            rows::rows(&source, start, 60)
        })
    });
    group.finish();
}

fn syntax(c: &mut Criterion) {
    // This crate's own source, repeated to about 2 MB.
    let source = include_str!("../../editor/src/text_view.rs");
    let text = source.repeat(2_000_000 / source.len() + 1);
    let language = plastext_syntax::detect(Path::new("big.rs"), b"").unwrap();
    let mut group = c.benchmark_group("syntax");
    group.sample_size(10);
    group.throughput(Throughput::Bytes(text.len() as u64));
    group.bench_function("parse 2 MB of Rust", |b| {
        b.iter(|| SyntaxTree::parse(language, black_box(text.as_bytes()), || false).unwrap())
    });
    group.finish();

    let tree = SyntaxTree::parse(language, text.as_bytes(), || false).unwrap();
    let start = text.len() / 2;
    let page = start..start + 60 * 50;
    c.bench_function("syntax/highlight a 60-row page", |b| {
        b.iter(|| tree.highlights(text.as_bytes(), black_box(page.clone())))
    });
}

fn search(c: &mut Criterion) {
    let source = FileSource::from_bytes(plastext_bench::log_bytes(SIZE));
    let mut group = c.benchmark_group("search");
    group.throughput(Throughput::Bytes(source.len()));
    group.sample_size(10);
    for query in ["status=500", "warn"] {
        let compiled = plastext_search::Query::new(query).unwrap();
        group.bench_function(format!("find {query:?} in 64 MB of log lines"), |b| {
            b.iter(|| {
                let mut count = 0;
                plastext_search::find_all(&source, &compiled, &AtomicBool::new(false), |found| {
                    count += found.len()
                })
                .unwrap();
                count
            })
        });
    }
    group.finish();
}

criterion_group!(benches, line_index, display_rows, syntax, search);
criterion_main!(benches);
