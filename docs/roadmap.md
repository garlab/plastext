# Current roadmap

The [milestones in the specification](specs.md#10-roadmap) remain the main plan.
M2 (Markdown and assets) is in progress; [M2 progress](m2-progress.md) records the
implemented features, measured limits and remaining exit checks. The accepted
[M1 performance baseline](m1-performance.md) remains the comparison point.

## Near term

1. Polish everyday navigation and search. Project search now has explicit case
   matching and alphabetical file groups. Include/exclude globs and expanding
   capped results remain part of the search specification; continuation should
   also make the retained subset deterministic when the result limit is reached.
2. **Completed:** [image previews](images.md) now have bounded large-image
   decoding, tiles for sharp zoom, AVIF/HEIC and additional native formats,
   animated GIF/WebP, and SVG source mode. See the image guide for codec and
   resource limits.
3. Complete the remaining Markdown reading features, especially rendered-text
   selection/copy, math in headings and document-scoped macros.
4. Validate large Markdown, CSV, PDF and TeX workloads. M2 still needs a passing
   10,000-frame Markdown run within the 8 ms p99 budget and reference-M1 checks.

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

These are lower-priority candidates, not requirements for M2 completion:

- **Chess records:** PGN games and FEN positions, with moves, variations and
  comments using the same interaction conventions as the SGF viewer.
- **ABC music notation:** start with score rendering and Source/Preview. Playback
  can be considered separately from notation support.
- **Audio and video:** pursue only if an optional backend can provide bounded
  streaming, seeking and prompt cancellation without materially increasing app
  size or idle resource use. Pause playback and release decoder resources when
  its preview is hidden.

Static board/score rendering can fit in `document-graphics`; navigation and
playback controls belong in the applications. Media playback needs its own
backend rather than passing continuous audio/video through a still-image worker.
