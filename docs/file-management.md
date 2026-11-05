# File management

Single-click a file in the navigation pane to preview it. Keyboard focus stays
in the file pane. Double-click or press Enter to keep the tab open and focus the
editor. Up and Down change the selected file.

Right-click a file or folder for these actions:

| Action | Behavior |
| --- | --- |
| Duplicate | Create a sibling named `name copy.ext`, adding a number if needed. |
| Copy / Cut | Prepare the selected file or folder for Paste. Cut leaves the original in place until the move succeeds. |
| Paste | Copy or move into the selected folder, or beside the selected file. |
| Rename… | Enter a new filename. Escape cancels. |
| Move… | Choose a destination folder. |
| Delete | Move to the system Bin. There is no permanent-delete fallback. |
| Copy Path / Copy Relative Path | Copy the absolute path or the path relative to the project folder. |

While the file pane has focus, use ⌘C, ⌘X and ⌘V for file operations, F2 to
rename, and ⌘⌫ to move the selected item to the Bin. These shortcuts do not
remove files while the editor has focus. On macOS, copied files can also be
pasted into Finder, and a file copied from Finder can be pasted into the file
pane.

Copies receive an unused name. Move and Rename refuse to replace an existing
item. Operations run in the background, and affected folders refresh when they
finish. Symlinks are copied or moved as links.

Move, Rename and Delete ask Save / Discard / Cancel when an affected file has
unsaved edits, including files open in other panes or windows. Edits are kept
if the filesystem operation fails. After a successful move or rename, affected
tabs reopen at their new paths and previous scroll positions; undo history is
reset. Saving afterwards uses the new path. Deleted files can be restored from
the system Bin.

Filesystem actions currently use macOS native APIs. Other platforms report that
the operation is unavailable; they do not substitute permanent deletion.
