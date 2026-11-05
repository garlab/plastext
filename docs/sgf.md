# SGF preview

Go records (`.sgf`, case-insensitive) open as a native board preview. No network,
external engine, JavaScript runtime or additional system installation is needed.
The reusable parser and renderer live in `document-graphics`; Plastext owns the
navigation, background jobs and caches.

Try the [small board study](fixtures/sample.sgf):

```sh
cargo run -p plastext-app -- . docs/fixtures/sample.sgf
```

Navigation follows [Sabaki's shortcuts](https://github.com/SabakiHQ/Sabaki/blob/master/src/menu.js):

- **Up / Down:** previous / next node along the selected variation.
- **Left / Right:** cycle variations at the same depth in the tree, wrapping at
  either edge. Click any move in the game tree to jump to it.
- **Home / End:** start / end of the selected line. On a Mac keyboard without
  these keys, use **Fn–Left / Fn–Right**.
- **Command–Up / Command–Down:** previous / next fork, or the start / end if
  there is no further fork. These skip the current node and follow the selected
  variation.
- **Numbers:** show surviving stones' move numbers.
- **Shift–Command–V:** switch between Preview and Source at the selected node.
- **Command–plus / minus:** scale controls and text. The full board always fits
  its pane. Split panes keep independent navigation positions.

Collections with multiple games have Previous game / Next game controls. Player
names, result, komi, date, capture counts and node comments appear in a persistent
side panel. Comments scroll independently, so changing moves does not resize the
board. The game tree shows branches side by side, highlights the current move and
its ancestors, and follows keyboard navigation. Numbered black and white circles
represent moves; squares represent setup or annotation nodes. A gold dot marks a
comment. Scroll the tree in either direction to explore distant branches.
The original source remains available when a file or position cannot be previewed.
This is a record viewer, without move editing, scoring or engine analysis.

## Boards in Markdown

Fences tagged `sgf` (case-insensitive) show a static board. By default this is
the first game's final main-line position. Optional fence selectors match mdviewer:

- `game=2` selects the second game in a collection (one-based).
- `move=1` selects the first move on the main line; passes count as moves.
- `move=0` selects the root, including setup stones and any root move.
- `move=last` selects the final main-line node, including its annotations.

```sgf move=1
(;FF[4]GM[1]SZ[9]AB[cc][dc]AW[cd];B[dd];W[ed];B[ec])
```

Each board has **Source / Preview** and **Copy** controls. Copy preserves the
record text; Find reveals matching source. Invalid records, unsupported selectors
and out-of-range games or moves show an explanation and the source. Use a standalone
`.sgf` file for interactive move and variation navigation.

Try the [Markdown fixture](fixtures/sgf.md). Boards render only near the viewport
and share Mermaid's 32 MiB CPU/GPU cache and two-job limit per view. Scrolling away
cancels pending work; hiding the preview releases its images. Rendering uses the
native SGF library in a background task, with no network, external resources or
child process. Each fence is limited to 16 KiB, 1,024 lines and 8,192 nodes; larger
fences retain all their text in virtualized source chunks. Rasters are bounded to
one million pixels. Parsed collections and board positions are discarded after
rendering. Resizing reuses the raster and keeps the board's proportions.

## Supported records and limits

The parser supports Go (`GM[1]`) records, sequences and variations, setup and
handicap stones, captures, passes and rectangular boards from 1 to 52 intersections
per side. It reconstructs recorded positions without enforcing competitive move
legality, ko or alternating turns. `HA` alone does not create handicap stones;
`AB` supplies their positions.

UTF-8 and ISO-8859-1 are supported. Files without `CA` use UTF-8 when valid and
otherwise ISO-8859-1. Other declared encodings require Source mode. Common shapes,
labels, arrows, lines, dimming and territory marks render on the board. Unknown
properties remain in the source; presentation hints such as `VW` cropping and
legacy markup are not interpreted. Board labels use the renderer's bundled fonts;
comments use the application's normal Unicode text rendering.

Preview limits are 8 MiB of source, 100,000 nodes, 1,024 games, 256 nested variations,
128 KiB per property value, and 512 marks / 512 lines per position. Labels are
limited to 32 characters. SVG output also obeys `document-graphics`' geometry,
text, element and source limits, so particularly dense annotated boards may need
Source mode. Comments display up to 16,384 characters and metadata is abbreviated;
the complete text remains in Source.

Parsing and rendering run off the UI thread. Navigation permits one active job
per pane, cancels obsolete work and coalesces rapid input to the latest request.
Only 32 compact board positions and one displayed raster are retained per pane;
rasters have at most one million pixels. Hidden previews release their images and
position caches, including GPU atlas entries. Resizing uses the existing raster,
fits both dimensions and preserves the board's proportions. Comments and the tree
have their own scrolling areas; the toolbar keeps a fixed height. The compact tree
index is built in the background and shared between split panes. Only visible
nodes and connectors are drawn, without reconstructing their board positions.

## Developing the shared backend

Plastext uses the `document-graphics` `sgf` feature. To develop
the library, use the optional sibling checkout and ignored Cargo override from
[local development](local-development.md#shared-rendering-libraries).
Run the library's `sgf_probe` example from its checkout to check every node and render root
and leaf positions in a directory without modifying its inputs:

```sh
cargo run --release --features sgf --example sgf_probe -- path/to/sgf-directory
```
