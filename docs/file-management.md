# File management

The native [menu bar](menus.md) also provides File and Edit commands. Use **⌘O**
to open files and **⌘⌥O** to open a project folder through the system picker.

Single-click a file in the navigation pane to preview it. Keyboard focus stays
in the file pane. Double-click or press Enter to keep the tab open and focus the
editor. Up and Down change the selected file.

Right-click empty space in the file pane for **New File**, **New Folder** and
**Paste File**. These actions target the project root, regardless of the previous
selection. Paste File is disabled when the clipboard contains no file.
The toolbar above the tree provides **New File**, **New Folder** and **Collapse
All Folders**. Creation targets the selected folder, the selected file's parent,
or the project root when nothing is selected. Type the name in the tree and press
Enter to create it. Escape or Enter with an empty name cancels without creating
anything. Invalid names and collisions show an inline error. New files open in
a tab; new Markdown files open in Source. New folders stay selected in the tree.

**⌘N** starts inline file creation when the navigation pane has focus. Elsewhere,
it opens an empty **Untitled** tab in the active editor pane, even without a
project folder. Untitled tabs keep their contents in memory until **⌘S** opens
the system Save dialog. Closing a nonempty draft offers Save, Cancel and Discard;
empty drafts close immediately. Saving keeps undo history and updates all split
views of the draft. No automatic saving is performed.

Right-click a file or folder for these actions:

| Action | Behavior |
| --- | --- |
| Duplicate | Create a sibling named `name copy.ext`, adding a number if needed. |
| Copy / Cut | Prepare the selected file or folder for Paste. Cut leaves the original in place until the move succeeds. |
| Paste | Copy or move into the selected folder, or beside the selected file. |
| Rename… | The filename stem is selected initially; the extension remains editable. Invalid names show an inline message and disable Rename. Escape cancels. |
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

Copy, Move and Delete currently use macOS native APIs. Other platforms report
that these operations are unavailable; they do not substitute permanent deletion.
