# Menu bar

Plastext uses the native macOS menu bar. Menu commands share the same actions as
keyboard shortcuts; editing commands apply to the focused control.

- **File:** New File, New Window, Open File, Open Folder, Save, Close Tab and
  Close Window. Open File adds selected files to the current window. Open Folder
  opens a project window, reusing one already showing that folder. New File uses
  the current project's name dialog.
- **Edit:** Undo/Redo, Cut/Copy/Paste, Select All, Find, Find Next/Previous and
  Find in Files. Clipboard commands work on editor text, search fields, Markdown
  or CSV selections, and files selected in the navigation pane. Read-only views
  do not offer text mutations.
- **View:** Command Palette, Files, Outline, Source/Preview, zoom, split panes
  and file icon themes. Source/Preview also switches PDF to Binary and CSV to
  its source view.
- **Go:** Back/Forward, Go to File, Line and Symbol.
- **Window:** Minimize, Zoom and Full Screen, plus the system's window list.
- **Help:** Documentation and issue reporting, opened in the default browser.
- **Plastext:** About, Services, Hide/Show and Quit.

Save remains explicit. Close Tab, Close Window and Quit use the existing
Save / Discard / Cancel prompt when there are unsaved changes.

| Shortcut | Command |
| --- | --- |
| ⌘N | New File |
| ⌘⇧N | New Window |
| ⌘O | Open File |
| ⌘⌥O | Open Folder |
| ⌘S | Save the active file, including while the navigation pane has focus |
| ⌘W / ⌘⇧W | Close Tab / Close Window |
| ⌘⌥B | Toggle Outline |
| ⇧⌘V | Switch Source / Preview |
| ⌘M | Minimize |
| ⌃⌘F | Toggle Full Screen |

Existing navigation shortcuts, including ⌘⇧O for symbols and ⌘D / ⌘⇧D for
splits, are unchanged. Menus display shortcuts and disable commands without a
handler in the current focus context.
