//! Text engine micro-benchmarks: `cargo bench -p plastext-bench`.

use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use plastext_buffer::{ByteSource, FileSource, LineIndex, rows};

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

criterion_group!(benches, line_index, display_rows);
criterion_main!(benches);
