# M2 progress — 2026-10-24

M2 is in progress. The accepted M1 performance baseline is commit `01fcbb3`; its [report and original measurements](m1-performance.md) remain unchanged apart from the acceptance note. This increment adds native Markdown and asset views without changing the M1 matcher or text renderer.

## Available now

- Markdown extensions route to a native GPUI preview. CommonMark/GFM parsing supplies paragraphs, headings and unique anchors, lists/tasks, aligned table rows, strikethrough, footnotes, definition lists and GitHub alerts. YAML/TOML front matter renders as a collapsible key/value table with independent virtualized rows. Basic safe inline HTML is styled; comments disappear and unsupported HTML stays inert source.
- Heading navigation uses the existing outline and symbol picker. Local links open in plastext, including heading and `#L42` destinations; supported external links open in the browser. Hover shows link targets, and headings offer copy-anchor links.
- `⇧⌘V` switches between rendered and source views at the corresponding source position. Split panes can independently show either view. This is not the synchronized Markdown split/edit mode planned for M3.
- Code fences use the bundled syntax highlighters lazily, with language labels and whole-fence copy. Rendered find (`⌘F` or `/`) searches case-sensitive literal text across inline markup, maps hits to source positions and navigates with Enter/Shift-Enter or `n`/`N`. Results are capped at 10,000. Matches do not cross virtualized block boundaries. Code-fence matches now show backgrounds while preserving syntax colors, including matches spanning token boundaries. Navigating to a metadata match expands its section automatically.
- Local and data-URI images decode near the viewport. SVG uses resvg with external image references disabled. Remote images remain blocked. Dedicated image tabs provide fit, original-size zoom and pan; downscaled previews are labeled.
- CSV/TSV tables use a sparse record index, virtualized rows, a sticky first-row header, sampled column widths and a Source toggle. Quoted multiline fields and doubled quotes are handled. Byte navigation maps to actual records rather than a file-size estimate.
- Markdown's prose font and centered text measure now honor `markdown.prose_font` and `markdown.max_width`. The default is `system` / `80ch`; `ch` uses the selected font's actual zero-glyph advance at the body size. Pixel measures such as `720px` are supported. Inline code now uses the configured editor font rather than a hard-coded Menlo face. Settings apply on restart.

Open the [demo](fixtures/m2.md), [CSV](fixtures/sample.csv) and [SVG](fixtures/sample.svg):

```sh
cargo run -p plastext-app -- docs/fixtures/m2.md
```

## Work bounds

The Markdown worker first parses a 64 KiB prefix, then replaces it with complete-document results in batches of at most 256 blocks through a bounded channel. The full parse resolves reference definitions. A short yield between publications prevents a continuously ready channel from occupying the UI thread. Only nearby blocks are laid out; long code fences split into short items and table rows are independent items. Text per block is at most 16 KiB, deep nesting is flattened after 128 levels, and oversized table rows fall back to bounded text blocks without dropping cell contents.

The current parser still materializes UTF-8 input and retains the parsed model. Preview is limited to the first 128 MiB; Source remains available for the entire file. These bounds do **not** establish the spec's total-memory target. Native scrolling measurements are recorded below; retained memory and stable anchoring still need validation.

Image decoding limits encoded input to 32 MiB and raster images to 16 megapixels, then reduces previews to a 1,600-pixel maximum edge. Larger raster images display an explicit limit message until tiled decoding exists. SVG dimensions are downscaled before allocating the output pixels. Markdown keeps at most four image jobs active and caps its image cache at 64 MiB/128 entries. Visible code highlights have a bounded cache and at most four jobs.

CSV stores a record offset every 512 records and reads pages of 64 rows on background workers. It retains eight recent pages plus the header page, with at most four page jobs active. Records above 64 KiB or 128 columns show a Source fallback message. LF and CRLF are supported; bare-CR record separators are not yet supported.

### Front-matter tables

YAML scalars (including quoted and folded text) display their decoded text; nested collections stay readable as YAML in the value cell. Aliases remain literal references and are never expanded. TOML tables flatten to dotted field paths, with array-of-table indices such as `links[0].url`; rows stay in source order. Empty collections are retained. See the [YAML](fixtures/m2.md) and [TOML](fixtures/m2-toml.md) demos.

Table parsing is limited to 64 KiB of metadata, 512 rendered rows, 2 KiB keys and 64 nesting levels; YAML also has an 8,192-event limit. TOML's preflight conservatively counts named-table paths twice to allow for array-of-table ancestors. Invalid input, complex YAML keys and over-limit content display an explanation and the complete metadata body as bounded source rows. No source bytes are changed. Long values and fallbacks split at 16 KiB or 32 lines per item. Collapsed bodies are excluded from the native list entirely. Expansion state survives source toggles and pane duplication, and source navigation reveals the owning metadata section.

The native list now maps visible rows to document blocks so collapsing metadata does not disturb outline, search, image-height updates or benchmark destinations. It stores contiguous ranges: one range for a fully expanded document, and at most one extra per collapsed section, rather than an index per block. Tests cover streamed batches, collapse/expand mapping, Unicode/CRLF offsets, aliases, empty collections, source-order TOML tables, oversized values and 2,000-level nesting. The YAML parser's iterator input is used with explicit scalar-to-byte offset conversion after a regression test exposed incorrect non-ASCII offsets in its string-input path.

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

### Native 100 MB follow-up

`read` now drives the actual Markdown view. Scrolling starts at the first preview paint while parsing continues. Once the model is complete, it alternates sequential scrolling with jumps to the first code fence, prose, middle and end. The final run completed **10,000 frames** on the same 100 MB fixture, using the default `80ch` prose measure.

| Measurement | Final run | Budget |
|---|---:|---:|
| First paint / first rendered preview | 18.48 / 18.52 ms | 50 / 100 ms |
| Full model painted while scrolling | 4,311.76 ms | Informational |
| Draw + submit wall time | 4.11 ms median; **8.44 ms p99**; 61.60 ms max | **8 ms p99 — fails** |
| Interval between native paint callbacks | 8.33 ms median; 9.35 ms p99; 68.69 ms max | Informational |

Only 3 of 10,000 measured frame intervals exceeded 16.67 ms. This describes pacing on this machine; it does not override the draw/submit budget, measure GPU completion or constitute visual QA. [Raw final measurements](benchmarks/m2-2026-10-24-native-markdown.json) include every timing sample and correctly report `complete: true`, `passed: false`.

The final run used `--foreground --keep-visible`. Keep-visible mode uses a temporary GPUI popup window above other apps because macOS can suspend drawing in covered windows. This differs from the normal-window configuration of the M1 baseline, which is retained unchanged. The runner now saves incomplete and failed runs, rejects missing/invalid paint and completion markers, and stops after 30 seconds without log progress. It also aborts if Markdown is switched to Source or the full preview cannot be produced.

Earlier runs are retained: a [1,000-frame normal-window probe](benchmarks/m2-2026-10-24-native-probe.json) measured 5.64 ms p99; a [normal-window attempt](benchmarks/m2-2026-10-24-native-interrupted.json) stopped making frame progress after 4,593 measured frames and recorded 8.71 ms p99. The idle benchmark process was terminated and its incomplete result retained. An [initial keep-visible run](benchmarks/m2-2026-10-24-native-quarter-jumps.json) completed 10,000 frames at 8.52 ms p99 but jumped by document quarters, which missed the fixture's short code-fence region. The final run fixes that coverage and adds frame-interval timing. None of these failed or incomplete runs is treated as a pass.

```sh
cargo build --release -p plastext-app -p plastext-bench
target/release/plastext-bench read /tmp/plastext-m2/large.md --frames 10000 --foreground --keep-visible --output /tmp/m2-native.json
```

### Front-matter increment comparison

The accepted M1 artifacts and the preceding 10,000-frame M2 recording are unchanged. After the front-matter and code-find changes, the same 100 MB fixture produced [5.32 ms initial core read/parse and 1,170.98 ms full parse](benchmarks/m2-2026-10-24-front-matter-core.json). There are now 330,080 blocks because the fixture's metadata header and field are separate items; maximum block text and batch sizes remain 1,359 bytes and 256 blocks. This is a single sample, not evidence of a speed improvement.

The attempted [10,000-frame comparison](benchmarks/m2-2026-10-24-front-matter-interrupted.json) ended when its window closed after 4,168 frames. It correctly records `complete: false` and `passed: false`; its partial 5.03 ms draw p99 is not accepted as a completed result. The draw maximum was 746.34 ms; the cause of the interruption and that outlier has not been established.

A separate [1,500-frame smoke run](benchmarks/m2-2026-10-24-front-matter-native.json) completed, with first preview at 20.68 ms, full preview at 7,934.14 ms, draw/submit median 5.03 ms, p99 **8.44 ms**, and maximum 18.15 ms. Frame intervals were 8.33 ms median / 9.47 ms p99. The runner correctly reports `complete: true`, `passed: false` for the existing 8 ms budget miss. Both runs used the same keep-visible benchmark mode. The short run does not replace the original 10,000-frame baseline, certify the M2 exit test, or provide visual QA of expanded tables.

## Math spike

An opt-in example evaluates [MiTeX 0.2.4](https://github.com/mitex-rs/mitex) conversion with Typst 0.15.1 layout and SVG output, static compatibility helpers and bundled MATH fonts. It uses no JavaScript runtime, filesystem resolver or network resolver. It is not linked into the app; Markdown currently shows the formula's TeX text.

```sh
cargo run --release -p plastext-math --features spike --example spike -- /tmp/plastext-math-spike
```

[The recorded run](benchmarks/m2-2026-10-24-math-spike.txt) compiled 18/20 representative expressions. An AMS-symbol expression failed on `sect` compatibility and a `\def` macro failed conversion; `\newcommand` passed. The first formula took 62.44 ms and subsequent successful formulas took 8.57–10.64 ms in that run, constructing an engine each time. The standalone release example was 44,703,584 bytes (42.6 MiB), including its fonts and linked code; this is not an app-size delta. Both cold latency and binary cost need attention before choosing the engine. The helper provenance, license and adaptations are [recorded separately](../crates/math/spike/mitex/README.md).

This is a compilation probe on 20 chosen examples, not the KaTeX corpus or visual-equivalence validation. The engine decision and the required ≥95% corpus result remain open.

### Upstream corpus follow-up

The [pinned KaTeX screenshot fixtures](../crates/math/spike/katex/README.md) now supply all 128 cases from upstream commit `8000fbd4e29cca2c0cb1e88b745af3ed2f436b66`, with their license and options retained. The [runner](../scripts/math-corpus.py) executes each expression in a fresh native process with a five-second timeout, preserves errors, and returns nonzero below 95% compilation.

The [recorded run](benchmarks/m2-2026-10-24-katex-corpus.json) compiled **86/128 (67.19%)**, with no timeouts. Unsupported commands include generalized fractions, rules, script fonts, verbatim text and several accents/arrows; there are also Typst symbol compatibility failures. HTML wrappers, CSS and error-display options are recorded but not implemented. No cases were removed from the denominator, including the deliberately invalid `UnsupportedCmds` case. This is the screenshot suite, not all KaTeX unit tests, and successful compilation does not establish correct visual output.

Decision: **do not integrate the current MiTeX/Typst adapter into the app yet**. Its coverage and the earlier size/cold-latency measurements do not justify selecting it. The same pinned fixtures can evaluate compatibility work or another engine. Native vector output, macro scoping, error fallbacks and visual comparisons remain required before an engine is accepted.

```sh
cargo build --release -p plastext-math --features spike --example spike
python3 scripts/math-corpus.py --output /tmp/plastext-math-corpus
```

Build the app separately without the spike feature for native performance runs, so optional math dependencies do not alter Cargo feature unification in that build.

## Remaining M2 work

- Native math integration, document-scoped macros, delimiter coverage, formula caching/selection and KaTeX corpus validation.
- Full safe HTML subset, remote-image opt-in and host allowlist, and rendered-text selection/copy.
- Huge-image subsampling/tiles, sharp deep zoom, animated GIF/WebP, AVIF/HEIC, checkerboard/pixel grid and SVG source mode.
- Reduce the 100 MB Markdown draw/submit p99 below 8 ms, run native 20M-row CSV scrolling tests, check retained memory, and visually review light/dark themes. The Markdown long run now completes, but layout has not been visually certified.

Workspace tests, strict Clippy and the GPUI dependency-boundary check cover this increment. M2's exit conditions have not been met. Markdown editing, task-list mutation, live preview and synchronized edit/preview belong to M3.
