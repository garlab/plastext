# Chess records

`.pgn` games and `.fen` positions open as offline chess boards. Extensions are
case-insensitive. No chess engine, network access or system installation is
needed. The board always fits within its pane, with comments and a variation
tree alongside it. Source remains available, including when a record cannot be
previewed.

## Reading a record

The controls follow the SGF viewer's Sabaki-style navigation:

- Up/Down: previous/next move along the selected line.
- Left/Right: cycle variations at the same tree depth. Click a tree node to jump
  directly to it. Nodes show SAN moves; dots indicate comments or annotations.
- Home/End: start/end of the selected line (Fn–Left/Fn–Right on macOS).
- Cmd–Up/Cmd–Down: previous/next fork, or the start/end when no fork remains.
- **Flip board** switches between White's and Black's perspective. The most
  recent move's source and destination squares are highlighted.
- **Previous game / Next game** switch games in a PGN collection, or positions
  in a FEN file containing one position per nonempty line.
- Shift–Cmd–V switches Source/Preview at the selected move. Split panes keep
  independent selections and board orientations.

Try the [variation collection](fixtures/chess/variations.pgn) or
[FEN positions](fixtures/chess/positions.fen):

```sh
cargo run -p plastext-app -- . docs/fixtures/chess/variations.pgn
```

## Supported notation

Standard chess, SAN moves, castling, captures, en passant, promotions and
underpromotions, disambiguation, nested variations, brace/semicolon comments,
annotation glyphs, game tags and results are supported. `FEN`/`SetUp` tags select
an initial position. Plain FEN uses the standard six fields. Text accepts UTF-8
and falls back to Latin-1 for older PGN annotations.

Moves are checked for legality when their position is requested. An illegal or
ambiguous move reports an error while navigation and source remain available;
it does not fabricate a board. Unsupported chess variants, null-move analysis
notation and malformed FEN retain source. PGN clock/evaluation/arrow extensions
remain comment text; they are not interpreted. This is a reader, without move
editing, analysis, playback, or repetition/draw adjudication.

## Resources and shared code

The optional `chess` module in `document-graphics` owns parsing, legal replay
and board rendering. It uses MIT-licensed `cozy-chess` and original vector piece
shapes. Plastext shares the board view, navigation, tree layout and caches with
SGF; the backend has no GPUI dependency.

Limits are 8 MiB of source, 100,000 nodes, 1,024 games, 128 variation levels,
16,384 half-moves per line and 64 KiB of comments per node. Tags and annotations
also have count/size limits. The index retains source ranges and tree links;
it does not keep a board or raster for every move.

Parsing and replay run off the UI thread, with cooperative cancellation and
one active job per pane. Superseded navigation requests are coalesced. Each
pane retains at most 32 compact positions and one raster capped at one million
pixels. Hidden previews release raster/position caches and GPU images. Resizing
fits the existing raster without launching another render. Only visible tree
nodes are painted.

Like SGF, the bounded chess parser and replay run in-process, followed by the
restricted SVG renderer. They are not in the Mermaid worker's OS sandbox and
have no hard wall-clock deadline. No document text becomes executable code,
external resources or SVG markup.

## Development dependency

Chess requires `document-graphics` **0.1.4**, currently prepared locally for
publication. Until it is published, use the sibling checkout through the
ignored `.cargo/config.toml`:

```toml
[patch.crates-io]
document-graphics = { path = "../document-graphics" }
```

The development lockfile resolves that local version. After publication, remove
this override, run `cargo update -p document-graphics --precise 0.1.4`, and commit
the registry source/checksum in `Cargo.lock`. See
[local development](local-development.md) for the shared-library workflow.
