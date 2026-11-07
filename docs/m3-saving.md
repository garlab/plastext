# Saving and external changes

The M3 file model is implemented in `plastext-buffer`. Its jobs can run on a
worker while input continues against the shared document session. [Text/code
editing](editing.md) uses its save and reload jobs, with filesystem watches and
interactive conflict resolution. Save As and recovery-copy actions remain.

## Saving existing files

`FileDocument::begin_save` captures an immutable snapshot and reserves one save
slot. `SaveJob::run` performs disk work; `finish_save` applies its result without
I/O. Only the saved snapshot becomes the checkpoint, so typing during a save
remains dirty. Undo history survives saving. Dropping an unstarted job releases
the slot; consumers must apply completed jobs rather than discard their results.
Completion methods borrow their jobs so the caller can retire the job snapshots
and any returned buffers together on a worker, avoiding large tree destruction
on the UI thread. Cancel superseded reload jobs to avoid parallel file copies.

The writer uses reusable 64 KiB input/output buffers and bypasses the source
cache. Original bytes, BOMs and line endings are preserved unless output settings
change. [Encoding and newline choices](m3-encoding.md) are versioned with the
snapshot; staging rejects lossy conversions before commit. Progress reports
staging, recovery-copy creation and commit separately. Staging counts working
input bytes; commit counts actual encoded output bytes, including conversion
and BOMs. In-place copying and truncation use the encoded staging file's length.

For ordinary files, saving writes a temporary file in the destination directory,
preserves metadata, syncs the file, replaces the destination and syncs its parent
directory on Unix. Symlinks are resolved to their target and rechecked before
commit; the symlink itself remains intact. A missing file is a conflict, not an
instruction to recreate it silently.

Hard-linked files and `SaveOptions { atomic: false }` use in-place writes. The
complete replacement and a synced pre-save recovery copy are prepared before
writing starts. On success the recovery copy is removed. On failure after commit
starts, its `.plastext-recovery-*` path is returned and retained; the document
stays dirty. The recovery copy contains bytes, not a complete metadata backup.
There is no automatic rollback that could overwrite a newer outside edit.

Cancellation is honored throughout preparation, including the final precommit
check. Once commit starts it finishes even if cancellation arrives. This matters
for in-place writes: cancelling halfway would leave a partial document. The
commit runs on a worker and continues reporting progress. A crash or disk error
during in-place commit can leave partial contents; the recovery copy and retained
document snapshot are the recovery sources. Recovery UI is still pending.

## Metadata and platforms

Unix saves preserve ownership, mode and readable extended attributes; macOS also
copies native ACLs. Metadata that cannot be preserved causes an error before
replacement. Attribute names are limited to 64 KiB and individual values to
8 MiB; unusually large resource forks are rejected instead of allocating without
a bound or silently dropping metadata. Device files, directories and FIFOs are
not save destinations.

Windows uses
[`ReplaceFileW`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-replacefilew)
to merge the original file's ACLs and named streams. Merge failures are not
ignored, and a backup is retained if replacement reports failure after moving
the original. File data is synced; Windows has no portable directory-fsync
equivalent in this implementation. Linux and Windows builds are checked here;
native filesystem behavior on those platforms still needs CI/runtime validation.

## Conflict checks

The disk checkpoint includes the resolved path, file identity, length, change and
modification times, link count and permissions. Saves compare it before staging
and immediately before commit, including both the open handle and the current
path. Edit preparation uses the same handle-based identity and metadata checks.
Same-length rewrites with restored modification times are detected when the
filesystem reports a different change time; replacing the path is detected
through file identity. A postcommit check catches changes observed before the
result is returned.

These are optimistic checks, not a filesystem transaction shared with other
editors. Locks serialize cooperating writers, but an outside writer can still
act between the final comparison and replacement; in-place writes are also
visible while they run. Timestamp resolution and delayed or weak metadata
updates can hide same-length rewrites that restore the modification time. These
checks do not guarantee protection against concurrent writers or power loss.
Uncertain outcomes retain dirty state and require a new disk check. Crash/failure injection and network-filesystem validation remain
release-hardening work.

## External changes and explicit resolution

After debouncing watcher events, create `external_check`, run it on a worker and
apply it with `finish_external`:

- Unchanged disk versions do nothing, including events from the app's own save.
- A clean document prepares an immutable replacement, then installs it only if
  its original session generation is still current. Pane selections and scroll
  positions are mapped to the new document; old contents/history are returned
  for off-thread retirement.
- A dirty document retains all edits and reports a conflict. Missing files also
  retain their in-memory contents.
- Checks overlapping a save are deferred. Older results are rejected after a
  newer check, save, reload or an edit/undo cycle. Schedule a fresh check after
  deferred/stale results and after a save completes so coalesced watcher events
  are not lost.

Conflict resolution is explicit. `begin_overwrite` takes the disk version the
user chose to replace and still rejects a later outside change.
`reload_discarding_edits` accepts a user-selected discard, but newer typing
invalidates that request. Neither action is selected automatically.

The application watches parent directories, so atomic replacements do not detach
the watch. Registration and teardown run off the UI thread. Events are coalesced
with a 300 ms document debounce, cancelled reloads retire their results on a
worker, and only one reload copy per document runs at a time. A two-second
metadata fallback catches lost events and unavailable watchers. Watches cover
open text/code and Markdown documents and resolved symlink targets, plus the
navigation tree's root and expanded folders. Renames, additions and removals
refresh affected directory listings in the background, even with no tabs open.
Unchanged directory polls only check metadata; collapsed folders refresh when
reopened. This avoids recursively watching or scanning the repository. Split
panes share the document and reload.

For versions up to 8 MiB, bounded patience line diffs map unchanged content and
trim shared bytes within changed regions. Diff work is capped at 100,000 lines
per version and one million line visits; ambiguous or larger repeated-line
regions become a single replacement. Larger documents map the registered pane
positions by line and byte column, loading only required cache blocks. Moving
to an unmapped position during preparation makes that large reload stale and
requests a fresh check. Markdown preview-only opening keeps its existing lazy
path; the first external update prepares an immutable version and retains the
reading byte offset. Subsequent snapshot-based reloads remap the reading anchor.

Dirty-file banners offer comparison, explicit discard/reload and Keep Mine.
Keep Mine dismisses the current warning; it never changes the save checkpoint.
Save then asks for Overwrite, bound to the reviewed disk version. A new outside
write still aborts it. Comparisons are read-only snapshots: unified diffs up to
8 MiB, otherwise two paged views. Deletion keeps the document and pins a preview
tab. Paired renames are followed only after checking the destination's identity
and that the old path is gone; unpaired moves show the missing-file banner.
Save As, recreation of deleted files and recovery-copy actions remain pending.

## Validation

Tests cover byte-preserving streaming, cache isolation, cancellation, concurrent
rewrites, path replacements, deletion, symlink retargeting, hard links, metadata,
native macOS ACLs, oversized resource forks and retained recovery copies. Model
tests cover serialized saves, continued typing, checkpoints/undo, clean reloads,
dirty conflicts, missing files and stale/explicit resolution requests.

The headless diagnostic writes only a temporary copy of its input and checks
every unchanged byte after both atomic and in-place saves:

```sh
cargo build --release --locked -p plastext-bench
target/release/plastext-bench save target/benchmarks/m2-fixtures/large.md --output target/benchmarks/m3-save.summary.json
```

Generated inputs and raw reports remain under `target/benchmarks`. The shareable
output contains aggregate timings and sampled process RSS. It does not measure
GUI responsiveness, certify cold-cache behavior or prove crash durability.

On an M2 Pro / 16 GiB, macOS 26.4.1, Rust 1.99.0 release build, the local runs
added one line and verified every remaining byte after each save:

| Fixture | Atomic save | In-place save, including recovery copy | Sampled peak RSS |
| --- | ---: | ---: | ---: |
| [100 MB Markdown](benchmarks/m3-2026-11-04-save-100mb.summary.json) | 62.72 ms | 146.67 ms | 11.14 MiB |
| [1 GB log](benchmarks/m3-2026-11-04-save-1gb.summary.json) | 1,018.33 ms | 2,468.36 ms | 16.77 MiB |

Each mode was measured once per fixture. Original preparation and byte
verification are timed separately in the summaries; the 1 GB fixture contains
1,000,000,086 bytes because the generator finishes its final log line. RSS covers
preparation, both saves and verification, with a 100 ms sampling interval and a
final sample. These are local diagnostics with filesystem caching, not reference
machine or GUI exit-test results. Existing read-mode baselines are unchanged.
