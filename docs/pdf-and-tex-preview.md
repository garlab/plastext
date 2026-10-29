# PDF and TeX preview design

Status: accepted direction; the first PDF increment is implemented below. TeX
compilation and package downloads are subsequent increments.

## Scope and modes

PDF files open in Preview, with a persistent Preview/Binary switch and
Cmd+Shift+V shortcut. Binary uses the existing hex reader. Each mode retains its
position. Preview failure must never prevent inspecting the file's bytes.

TeX files will offer Source/Preview. Preview compiles a document to PDF locally;
formula rendering alone cannot reproduce a complete LaTeX document.

## Performance and isolation

No workers, installation discovery, network requests or polling run until a
preview needs them. Measure startup and idle memory against the accepted
baseline, including when no PDF has been opened.

The UI never parses PDF or invokes a TeX engine. PDF parsing/rasterization runs
in a disposable helper process; compilation runs in a separate process tree.
One global compilation slot and bounded raster concurrency prevent every tab
from launching expensive work at once. Pending work is bounded and obsolete
requests are discarded. Helpers run at background priority and are released
when unused. Cancellation must terminate stuck native work, not merely drop a
future waiting for it.

Rasterize visible tiles, with limited prefetch. The initial target is a shared
128 MiB tile budget across windows, tabs and splits, including CPU pixels and
their GPU copies. This is not a total process-memory limit: PDF engines retain
objects, fonts and decoded images independently. Monitor worker memory and
time; stop oversized or stalled jobs with a recoverable error. Sampled memory
limits can overshoot. Set final limits from measurements, with explicit ways
to retry expensive documents rather than silently consuming unlimited memory.

Keep source PDFs file-backed. Render from immutable versions so an external
truncate or replacement cannot invalidate an in-progress render. Snapshot
creation is cancellable, off the UI thread and bounded in disk usage. Do not
keep an entire PDF, every page bitmap or all extracted text in app memory.
Disk caches need quotas and cleanup, including failure/cancellation paths.

TeX generally recompiles the whole document. Its work cannot be proportional
only to the viewport. The UI's existing frame/input budgets still apply while
compiling; compile completion needs its own measured latency budget.

## Platforms and dependencies

Preserve the existing macOS 13+ Apple Silicon baseline, with Intel best effort.
Use system Core Graphics/PDFKit APIs on macOS and verify the oldest supported
OS. PDF viewing needs no TeX installation, browser or network connection.

Keep the backend boundary independent of the UI. Linux and Windows can use a
maintained PDFium build, with JavaScript and XFA disabled. These platforms need
their own dependency, deployment and security validation.

TeX compilation is optional. Initially use an explicitly selected installed
engine with the packages/fonts required by the document. Tectonic is another
optional engine, not a universal replacement for pdfLaTeX, XeLaTeX or LuaLaTeX.

## Rebuild scheduling

Internal buffer revisions and external filesystem events feed one scheduler.
Watch the root document and discovered dependencies (includes, bibliography,
images, styles and generated inputs), excluding our outputs. Coalesce atomic
saves and start after approximately 500 ms of quiet, adapting for long builds.

Keep one active build and only the latest pending revision. On replacement,
terminate and reap the entire obsolete process tree, escalating from a short
grace period to forced termination. Use versioned private working directories
and overlay unsaved buffers. A working directory alone is not an atomic input
snapshot: validate dependency revisions before publication, including inputs
discovered during the build.

Publish only a complete PDF whose inputs still match the current revision.
Keep the last successful PDF visible during rebuilding and errors, with a
small status indicator. Preserve zoom and reading position. Continuous editing
must not cause blank previews or an unbounded cancel/restart loop. Manual
refresh/pause remains available for expensive projects.

## Privacy and security

PDF viewing and compilation are offline by default. Local compilation is
enabled explicitly per project. Never automatically execute project Makefiles,
latexmk configuration, shell commands or document-selected executables. Disable
shell escape and restrict reads to authorized document inputs and installed TeX
resources; writes belong in the private build directory.

Process isolation contains crashes and permits cancellation, but is not by
itself a filesystem/network sandbox. Validate confinement, including bare
`cargo run` development launches, before enabling automatic compilation.
The PDF worker must not execute PDF actions, scripts or embedded programs.

Downloading Tectonic resources is a separate opt-in setup action. Keep the
downloader separate from compilation and deny it access to manuscript contents
and logs. Use trusted, pinned bundles and verified integrity; support local
offline bundles. The provider still sees the client's IP and download requests.
Never silently download after an edit. Tectonic's `--only-cached` and
`--untrusted` options supplement, rather than replace, OS confinement.

## Validation and delivery

1. Isolated PDF viewer, Preview/Binary switching, cancellation, bounded tiles,
   resource controls and stress fixtures.
2. Offline local-TeX compilation with dependency tracking, latest-revision
   scheduling and last-successful-preview behavior.
3. Optional Tectonic setup/downloads after confinement and privacy validation.

Exercise thousands of pages, gigabyte-scale scans, complex vectors/transparency,
embedded fonts, huge page dimensions, extreme zoom, malformed/encrypted files,
external replacement, rapid scrolling, repeated cancellation and many previews.
Measure time to first visible tile, UI frames/input, process-tree peak memory,
GPU/cache charges, disk use and cancellation latency. Small-paper compilation
times are not large-document certification. Keep raw artifacts local and commit
only useful summaries. A difficult page may fail recoverably; it must not make
the application unresponsive.

## References

- [Platform and performance specification](specs.md)
- [Accepted performance baseline](m1-performance.md)
- [Tectonic CLI controls](https://tectonic-typesetting.github.io/book/latest/ref/v1cli.html)
- [PDFium build configuration](https://pdfium.googlesource.com/pdfium/+/refs/heads/main/README.md)

## First PDF increment

PDFs open in a paged preview on macOS. Use the toolbar or Cmd+Shift+V to switch
between Preview and Binary. The page position and hex offset are independent;
both modes read the same snapshot after preview opens. Arrow keys change pages,
Home/End jump to the first/last page, and Ctrl+G accepts a page number. Fit page,
fit width, scrolling/panning and the existing Cmd+/Cmd− zoom levels are available.
Reload (Cmd+R) explicitly reads a new version from disk. Malformed or encrypted
PDFs report an error while preserving access to Binary mode.

The app executable doubles as a private headless worker. It enters worker mode
before GUI initialization and uses Core Graphics to read and rasterize PDF.
One broker serves all previews, with a bounded request queue and one outstanding
request per view. An obsolete request can kill/reap the worker, including during
a native render. Requests have a 10-second deadline and a sampled 512 MiB worker
RSS limit. The child also has a 12-second native-work alarm in case its parent
exits. The worker exits after five seconds idle; the broker then blocks without
polling. Hidden previews and Binary mode cancel pending rendering.

Visible 512-pixel tiles share a 128 MiB cache, charged for CPU pixels plus GPU
copies; atlas/driver overhead and in-flight messages are additional. Active
previews share the budget, reducing raster resolution when necessary rather
than repeatedly evicting each other's visible tiles. GPU atlas entries are
explicitly released on eviction. PDF-native allocations are covered separately
by the worker watchdog, which can miss short memory peaks.

Resizing and zooming immediately scale the displayed tiles. New raster work
starts after 120 ms without a scale change, and a complete replacement viewport
is presented together. Displayed tiles receive eviction priority; each preview
reserves half its cache share for the replacement, keeping both generations
inside the existing 128 MiB budget. Ordinary scrolling and initial loading
remain progressive. Hiding a preview, changing page or reloading clears the
retained presentation so another document or page cannot appear as a fallback.

After the visible page is ready, the viewer caches dimensions and preloads the
first viewport of up to two pages ahead and two behind, prioritizing the direction
of travel. Preloaded tiles use the exact display resolution, so cached page turns
do not briefly show enlarged thumbnails or change text weight. Only complete
viewports that fit the view's remaining share of the 128 MiB cache are admitted;
large viewports or many split panes reduce the number of pages preloaded instead
of lowering their quality. Metadata is limited to five pages. Preloading never
evicts another pane's images; visible work can reclaim speculative tiles and
interrupt their rendering. Resizing, page changes, hiding and Binary mode cancel
obsolete work. Toolbar controls return keyboard focus to the preview, so arrow
navigation remains available after changing the fit mode.

Snapshots use anonymous temporary files, copied in 256 KiB chunks with
cancellation and source-change checks. They have a shared 2 GiB reservation
limit and are unlinked immediately, so a crash cannot leave named cache files.
The worker receives a snapshot descriptor, not a source pathname. Binary reads
use the existing file-backed block cache with an 8 MiB allowance. No complete
page bitmap or document-sized byte buffer is retained by the UI.

The macOS worker denies networking, filesystem writes and child execution using
Seatbelt, failing closed if the restrictions cannot be installed. File reads
remain available for system PDF/font services. This deprecated API is defense
in depth, not the future TeX filesystem sandbox; a signed helper/confinement
strategy still needs validation before automatic compilation. Running inside
an existing OS sandbox can prevent the worker from starting. There is no TeX
execution, package installation or downloading in this increment.

Native tests generate their own PDFs and cover rotation, clipping, Retina
enlargement, BGRA/row orientation, page 10,000, malformed inputs and protocol
limits. Broker tests cover cancellation/reaping and immutable snapshots;
viewport tests cover large pages, centering and resolution under cache pressure.
The headless probe can inspect a local PDF without opening a window:

```sh
cargo run --release -p plastext-platform --example pdf_probe -- /path/to/document.pdf
# Optional bounded first-page thumbnail; keep artifacts outside git:
cargo run --release -p plastext-platform --example pdf_probe -- /path/to/document.pdf target/pdf-preview.ppm
```

Still pending: continuous multipage scrolling, text selection/search, outline
and links, password entry, automatic external-change reload, configurable
resource limits, Linux/Windows rendering and validation on the oldest supported
macOS. Native UI interaction and the large/complex PDF performance matrix remain
acceptance work. A 10,000-page correctness test is not scrolling or memory
certification. The accepted M1/M2 performance baseline is unchanged.
