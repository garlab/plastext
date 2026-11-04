# M3 editing

M3 has started with the headless buffer foundation. The app remains a reader:
this increment does not expose editing or write back to user files. M2's open
[validation checks](m2-validation.md) and large-diagram controls can proceed
independently; they do not block work on the editing model.

## Implemented

`plastext-buffer::EditBuffer` stores a persistent AVL piece tree over immutable
original bytes and bounded added chunks. Unchanged subtrees are shared across
versions. Leaves contain at most 64 KiB; byte edits and LF line lookups descend
the tree and scan only the relevant boundary leaves. A trailing LF exposes an
empty final editor line. Invalid bytes and existing line-ending bytes are kept
exactly; the tree does not decode or normalize the document.

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

## Validation

The buffer tests compare 38,400 randomized edit/undo/redo operations against a
byte-vector model, checking retained snapshots, tree balance and line lookup.
Another 20,000 insertions exercise front, middle and end edits. Tests also cover
large leaf boundaries, Unicode/invalid-byte preservation, grouped typing,
history eviction, save checkpoints, concurrent snapshot reads, translated cache
misses, cancellation, atomic path replacement during preparation and external
overwrite/truncation after preparation. The workspace passes strict Clippy;
the buffer also cross-compiles with its tests for Linux and Windows.

Run the core checks and performance diagnostic with:

```sh
cargo test --locked -p plastext-buffer -p plastext-bench
cargo build --release --locked -p plastext-bench
python3 scripts/m2-fixtures.py target/benchmarks/m2-fixtures
target/release/plastext-bench edit target/benchmarks/m2-fixtures/large.md --edits 10000 --output target/benchmarks/m3-edit.summary.json
```

The diagnostic separately times original preparation, prefetch, cached byte
replacement, snapshots, undo and redo. Each cycle replaces one byte at a moving
position, checks undo/redo, and restores the original before the next cycle. It
uses an 8 MiB source cache and retains 64 earlier edited snapshots to verify
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

## Next work and limits

- Add character/UTF-16 counts, grapheme-aware movement and deletion, encoding
  detection/conversion, and CR-only/mixed-ending editing semantics. Byte edits
  currently require the caller to choose valid character boundaries.
- Connect one document session to all its panes and background jobs. Existing
  reading views still use `FileSource` and are not rebuilt for each edit.
- Implement the save/external-change state machine before enabling GUI writes:
  streaming and cancellation, fsync/atomic replacement, permission/xattr and
  symlink/hard-link handling, clean reload and dirty conflict handling.
- Integrate input/IME, editing commands, Markdown synchronization, tail mode and
  `--wait`, then add fuzz targets and native typing/performance tests.

Preparing a large original currently costs a full copy and temporary disk space
proportional to its size; no filesystem-clone optimization is implemented yet.
Snapshot creation after preparation is constant-time. Large history reclamation
can free many nodes; the GUI integration must retire those versions off the UI
thread. None of this preparation is added to ordinary read-mode opening.
