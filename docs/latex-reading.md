# Native LaTeX reading

Files ending in `.tex` offer three modes:

- **Source** edits the document. Save explicitly with Cmd+S.
- **Rendered** reads LaTeX directly with `latex-reader` and renders native text
  and KaTeX math. No compiler, package download, shell command or network access
  is involved. This is the default; `[tex] preview = false` opens Source instead.
- **PDF Preview** compiles saved files with the existing offline pdfLaTeX
  integration. It preserves exact typesetting, package behavior and project
  input discovery. A notice identifies unsaved changes that are absent from the
  compiled PDF. See [PDF and TeX preview](pdf-and-tex-preview.md).

Click a mode; Cmd+Shift+V toggles Source and the last selected reading mode.
Split with Cmd+D or Cmd+Shift+D to edit
Source beside Rendered. Each pane retains its own mode and reading position;
all panes share edits, undo, explicit saving and dirty-file close protection.
Enable **View > Synchronize Scrolling** (or **Sync Scroll** in the status bar) to
link Source and Rendered panes showing the same file. It is off by default and
supports more than two panes. Source alignment uses the existing block source
map; two Rendered panes can also follow positions inside included files. PDF
Preview stays independent until a separate SyncTeX integration is available.
The [example document](fixtures/latex-reading.tex) exercises the three modes.

## Live updates

Visible rendered panes update from immutable unsaved snapshots after a 250 ms
debounce. A new edit cancels obsolete work. Publication checks the current
document version, and the previous completed rendering stays visible during
indexing or after a failed update. Hidden rendered panes do not refresh from
edits; showing one requests the latest version immediately.

Saved changes to included files use the workspace's filesystem watches and
metadata fallback checks. They trigger the same debounced refresh; unchanged
metadata does not rebuild the document. Included files are read from disk;
unsaved edits to an included file in another tab are not yet incorporated.

The PDF compiler is activated only by visible PDF Preview panes. Native
rendering has no dependency on PDF workers or a TeX installation.

## Reading model

LaTeX events map directly to the existing reading view's blocks; no Markdown
text or PDF is generated. Headings and titles, paragraphs, common inline styles,
lists, quotations, code, tables, theorem/proof headings, references, bibliography
labels, and inline/display math use native layout. Formula contexts preserve
scoped definitions, and the reader supplies equation tags independently of
scrolling order. The view reuses zoom, outline navigation, rendered-text find,
selection/copy and the bounded page cache.

Indexing and page parsing run in the background. The index retains source
locations and adapter state at reader checkpoints rather than a complete tree
of rendered blocks. Only requested pages keep parsed text and spans.

## Included files

Local `\input`, `\include`, `\subfile` and `\import` commands share the root
document's macros, labels and numbering. `\lstinputlisting`, `\VerbatimInput`
and `\inputminted` display code, including supported line-range options.
Plain filename macros and `\IfFileExists` select local paths. A file-based
bibliography can read an existing `.bbl`; Rendered mode never runs BibTeX or
generates missing assets.

Files must resolve inside the nearest ancestor Git repository, or inside the
document's directory when there is no repository. Paths through symlinks are
checked against that boundary too. Expansion is limited to 15 levels, 256 file
probes, 32 MiB of included reads and a 32 MiB expanded document. Missing,
unsupported or out-of-bounds inputs keep a visible source fallback.

Source/Rendered navigation maps unchanged root text exactly. Included content
maps back to the root include command; it does not open a child editor. Outline
and rendered-text find also cover included content.

## Current bounds and differences from PDF

- The reader requires a contiguous UTF-8 snapshot, limited here to 32 MiB.
  Source and PDF Preview remain available above the native limit.
- The adapter charges source, blocks and conservative reader/checkpoint
  allowances against a 64 MiB index budget. Reader limits include 2,048
  assignments, 256 KiB of definition bodies, 8,192 labels, 64 nesting levels
  and 2,048 checkpoints. A parsed page is limited to 4,096 blocks / 8 MiB;
  ordinary text blocks split at 16 KiB. The existing parsed-page cache is shared
  with the Markdown reading implementation.
- Local figures with explicit supported paths use the existing image policy.
  Automatic extension lookup, `\graphicspath`, PDF/EPS figures and TikZ are
  not implemented in this adapter.
- Layout is reflowed to the pane. Page breaks, exact spacing, justification,
  font-size declarations and complex table spans are approximated. Footnotes
  remain readable inline. Unsupported commands remain visible; parser
  diagnostics appear in the view's notice.

This mode favors a fast reading/editing loop. The decision to retain or replace
PDF Preview is deferred; all three modes are supported.
