# Markdown memory design

The preview retains a compact source index and a byte-budgeted cache of parsed pages. Visiting more of a document must not retain more parsed text and inline spans indefinitely.

## Requirements

- Index source ranges and block kinds, metadata ownership, heading IDs and equation-number prefixes. Keep global reference definitions so links resolve across page boundaries.
- Parse on background workers, with bounded source windows, queues and caches. Indexing must not construct the full parsed document, even temporarily.
- Preserve the context required by lists, quotes, code fences and tables. Large fences and tables remain virtualized. Any parsing limit must be explicit and retain access to the source.
- Render cached blocks near the viewport and prefetch a margin. Preserve the content anchor when placeholder heights change.
- Search rendered text in independent, cancellable batches, retaining only bounded results. Matches across inline formatting and entities retain their source mappings. Search must not populate the viewport cache.
- Keep outline, anchors, front-matter expansion and equation numbering independent of which pages are cached. Reject work from obsolete document generations.

## Validation

Compare eager and indexed parsing on representative Markdown and boundary cases. Exercise late reference definitions, duplicate heading IDs, Unicode/CRLF, nested containers, large tables/fences, metadata, distant navigation and cache eviction.

Measure peak and retained memory during indexing, cold jumps, revisiting evicted pages and find. Report index, parsed-cache and in-flight memory separately. An approximately 10 MB index for the 100 MB fixture is an estimate, not a total resident-memory target.

After integration, rerun the 10,000-frame native scrolling benchmark, including cold jumps. Keep the accepted performance baseline unchanged and store only benchmark summaries in Git. Span packing is a later optimization if profiles justify it.

## Implementation limits

The index stores 32-byte block records and bounded global metadata. Parsing uses source pages near 64 KiB, extending to 256 KiB to keep a container intact. Large top-level tables and code fences have explicit continuation context. Other constructs exceeding that window, and table headers or fence openers exceeding 64 KiB, show bounded source blocks with a notice. The index is limited to 64 MiB and reference definitions to 16 MiB; larger indexes require Source view. The full-file 128 MiB preview cutoff is removed.

The parsed-page cache is charged conservatively by bytes, capped at 32 MiB. Two page workers may run concurrently. Search uses a separate page at a time and a four-batch result channel, retaining at most 10,000 hits. Page fingerprints reject source changes; document identity and cancellation prevent obsolete results from reaching the view. Copying a complete code fence reads its pages on a worker and has a 32 MiB clipboard limit.

The native benchmark counts a destination only after its real content is available. It alternates the required table/code/prose/end destinations with deterministic distant jumps, records page reloads and placeholder frames, and samples app RSS every 500 ms. Core measurements additionally traverse all pages in both directions and scan rendered text without changing the viewport cache.
