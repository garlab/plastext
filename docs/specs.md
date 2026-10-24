# plastext

**A fast, native, read-first viewer and light editor for code, Markdown and assets.**

plastext is for people who now write most of their code through LLM agents in the terminal. You still need to *read* the code, *review* what changed and *tweak* the odd line, but an IDE is more than that job needs. plastext does those three things, natively, and stays fast whatever the size of the file or repository.

> **Status:** specification, draft v0.1 (2026-10-24). Nothing is implemented yet.

---

## Contents

1. [Why](#1-why)
2. [Principles](#2-principles)
3. [Scope](#3-scope)
4. [User experience](#4-user-experience)
5. [Functional specification](#5-functional-specification)
6. [Performance requirements](#6-performance-requirements)
7. [Architecture](#7-architecture)
8. [Security and privacy](#8-security-and-privacy)
9. [Testing and benchmarking](#9-testing-and-benchmarking)
10. [Roadmap](#10-roadmap)
11. [Open questions](#11-open-questions)
12. [Appendix: UI stack alternatives](#appendix-ui-stack-alternatives)

---

## 1. Why

Coding has moved into the terminal. The agent writes and edits code, and the human reads, reviews and sometimes corrects it. IDEs are built for writing, with autocomplete, refactoring, debuggers and extension ecosystems, and they pay for it in startup time, memory and screen space. Once writing goes to the agent, three jobs remain:

- **Read** code and docs comfortably: rendered Markdown with math, highlighted code, images.
- **Review** what changed, live while the agent works, and across git history.
- **Tweak** a line, a heading or a config value without starting a heavy editor.

plastext does these jobs and leaves everything else out.

## 2. Principles

1. **Read first.** Files open for reading. Editing is a deliberate switch, one keystroke away. A `.md` file opens rendered.
2. **Never block the UI thread.** Every IO, parse, search and git operation runs asynchronously, can be cancelled and streams its results. The UI draws whatever is ready and fills in the rest as it arrives.
3. **Cost follows what's on screen, not what's on disk.** A 10 GB file opens as fast as a 10 KB file, and a 1M-file repository opens as fast as a 10-file one.
4. **Native, with no web engine.** No Electron, Tauri, WebView or embedded JS runtime anywhere, including the Markdown and math renderers.
5. **The disk is shared.** Agents change files constantly. External changes are the normal case, not an edge case.
6. **Small surface.** Every feature must serve reading, reviewing or light editing. There is no plugin system.

## 3. Scope

### 3.1 Goals (v1)

- View and edit **Markdown** with high-quality rendering, including **math** using MathJax/KaTeX-compatible TeX syntax.
- View and edit **code** with syntax highlighting for about 45 languages.
- View **images and static assets**, with a hex view for any binary file.
- **Git-aware**: status, live diff review, history, blame and comparing revisions.
- Native performance on **macOS**, with an architecture that ports to Linux and Windows.
- Stay fast in **extreme cases**: multi-GB files, files with millions of lines, single-line files of hundreds of MB, and repositories with 1M+ files or 1M+ commits.

### 3.2 Non-goals

- Terminal integration: no embedded terminal, task runners or build/run buttons.
- LLM/AI integration of any kind: no chat, completions or agents.
- LSP, autocomplete, IntelliSense, refactoring, debuggers or linters.
- A plugin/extension system or marketplace.
- Remote development (SSH, containers). Use a network mount if needed.
- Real-time collaboration.
- Git write operations in v1 (commit, stage, push, rebase…). Use the terminal; see [open questions](#11-open-questions).
- WYSIWYG rich-text editing of Markdown in v1. v1 offers source view and rendered view kept in sync.
- Full HTML/CSS rendering inside Markdown. Only a safe subset is rendered.

### 3.3 Platforms

| Tier        | Platform                   | When    |
| ----------- | -------------------------- | ------- |
| 1           | macOS 13+ on Apple Silicon | v1      |
| 2           | Linux (Wayland and X11)    | post-v1 |
| 2           | Windows 10+                | post-v1 |
| Best effort | macOS 13+ on Intel         | v1      |

**Intel Macs** are supported only as long as they cost nothing. An Intel build ships if it compiles from the same code with no Intel-specific paths, workarounds or tuning. It's a separate download, not a universal binary, so Apple Silicon users don't pay for it in bundle size. Performance budgets (§6) and CI benchmarks cover Apple Silicon only. Intel-only bugs are low priority, and Intel support is dropped the moment it forces a tradeoff.

**Portability rule:** OS-specific code sits behind traits in a `platform` crate. All UI-agnostic crates must build and pass tests on all three OSes from day one (CI matrix), long before the UI ships there.

### 3.4 Requirement conventions

Requirements have IDs (`MD-3`, `GIT-4`…) so they can be referenced in issues, commits and prompts. Every requirement is **MUST for v1** unless it carries a tag:

- *(should)*: wanted for v1, but can slip.
- *(later)*: post-v1. It's listed so the architecture doesn't rule it out.

---

## 4. User experience

### 4.1 Layout

```
┌──────────────────────────────────────────────────────────────────────────┐
│ ◀ ▶   ~/projects/foo                         main ↑2 ●3      ⌘P  ⇧⌘F     │
├────────────┬──────────────────────────────────────────────┬──────────────┤
│ Files      │ README.md │ src/lib.rs ● │ Changes (3)       │ Outline      │
│ Changes 3  │──────────────────────────────────────────────│              │
│ History    │                                              │ # foo        │
│ Search     │   # foo                                      │   ## Install │
│────────────│                                              │   ## Usage   │
│ ▾ src      │   A tiny library that does one thing well.   │   ## Math    │
│   lib.rs M │                                              │              │
│   main.rs  │   ## Math                                    │              │
│ ▸ docs     │        ∫₀^∞ e^{-x²} dx = √π / 2              │              │
│   README.md│                                              │              │
├────────────┴──────────────────────────────────────────────┴──────────────┤
│ READ │ Markdown │ UTF-8 │ LF │ Ln 12, Col 4 │ main │ ⟳ following         │
└──────────────────────────────────────────────────────────────────────────┘
```

- **Sidebar** with tabs: Files, Changes, History and Search. It can be collapsed.
- **Editor area** with tabs and splits (horizontal and vertical).
- **Outline panel** on the right (*should*): Markdown headings or code symbols. It can be collapsed.
- **Status bar** showing the mode, language, encoding, line endings, position, branch and follow state.

### 4.2 Read mode and edit mode

| | Read mode (default) | Edit mode |
|---|---|---|
| Markdown | Rendered | Source, or split source + rendered |
| Code | Highlighted, selectable, copyable, no caret | Full caret/selection editing |
| Keys | Single-key navigation (below) | Standard macOS text-editing keys, including Emacs-style `⌃A`/`⌃E` |
| Enter / leave | Opening a file lands here; `Esc` returns here when nothing else is active | `e`, `↩`, or double-click on content |

- A buffer with unsaved changes stays in edit mode.
- `editor.open_mode = "edit"` lets users who prefer it open files straight into edit mode.
- Typing a printable key in read mode shows a brief hint ("press `e` to edit") and doesn't change the file.

**Read-mode keys** (defaults, rebindable; they never conflict because read mode doesn't accept text):

| Key | Action | Key | Action |
|---|---|---|---|
| `j` / `k`, `↓` / `↑` | scroll line | `space` / `⇧space` | page down / up |
| `g` / `G` | top / bottom | `/` | find |
| `n` / `N` | next / previous match | `]` / `[` | next / previous git change |
| `o` | toggle outline | `b` | toggle blame |
| `d` | diff this file against HEAD | `e` / `↩` | edit here |
| `f` | toggle follow mode | `⌃-` / `⌃⇧-` | back / forward |

### 4.3 Core workflows

1. **"What did the agent just do?"** The Changes view shows every uncommitted change in the repo as one scrollable review page. It updates live while the agent writes. You can tick files off as viewed.
2. **"Watch the agent work."** Follow mode opens whichever file just changed on disk and scrolls to the changed lines.
3. **"Read this project."** `plastext .` → rendered README → follow relative links → fuzzy-open files → outline → read.
4. **"Fix one line."** From any view or diff press `e`, edit, `⌘S`. The diff updates.
5. **"What's on this branch?"** Compare the branch against its merge base with `main`, laid out like a pull request.
6. **"Look at this log."** Open a 5 GB log instantly, jump to a line, search, and follow it as it grows.
7. **From the terminal:** `plastext src/lib.rs:120`, `plastext diff`, `git log -p | plastext -`.

---

## 5. Functional specification

### 5.1 Projects and file tree

- **PRJ-1** Open a folder (a project) or individual files. Multiple windows, one project per window.
- **PRJ-2** The tree loads lazily: only expanded directories are read. A directory with 500k entries lists progressively and scrolls smoothly (virtualized).
- **PRJ-3** Respects `.gitignore`, `.ignore`, `.git/info/exclude` and the global gitignore. Ignored files can be hidden, dimmed (default) or shown. Hidden files are toggleable. `files.exclude` globs are always hidden.
- **PRJ-4** Git status decorations (modified, added, deleted, renamed, untracked, conflicted) on files, rolled up to their ancestor directories.
- **PRJ-5** Filesystem watching: one FSEvents stream per root on macOS. On Linux, inotify watches expanded directories, open files and `.git` only, with polling as a fallback (inotify limits don't scale to 1M directories). On Windows, ReadDirectoryChangesW. Events are coalesced and debounced, and the tree, open buffers and git status update automatically.
- **PRJ-6** File operations: new file or folder, rename, move (drag), delete to Trash, duplicate, reveal in Finder, copy absolute or relative path.
- **PRJ-7** Symlinks are shown and followed on open; directory cycles are detected and never recursed. Works correctly on case-insensitive filesystems.
- **PRJ-8** Session restore: windows, tabs, splits, scroll positions and the mode of each tab.
- **PRJ-9** Tabs beyond a configurable count keep only metadata. Their content is dropped and reloaded on focus.

### 5.2 Text engine

The foundation, which every text view (code, Markdown source, diffs, logs) shares.

- **TXT-1** First paint for any file is independent of file size: only the bytes the viewport needs are read.
- **TXT-2** **Storage.** Files up to `limits.large_file_threshold` (default 64 MB) are read fully into memory. Larger files go through a **chunked reader** (`pread` of e.g. 256 KB blocks) with a bounded LRU block cache. There is **no whole-file mmap**, so a file truncated externally can't trigger SIGBUS.
- **TXT-3** **Buffer.** A piece tree (a balanced tree of pieces over the original file plus an append buffer) whose nodes carry byte, line-break and char/UTF-16 counts. Edits and offset↔line↔column conversions are O(log n). Edits never copy the original file.
- **TXT-4** **Snapshots.** A buffer hands out cheap immutable snapshots (persistent tree, `Arc` nodes). Background tasks (highlighting, search, save, diff) read snapshots without locking.
- **TXT-5** **Line index for large files.** Built by a background SIMD newline scan (`memchr`) and streamed. The index is sparse (a line count per block), so memory is O(file size / block size): about 16k entries for 1 GB. Until the index is complete, the scrollbar is estimated from byte position and unindexed regions show byte offsets instead of line numbers. Jumping to a line beyond the indexed prefix shows progress and stays responsive.
- **TXT-6** **Long lines.** Only the visible horizontal slice of a line is laid out. Beyond `limits.long_line_threshold` (default 10k chars), wrap rows are computed lazily and highlighting stops for the rest of that line. A 500 MB single-line file must scroll and wrap smoothly.
- **TXT-7** **Encoding.** BOM detection; UTF-8 by default; UTF-16 LE/BE; heuristic fallback for legacy encodings (`chardetng` + `encoding_rs`). Invalid bytes show as replacement glyphs but **round-trip byte-for-byte** on save. Reopen-with-encoding command.
- **TXT-8** Line endings are detected (LF, CRLF, CR, mixed) and preserved. A command converts them.
- **TXT-9** **Binary detection** (a NUL byte in the first 8 KB, or a high ratio of invalid UTF-8) routes the file to the asset viewer or the hex view. The user can force text view.
- **TXT-10** **Saving** is atomic: write a temp file in the same directory, fsync, rename. Permissions and extended attributes are preserved, and symlinks write through to their target. Hard-linked files are written in place (`editor.atomic_save = false` forces in-place writes). Large files stream to disk with progress, can be cancelled, and never block the UI.
- **TXT-11** Undo/redo with coalescing of typing runs. History is bounded by a memory budget and survives saves (not restarts).
- **TXT-12** Unicode: grapheme-cluster-aware cursor movement and deletion, wide (CJK) characters, emoji, configurable tab width, visible control characters. IME composition (CJK input) works in edit mode. Bidirectional text is *(later)*.

### 5.3 Code: viewing and editing

- **CODE-1** **Syntax highlighting** with tree-sitter: incremental re-parse on edit, highlighting only the visible range plus a margin. Grammars and queries are **statically linked**, with no runtime downloads. Language injections cover at least Markdown fences, HTML `<script>`/`<style>`, and Vue/Svelte blocks *(should)*.
- **CODE-2** **Budgets.** Parsing is time-sliced and never stalls a frame. Above `limits.syntax_max_file_size` (default 32 MB), a fast line-local lexical highlighter takes over (keywords, strings, comments and numbers from a per-language table), and huge files fall back to no highlighting.
- **CODE-3** **Initial languages:** Rust, C, C++, Objective-C, Swift, Go, Zig, Python, JavaScript, TypeScript, TSX/JSX, Java, Kotlin, Scala, C#, Ruby, PHP, Lua, Elixir, Erlang, Haskell, OCaml, Dart, R, Julia, Nix, Bash/Zsh/Fish, SQL, HTML, CSS/SCSS, JSON/JSONC, YAML, TOML, XML, Markdown, Dockerfile, Makefile, CMake, HCL/Terraform, Protobuf, GraphQL, diff/patch, git config/commit/rebase, INI/.env.
- **CODE-4** **Language detection** by filename, extension, shebang, Vim/Emacs modelines and `.gitattributes` (`linguist-language`). Manual override per buffer.
- **CODE-5** **Gutter:** line numbers, git change markers (GIT-3), fold markers. Folding uses tree-sitter, with indentation-based folding as a fallback.
- **CODE-6** **Outline:** symbols from tree-sitter tag queries (functions, types, impls, classes), used for the outline panel and "go to symbol in file".
- **CODE-7** **Sticky scroll** *(should)*: the enclosing scopes stay pinned at the top while reading.
- **CODE-8** **Editing:** caret and selection (mouse, keyboard, shift-click, double-click word, triple-click line), multiple cursors (`⌥`-click, `⌘D` add next occurrence), column selection *(should)*, find/replace in the file (literal or regex, case, whole word, in selection), auto-indent, indent/outdent, toggle comment, move line up/down, duplicate line, bracket matching, auto-closing pairs (off by default), soft-wrap toggle, show whitespace.
- **CODE-9** **Formatting conventions:** indentation detection (tabs vs spaces, width) and EditorConfig support (indent, charset, end of line, trim trailing whitespace, final newline).
- **CODE-10** `⌘`-click opens file paths (`src/foo.rs:42:7`) and URLs found in any text.
- **CODE-11** *(later)* Project-wide symbol index from tree-sitter tags, giving search-based "go to definition" by name, with no LSP.

### 5.4 Markdown

- **MD-1** **Syntax:** CommonMark plus GFM: tables with alignment, task lists, strikethrough, autolinks, footnotes, heading IDs/attributes, GitHub alerts (`> [!NOTE]`, `[!TIP]`, `[!IMPORTANT]`, `[!WARNING]`, `[!CAUTION]`), and definition lists, superscript and subscript *(should)*. Emoji shortcodes *(should)*. Wikilinks *(later)*. Extensions `.md`, `.markdown`, `.mdown`, `.mkd`; `.mdx` renders with JSX shown as source.
- **MD-2** **Front matter** (YAML `---` or TOML `+++`) renders as a collapsible key/value table.
- **MD-3** **Math**, MathJax/KaTeX-compatible:
  - Delimiters: inline `$…$` and `\(…\)`; display `$$…$$`, `\[…\]` and `` ```math `` fences.
  - Coverage target: the KaTeX supported-functions list, including `align`/`aligned`, `gather`, `cases`, `matrix`/`pmatrix`/`bmatrix`/`vmatrix`, `array`, `\frac`, `\sqrt`, `\sum`/`\int` with limits, `\mathbb`/`\mathcal`/`\mathfrak`/`\mathrm`, `\text`, `\operatorname`, accents, the AMS symbol set, `\tag`, `\newcommand`/`\def` macros scoped to the document, and `\color`.
  - Rendered **natively** as vector glyph layout using an OpenType MATH font (bundled, e.g. STIX Two Math or Latin Modern Math). Selectable as source text; copy yields the TeX.
  - Laid-out formulas are cached by (expression, font size, theme).
  - Errors never break the document: the formula's source shows in an error color with the message on hover.
  - Engine choice is a spike; see [§7.2](#72-key-libraries).
- **MD-4** **Code fences** use the code view's highlighter (CODE-1), with a language label and a copy button.
- **MD-5** **Images:** relative and absolute local paths, data URIs, SVG. Decoding is lazy and downscaled to display size. **Remote images are blocked by default**: a per-document "Load remote images" action is available, plus a `markdown.remote_image_hosts` allowlist.
- **MD-6** **Links:** relative links to other files open in plastext (`./other.md`, `other.md#heading`, `src/lib.rs#L42`). `#anchor` links scroll within the document. External URLs open in the default browser, and hovering shows the target. Headings get a copy-anchor-link affordance.
- **MD-7** **Inline HTML, safe subset:** `<details>`/`<summary>`, `<img>` (`width`, `height`, `align`), `<picture>`/`<source>` (picks the theme-matching source), `<br>`, `<sub>`, `<sup>`, `<kbd>`, `<mark>`, `<a>`, `<b>`/`<strong>`/`<i>`/`<em>`/`<code>`, `<p align>`/`<div align>`, `<hr>`, and HTML comments (hidden). Any other HTML shows as dimmed source. Nothing executes: no scripts, styles, iframes or forms.
- **MD-8** **Views:** *Rendered* (read-mode default), *Source* (edit-mode default) and *Split*, which places source and preview side by side with **two-way scroll sync** driven by source maps. **Double-clicking a rendered block** enters edit mode with the caret at the matching source position.
- **MD-9** **Live preview:** in split view the preview updates on every keystroke by re-parsing only the affected top-level blocks. Edit-to-preview takes ≤ 16 ms for a 1 MB document.
- **MD-10** **Task lists** can be toggled by clicking in rendered view. The click edits the source and marks the buffer dirty; it doesn't auto-save.
- **MD-11** **Outline/TOC** from headings, with scroll-spy highlighting the current section.
- **MD-12** **Block virtualization for large documents.** Parsing streams the document into top-level blocks, each with its byte range. Only blocks near the viewport are laid out. Heights of off-screen blocks are estimated and corrected when measured, and scroll position is anchored to content so corrections never make the view jump. The first screen renders as soon as its blocks are parsed. Later fixes, such as link reference definitions found further down, re-render in place.
- **MD-13** **Typography:** a proportional body font, a monospace font for code, and a readable measure (`markdown.max_width`, default `80ch`, centered). The default style resembles GitHub's rendering, in light and dark.
- **MD-14** Find in rendered view searches the rendered text and maps hits back to the source.
- **MD-15** Pathological input (nesting thousands of levels deep, a 100k-row table) is handled with bounded recursion and virtualized table rows. No stack overflow, no stall.
- **MD-16** *(should)* "Copy as HTML" and "Copy as rich text" for a selection or the whole document.
- **MD-17** *(later)* Mermaid diagrams rendered natively. Until then, `mermaid` fences display as highlighted code.
- **MD-18** *(later)* Export to PDF and print.

### 5.5 Images and static assets

- **AST-1** **Raster images:** PNG, JPEG, GIF (animated), WebP (animated), BMP, ICO, TIFF, AVIF, and HEIC (macOS through ImageIO). Zoom to fit, 1:1, or by pinch and scroll; pan; pixel grid at high zoom; transparency checkerboard; info panel (dimensions, file size, color profile, EXIF *(should)*).
- **AST-2** **Huge images** (e.g. 40k × 40k) decode with subsampling for the overview and **tiles** generated in the background for deep zoom. The decoded-pixel limit guards against decompression bombs.
- **AST-3** **SVG** rendered natively (`resvg`). Read mode shows the image and edit mode shows the XML source.
- **AST-4** **Hex view** for any file of any size: virtualized offset/hex/ASCII columns, go to offset, selection and copy (hex or raw). Read-only in v1.
- **AST-5** *(should)* **CSV/TSV table view**: virtualized rows, column widths sampled from the first rows, sticky header, toggle back to raw text. Works on files with 20M lines through the chunked reader.
- **AST-6** *(should)* **PDF** page rendering (PDFKit on macOS; pdfium elsewhere).
- **AST-7** *(later)* Font specimen view (TTF/OTF/WOFF2), audio/video playback through the platform player, JSON tree view.
- **AST-8** Unsupported binaries show file metadata plus a hex view and "Open with default app".

### 5.6 Search and navigation

- **NAV-1** **Quick Open** (`⌘P`): fuzzy file finder over the whole project. The file index builds in the background and results stream in, so it's usable immediately and fills in progressively. Matching is nucleo-style; ranking combines match score, recency and path proximity. Accepts `path:line:col`. Each keystroke takes ≤ 16 ms with 1M files.
- **NAV-2** **Project search** (`⇧⌘F`): literal or regex, case, whole word, include/exclude globs, honors ignore files. Results stream in grouped by file in a virtualized list, a new query cancels the previous one, and results are capped with "show more". Built on ripgrep's library crates.
- **NAV-3** **Command palette** (`⇧⌘P`) lists every command with its keybinding.
- **NAV-4** **Go to line** (`⌃G`) and **go to symbol in file** (`⇧⌘O`).
- **NAV-5** **Back/forward history** across files and positions: `⌃-`/`⌃⇧-`, mouse buttons, trackpad swipe.
- **NAV-6** **Find in file** (`⌘F`) with incremental highlighting of every match. On huge files, matching streams in the background and shows a running count.

### 5.7 Git

Git support is for **visualizing**. v1 never writes to a repository.

- **GIT-1** **Discovery:** finds the repo root above the opened folder. Nested repos, submodules and worktrees are each tracked.
- **GIT-2** **Status** of the working tree and index, computed in the background. It refreshes on filesystem events (debounced) and on changes under `.git` (HEAD, index, refs). With a million files the UI never blocks: it shows the last known status with a refresh indicator and uses the untracked cache or fsmonitor data when present.
- **GIT-3** **Gutter markers** for added, modified and deleted lines compared with the index (default) or HEAD (`git.gutter_base`). Clicking a marker peeks the hunk inline *(should)*.
- **GIT-4** **Changes view (key screen).** All uncommitted changes as one scrolling **review page**:
  - Sections per file, collapsible and grouped into *staged* and *unstaged*. Untracked files appear as additions and renames are detected. Binary and image changes render as asset diffs (GIT-7).
  - Updates live as files change on disk.
  - **"Viewed" checkbox per file**, stored locally per repo and keyed by path plus content hash, so it clears when the file changes again.
  - Keys: `n`/`p` next/previous hunk, `]`/`[` next/previous file, `v` toggle viewed, `e` edit at this line, `↩` open the file here.
  - Lazy: with 50k changed files the list appears immediately and diffs are computed as sections scroll into view.
- **GIT-5** **Diff viewer:** unified and side-by-side; syntax-highlighted on both sides; word-level highlighting within lines; unchanged regions collapse and expand on demand; ignore-whitespace toggle; histogram diff by default, Myers as an option. A 100k-line diff scrolls smoothly. Above a size threshold it falls back to a line-hash diff without intra-line highlighting and says so.
- **GIT-6** *(should)* **Rendered Markdown diff:** a diff in rendered view, with inserted and removed blocks and word changes inline.
- **GIT-7** **Image diff:** side-by-side, swipe, onion skin and pixel-difference modes.
- **GIT-8** **History:** repository log with a branch graph, ref labels (branches, tags, HEAD, remotes) and filters (author, message, path, date). Virtualized and loaded incrementally: the first page appears in ≤ 100 ms with 1M commits, using commit-graph files when present.
- **GIT-9** **Commit view:** message, author, committer, dates, parents, changed files and diffs, rendered with the same review page as GIT-4.
- **GIT-10** **File history:** the log for one file, following renames. Pick any two revisions to diff, or open the file **at any revision** as a read-only buffer with all view features.
- **GIT-11** **Blame:** gutter annotations (author, relative date, summary) colored by age. Click to open the commit; "blame parent" re-runs blame at the commit's parent. Computed in the background and displayed progressively.
- **GIT-12** **Compare:** any two refs or commits; **branch vs. merge base** (pull-request style, e.g. `feature` vs `main`); working tree vs. any ref.
- **GIT-13** Conflicted files are flagged and conflict markers highlighted. Browsing branches, tags and stashes *(should)*.
- **GIT-14** **Lock-free and side-effect-free.** plastext never takes `index.lock` or any other git lock, never runs hooks, and never refreshes and writes the index. Agents run git concurrently, and plastext must never be the cause of "another git process seems to be running".

### 5.8 External changes and follow mode

- **EXT-1** An **unmodified** buffer reloads automatically when the file changes on disk. Scroll position and selection stay anchored to content (remapped through a diff for normal files, by line for large ones).
- **EXT-2** A **modified** buffer gets a non-modal banner, "Changed on disk", with *Show diff* (disk vs. buffer), *Reload* (discard mine) and *Keep mine*. Saving over a file that changed since it was loaded asks for confirmation. A three-way merge using the last-loaded content as the base is *(later)*.
- **EXT-3** If a file is **deleted or renamed** on disk, its buffer stays open with a banner. When the watcher pairs the events as a rename, the buffer follows the new path.
- **EXT-4** **Tail mode** for growing files: auto-scroll while the view is at the bottom, indexing only the appended bytes. Truncation or rotation is detected and the file reloads.
- **EXT-5** **Follow mode** (per window, `f`): when a project file changes on disk, plastext focuses or opens it and scrolls to the first changed hunk, in the normal view or a diff (`follow.view`). Changes are debounced (default 300 ms). Ignored files and `.git` don't trigger it. It pauses while the user is interacting (input in the last few seconds) and shows in the status bar.
- **EXT-6** **Recent activity** list (session-scoped, newest first) of files changed on disk. Each entry opens a diff against the version plastext last saw (kept in a bounded in-memory snapshot cache for open and small files), or against HEAD when no snapshot exists.

### 5.9 Command line and OS integration

- **CLI-1** `plastext [paths…]` opens folders or files and accepts `path:line[:col]`. A single running instance is reused through a Unix domain socket (a named pipe on Windows); the window that owns the path gets focus. `-n` forces a new window.
- **CLI-2** `plastext --wait <file>` blocks until the tab closes, so plastext works as `$EDITOR` / `GIT_EDITOR`.
- **CLI-3** `plastext diff [<rev>[..<rev>]] [-- <paths>]` opens the Changes or compare view. `plastext log [<path>]` and `plastext blame <file>` open those views directly.
- **CLI-4** `plastext -` reads stdin into a streaming read-only buffer, unbounded like a pager, with `--lang <name>` (e.g. `git log -p | plastext - --lang diff`).
- **CLI-5** The launcher is a tiny separate binary. When the app is already running, it hands off and exits in ≤ 20 ms.
- **CLI-6** macOS integration: a `plastext://open?path=…&line=…` URL scheme (for terminal hyperlinks), "Open With" registration for text, Markdown, code and image types (it doesn't claim to be the default), drag-and-drop onto the Dock icon or window. A Quick Look extension is *(later)*.

### 5.10 Configuration

- **CFG-1** A single TOML file: `~/.config/plastext/config.toml` on macOS and Linux (`$XDG_CONFIG_HOME` respected), `%APPDATA%\plastext\config.toml` on Windows. An optional per-project `.plastext.toml` can override display settings but can't execute anything. Changes are hot-reloaded. v1 has no settings UI: "Open Settings" opens the file, whose commented defaults document every option.
- **CFG-2** **Themes:** follow the OS light/dark setting. Bundled default light and dark themes; custom themes in TOML, with syntax colors mapped from tree-sitter capture names. Importing VS Code / TextMate themes is *(should)*.
- **CFG-3** **Keybindings** in TOML, scoped by context (read, edit, diff, tree, palette). Defaults follow macOS conventions plus the read-mode keys.
- **CFG-4** **Fonts:** separate settings for the monospace (code) and proportional (prose) fonts, size, line height and ligatures.

Default configuration (abridged):

```toml
[editor]
open_mode = "read"            # "read" | "edit"
font = "Menlo"                # SF Mono is private to Apple's own apps
font_size = 13
line_height = 1.5
soft_wrap = false
atomic_save = true

[markdown]
default_view = "rendered"     # "rendered" | "source" | "split"
prose_font = "system"
max_width = "80ch"
load_remote_images = false
remote_image_hosts = []

[files]
show_hidden = false
show_ignored = "dim"          # "hide" | "dim" | "show"
exclude = ["**/.DS_Store"]
max_open_tabs = 50            # beyond this, tab contents are unloaded

[limits]
large_file_threshold = "64MB" # above: chunked reader instead of full load
syntax_max_file_size = "32MB" # above: lexical highlighter
long_line_threshold = 10000   # chars; above: lazy wrap, no highlighting
block_cache = "256MB"
snapshot_cache = "128MB"

[git]
gutter_base = "index"         # "index" | "HEAD"
diff_algorithm = "histogram"  # "histogram" | "myers"

[follow]
enabled = false
debounce_ms = 300
view = "diff"                 # "diff" | "file"
```

---

## 6. Performance requirements

Performance is a feature with budgets, and the budgets are enforced in CI ([§9](#9-testing-and-benchmarking)).

**Reference machine:** a base M1 MacBook Air (8 GB). It's modest on purpose: if plastext is fast there, it's fast everywhere.

### 6.1 Budgets

| Scenario | Budget |
|---|---|
| Cold launch to interactive window | ≤ 200 ms (≤ 350 ms when restoring a session) |
| `plastext file` with app running → file visible | ≤ 50 ms |
| Open any text file → first paint, **any size** | ≤ 50 ms |
| Open a 10 MB source file → fully highlighted viewport | ≤ 100 ms |
| Keystroke → pixels (edit mode) | ≤ 10 ms p99 |
| Scrolling (code, Markdown, diff, log, tree) | holds display refresh: frame ≤ 8 ms p99 at 120 Hz |
| Markdown, 1 MB document → first paint / edit → preview | ≤ 50 ms / ≤ 16 ms |
| Quick Open on a 1M-file repo | first results ≤ 100 ms after window open; ≤ 16 ms per keystroke |
| Project search, warm cache | first result ≤ 50 ms |
| `git status`, 100k-file repo, warm | ≤ 300 ms, in the background |
| Commit log first page, 1M commits | ≤ 100 ms |
| Memory: idle, one small project | ≤ 80 MB RSS |
| Memory: 1 GB file open | ≤ 64 MB + `block_cache` |
| CPU when idle | 0%: no polling, timers or animations while idle |
| App bundle size | ≤ 25 MB (Apple Silicon build), target |

### 6.2 Extreme cases

Each case below is a fixture in the benchmark suite with a required behavior:

| Case | Required behavior |
|---|---|
| 10 GB log file | Opens instantly; dragging the scrollbar reaches anywhere; jump to line or byte; find streams; tail mode works |
| 500 MB single-line minified JSON | Opens instantly; horizontal scroll and soft wrap stay smooth; highlighting stops past the threshold |
| 20M-line CSV | Text view and table view are both virtualized and smooth |
| One directory with 500k entries | The tree lists progressively; scrolling stays smooth |
| Repo with 1M+ files (Chromium-scale) | Opens instantly; Quick Open streams; status arrives in the background |
| Repo with 1M+ commits (Linux-kernel-scale) | The log pages in; blame shows progressively; file history follows renames |
| 50k changed files (mass reformat) | The Changes view lists them immediately; diffs are computed on scroll |
| Edit one line of a 1 GB file and save | Streaming save with progress; UI stays responsive; byte-exact elsewhere |
| File truncated or rewritten externally while open | No crash, no SIGBUS; reloads |
| 40k × 40k PNG | Subsampled preview immediately; tiled deep zoom |
| Binary file named `.txt` | Detected; hex view offered |
| Deeply nested or adversarial Markdown | Bounded recursion; no stall, no crash |

---

## 7. Architecture

### 7.1 Stack decision

**Rust** throughout, with **[GPUI](https://www.gpui.rs/)** (the UI framework behind the Zed editor) for windows, input and GPU rendering.

- **Why GPUI:** it was built for exactly this, a code editor running at 120 fps. It renders on the GPU through Metal on macOS (with Linux and Windows backends), has an integrated async executor, handles IME input, and is Apache-2.0 licensed.
- **Risks:** it's pre-1.0, its API changes often, docs are thin, its roadmap follows Zed's needs, and its accessibility support is still maturing.
- **Mitigations:** pin a GPUI revision and upgrade deliberately. Confine GPUI to the `ui` and `editor` crates so all core logic stays UI-agnostic, leaving a native AppKit shell over the same core as a fallback. Write **our own editor element**: Zed's `editor` crate is GPL-3.0 and tightly coupled to Zed, so it's neither usable nor desirable here. `gpui-component` is a useful reference for widgets.

See the [appendix](#appendix-ui-stack-alternatives) for the alternatives considered.

### 7.2 Key libraries

| Concern | Choice | Notes |
|---|---|---|
| UI, rendering, text shaping | `gpui` | Platform text system (Core Text on macOS) |
| Text buffer | in-house piece tree (sum tree) | Persistent and snapshot-able; see TXT-3/TXT-4 |
| Syntax | `tree-sitter` + bundled grammars | highlight, injection, tags and fold queries |
| Markdown | `pulldown-cmark` | Fast, with an offset iterator for source maps (MD-8) |
| Math layout | **spike in M2** | Candidates: (a) `mitex` (TeX → Typst math) laid out by Typst's math engine; (b) MicroTeX (C++, pluggable graphics backend); (c) a purpose-built KaTeX-style layout engine on an OpenType MATH font. Decide on KaTeX test-corpus coverage, speed and binary size |
| Fuzzy matching | `nucleo` | |
| File walking and ignore rules | `ignore` | |
| Content search | `grep-searcher`, `grep-regex` | ripgrep's engine |
| FS watching | `notify` (FSEvents backend) | Or direct FSEvents if needed |
| Git | `gix` (gitoxide) | `git` CLI fallback behind the same trait, only for read-only commands gix lacks |
| Diff | `imara-diff` | Histogram and Myers |
| Images | ImageIO (macOS); `image` crate elsewhere | |
| SVG | `resvg` | |
| PDF | PDFKit (macOS); pdfium elsewhere | *(should)* |
| Encoding | `encoding_rs`, `chardetng` | |
| Config | `serde` + `toml` | |

### 7.3 Workspace layout

```
crates/
  app/          # binary: startup, windows, single-instance server, session restore
  cli/          # tiny `plastext` launcher: arg parsing, hand-off over the socket
  ui/           # GPUI views: workspace, sidebar, tree, tabs, splits, palette, banners
  editor/       # text view element: layout, wrap, selection, input/IME, gutter, folding
  buffer/       # piece tree, chunked file source, line index, undo, encoding, save
  syntax/       # tree-sitter integration, language registry, highlighter, lexical fallback
  markdown/     # streaming parse → block model → layout model; source maps; HTML subset
  math/         # TeX math parsing and layout
  assets/       # image (tiling), SVG, PDF, hex, CSV view models
  project/      # workspace model, file tree, ignore rules, watcher, file index, snapshots
  search/       # quick open + content search
  git/          # repo model over gix: status, log/graph, blame, refs, compare
  diff/         # diff computation, word diff, review-page model, image diff
  settings/     # config, themes, keymaps, hot reload
  platform/     # OS integration: URL scheme, Trash, reveal, IPC, file associations
  bench/        # fixture generators + benchmarks
```

**Dependency rule:** only `app`, `ui` and `editor` depend on GPUI. Every other crate is headless, deterministic and testable on all platforms.

### 7.4 Data flow

```
 FS events ──► project (tree, file index, snapshots) ──► search
                     │
                     ▼
               buffer (piece tree, immutable snapshots)
                 │            │                 │
                 ▼            ▼                 ▼
           syntax (bg)   markdown (bg)       diff (bg) ◄── git (gix, bg)
                 │            │  └─► math        │           status / log / blame
                 ▼            ▼                  ▼
            editor element / markdown view / review page ──► GPUI paint (GPU)
```

### 7.5 Threading and rendering model

- **Main thread:** input, layout and paint only. Debug builds assert that no filesystem syscall or process spawn happens on it.
- **Background executor:** parsing, highlighting, indexing, search, git and diff. Every task carries a cancellation token or generation number, so stale results are dropped and never applied.
- **Streaming:** producers send results over channels and the UI coalesces them once per frame.
- **Virtualize everything:** editor rows, tree rows, search results, Markdown blocks, log rows, hex rows and diff hunks. Only what's visible, plus a margin, gets laid out.
- **Caches** (line layouts, glyphs, highlights, rendered formulas, decoded images) are keyed by content version, width and theme, bounded by LRU, and governed by the `limits.*` memory budgets.

---

## 8. Security and privacy

- **No network by default.** The only outbound traffic is remote Markdown images the user explicitly allowed. Links open in the system browser. No telemetry, no crash upload, no update pings: updates come through Homebrew or a manual download, and an update check is *(later)* and opt-in.
- **Nothing executes.** There's no JS engine in the binary. Markdown and HTML rendering is purely declarative, git hooks never run (GIT-14), and project config can't run commands.
- **Hostile input.** The Markdown, image, SVG and math parsers and the buffer are fuzzed. Decoded image size is capped (decompression bombs). Parser recursion is bounded.
- **macOS distribution:** signed with the hardened runtime and notarized, distributed outside the App Store, not sandboxed. A sandbox would block opening arbitrary folders from the CLI, the same tradeoff other editors make.

## 9. Testing and benchmarking

- **Buffer:** property-based tests (random edit sequences, undo/redo, offset conversions, save round-trips) checked against a naive `String` model; `cargo-fuzz` targets.
- **Markdown:** the CommonMark and GFM spec suites; snapshot tests of the layout model as text dumps; a small set of pixel golden tests.
- **Math:** the KaTeX test corpus, with layout snapshots and golden images.
- **Syntax:** tree-sitter query tests per bundled language.
- **Git:** fixture repos generated by scripts (renames, submodules, worktrees, conflicts, huge histories), with results checked against the `git` CLI.
- **UI:** GPUI's test harness for view-level tests with simulated input. Manual QA checklist for IME, VoiceOver, trackpad gestures and multiple displays.
- **Performance:** `crates/bench` generates the extreme-case fixtures (§6.2) and runs micro-benchmarks (`criterion`) and scripted end-to-end benchmarks (launch, open → first paint, scrolling 10k frames, typing latency) on a dedicated Apple Silicon runner. Results are tracked over time. **A regression over 10% on any budget fails CI.**
- **Perf HUD:** a debug overlay shows frame time, a per-phase breakdown (layout, paint, highlight) and background queue depth.

---

## 10. Roadmap

Each milestone ends with a usable build and explicit exit criteria.

| Milestone | Scope | Exit criteria |
|---|---|---|
| **M0: Skeleton** | Workspace, window, lazy file tree, tabs and splits, piece tree + chunked reader + line index, plain-text view, CLI + single instance, settings file, CI matrix for core crates, bench fixtures, launch benchmark that measures the cost of runtime shader compilation (see M5) | `plastext .` opens a folder; any text file, including 10 GB, scrolls at 120 Hz as plain text |
| **M1: Read code** | tree-sitter highlighting with budgets, lexical fallback, outline, folding, sticky scroll, Quick Open, project search, find in file, go to line/symbol, navigation history, themes, binary detection, hex view | Reading a large codebase (Linux kernel) feels complete and meets §6 budgets |
| **M2: Read Markdown and assets** | Markdown rendering (GFM, alerts, front matter, safe HTML, images, links), math spike → engine, outline and scroll-spy, block virtualization, image/SVG viewer with tiling, CSV table | A typical README with math renders correctly; the KaTeX corpus passes at ≥ 95%; a 100 MB Markdown file stays smooth |
| **M3: Edit** | Edit mode, selections, multi-cursor, undo/redo, find/replace, indentation and EditorConfig, atomic and streaming save, Markdown split view with scroll sync, double-click-to-source, external change handling, tail mode, `--wait` | Usable as `$EDITOR`; property tests and fuzzing are clean; an agent rewriting open files never loses user edits |
| **M4: Git** | Status decorations, gutter markers, Changes review page with viewed state, diff viewer (unified/split/word, image diff), history graph, commit view, file history, blame, compare/merge base, follow mode, activity list, `plastext diff/log/blame` | Reviewing an agent's work end to end needs no other tool; GIT-14 holds under concurrent `git` stress |
| **M5: v1.0** | Hardening against the §6.2 table, accessibility pass, PDF view, packaging (signed and notarized `.app`, Homebrew cask), precompiled Metal shaders for release builds (drop GPUI's `runtime_shaders`; needs the Xcode Metal toolchain; dev builds may keep runtime compilation). Measured in M0: runtime compilation costs ~150 ms only on a cold Metal shader cache (first launch after an install or OS update), under 1 ms after, user docs | Every §6 budget green in CI; daily-driven for 2 weeks without falling back to another editor |

**Post-v1 candidates:** Linux and Windows releases, rendered Markdown diff, native Mermaid, project symbol index / search-based go-to-definition, hunk staging/discarding, three-way merge for external changes, Quick Look extension, JSON tree view, bidirectional text.

---

## 11. Open questions

1. **Git writes.** Should the Changes view gain *stage hunk*, *unstage* and *discard hunk*? They'd help curate agent output, but they break the read-only, lock-free guarantee (GIT-14). Proposal: post-v1, opt-in, using the same locking semantics as git.
2. **Math engine.** To be decided by the M2 spike (§7.2).
3. **Inline Markdown editing.** Typora-style editing in place, or keep source/split plus double-click-to-source? Revisit after M3 based on daily use.
4. **Mermaid and diagrams.** Write a native renderer, or keep them permanently out of scope?
5. **Accessibility.** VoiceOver quality under GPUI must be evaluated in M0. If it's inadequate, that's a risk to the framework choice.
6. **Name and CLI.** Keep `plastext`, and should it also ship a short alias (e.g. `px`)?
7. **License (decided).** Dual-licensed MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`).

---

## Appendix: UI stack alternatives

| Option | Verdict |
|---|---|
| **GPUI (Rust)** | **Chosen.** Purpose-built for a high-performance editor, GPU-rendered and portable. Risk: pre-1.0 churn (mitigated in §7.1). |
| Swift + AppKit | Best native feel on macOS, but no portability. TextKit/NSTextView don't scale to GB files, so a custom text view would be needed anyway. Kept as the fallback shell over the Rust core. |
| Qt 6 (C++/QML) | Portable and mature, but brings C++, a large runtime, and text components unsuited to GB files. |
| egui | Simple and portable, but weak for large text layout, rich Markdown typography and IME. |
| Iced / Slint / Xilem | Promising but less proven for a high-performance text editor. Worth re-evaluating later. |
| Electron / Tauri / any WebView | Excluded by requirement. |
