# Editing encodings and line endings

The headless M3 editing model supports UTF-8, UTF-16LE/BE and the stateless
legacy encodings provided by `encoding_rs`. Text/code editing now uses this backend; GUI encoding commands
are not enabled yet. Ordinary read-mode opening remains unchanged.

## Opening and reopening

`EditBuffer::open_text` and `FileDocument::open` prepare an immutable original on
a worker. BOMs identify UTF-8 and UTF-16. Without a BOM, valid UTF-8 is preferred;
a bounded 128 KiB sample falls back to `chardetng` for legacy detection. Detection
is heuristic, not a guarantee. An explicit `EncodingHint` can select a different
encoding; UTF-16 without a BOM requires that override. A selected encoding that
conflicts with an unambiguous BOM is rejected.

UTF-32 and stateful ISO-2022-JP are not supported for editing. An explicit UTF-16
selection can disambiguate a UTF-16 file beginning with NUL from a UTF-32-looking
prefix. Legacy labels follow the WHATWG mappings used by `encoding_rs` (for
example, ISO-8859-1 selects Windows-1252).

`FileDocument::reopen_with_encoding` prepares a replacement only for a clean
document. The current document remains available while decoding runs. Edits,
newer requests and pending saves prevent stale replacements from being applied;
failed decoding retains the current text. Successful saves retain the saved
encoding for subsequent external reloads.

The lower-level `EditBuffer::open` and `from_bytes` APIs remain raw-byte APIs.
They neither detect encodings nor remove BOMs.

## Preservation and resource use

Editor coordinates use UTF-8 bytes, with indexed Unicode scalar/UTF-16 offsets.
An initial BOM is metadata rather than editable text. For UTF-16 and legacy
files, a separate immutable UTF-8 projection supports cached reads and navigation.
A compact checkpoint per decoded block maps surviving pieces back to original
encoded units. Large originals and projections use private temporary files and
bounded read caches; there is no per-character provenance allocation.

Saving unchanged text copies its original encoded units. This preserves legacy
aliases that decode identically but would be re-encoded differently. Malformed
source bytes display as replacements while retaining their original bytes;
UTF-16 treats a malformed code unit, or an odd trailing byte, as one replacement.
UTF-8's existing raw-byte model retains each malformed byte independently.

New text must be representable without replacement or compatibility substitutions
in the selected output encoding. Converting malformed source bytes to another
encoding fails until those bytes are explicitly removed or replaced. A save also
fails if an edit would join malformed units into a different character—for
example, deleting the separator between an isolated high and low surrogate.
These errors occur while staging, before the destination is changed.

Preparation copies the original, then decodes non-UTF-8 input in another pass.
Preparation progress reserves half for each pass and never moves backward.
Temporary disk use includes both representations; small representations can
remain in memory up to the configured threshold. Working buffers and checkpoint
metadata are bounded by block size and block count. Preparation and serialization
are cancellable between blocks and belong on workers. Read-mode opening does
not pay these costs. The codecs are Rust dependencies with no system converter,
network access or external process requirement.

## Line endings and output settings

LF, CRLF and CR each end one logical editor line, including pairs crossing tree
pieces. Mixed endings are counted separately and preserved by default. Newline
insertion uses the dominant existing convention, with LF then CRLF winning ties
and LF for an empty file. An explicit output convention overrides that choice.

`TextFormat` versions encoding, BOM and optional newline conversion with the
text. Changing it creates an undoable transaction, marks the document dirty and
invalidates stale session commands without moving selections. An older save
completion cannot mark a newer format choice clean.

Newline conversion is applied during serialization. It does not create one edit
per line or rewrite the in-memory coordinates; the original ending counts still
describe the working text, while `TextFormat` describes the output choice. This
keeps conversion cheap even for millions of lines. Undo restores the previous
output policy. A later reload indexes the newly saved byte representation.

Serialization uses bounded input/output buffers and bypasses source read caches.
Writing progress counts working UTF-8 input bytes; commit progress counts actual
encoded output bytes. Both atomic and in-place saves use the staged file's length,
including BOMs and conversion, so truncation and copying remain correct.

## Validation

Tests cover BOM-only documents, explicit and guessed encodings, both UTF-16 byte
orders, malformed surrogates and odd bytes, legacy aliases, multi-scalar Big5
units, UTF-8 scalars split across storage pieces, and random byte roundtrips in
12 encodings. They also check unsafe joins, unrepresentable characters, cancelled
preparation, immutable paged originals, clean/stale reopens, format undo/save
checkpoints, and both save modes when output length differs from working length.
The existing randomized edit model now checks CR/LF/CRLF line coordinates.
The macOS suite passes 82 buffer tests and 7 benchmark tests, along with the
workspace tests and strict Clippy. Buffer tests also compile for Linux and
Windows; those platforms have not been validated natively.

The save diagnostic accepts `--encoding auto` or an explicit label. It edits only
a private copy, inserts a small encoded prefix and verifies every remaining
original byte after atomic and in-place saves:

```sh
cargo test --locked -p plastext-buffer -p plastext-bench
cargo build --release --locked -p plastext-bench
target/release/plastext-bench save path/to/utf16.txt --encoding auto --output target/benchmarks/m3-encoding.summary.json
```

Raw reports and fixtures remain in `target/benchmarks/`. Performance summaries
are recorded separately from the accepted M1/M2 baselines; these headless checks
do not measure native input, IME, painting or keystroke latency.

On an M2 Pro / 16 GiB, macOS 26.4.1, Rust 1.99.0 release build, the
[100 MB UTF-16LE run](benchmarks/m3-2026-11-04-encoding-utf16.summary.json)
measured **1,096.21 ms preparation**, **493.49 ms atomic save** and **574.58 ms
in-place save**, with **12.02 MiB** peak sampled process RSS. The fixture repeats
`日本語😀` followed by CRLF, encoded as UTF-16LE with a BOM; its decoded working
text is about 107 MB. Both representations exceed the in-memory threshold.
Every unchanged original byte passed verification in both save modes.

The [100 MB, 10,000-cycle edit run](benchmarks/m3-2026-11-04-newline-core.summary.json)
measured **0.02504 ms cached-replacement p99**, against **0.02367 ms** in the
previous Unicode-core run. UTF-16 coordinate roundtrip p99 was **0.12521 ms**
(previously 0.12354 ms); sampled peak RSS was **42.52 MiB** (previously 42.39 MiB).
All diagnostic checks passed. These are separate local runs, not controlled
cold-cache measurements or a native typing-latency certification.
