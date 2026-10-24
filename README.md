# plastext

A fast, native, read-first viewer and light editor for code, Markdown and assets. It's built for people who write code through LLM agents in the terminal and mostly need to read, review and tweak it.

> **Status:** M2 in progress (Markdown and assets). M1's code reader, navigation, search, themes and hex view remain available. Markdown now opens in a virtualized native preview with headings, links, code fences and rendered-text find; local images/SVGs and paged CSV/TSV tables have dedicated views. Math rendering and large-image tiling are still pending. Editing and git come later. See [M2 progress and limits](docs/m2-progress.md), the [accepted performance baseline](docs/m1-performance.md), and the [full specification](docs/specs.md).

## Building

You need macOS on Apple Silicon and a stable Rust toolchain (1.99 or later). The Xcode Command Line Tools are enough; full Xcode isn't required.

```sh
cargo run -p plastext-app -- .   # open the current folder
cargo test --workspace
cargo build --release            # builds target/release/plastext and plastext-app
```

With full Xcode and its Metal compiler installed, precompile the shaders to avoid runtime shader compilation on a cold launch:

```sh
cargo build --release -p plastext-app --no-default-features
cargo build --release -p plastext-cli
```

Default builds use the `runtime-shaders` feature so development works with Command Line Tools alone. Use the package-specific command above: building the whole workspace enables the UI crates' default features again.

`plastext` is the command-line launcher. It hands its arguments to the running app, or starts the `plastext-app` binary sitting next to it, so keep the two in the same directory. Settings are read from `~/.config/plastext/config.toml`; see spec §5.10.

## Keys

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
| `⌘\` / `⌘⌥\` | Split right / down |
| `⌘C`, `⌘A`, `⌘W` | Copy, select all, close tab |
| `⇧⌘V` in Markdown | Toggle rendered preview / source, preserving source position |

## Benchmarks

```sh
cargo bench -p plastext-bench --bench text              # text engine micro-benchmarks
cargo build --release -p plastext-app -p plastext-bench
target/release/plastext-bench launch -- .               # time to first frame, 10 launches
target/release/plastext-bench log 10GB /path/big.log    # spec §6.2 fixtures; also long-line, wide-dir
target/release/plastext-bench m1 /path/linux-6.12 --million --output /tmp/m1.json
target/release/plastext-bench read /path/linux-6.12/kernel/sched/core.c --root /path/linux-6.12 --foreground
python3 scripts/m2-fixtures.py /tmp/plastext-m2          # 100 MB Markdown + 20M CSV records
target/release/plastext-bench m2 /tmp/plastext-m2/large.md --output /tmp/m2-markdown.json
target/release/plastext-bench csv /tmp/plastext-m2/large.csv
```

`m1` measures first streamed Quick Open results and completed query rankings separately (including cached backspacing), plus warm project search. `--million` also tests one million synthetic paths derived from the real tree. `read` drives 10,000 native scroll frames and measures first paint, initial viewport highlighting, full-tree availability and GPUI frame drawing/submission. `--foreground` activates its window; leave it visible because macOS suspends drawing for covered windows. Both runners return a nonzero exit code when a measured budget fails. These checks do not substitute for the reference M1 Air run or measure GPU completion, idle memory, or features from later milestones. See [the M1 performance report](docs/m1-performance.md) for results and remaining gaps.

The `m2` and `csv` commands measure core parsing/indexing, not native scrolling or milestone sign-off. Open [the M2 demo](docs/fixtures/m2.md) to exercise the new views. Markdown find currently uses case-sensitive literal text; Source offers the existing text reader's search and selection.

## Themes and folding

Folding uses bundled tree-sitter queries when a full parse is available, including unindented blocks and multiline comments/literals. Large source files first parse a 64 KiB prefix for viewport coloring, then replace it with the full tree in the background. Before folding queries finish, and for files without syntax support, folding falls back to indentation. Quick Open boosts matching recent files in most-recent-first order, followed by other files in fuzzy relevance order. It streams and refines the best 256 results; type more to narrow a broad query.

To import a local VS Code JSON or JSONC color theme, add its path to `config.toml` and restart plastext:

```toml
[theme.dark]
import = "themes/my-dark-theme.json" # relative to config.toml; absolute paths also work

[theme.dark.syntax]
comment = "#8b949e"                 # explicit TOML colors override imported ones
```

The same setting works under `[theme.light]`. Imports support comments, trailing commas, relative `include` files, RGB/RGBA colors and common TextMate token scopes. Unsupported workbench colors, contextual token selectors, semantic token styles and font styles are ignored. Standalone `.tmTheme` files and extension packages are not imported.

## Layout

The workspace follows spec §7.3, with one crate per concern under `crates/`. Only `app`, `ui` and `editor` may depend on GPUI. Every other crate is UI-agnostic and must build and pass its tests on macOS, Linux and Windows. CI enforces both rules; the first is checked by `scripts/check-gpui-boundary.sh`.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in plastext by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
