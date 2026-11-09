# plastext

A fast, native, read-first viewer and light editor for code, Markdown and assets
on macOS. It's built for people who write code through LLM agents in the terminal
and mostly need to read, review and tweak it.

## What it opens

- **Code and text**, with syntax highlighting, folding, an outline, go to symbol,
  search and a hex view. Plain text, code and Markdown source can be
  [edited and saved](docs/editing.md).
- **Markdown** in a virtualized native preview: headings, links, code fences,
  front matter tables, [Mermaid diagrams](docs/mermaid.md), math rendered
  natively (KaTeX through [katex-native](https://crates.io/crates/katex-native)),
  and find in the rendered text.
- **Images** with bounded large-image tiles, AVIF/HEIC, animated GIF/WebP and an
  SVG source mode; see [image previews](docs/images.md).
- **CSV/TSV** in paged tables.
- **Go and chess records**: [SGF](docs/sgf.md) and [PGN/FEN](docs/chess.md)
  open as interactive boards with move and variation navigation.
- **PDF** in an isolated, tiled preview with a Binary toggle, and **TeX** through
  an installed pdfLaTeX; see [PDF and TeX preview](docs/pdf-and-tex-preview.md).

Status: editing (milestone M3) is in progress, and Git review follows in M4. See
[M3 progress](docs/m3-progress.md), [M2 progress and limits](docs/m2-progress.md),
and the [specification](docs/specs.md).

## Install

You need macOS on Apple Silicon and a stable Rust toolchain (1.99 or later). The
Xcode Command Line Tools are enough; full Xcode isn't required. From a checkout,
install the app and its launcher together:

```sh
cargo install --path crates/app --locked
cargo install --path crates/cli --locked
plastext .   # open the current folder
```

`plastext` hands its arguments to the running app, or starts the `plastext-app`
binary next to it, so keep the two in the same directory (`~/.cargo/bin` by
default). Without a path, the welcome screen offers Open Folder, Open File and
recent folders. To update, run both install commands again and relaunch the app.
See [local development](docs/local-development.md) for running a development
build alongside the installed app.

## Using plastext

| Keys | Action |
|---|---|
| `⌘P` / `⌃G` | Quick Open (`path:line` works) / go to line |
| `⇧⌘P` | Command palette: every command and its shortcut |
| `⇧⌘O` / `o` | Go to symbol / toggle the outline panel |
| Gutter chevrons or `⌘⌥[` | Toggle the fold on the selected line, or the top visible line without a selection |
| `⌘K ⌘0` / `⌘K ⌘J` | Fold all top-level blocks / unfold all |
| `⌘F` or `/`, then `n` / `N` | Find in file, next / previous match |
| `⇧⌘F` / `⇧⌘E` | Search the project / show files |
| `j` `k`, `space` `⇧space`, `g` `G` | Scroll by line, by page, to top / bottom |
| `⌃-` / `⌃⇧-` (or `⌘[` / `⌘]`) | Go back / forward |
| `⌘D` / `⇧⌘D` | Split right / down |
| `⌘C`, `⌘A`, `⌘W` | Copy, select all, close tab |
| `⇧⌘V` in Markdown | Toggle rendered preview / source, preserving source position |
| `⇧⌘V` in SGF/chess | Toggle board preview / source |
| `↑` / `↓`, `Home` / `End` in SGF/chess | Previous / next node, start / end of selected line |
| `←` / `→` in SGF/chess | Cycle variations at the same tree depth |
| `⌘↑` / `⌘↓` in SGF/chess | Previous / next fork (or start / end) |
| `⇧⌘V` in PDF | Toggle Preview / Binary, preserving each position |
| `⇧⌘V` in TeX | Toggle Source / Preview; preview opens automatically with installed pdfLaTeX |
| `⌘R` in TeX | Retry preview and reload advanced TeX settings |
| `←` / `→`, `Home` / `End` in PDF preview | Previous / next page, first / last page |
| `⌃G` in PDF preview | Jump to a page number |
| `⌘R` in PDF preview | Reload the file from disk |
| `⌘+` / `⌘-` / `⌘0` | Zoom documents in / out / reset (50–200%); `⌘=` also zooms in |

**Files and tabs.** Single-click a file in the navigation pane to open a
temporary preview tab, shown with an italic title; clicking another file reuses
it. Double-click the file or its tab to keep it open; each split pane has its own
preview tab. Files opened through Quick Open, search results or the command line
stay open. Right-click a tab for **Close Others**, **Close Right**, **Close Left**
or **Close Saved Files**, which apply to that pane; the first three keep the
clicked tab open, and Close Saved Files keeps untitled drafts, modified files and
files with unresolved disk changes. Right-click a file or folder for **Copy Path**
or **Copy Relative Path**. Drag the divider beside the Files/Search pane or
between split panes to resize them.

**Git status** colors files and folders in the tree and filenames in tabs: green
for staged changes, amber for unstaged changes, purple for both, and red for
conflicts. Untracked files stay neutral until staged, and status refreshes in the
background. See [Git status](docs/git-status.md).

**Project search** has **Aa** (Match Case), **ab** (Match Whole Word) and
**`.*`** (Regular Expression) toggles; case matching is off by default. Results
are grouped by folder and filename, with matches in line order. **File filters**
adds include/exclude globs, and **Show more** continues after each 10,000
matching lines. See [project search](docs/search.md).

**Quick Open** boosts recent files, then other files by fuzzy relevance. It
streams the best 256 results; type more to narrow a broad query.

**Folding** uses tree-sitter when a full parse is available, including
unindented blocks and multiline comments and literals, and falls back to
indentation otherwise. Large files color a 64 KiB prefix first, then the full
file in the background. HTML highlights JavaScript in `<script>` (including
modules and JSON data) and CSS in `<style>`.

**TeX** opens in preview with an installed TeX Live pdfLaTeX. Roots and local
inputs are detected automatically, and external saves update the preview. Split
with `⌘D` or `⇧⌘D`, then switch either pane to Source for side-by-side reading.
Advanced overrides live in **Settings…** and `.plastext/tex.toml`. See
[setup, behavior and limits](docs/pdf-and-tex-preview.md#local-tex-increment).

## Configuration

Settings are read from `~/.config/plastext/config.toml` (spec §5.10). Markdown
typography applies after a restart:

```toml
[markdown]
prose_font = "system" # or an installed family, e.g. "Georgia"
max_width = "80ch"   # zero-glyph widths in the prose font; "720px" also works
mermaid = true      # false keeps diagram fences as source
```

Code and inline code use `editor.font` independently of the prose font.

To import a local VS Code JSON or JSONC color theme, add its path and restart:

```toml
[theme.dark]
import = "themes/my-dark-theme.json" # relative to config.toml; absolute paths also work

[theme.dark.syntax]
comment = "#8b949e"                 # explicit TOML colors override imported ones
```

The same setting works under `[theme.light]`. Imports support comments, trailing
commas, relative `include` files, RGB/RGBA colors and common TextMate token
scopes. Unsupported workbench colors, contextual token selectors, semantic token
styles and font styles are ignored; standalone `.tmTheme` files and extension
packages are not imported.

The file tree shows file-type icons. Use `⇧⌘P` → **File Icon Theme** to choose
Material, None, or an imported VS Code icon theme such as Seti; see
[icon themes](docs/icon-themes.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for building from a checkout, the checks
and benchmarks, and the workspace layout.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option.

File icons are from [Material Icon Theme](https://github.com/material-extensions/vscode-material-icon-theme) under its MIT licence. Their copyright and licence are retained in [Third-party notices](THIRD_PARTY_NOTICES.md), also available with `plastext --notices`.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in plastext by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
