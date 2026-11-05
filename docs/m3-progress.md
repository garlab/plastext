# M3 editing

M3 now includes a persistent buffer, shared document model and the first
[text/code editing UI](editing.md), including background saves. M2's open
[validation checks](m2-validation.md) and large-diagram controls can proceed
independently; they do not block work on the editing model.

## Implemented

`plastext-buffer::EditBuffer` stores a persistent AVL piece tree over immutable
original bytes and bounded added chunks. Unchanged subtrees are shared across
versions. Leaves contain at most 64 KiB; byte edits and CR/LF/CRLF line lookups
descend the tree and scan only the relevant boundary leaves. A trailing line
terminator exposes an empty final editor line. Invalid bytes and existing line-ending bytes are kept
exactly; indexing does not normalize or rewrite the document.

`TextSnapshot` implements the existing nonblocking `ByteSource` interface.
Taking a snapshot clones tree/identity handles, not the document. Uncached reads
or edit boundaries return `Missing` with snapshot-relative coordinates; callers
load those ranges on a worker and retry. A failed edit leaves content, revision,
dirty state and undo history unchanged.

Preparing edit mode is a separate, cancellable background operation. Files above
the configured large-file threshold are copied to a private temporary file and
indexed using a 256 KiB copy buffer and compact per-leaf metadata. Subsequent
reads use the existing bounded block cache. Small originals stay in memory.
The source descriptor and path are checked for detectable changes during the
copy; after preparation, external overwrites and truncation cannot mutate the
private original. The original path is never written by this API.

Undo/redo restores tree versions, supports explicit typing groups and retains
history across save checkpoints. History defaults to a conservative 128 MiB
charge for added/removed pieces and copied tree paths; oldest transactions are
evicted first. A transaction exceeding the budget is not retained. Original
storage/read-cache budgets, current content, allocator overhead and snapshots
held by background consumers are separate. Completing a save for an older
snapshot cannot mark newer edits clean, and a save checkpoint cannot belong to
a different buffer.

The tree also indexes Unicode scalar and UTF-16 counts. Coordinate lookups
descend the tree and scan at most one 64 KiB leaf; counts remain correct when a
scalar spans storage pieces. Splits scan only the shorter side and derive the
other count from the original summary. Each malformed UTF-8 byte remains one
independently addressable unit. Text edits reject byte offsets inside valid
scalars, and UTF-16 offsets inside surrogate pairs require an explicit rounding
bias. UTF-16-encoded files use a separate UTF-8 editing projection.

`GraphemeSearch` implements resumable extended-grapheme movement, using the
existing `unicode-segmentation` dependency. Each work unit visits at most 4 KiB
plus scalar-boundary context. Emoji sequences, combining marks, flags, Indic
clusters and CRLF stay together. Very long clusters or preceding context yield
between chunks; cache misses can be loaded on a worker before retrying the same
search. The search retains its immutable version throughout.

`DocumentSession` owns one buffer and gives each attached pane an independent
selection. Edits and grouped undo/redo rebase all pane anchors. Commands carry
both a document generation and a pane-selection generation: changes, pane
closure and edit/undo cycles invalidate obsolete work. Cursor movement and
deletion use the bounded grapheme search and apply only after checking those
generations again. GPUI text/code panes now share these sessions, including undo history and rebased
selections; see [editing controls and current limits](editing.md).

`FileDocument` now adds [streaming saves and external-change handling](m3-saving.md):
atomic replacement, symlink write-through, in-place hard-link saves with recovery
copies, metadata preservation and clean reload/dirty conflict decisions. Save and
reload jobs run separately from the session owner; stale completions cannot clean
newer typing or replace it. Explicit overwrite/discard requests also carry version
checks. The backend is tested independently of GUI commands and watchers.

[Encoding and newline handling](m3-encoding.md) now adds BOM/legacy detection,
lossless UTF-16 and legacy originals, explicit clean reopens, and undoable output
settings. Mixed endings remain intact unless conversion is requested. Format
changes preserve pane selections and share the same stale-command/save checks
as text edits. Conversion failures leave the destination unchanged. The
[encoding diagnostic](m3-encoding.md#validation) records bounded-memory 100 MB
UTF-16 saves and a new edit-core comparison.

## Validation

The buffer tests compare 38,400 randomized edit/undo/redo operations against a
byte-vector model, checking retained snapshots, tree balance and line lookup.
Another 20,000 insertions exercise front, middle and end edits. Tests also cover
large leaf boundaries, Unicode/invalid-byte preservation, grouped typing,
history eviction, save checkpoints, concurrent snapshot reads, translated cache
misses, cancellation, atomic path replacement during preparation and external
overwrite/truncation after preparation. The workspace passes strict Clippy;
the buffer also cross-compiles with its tests for Linux and Windows.

The buffer tests also include 1,200 randomized raw-byte edits checked against
independent scalar/UTF-16 coordinates, every split of representative valid and
malformed strings, and a 1.2 MB grapheme crossing evicted source-cache blocks.
Session tests cover independent selections, grouped history, stale commands,
closed panes, cross-document requests and asynchronous save checkpoints.
The macOS run passes 84 buffer tests and 9 benchmark tests, including save,
conflict and encoding cases. The [save diagnostic](m3-saving.md#validation) verifies
100 MB and 1 GB files in both save modes with bounded streaming memory.

Run the core checks and performance diagnostic with:

```sh
cargo test --locked -p plastext-buffer -p plastext-bench
cargo build --release --locked -p plastext-bench
python3 scripts/m2-fixtures.py target/benchmarks/m2-fixtures
target/release/plastext-bench edit target/benchmarks/m2-fixtures/large.md --edits 10000 --output target/benchmarks/m3-edit.summary.json
```

The diagnostic separately times original preparation, prefetch, cached byte
replacement, snapshots, undo, redo, UTF-16 coordinate roundtrips and one bounded
grapheme poll. Each cycle replaces one byte at a moving position, checks undo/redo,
and restores the original before the next cycle. It uses an 8 MiB source cache
and retains 64 earlier edited snapshots to verify
isolation. Raw timings stay in `target/benchmarks/`; `--output` contains
aggregates only. This does not measure keystroke-to-pixel latency or certify the
reference M1 Air.

The [first 100 MB, 10,000-cycle run](benchmarks/m3-2026-11-04-edit-core.summary.json)
on an M2 Pro / 16 GiB, macOS 26.4.1, Rust 1.99.0 release build measured
**0.00504 ms cached-replacement p99**, with prefetch measured separately at
0.02633 ms p99. Preparing the private original and tree took **473.16 ms**.
Sampled process RSS peaked at **26.84 MiB**, with a final sample included.
All byte/revision/snapshot checks passed. This is a core baseline from one run;
it does not measure sustained typing history, bulk-history reclamation or native
input/painting. Earlier M1/M2 baselines are unchanged.

The [Unicode-core run](benchmarks/m3-2026-11-04-unicode-core.summary.json) repeats
the 100 MB, 10,000-cycle workload on the same machine and toolchain:

| Measurement | Initial byte-only core | Unicode core |
| --- | ---: | ---: |
| Cached replacement p99 | 0.00504 ms | 0.02367 ms |
| UTF-16 roundtrip p99 | — | 0.12354 ms |
| One bounded grapheme poll p99 | — | 0.00329 ms |
| Private original preparation | 473.16 ms | 91.35 ms |
| Sampled process RSS peak | 26.84 MiB | 42.39 MiB |

All coordinate, byte, revision and retained-snapshot checks passed; every
grapheme poll completed within one work unit on this fixture. The added indexes
increase cached edit cost. These are separate single runs, not a controlled
cold-cache comparison: preparation is sensitive to filesystem caches, and RSS
includes the expanded diagnostic and allocator retention. Neither run measures
session selection rebasing, sustained typing history or native input/painting.

The GUI integration adds regression tests for independent split selections,
shared undo, composition grouping and UTF-16 replacement ranges, delayed input,
stale composition rejection, CR/LF/CRLF and empty trailing rows, key dispatch,
horizontal caret visibility, save completion during typing, and disk conflicts.
Snapshot searches bypass the viewport cache. Evicted undo history and superseded
view snapshots retire on workers; the original reader cache is released when
its editing snapshot takes over. Ordinary read-mode opening still prepares no
editing copy.

## Next work and limits

- Extend the initial text/code editing UI with multi-cursor, find/replace,
  word movement, indentation, EditorConfig and editing settings. History currently
  rebases pane anchors rather than restoring historical selections. Native IME
  interaction, close dialogs and keystroke-to-pixel latency still need validation.
- Wire save/reload jobs to debounced filesystem events, conflict/recovery UI and
  new-file/Save As handling. Saves already reject disk conflicts and preserve
  the dirty buffer. Complete native Windows/
  Linux and crash/failure validation; see the [save limits](m3-saving.md).
- Add Markdown synchronization, tail mode and `--wait`, then fuzz targets and
  native typing/performance tests.

Preparing a large original currently costs a full copy and temporary disk space
proportional to its size; no filesystem-clone optimization is implemented yet.
Snapshot creation after preparation is constant-time. Large history reclamation
can free many nodes; GUI sessions now retire evicted versions off the UI thread.
None of this preparation is added to ordinary read-mode opening.
