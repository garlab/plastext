# Current roadmap

The [milestones in the specification](specs.md#10-roadmap) remain the main plan.
M2 (Markdown and assets) is in progress. Completed reading and preview features
are documented in [M2 progress](m2-progress.md), [search](search.md),
[images](images.md), [Markdown reading](markdown-reading.md), [Go](sgf.md) and
[chess](chess.md). The accepted [M1 performance baseline](m1-performance.md)
remains the comparison point.

## Shared rendering

1. **In progress: extract bounded image decoding into `document-graphics`.**
   Use optional features for portable decoding and native codecs. Share metadata,
   thumbnails, orientation, regions, streaming PNG tiles and animation-frame
   decoding between Plastext and mdviewer. Applications retain file-access policy,
   snapshots, queues, caches, fitting and playback controls. Compare output,
   memory, startup and binary size before accepting the extraction.
2. **Consolidate SVG rendering with the image extraction.** Keep the strict
   document SVG subset and the broader isolated image renderer explicit. Preserve
   their respective font policies, resource limits and source fallbacks.
3. **Share private worker utilities.** Reuse cancellation, deadlines, memory
   monitoring and process cleanup where the workers need the same mechanisms.
   Keep format-specific protocols and sandbox rules separate; do not add another
   public crate or a general-purpose worker framework.

Math remains in `katex-native`. Markdown/HTML parsing, layout, navigation and
icon-theme loading remain application concerns. TeX compilation stays with
project/tool orchestration. Defer PDF extraction until another consumer needs
it. Keep new backends optional and initialize them only when used.

## Remaining M2 work

- Finish the [large-workload checks](m2-validation.md): CSV still misses the 8 ms
  frame target; reference M1 Air measurements and native PDF/TeX interaction
  review remain outstanding.
- Add zoom/pan or tiled rendering for reading large Mermaid graphs at full
  detail, and expose more specific renderer rejection reasons. Preserve the
  [existing diagram limits](mermaid.md#resource-bounds).

These precede M3 editing (undo/redo, reliable saves, external changes and a
synchronized Markdown source/preview), then M4 Git review and M5 release
hardening and packaging.

## Additional previews

These are not requirements for M2 completion:

- **ABC music notation:** put bounded score rendering in an optional
  `document-graphics` module, with Source/Preview in the applications. Consider
  playback separately from notation support.
- **Audio and video:** pursue only if an optional backend can provide bounded
  streaming, seeking and prompt cancellation without materially increasing app
  size or idle resource use. Pause playback and release decoder resources when
  its preview is hidden. Media playback needs its own backend rather than a
  still-image worker.
