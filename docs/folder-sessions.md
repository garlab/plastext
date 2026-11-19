# Folder sessions

Opening a folder again restores its tab order, selected tab in each pane,
active pane, split directions and proportions, sidebar width, UI zoom and
synchronized-scrolling setting. This applies to Open Folder, recent folders and
command-line folder arguments. Files explicitly requested alongside a folder
open in addition to the restored tabs. An already-open folder keeps its current
workspace.

Each tab retains its Source, Rendered/Preview or Binary mode. Text, Markdown and
native TeX reading positions use source offsets and fractional scroll anchors;
PDFs retain the page, fit setting and pan. CSV tables and Go/chess records retain
their row or node through a source offset. Positions are best effort if files
change between sessions or the window size changes. Image preview pan/zoom,
CSV column widths, selections, folds and undo history are not stored.

Only the selected file in each pane loads when restoring. Other tabs keep their
headers and saved state until selected. A missing or inaccessible file remains
as a tab with an error; selecting it again retries. The other tabs still open.
Untitled files are excluded; saving one gives it a path and makes it eligible.

## Storage and lifecycle

The layout is recorded after the usual Save/Discard confirmation on normal
window close or Quit. Cancelling keeps the window open and does not write a
session. Document contents and unsaved edits are never stored here. All folder
windows are saved together when quitting.

There are no timers or writes triggered by scrolling, typing or resizing.
Session reads and atomic replacements run on a background worker. An unchanged
snapshot does not rewrite the file. Only metadata is retained, in
`sessions.json` beside `config.toml`, outside the opened folder by default.
Storage is bounded to 20 folder sessions and 4 MiB overall, with at most 64 panes
and 4,096 tabs per folder. Older records are evicted to stay within these bounds.

A crash or force-quit leaves the last normally closed session available; changes
to the layout since then are not checkpointed. Folder sessions do not reopen all
previous windows automatically on an empty launch, or restore window placement.
Clear Recent clears the welcome screen's folder history; delete `sessions.json`
while Plastext is closed to reset saved layouts.

Startup and reading benchmarks disable session storage so existing baselines
continue to exercise their explicit fixtures.
