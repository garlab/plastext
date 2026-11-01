# Mermaid diagrams

Markdown fences tagged `mermaid` show diagrams automatically. Each fence has a **Source / Preview** toggle and **Copy** copies its original code. Find reveals matching source so labels and syntax remain searchable. Diagrams fit the reading column and follow document zoom; resizing a pane reuses the existing image. Source / Preview also reuses a cached image, avoiding a loading placeholder and scroll jump when returning to a rendered diagram.

Try the [demo](fixtures/mermaid.md):

```sh
cargo run -p plastext-app -- docs/fixtures/mermaid.md
```

To keep all diagrams as code, set this in `config.toml` and restart:

```toml
[markdown]
mermaid = false
```

## Supported syntax

The native subset covers flowcharts (`flowchart` / `graph`), sequence, class and state diagrams, ER relationships and attribute tables, pie charts, mind maps, timelines without sections, user journeys and quadrant charts. The shared document-graphics library carries a small patch to [mermaid-rs-renderer 0.3.1](https://github.com/1jehuang/mermaid-rs-renderer) for embedded-font metrics and chart correctness, with bundled KaTeX/Noto Sans fonts and resvg. No browser, Node installation, external command, download or network access is needed.

This is partial Mermaid compatibility. Labels support Latin (including accents), Greek and Cyrillic. Missing glyphs such as CJK/emoji, timeline sections, other diagram families, embedded icons, clickable nodes, front matter, initialization directives, invalid syntax and oversized diagrams retain their complete source. Layout can differ from Mermaid.js. The source is never rewritten.

## Resource bounds

The Markdown parser keeps diagram fences whole up to 16 KiB / 256 lines. Larger fences remain bounded, virtualized source chunks; no partial diagram is rendered. Ordinary code fences keep their existing 32-line chunks.

A valid Mermaid diagram can exceed these limits. The preview reports the fence-size limit when source has been split into chunks. A compact diagram can also exceed the renderer's 128-node or 256-edge limits, or its layout, SVG and raster budgets. Removing newlines does not bypass those checks.

Only viewport and list-overdraw blocks request diagrams, with at most two requests per view. One lazy worker serializes rendering across all windows, through an eight-request queue. Scrolling away cancels pending work; hiding or closing a tab, or switching the whole document to source, also releases its diagram images. A diagram's own Source toggle cancels pending work but keeps a successful image in the bounded cache, available for eviction while source is shown. The worker exits after five idle seconds and recycles after 32 requests. Documents without diagrams never start it.

Each request has a two-second rendering deadline. On macOS, the parent samples worker resident memory and stops it above 256 MiB. The worker's sandbox denies file-data reads, writes, networking and child processes; fonts are embedded. Unix workers also have an eight-second cumulative CPU backstop and disabled core dumps. No document path or inherited environment is sent to the worker. A failed, cancelled or timed-out worker is killed and reaped before replacement. The Rust rendering path is portable; the macOS sandbox and resident-memory guard require platform equivalents before offering the same guarantees elsewhere.

Preflight limits include 128 nodes, 256 edges, 16 subgraphs, 32 levels of lexical nesting and a 256 KiB generated SVG. SVG permits bounded geometry, text and simple arrow markers; external resources, reuse, filters and foreign content are rejected. Raster output is capped at one million pixels and an 8,192-pixel edge, normally at 2× logical resolution. The application shares a 32 MiB diagram cache across tabs and panes, charging both CPU pixels and GPU image storage, with at most 128 entries. Visible diagrams are protected from eviction; insufficient cache space leaves source available. Source → Preview retries a failed preview.

## Validation

Tests cover all supported families in both appearances, source fallback, oversized and external content, generated-SVG restrictions, bounded worker messages, cancellation, timeout/reaping, and worker recycling. The native integration test uses the same worker entry point as the app, without opening a window:

```sh
cargo test -p plastext-platform --test mermaid_preview
cargo test -p plastext-ui markdown_view::mermaid
```

To save diagnostic PNGs under the ignored workspace `target/` directory:

```sh
cargo test -p plastext-platform --test mermaid_preview -- --render-dir target/mermaid-qa
```

These checks do not certify native scrolling or UI interaction performance; the accepted M1/M2 performance baselines remain unchanged.

Rendering, restricted SVG/font handling and worker supervision come from the
`document-graphics` crate, maintained in its own repository. Plastext owns the
app-wide queue, idle retirement and GUI image cache. `katex-native` also comes from
crates.io. Shared renderer/protocol tests live with the library; this repository
retains app integration tests. See [shared-library development](local-development.md#shared-rendering-libraries)
and [third-party notices](../THIRD_PARTY_NOTICES.md).
