# Text and code editing

Text and code files open ready to edit. Markdown opens in Preview; choosing
**Source** (`Cmd-Shift-V`) makes it editable immediately. Source stays in edit
mode, including after Escape or saving. Read-only files and binary/hex views
remain non-editable. Write-access checks and preparation run on a worker.

A single click in the navigation pane previews a file while keeping focus on
the selected file. Click the editor, double-click the file or press Enter to
focus the editor. See [file management](file-management.md) for navigation-pane
Copy/Cut/Paste, Rename, Move and Delete shortcuts.

The status bar shows **EDIT**, and an unsaved tab gets a dot. A modified preview
tab becomes permanent. Saving is explicit: press **Cmd-S**. There is no autosave;
typing, waiting, switching tabs and switching to Preview leave the disk file alone.

| Action | macOS shortcut |
| --- | --- |
| Save | Cmd-S |
| Undo / redo | Cmd-Z / Cmd-Shift-Z |
| Copy / cut / paste | Cmd-C / Cmd-X / Cmd-V |
| Select all / find | Cmd-A / Cmd-F |
| Move / extend selection | Arrow / Shift-Arrow |
| Line start / end | Cmd-Left / Cmd-Right; Ctrl-A / Ctrl-E |
| Document start / end | Cmd-Up / Cmd-Down |
| Extend to line or document boundary | Add Shift to the Cmd shortcut |
| Scroll a page | Page Up / Page Down |

Mouse selection, Shift-click, double-click words and triple-click lines work in
edit mode. Backspace/Delete move through complete grapheme clusters. Return
uses the document's preferred line ending; Tab inserts a tab. Native input uses
UTF-16 coordinates and supports marked text for IME composition.

Keyboard movement and typing scroll just enough to keep three lines of context
above and below the caret. Short panes reduce that margin automatically. Clicking
or selecting text with the mouse leaves the viewport in place.
The focused caret blinks every 500 ms while idle. Typing, movement and focus
changes make it visible immediately and restart the delay. Composition keeps it
steady; inactive windows and unfocused panes do not run blink timers.

Right-click text to open Cut, Copy and Paste, plus these reference formats:

| Menu item | Copied text |
| --- | --- |
| Copy Location | `file.rs:34:2` |
| Copy Selected Line Range | `file.rs:22-26` |
| Copy Text with Location | `in file.rs:33: selected text` |

References use the filename and one-based line/column numbers from the current
unsaved buffer. Columns count Unicode characters; a tab counts as one. A
selection ending at the start of the next line excludes that next line from the
range. Multiline excerpts start on the line after `in file.rs:22-26:`.
Without a selection, Copy Text with Location includes the current line.
Right-clicking within a selection preserves it; clicking elsewhere places the
caret there. Cut and Paste are disabled in read-only views. Large copies run on
a worker within the existing 64 MB text limit.

Split panes share content, saved state and undo history, with independent
selections and scroll positions. Find searches the current unsaved version.
Markdown Preview reads the unsaved version. Switching back to Preview refreshes
it immediately; a visible preview in another pane updates after a 250 ms pause
in typing. Hidden previews do not reparse during typing. Each refresh builds the
compact block index on a worker and only retains parsed pages within the existing
cache budget. New edits cancel obsolete refreshes; late results cannot replace
newer content. `Cmd-S` also works while Preview is focused. Continuous source/preview
scroll synchronization and double-clicking rendered blocks to edit remain to do.

Entering edit mode preserves syntax highlighting. Edits refresh it on a worker
after a 40 ms pause, reusing unchanged syntax-tree nodes; newer edits cancel older
work. Parsing retains the configured file-size limit. While a refresh is pending,
unchanged text keeps its parsed colors through insertions, deletions and undo,
including text shifted to new offsets. Newly inserted text uses lexical colors
until the parse completes. The coordinate cache is bounded and shared-session
edits update each pane independently. Syntax-tree folding, outline and sticky definitions are not
yet connected to edited snapshots.

Saves run on a worker, preserve the existing encoding and line endings, and keep
undo history. Typing during a save remains dirty until that newer version is
saved. Atomic saves preserve metadata and write through symlinks; hard-linked
files use the backend's in-place save with a recovery copy. If the disk file has
changed, saving reports the conflict and leaves both disk contents and local
edits intact. See [save semantics and limits](m3-saving.md).

Closing the last tab for a dirty document, closing its window, or quitting offers
Save, Cancel and Discard. A newer edit invalidates a pending discard decision.
Closing one split copy keeps the document open in the remaining pane. Pending
input or saves must finish before the document can close.

## Current scope

This is the first GUI editing increment in M3:

- Automatic editing applies to plain text, code and Markdown Source. Other preview/source
  toggles remain read-only until their synchronization is connected.
- Multicursor, word movement, replace, indentation commands, EditorConfig,
  new files, Save As and encoding/newline controls remain to implement.
- Filesystem reload notifications and interactive conflict resolution are not
  connected yet. A save refuses changed disk contents; it does not overwrite
  them automatically.
- `editor.open_mode`, `editor.atomic_save` and other editing preferences from the
  specification are not exposed yet.
- Cut/copy are limited to 64 MiB. A cut whose selection is uncached first loads
  it and asks for Cut again; it never deletes before copying.

Large-file preparation is cancellable background work and uses a private copy
on disk. Input waits for missing cache blocks without blocking the UI; obsolete
explicit selection/composition coordinates are rejected after another pane edits.
Undo eviction and snapshot destruction run on workers. Native IME, close/quit
interaction and the typing-latency target still require manual/performance
validation; automated coverage is listed in [M3 progress](m3-progress.md).
