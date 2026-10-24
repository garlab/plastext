# M2 progress — 2026-10-24

M2 is in progress. The accepted M1 performance baseline is commit `01fcbb3`; its [report and original measurements](m1-performance.md) remain unchanged apart from the acceptance note. This increment adds native Markdown and asset views without changing the M1 matcher or text renderer.

## Available now

- Markdown extensions route to a native GPUI preview. CommonMark/GFM parsing supplies paragraphs, headings and unique anchors, lists/tasks, aligned table rows, strikethrough, footnotes, definition lists and GitHub alerts. YAML/TOML front matter is collapsible source text. Basic safe inline HTML is styled; comments disappear and unsupported HTML stays inert source.
- Heading navigation uses the existing outline and symbol picker. Local links open in plastext, including heading and `#L42` destinations; supported external links open in the browser. Hover shows link targets, and headings offer copy-anchor links.
- `⇧⌘V` switches between rendered and source views at the corresponding source position. Split panes can independently show either view. This is not the synchronized Markdown split/edit mode planned for M3.
- Code fences use the bundled syntax highlighters lazily, with language labels and whole-fence copy. Rendered find (`⌘F` or `/`) searches case-sensitive literal text across inline markup, maps hits to source positions and navigates with Enter/Shift-Enter or `n`/`N`. Results are capped at 10,000. Matches do not cross virtualized block boundaries; code-fence matches navigate but currently keep syntax colors rather than match backgrounds.
- Local and data-URI images decode near the viewport. SVG uses resvg with external image references disabled. Remote images remain blocked. Dedicated image tabs provide fit, original-size zoom and pan; downscaled previews are labeled.
- CSV/TSV tables use a sparse record index, virtualized rows, a sticky first-row header, sampled column widths and a Source toggle. Quoted multiline fields and doubled quotes are handled. Byte navigation maps to actual records rather than a file-size estimate.

Open the [demo](fixtures/m2.md), [CSV](fixtures/sample.csv) and [SVG](fixtures/sample.svg):

```sh
cargo run -p plastext-app -- docs/fixtures/m2.md
```

## Work bounds

The Markdown worker first parses a 64 KiB prefix, then replaces it with complete-document results in batches of at most 256 blocks through a bounded channel. The full parse resolves reference definitions. A short yield between publications prevents a continuously ready channel from occupying the UI thread. Only nearby blocks are laid out; long code fences split into short items and table rows are independent items. Text per block is at most 16 KiB, deep nesting is flattened after 128 levels, and oversized table rows fall back to bounded text blocks without dropping cell contents.

The current parser still materializes UTF-8 input and retains the parsed model. Preview is limited to the first 128 MiB; Source remains available for the entire file. These bounds do **not** establish the spec's total-memory target. Native 100 MB scrolling, memory and stable anchoring still need measurement.

Image decoding limits encoded input to 32 MiB and raster images to 16 megapixels, then reduces previews to a 1,600-pixel maximum edge. Larger raster images display an explicit limit message until tiled decoding exists. SVG dimensions are downscaled before allocating the output pixels. Markdown keeps at most four image jobs active and caps its image cache at 64 MiB/128 entries. Visible code highlights have a bounded cache and at most four jobs.

CSV stores a record offset every 512 records and reads pages of 64 rows on background workers. It retains eight recent pages plus the header page, with at most four page jobs active. Records above 64 KiB or 128 columns show a Source fallback message. LF and CRLF are supported; bare-CR record separators are not yet supported.

## Measurements

Apple M2 Pro, 16 GiB RAM, macOS 26.4.1, Rust 1.99 release build with thin LTO. Commands ran serially after compilation. These are single-run core measurements, **not** native paint/scroll timing or reference-machine certification.

| Fixture / measurement | Result |
|---|---:|
| 100,000,000-byte Markdown: initial 64 KiB read and parse | 1.78 ms |
| Same document: full parse, excluding full-input read | 1,140.47 ms |
| Parsed blocks / headings | 330,079 / 45,391 |
| Maximum block text / publication batch | 1,359 bytes / 256 blocks |
| 546,670,014-byte CSV: 20,000,001 records including header | 489.51 ms to index |
| CSV: decode final 60 records | 0.96 ms |

Raw results: [Markdown](benchmarks/m2-2026-10-24-markdown.json), [CSV](benchmarks/m2-2026-10-24-csv.json). The Markdown runner consumes and drops batches, so it does not measure retained UI-model memory. Fixtures come from [m2-fixtures.py](../scripts/m2-fixtures.py): Markdown includes a 100k-row table, a 100k-line code fence and repeated rich paragraphs; CSV contains 20M quoted records. SHA-256: Markdown `e9bfb6bde242a6b19e94c9198ffca19cc07b6f9c8f88cee014818c01a443018f`; CSV `5883e92f379ed796b2282f2123f487da89d2d0f38ee47c8c83f59195efad71ca`.

The [M1 core comparison](benchmarks/m2-2026-10-24-m1-regression.json) measured million-path first-result p99 at 2.95 ms and completed-ranking p99 at 27.09 ms, versus the accepted baseline's 2.28 / 28.68 ms. Kernel-path completed-ranking p99 was 3.02 ms versus 2.64 ms. These samples do not establish a statistical improvement or regression. The runner correctly still exits nonzero for the known 16 ms completed-ranking miss. M1 native timings were not replaced or recertified.

```sh
cargo build --release -p plastext-app -p plastext-bench
python3 scripts/m2-fixtures.py /tmp/plastext-m2
target/release/plastext-bench m2 /tmp/plastext-m2/large.md --output /tmp/m2-markdown.json
target/release/plastext-bench csv /tmp/plastext-m2/large.csv
target/release/plastext-bench m1 /tmp/linux-6.12 --million --output /tmp/m2-m1-comparison.json
```

## Math spike

An opt-in example evaluates [MiTeX 0.2.4](https://github.com/mitex-rs/mitex) conversion with Typst 0.15.1 layout and SVG output, static compatibility helpers and bundled MATH fonts. It uses no JavaScript runtime, filesystem resolver or network resolver. It is not linked into the app; Markdown currently shows the formula's TeX text.

```sh
cargo run --release -p plastext-math --features spike --example spike -- /tmp/plastext-math-spike
```

[The recorded run](benchmarks/m2-2026-10-24-math-spike.txt) compiled 18/20 representative expressions. An AMS-symbol expression failed on `sect` compatibility and a `\def` macro failed conversion; `\newcommand` passed. The first formula took 62.44 ms and subsequent successful formulas took 8.57–10.64 ms in that run, constructing an engine each time. The standalone release example was 44,703,584 bytes (42.6 MiB), including its fonts and linked code; this is not an app-size delta. Both cold latency and binary cost need attention before choosing the engine. The helper provenance, license and adaptations are [recorded separately](../crates/math/spike/mitex/README.md).

This is a compilation probe on 20 chosen examples, not the KaTeX corpus or visual-equivalence validation. The engine decision and the required ≥95% corpus result remain open.

## Remaining M2 work

- Native math integration, document-scoped macros, delimiter coverage, formula caching/selection and KaTeX corpus validation.
- Full safe HTML subset, front-matter key/value rendering, configurable Markdown typography, remote-image opt-in and host allowlist, and rendered-text selection/copy.
- Huge-image subsampling/tiles, sharp deep zoom, animated GIF/WebP, AVIF/HEIC, checkerboard/pixel grid and SVG source mode.
- Native 100 MB Markdown and 20M-row CSV scrolling tests, retained-memory checks, and visual review in light/dark themes. A debug app launch succeeded, but screen capture was unavailable in this session, so layout has not been visually certified.

Workspace tests, strict Clippy and the GPUI dependency-boundary check cover this increment. M2's exit conditions have not been met. Markdown editing, task-list mutation, live preview and synchronized edit/preview belong to M3.
