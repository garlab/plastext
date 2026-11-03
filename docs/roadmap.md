# Current roadmap

The [milestones in the specification](specs.md#10-roadmap) remain the main plan.
M2 (Markdown and assets) is in progress; [M2 progress](m2-progress.md) records the
implemented features, measured limits and remaining exit checks. The accepted
[M1 performance baseline](m1-performance.md) remains the comparison point.

## Near term

1. **Completed:** [project search](search.md) has explicit case matching,
   include/exclude file globs, deterministic alphabetical results and resumable
   “Show more” pages. Recent files rank higher among matching Quick Open results.
2. **Completed:** [image previews](images.md) now have bounded large-image
   decoding, tiles for sharp zoom, AVIF/HEIC and additional native formats,
   animated GIF/WebP, and SVG source mode. See the image guide for codec and
   resource limits.
3. **Completed:** [Markdown reading](markdown-reading.md) supports rendered-text
   selection/copy, math in headings, document-scoped macros and the safe native
   HTML subset. Previews remain offline; remote-image loading is not planned.
4. **Validation in progress:** [large-workload checks](m2-validation.md) cover
   Markdown, CSV, PDF and TeX with reproducible fixtures. A 10,000-frame Markdown
   run now passes locally; CSV still misses the 8 ms frame target. Reference M1
   Air checks and native PDF/TeX interaction review remain outstanding.

The shared renderer now applies its fast-routing policy to large flowcharts.
Published releases support up to 512 nodes / 1,024 edges with bounded generated
SVG, while retaining the two-second deadline and one-million-pixel output cap.
See [Mermaid resource bounds](mermaid.md#resource-bounds). Diagram zoom/pan or tiled
rendering remains necessary for reading large graphs at full detail. More
specific renderer rejection reasons are also pending.

These precede M3 editing (undo/redo, reliable saves, external changes and a
synchronized Markdown source/preview), then M4 Git review and M5 release
hardening and packaging.

## Additional previews

Additional previews are not requirements for M2 completion:

- **Implemented:** [chess records](chess.md), including PGN games and FEN
  positions, legal move replay, variations and comments using the SGF viewer’s
  navigation conventions. The shared backend awaits publication as 0.1.4.
- **ABC music notation:** start with score rendering and Source/Preview. Playback
  can be considered separately from notation support.
- **Audio and video:** pursue only if an optional backend can provide bounded
  streaming, seeking and prompt cancellation without materially increasing app
  size or idle resource use. Pause playback and release decoder resources when
  its preview is hidden.

Static board/score rendering can fit in `document-graphics`; navigation and
playback controls belong in the applications. Media playback needs its own
backend rather than passing continuous audio/video through a still-image worker.
