# Shared image extraction

Bounded image decoding moved into `document-graphics` in release 0.1.5. Plastext
enables `sgf`, `chess` and `image-native`; gloss enables only `images`.
Applications retain file-access policy, queues, caches,
fitting, playback and idle retirement. No new crate or service was introduced.

Portable PNG/JPEG thumbnails share one byte-decoding API. The isolated image
worker owns metadata, regions, streaming PNG/Adam7, native color/orientation,
animation frames and broader SVG. The strict SVG API retains its bundled fonts
and rejection policy; broader SVG stays worker-only with nested images disabled.
Icon-theme parsing and PDF/TeX orchestration remain in Plastext. macOS resident
memory sampling is shared privately between the Mermaid and image workers.
The follow-up below consolidates request guards and child cleanup.

## Comparison

Measured 2026-11-03 on Apple M2 Pro / 16 GiB, macOS 26.4.1, Rust 1.99.0, using
release binaries. Plastext's comparison binary is from `2c78f11`; gloss's
comparison binary uses published `document-graphics` 0.1.3. Both use the same
published Mermaid backend 0.1.3. Builds and tests finished before timing.

Eight image cases retained identical metadata and decoded RGBA for overview and
bottom-right regions: AVIF, HEIC, oriented JPEG, P3 PNG, Adam7 PNG, simple SVG,
SVG local reuse, and a generated 40,000 × 40,000 PNG. This is output equivalence
for the fixtures, not a visual review of every codec or GUI interaction.

Image measurements use one warm-up and three fresh processes per version;
latencies below are medians, and RSS is the largest `wait4` peak in those runs.
The huge PNG is generated with Plastext's `scripts/make-large-image.py`. Its
bounded overview is 1,592 × 1,592; the comparison tile is 64 × 64 at the bottom
right. Timings cover the helper protocol, excluding UI/GPU display.

| Measurement | Before | After |
| --- | ---: | ---: |
| Huge PNG overview render | 1,228.1 ms | 1,214.7 ms |
| Huge PNG tile render | 129.9 ms | 129.3 ms |
| Huge PNG worker peak RSS | 57.86 MiB | 58.11 MiB |
| Plastext first-frame upper bound | 208.3 ms | 191.4 ms |
| Plastext release binary | 37,760,816 bytes | 38,082,416 bytes |
| gloss release binary | 16,995,264 bytes | 16,995,312 bytes |

Plastext grows 314 KiB (0.85%); gloss grows 48 bytes. First-frame measurements
use a separate empty workspace/configuration and the app's startup profiler,
with one warm-up and three launches. These sequential short runs do not
establish a startup improvement or replace the accepted M1 baseline.

Terminal measurements alternate before/after order, with one warm-up and nine
fresh processes per version and case. The drained PTY is 40 rows with 10 × 20
pixel cells, dark appearance and automatic graphics. Width follows the colon in
the table. Full terminal output, including image payloads, is byte-identical.
Wall time includes startup and workers but excludes emulator painting.

| Terminal fixture:columns | Before | After |
| --- | ---: | ---: |
| images.md:100 | 6.21 ms | 6.18 ms |
| images.md:24 | 5.40 ms | 5.40 ms |
| inline.md:100 | 5.71 ms | 5.72 ms |
| inline.md:24 | 5.45 ms | 5.50 ms |
| mermaid.md:100 | 45.32 ms | 45.04 ms |
| mermaid.md:24 | 36.48 ms | 36.37 ms |

The paired terminal results show no meaningful change in these small fixtures.
Raw logs, generated images and comparison binaries stay local under ignored
`target/`; only this summary is tracked.

## Validation and release

Checks passed: Plastext workspace library tests, snapshot/broker integration and
Clippy; document-graphics unit, native/portable worker and doc tests, Clippy,
Rust 1.90, and package verification including all optional features; gloss
68 Rust tests, 44 terminal tests and 11 pager tests. The shared crate also
cross-compiles for Linux and Windows; runtime validation here was on macOS.
Windows has the portable thumbnail API, not an image worker. Non-macOS Unix
workers do not yet have equivalent OS sandbox enforcement.

The source/snapshot, cache and worker limits are preserved. Failed image-worker
requests now stop and reap the child before returning to a public caller. The
portable API also enforces tiny output budgets for extremely narrow images.
Native GUI zoom/playback interactions still need manual review.

The migration was validated with both consumers pinned to published 0.1.5,
with registry sources and checksums in their lockfiles and no local overrides.
The Mermaid backend and math dependency versions were unchanged at that stage.

## Private worker cleanup

The Mermaid and image clients now share cancellation/deadline guards, macOS
memory-poll scheduling and direct-child termination/reaping. Their protocols,
I/O loops, timeout values, error kinds and sandbox policies remain separate.
Image requests still require a successful memory sample; Mermaid keeps its
best-effort sampling policy. No public API, dependency or feature change is
needed; this is included in 0.1.5.

The follow-up comparison against library commit `0fa9ed2` used the same machine,
release profiles and eight image/six terminal cases above, with one warm-up and
three fresh processes per version. Image metadata/RGBA and complete terminal
output remained identical. The huge PNG overview took 1,199.5 → 1,205.8 ms, the
64 × 64 tile 130.4 → 130.7 ms, and worker peak RSS 58.11 → 57.89 MiB. The terminal
Mermaid fixture took 44.84 → 45.46 ms at 100 columns and 37.34 → 35.87 ms at 24.
These short sequential runs show no material regression; they are smoke checks,
not new performance guarantees. Plastext's binary shrank 32 bytes and gloss's
grew 16 bytes. Raw samples remain local.

Default/all-feature library tests, Clippy, Rust 1.90 and package verification
passed, together with Linux/Windows cross-compilation checks. Both Plastext
worker adapters passed integration tests. gloss passed its 68 Rust tests,
44 terminal checks and 11 pager checks. Cancellation, partial replies, late
completion, memory limits and process reaping remain covered by regression tests.
