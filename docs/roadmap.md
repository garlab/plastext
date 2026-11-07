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
gloss retains local-file policy, invocation budgets and terminal placement.
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

The [M3 foundation](m3-progress.md) now includes a headless persistent piece tree,
immutable snapshots, bounded grouped undo/redo, Unicode/UTF-16 addressing and
resumable grapheme navigation. A shared document model provides independent
pane selections and rejects stale background commands. The
[save backend](m3-saving.md) supports streaming atomic/in-place saves, metadata,
symlinks and clean reload/dirty conflict handling.
[Encoding support](m3-encoding.md) adds lossless UTF-16/legacy editing originals,
CR/LF/CRLF indexing and undoable encoding/newline output settings.
[Text/code and Markdown Source editing](editing.md) now connects these sessions to split panes, with
caret/selection, native input and composition, shared undo/redo, background saves,
and dirty-file close guards. Markdown previews refresh from unsaved snapshots,
with debounced updates in visible split panes. Existing read-mode opening keeps its original path;
core performance diagnostics remain separate from the accepted baselines.
External-change handling now connects parent-directory watches to clean reloads,
dirty-file banners, disk comparisons, confirmed overwrite, deletion retention
and identity-checked paired renames. [Bounds and remaining limits](m3-saving.md#external-changes-and-explicit-resolution)
include line-based mapping and paged comparisons for large files.
[File management](file-management.md) adds Duplicate, Copy/Cut/Paste, Rename,
Move and system-Bin deletion, with separate navigation-pane focus and guards
for unsaved edits.

Next, in order:

1. Validate native IME, close/quit dialogs and typing latency. Editing commands
   are implemented: multi-cursor, word movement/deletion, find/replace, line and
   indentation commands, EditorConfig, encoding/newline menus and editing settings.
   See [controls and limits](editing.md#editing-commands-and-settings).
2. Add Save As and recovery-copy actions, then validate native platform behavior
   and injected save failures. Debounced external reloads and conflict choices
   are implemented, as is New File. Finish content anchoring on the first reload
   of preview-only documents, which currently retains the reading byte offset.
3. Add continuous Markdown source/preview scroll sync, double-click-to-source, tail mode
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
