# Large-workload validation

This suite exercises the current published renderer dependencies and preserves
the accepted M1 and earlier M2 measurements. Only aggregate `*.summary.json`
reports belong in Git; fixtures, frame logs and CPU profiles stay under `target/`.

## Reproduce

Use a release build and run workloads serially. Finish compilation and tests
before timing. The native runs open a temporary foreground window and keep the
display awake. Leave that window visible; `--keep-visible` uses an always-on-top
popup if other windows would cover it. Record that choice when comparing runs.
Do not compare a run collected with a CPU profiler against an unprofiled run.

```sh
cargo build --release --locked -p plastext-app -p plastext-bench
python3 scripts/m2-fixtures.py target/benchmarks/m2-fixtures
python3 scripts/m2-preview-fixtures.py target/benchmarks/m2-preview-fixtures

target/release/plastext-bench m2 target/benchmarks/m2-fixtures/large.md --output target/benchmarks/m2-markdown-core.summary.json
target/release/plastext-bench csv target/benchmarks/m2-fixtures/large.csv --output target/benchmarks/m2-csv-core.summary.json
target/release/plastext-bench read target/benchmarks/m2-fixtures/large.md --frames 10000 --foreground --keep-visible --output target/benchmarks/m2-markdown-native.summary.json
target/release/plastext-bench read target/benchmarks/m2-fixtures/large.csv --frames 10000 --foreground --keep-visible --output target/benchmarks/m2-csv-native.summary.json
target/release/plastext-bench read target/benchmarks/m2-fixtures/wide.csv --frames 1500 --foreground --keep-visible --output target/benchmarks/m2-csv-wide.summary.json
PLASTEXT_PROFILE_CSV_SWEEP=1 target/release/plastext-bench read target/benchmarks/m2-fixtures/wide.csv --frames 1500 --foreground --keep-visible --output target/benchmarks/m2-csv-horizontal.summary.json

for fixture in many-pages dense-vectors large-page large-images; do
  target/release/plastext-bench pdf "target/benchmarks/m2-preview-fixtures/$fixture.pdf" --output "target/benchmarks/m2-$fixture.summary.json"
done
target/release/plastext-bench tex target/benchmarks/m2-preview-fixtures/tex-project/main.tex --output target/benchmarks/m2-tex.summary.json
```

PDF and TeX measurements require macOS. TeX also requires a local TeX Live
installation with `article` and `amsmath`. No package downloads are attempted.
The TeX benchmark snapshots its input read-only; save-burst, error recovery and
stuck-build checks use separate disposable files.

The wide CSV has 20,000 data rows and 128 columns. The optional
`PLASTEXT_PROFILE_CSV_SWEEP` also scrolls horizontally between the first, middle
and last columns. Its summary must show all three `visited_column_targets`,
matching `required_column_targets`; `max_visible_columns` reports the largest
painted header range. The 1,500-frame wide runs are additional diagnostics, not
replacements for the 10,000-frame, 20-million-row check. To compare builds,
preserve the earlier executable and pass it with `--app` under the same window
mode, without the optional sweep for builds that predate that instrumentation.

## What counts as a pass

- Markdown: 10,000 native content frames, all five table/code/prose/distant/end
  destinations visited, first paint within 50 ms, first content within 100 ms,
  draw/submit p99 within 8 ms, and parsed pages within the 32 MiB cache.
- CSV: 10,000 native content frames, start/middle/end records rendered after the
  complete index is available, the same frame budgets, and no more than nine
  cached pages / 576 records (eight recent pages plus the header).
  At least half the requested frames must actually advance the visible row;
  redraws of a stationary table cannot certify scrolling.
- PDF: snapshot and repeated first/middle/last-page reads at 0.125×, 1×, 2× and
  8×; every raster response remains within a 512×512 tile. The existing worker
  deadlines and RSS watchdog remain enabled. Latency is reported, not given an
  invented UI budget.
- TeX: repeated offline builds, last-page PDF rasterization, a stuck compiler
  cancelled within two seconds, plus a 100-page live project surviving an
  obsolete in-flight build, 21 rapid saves, an error and recovery while retaining
  its last successful PDF. Build latency is informational.

Native frame timings include layout, drawing and Metal submission, including
waiting for a drawable; they do not measure GPU completion. CSV/Markdown RSS is
sampled every 500 ms. Headless PDF/TeX sample parent and helper-process RSS every
100 ms; snapshots can miss shorter peaks. These headless checks do not certify
PDF pane resizing, page-turn presentation or GPU-cache behavior.

## Reference hardware

The specification requires a base **M1 MacBook Air with 8 GB RAM**. Run the same
commands there, retaining failures as well as successes, and record OS/toolchain,
display refresh and window mode. M2 Pro results do not certify that requirement.
Reference checks also need visual review of light/dark rendering, anchoring,
PDF resize/page transitions and TeX source/preview updates.

## Local results — 2026-11-03

Apple M2 Pro, 16 GiB RAM, macOS 26.4.1, Rust 1.99.0, release builds with thin
LTO; `document-graphics` and `document-graphics-mermaid` 0.1.3 from crates.io.
Native windows used `--foreground --keep-visible`. Compilation, tests and
measured workloads ran separately for the final runs.

The 100 MB Markdown fixture completed 10,000 content frames and rendered all
five required destinations. Its [passing full run](benchmarks/m2-2026-11-03-validation-markdown-native-repeat.summary.json)
measured **2.21 ms draw/submit p99**, first content at 17.59 ms and full indexing
at 1,860.74 ms. There were 23 placeholder frames, 105 page loads and ten reloads.
The index retained 14.93 MiB, the final parsed cache 31.75 MiB, and sampled app
RSS peaked at 267.09 MiB.

The [preceding full run](benchmarks/m2-2026-11-03-validation-markdown-native.summary.json)
failed at **16.31 ms p99** and is retained. Short diagnostic runs also varied;
a CPU stack sample found substantial waiting in Metal's `nextDrawable`.
That sample does not establish the cause of every slow frame. The pass above
meets the local frame target, but does not demonstrate stable performance across
all display/system conditions or certify the reference M1 Air. Earlier baselines
and the 8 ms target are unchanged.

The [Markdown core check](benchmarks/m2-2026-11-03-validation-markdown-core.summary.json)
also passes: 14.93 MiB index, 1.03 ms page-parse p99, and the parsed cache stayed
below 32 MiB through forward/backward traversal and rendered find. Core RSS
peaked at 115.03 MiB. The benchmark binary now links the platform workers for
PDF/TeX, so its RSS is not directly comparable to older core-only binaries;
index/cache byte counts remain comparable.

### CSV

The [20-million-row native run](benchmarks/m2-2026-11-03-validation-csv-native.summary.json)
completed 10,000 content frames, including **8,506 actual changes of the first
visible row** and all three destinations. First content took 24.10 ms, full
indexing 474.95 ms, and draw/submit p99 was **8.46 ms: this run failed the 8 ms budget**.
Frame-interval p99 was 9.21 ms. The cache held nine pages / 576 records, and sampled
app RSS peaked at 133.64 MiB.

Prefetch now reads one page on either side after visible requests, within the
existing cache and four-job limit. Placeholder frames fell from **421 to 32**.
The [before-prefetch diagnostic](benchmarks/m2-2026-11-03-validation-csv-before-prefetch.summary.json)
retains its 8.43 ms draw/submit p99 and 16.64 ms interval p99. Its movement counter
included layout probes, so it is explicitly incomplete for certification.
The final counter runs during painting, and the runner rejects stationary or
over-counted scrolling. Prefetch improves loading gaps; these timings do not
establish a draw/submit budget pass or a stable change in rendering cost.

The [core CSV check](benchmarks/m2-2026-11-03-validation-csv-core.summary.json)
indexed 20,000,001 records including the header in 579.39 ms and decoded the final
60 records in 0.93 ms. It reports failures independently of native timing.

### CSV visible-column follow-up

On the same hardware and window mode, the unchanged app at `7647347` first
[passed 10,000 frames at 4.83 ms p99](benchmarks/m2-2026-11-03-csv-columns-large-before.summary.json).
That pass predates the optimization below: the earlier 8.46 ms failure is not
consistently reproducible, and remains part of the display/system variability
described above. Both follow-up builds used local `document-graphics` 0.1.5 at
`8d4ab41` (the shared-rendering cleanup).

CSV rows now defer cell construction until their visible bounds are known.
Only intersecting columns are built and laid out, including partial edge cells;
off-screen columns retain their logical widths. Fixed-height sizing probes no
longer build cells. Selection and copying continue to use complete source
records, independently of which columns are visible.

| Native workload | Frames | Draw/submit p99 before → after | First content before → after | Peak sampled app RSS before → after |
|---|---:|---:|---:|---:|
| [20M rows before](benchmarks/m2-2026-11-03-csv-columns-large-before.summary.json) / [after](benchmarks/m2-2026-11-03-csv-columns-large-after.summary.json) | 10,000 | 4.83 → **2.88 ms** | 24.99 → 24.83 ms | 131.75 → 131.77 MiB |
| [128 columns before](benchmarks/m2-2026-11-03-csv-columns-wide-before.summary.json) / [after](benchmarks/m2-2026-11-03-csv-columns-wide-after.summary.json) | 1,500 | 46.34 → **4.99 ms** | 157.17 → 42.01 ms | 336.38 → 182.34 MiB |

The large-file run changed the first visible row 8,506 times, reached all three
destinations and retained nine pages / 576 records. Its 32 placeholder frames
match the unchanged build. The wide run built at most seven columns per row.
A separate [horizontal sweep](benchmarks/m2-2026-11-03-csv-columns-horizontal.summary.json)
reached the first, middle and last columns and all three row destinations at
**4.45 ms p99**, with nine pages / 545 records cached. These are single runs of
each workload, collected without overlapping compilation or tests.

The wide fixture is 46,081,408 bytes, with 20,000 data rows and 128 columns;
SHA-256 `dfc86cdd9de0aaa5495b9f004c495dfdb4672bdb606ef5c19329824a2b1c98b5`.
Regression checks cover partial edge cells, ragged rows, resized widths,
coordinate calculations at 50%, 100% and 200% zoom, existing selection/copy
behavior, and rejection of incomplete horizontal benchmark runs. These checks
do not replace manual interaction or reference-hardware review.

### PDF and TeX

All four PDF cases completed 160 tile requests, with existing watchdogs enabled,
no rejected requests and at most 1 MiB of pixel output per tile. RSS below is the
sampled sum of helpers, including a final sample so fast runs cannot miss their
retained worker entirely. Parent-process RSS stayed below 13 MiB.

| PDF fixture | Pages | Open / snapshot (ms) | Tile p99 (ms) | Helper peak RSS (MiB) |
|---|---:|---:|---:|---:|
| [10,000 pages](benchmarks/m2-2026-11-03-validation-pdf-many-pages.summary.json) | 10,000 | 34.40 | 11.38 | 40.69 |
| [50,000 vector strokes per page](benchmarks/m2-2026-11-03-validation-pdf-dense-vectors.summary.json) | 100 | 16.03 | 508.03 | 18.17 |
| [14,400 × 14,400 point pages](benchmarks/m2-2026-11-03-validation-pdf-large-page.summary.json) | 10 | 15.81 | 135.40 | 17.81 |
| [144 MiB image-heavy PDF](benchmarks/m2-2026-11-03-validation-pdf-large-images.summary.json) | 12 | 111.55 | 21.65 | 239.69 |

The dense vector fixture is expensive despite bounded memory. These worker
results do not establish PDF page-turn latency, GPU memory or smooth resizing.
Those native interaction checks remain open.

The [TeX check](benchmarks/m2-2026-11-03-validation-tex.summary.json) compiled the
100-input-file, 200-page project three times using installed TeX Live 2024:
845.75 ms median / 850.02 ms maximum, with 52.41 MiB sampled helper peak RSS.
Cancelling a stuck build took 45.84 ms. A separate 100-page watched project
cancelled an obsolete build, coalesced 21 saves into the latest 101-page PDF,
retained that PDF during an error and recovered after the next save. The final
save reached its PDF in 879.37 ms. No source project was modified.

### M1 core comparison and remaining checks

The [kernel-tree / million-path comparison](benchmarks/m2-2026-11-03-validation-m1-comparison.summary.json)
used 86,280 kernel files and five runs. Search first-result p99 was 1.07 ms;
kernel-path completed ranking was 2.62 ms. Million-path first matching results
were 2.22 ms, while completed ranking was **26.34 ms**, still above the 16 ms
budget (the accepted baseline was 28.68 ms). This known miss remains a failure;
the samples do not establish a statistical improvement. This is a comparison
with the M1 milestone's core baseline, not a reference-machine check.

Local Markdown and CSV runs now meet the frame budget. The base M1 Air / 8 GB
runs and native PDF/TeX interaction and visual review remain open. Local passes
and bounded headless workers do not certify all M2 exit conditions.
