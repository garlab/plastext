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
| Move / select a word | Option-Left/Right / Option-Shift-Left/Right |
| Delete a word | Option-Backspace / Option-Delete |
| Add/remove a cursor | Option-click |
| Add next occurrence / select all occurrences | Cmd-Option-G / Cmd-Shift-L |
| Add cursor above / below | Cmd-Option-Up / Cmd-Option-Down |
| Replace | Cmd-Option-F |
| Indent / outdent | Cmd-] / Cmd-[; Tab / Shift-Tab for selected lines |
| Toggle comment | Cmd-/ |
| Move line up / down | Option-Up / Option-Down |
| Duplicate line up / down | Option-Shift-Up / Option-Shift-Down |
| Go to matching bracket | Cmd-Shift-\ |
| Settings | Cmd-, |

Mouse selection, Shift-click, double-click words and triple-click lines work in
edit mode. Backspace/Delete move through complete grapheme clusters. Return
uses the document's preferred line ending and inherits indentation; Tab uses
the resolved tabs/spaces setting. Native input uses
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

Saves run on a worker, preserve encoding and line endings unless explicitly configured, and keep
undo history. Typing during a save remains dirty until that newer version is
saved. Atomic saves preserve metadata and write through symlinks; hard-linked
files use the backend's in-place save with a recovery copy. If the disk file has
changed, saving reports the conflict and leaves both disk contents and local
edits intact. See [save semantics and limits](m3-saving.md).

Closing the last tab for a dirty document, closing its window, or quitting offers
Save, Cancel and Discard. A newer edit invalidates a pending discard decision.
Closing one split copy keeps the document open in the remaining pane. Pending
input or saves must finish before the document can close.

## Editing commands and settings

**Edit** contains selection, line, replacement, encoding and newline commands;
**View → Show Whitespace** shows spaces and tabs without changing copied text.
Bracket matching uses the current syntax tree and respects strings/comments.
It is available for parsed languages within the syntax-size limit. Auto-closing
brackets and quotes are off by default; enable `editor.auto_close_pairs` to insert
pairs, wrap selections and step over an existing closing delimiter.

`Cmd-D` and `Cmd-Shift-D` remain Split Right and Split Down. To add occurrences,
select text and use `Cmd-Option-G`; without a selection, it first selects the word
at the caret. Escape keeps only the primary cursor. Overlapping selections merge.
Copy/cut joins nonempty selections with newlines in document order. Paste distributes
lines when their count matches the number of cursors; otherwise it inserts the
same text at every cursor. Multi-cursor changes are atomic and undo together.
IME composition uses the primary selection and clears additional cursors.

Find exposes **Aa**, **ab** and **.*** for case, whole-word and regex matching.
**Selection** uses the selection captured when Find opened and follows it through
edits. Replace All uses one immutable document version and one undo transaction;
changing the document, selection or query invalidates pending work. Regex
replacements support `$1`, `${name}` and `$$`; literal replacements are verbatim.
Regex matching is per logical line (LF, CRLF or CR), including zero-width anchors;
literal search also supports line breaks. Multiline regex matching is not implemented.

Bulk line commands, paste, multi-cursor typing and replacement prepare their
changes on workers. Commands are queued in order; a stale operation cannot overwrite
another pane's changes. Limits are 4,096 cursors, 100,000 lines/matches per bulk
operation and 64 MiB of inserted/copied command text. Regex searches reject lines
over 8 MiB; literal search streams longer lines. A limit error applies no partial
replacement. Ordinary typing, movement and viewport drawing keep their bounded
paths; these limits do not certify native keystroke-to-pixel latency.

**Settings** opens the public user configuration with commented editing defaults.
Restart to apply changed defaults. `editor.open_mode` accepts `edit` (default) or
`read`; `editor.atomic_save` defaults to true. Indentation uses detected tabs/spaces,
then `.editorconfig` overrides. Set `detect_indentation = false` for fixed
`indent_size`, `tab_size` and `insert_spaces`. `auto_indent`, `auto_close_pairs`,
`show_whitespace` and `editorconfig` can also be configured.

EditorConfig discovery respects `root`, section globs, ancestor precedence and
`unset`. Files are read on a worker, limited to 64 KiB each and 128 ancestors;
indent widths are clamped to 1–16 columns. `trim_trailing_whitespace` and
`insert_final_newline` run only on explicit Save and are undoable. False disables
final-newline insertion and preserves existing endings. `charset` and `end_of_line`
set the save format. UTF-8 (with/without BOM) and UTF-16 LE/BE are supported;
EditorConfig's ISO-8859-1 output is rejected explicitly rather than substituted
with Windows-1252.

**Edit → Save with Encoding / Line Endings** sets an undoable output policy for
the current document; the status bar shows it. These menus do not write to disk.
An explicit choice overrides that file's EditorConfig output setting for the
session. **Reopen with Encoding** reinterprets a clean file and is disabled with
unsaved edits. Encoding failures leave the destination unchanged. If typing
changes a document during save cleanup, it stays dirty and requests another Save.

## Current scope

Remaining M3 work:

- Automatic editing applies to plain text, code and Markdown Source. Other preview/source
  toggles remain read-only until their synchronization is connected.
- Save As, column selection and restoring historical cursor selections on undo
  remain to implement. New File is available from the File menu and navigation pane.
- Filesystem reload notifications and interactive conflict resolution are not
  connected yet. A save refuses changed disk contents; it does not overwrite
  them automatically.
- Cut/copy are limited to 64 MiB. A cut whose selection is uncached first loads
  it and asks for Cut again; it never deletes before copying.

Large-file preparation is cancellable background work and uses a private copy
on disk. Input waits for missing cache blocks without blocking the UI; obsolete
explicit selection/composition coordinates are rejected after another pane edits.
Undo eviction and snapshot destruction run on workers. Native IME, close/quit
interaction and the typing-latency target still require manual/performance
validation; automated coverage is listed in [M3 progress](m3-progress.md).
