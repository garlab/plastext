# Current roadmap

The [milestones in the specification](specs.md#10-roadmap) remain the main plan.
M3 (editing) has started; M2's remaining validation is still open. Completed
reading and preview features
are documented in [M2 progress](m2-progress.md), [search](search.md),
[images](images.md), [Markdown reading](markdown-reading.md), [Go](sgf.md) and
[chess](chess.md). The accepted [M1 performance baseline](m1-performance.md)
remains the comparison point.

## Shared rendering

Bounded image decoding and both SVG paths now live in `document-graphics`,
behind optional features. Plastext retains snapshots, queues, caches and playback;
mdviewer retains local-file policy, invocation budgets and terminal placement.
The strict document SVG subset and broader isolated image SVG renderer keep
their own font/resource policies. See [shared image rendering](shared-images.md)
for the extraction, validation and performance comparison.

The Mermaid and image workers also share private cancellation/deadline checks,
macOS memory polling and child cleanup. Their protocols, I/O loops and sandbox
policies remain separate. This completes the planned shared-rendering cleanup;
no additional public crate or worker framework is needed.

Math remains in `katex-native`. Markdown/HTML parsing, layout, navigation and
icon-theme loading remain application concerns. TeX compilation stays with
project/tool orchestration. Defer PDF extraction until another consumer needs
it. Keep new backends optional and initialize them only when used.

## M3 editing

The first [M3 increment](m3-progress.md) adds a headless persistent piece tree,
immutable file snapshots and bounded, grouped undo/redo. It does not enable GUI
editing or saving yet. Existing read-mode paths and performance baselines remain
unchanged.

Next, in order:

1. Add Unicode/grapheme and UTF-16 addressing, and a shared document session for
   split panes. Keep reads and input independent of disk I/O.
2. Implement streaming/atomic saves and external-change handling, including
   stale-save completion, symlinks, permissions and concurrent agent rewrites.
   Resolve this before exposing file writes in the UI.
3. Wire edit mode, caret/selection/IME, multi-cursor, find/replace, indentation
   and EditorConfig into the text view.
4. Add synchronized Markdown source/preview, double-click-to-source, tail mode
   and CLI `--wait`; validate `$EDITOR`, fuzzing and typing latency.

M4 Git review and M5 release hardening follow M3.

## M2 follow-ups

- Finish the [large-workload checks](m2-validation.md): local 10,000-frame
  Markdown and CSV runs now pass the 8 ms target. Reference M1 Air measurements
  and native PDF/TeX interaction review remain outstanding.
- Add zoom/pan or tiled rendering for reading large Mermaid graphs at full
  detail, and expose more specific renderer rejection reasons. Preserve the
  [existing diagram limits](mermaid.md#resource-bounds).

These remain tracked independently of M3 development. Local passes do not
certify M2's reference-machine exit requirements.

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
