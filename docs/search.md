# Project search

Open Search with **⇧⌘F**. **Aa** enables case-sensitive matching; **ab** matches
whole words; **`.*`** enables regular expressions. Case matching is off by
default, including for uppercase queries. Invalid expressions report an error.

**File filters** shows **Files to include** and **Files to exclude**. Enter
comma-separated globs, for example:

| Pattern | Matches |
|---|---|
| `*.rs` | Rust files in any directory |
| `src/**` or `./src` | Files under the root's `src` directory |
| `*.{md,txt}` | Markdown and text files anywhere |
| `target, **/*.min.js` | Any `target` directory and minified JavaScript |

Commas inside brace alternatives or character classes are not separators;
escape a literal comma as `\,`. Patterns containing a slash are relative to the
project root; `./` explicitly anchors a pattern there. Exclusions take precedence.
Project ignore files, hidden-file settings and `files.exclude` still apply;
an include pattern does not override them. Hiding the filter fields keeps the
filters active, indicated beside **File filters**.

Results are grouped alphabetically by directory and filename, ignoring case,
with original names breaking ties at each path component. Matching lines remain
in source order. Click a filename row to collapse or expand its matches; collapsed
groups stay closed as more results arrive. Click a match to open that line.
The first page contains up to 10,000 matching lines. **Show more**
resumes the same search and appends another page without clearing results or
moving the current scroll position. “Search paused” means unsearched work
remains; proving that there are no additional matches requires continuing it.

The walker sorts each directory as it reaches it. It does not need to enumerate
or read the whole repository to select a deterministic first page. At most eight
files are searched ahead, retaining at most 256 matching lines per file.
Continuation resumes at a line boundary inside a large file rather than scanning
its earlier contents again. Matching-line display is bounded to 240 bytes around
the first match. Binary files are skipped. Read errors or lines exceeding the
searcher's 8 MiB buffer limit stop that file's scan, and the summary reports how
many files could not be fully searched. Other files continue normally. A changed
partially searched file asks for a new search; this is not a filesystem snapshot. Rerun a search to refresh file listings.

Typing, changing a filter or switching a query option cancels obsolete work.
Literal/regex matching uses ripgrep's library crates. Recent files receive a
ranking boost in Quick Open (**⌘P**) when they match its query.
