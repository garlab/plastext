# File-icon themes

Open the command palette (`⇧⌘P`) and choose **File Icon Theme**. Select **Material**, **None**, or an imported theme. The choice applies to all open windows and is saved for the next launch. Colour themes are selected independently.

Choose **Import Icon Theme…** from that picker or directly from the command palette. Select either:

- A VS Code file-icon theme JSON/JSONC file, with its image or font assets alongside it.
- An extracted VS Code extension folder containing `package.json`. A folder containing an `extension/` subdirectory is also accepted. If the extension contributes several icon themes, select one from the picker.

Imports reference the original local files. Keep the theme folder in place; moving it requires importing it again. Re-select a theme to reload its definitions and images after editing them. If the selected theme cannot load on startup, Material remains available and the status bar explains the error. A failed switch keeps the current theme.

## Using VS Code’s Seti theme

On macOS, copy the theme out of the application bundle so the file picker can reach it:

```sh
cp -R "/Applications/Visual Studio Code.app/Contents/Resources/app/extensions/theme-seti" "$HOME/Downloads/vscode-seti-theme"
```

Choose **Import Icon Theme…**, then select **vscode-seti-theme** in Downloads. Keep the folder in place after importing. You can also import compatible icon extensions from `~/.vscode/extensions/`.

## Compatibility

Plastext imports images and outline-font glyphs from the [VS Code file-icon theme format](https://code.visualstudio.com/api/extension-guides/file-icon-theme):

- SVG and PNG `iconPath` definitions, with relative paths resolved from the theme file.
- WOFF 1, TTF and OTF font sources, including Seti. `fontCharacter` accepts a Unicode character or a CSS escape such as `\E001`. `fontId`, hex `fontColor`, percentage `fontSize`, the first-font default, and inherited generic-file font properties are supported. Glyphs are centred and contained in the icon slot.
- Filename, compound-extension and language-ID associations. Matching names is case-insensitive; filenames take precedence over extensions, then language IDs. Within a match type, a parent-folder qualifier takes precedence; longer extensions such as `d.ts` precede `ts`.
- Generic and named folders, expanded folders, and root-folder definitions. The current file tree displays the root's children rather than a root row.
- Light and high-contrast association overrides, plus `hidesExplorerArrows`.
- JSON comments, trailing commas, and `contributes.iconThemes` discovery from extension manifests.

Language IDs use plastext's existing filename-based language detection. Additional language extensions and their default icons are not loaded. WOFF2, font collections, bitmap/colour-font glyphs, and multi-character ligatures are not supported. Fonts use the outlines in the supplied file, without synthesizing weight or style. Extension code, Marketplace installation and direct `.vsix` installation are not supported.

Theme JSON is limited to 2 MiB. Each icon may contain up to 256 KiB; PNG inputs may be up to 1024×1024 pixels. Assets must stay inside the imported theme or extension directory, including through symlinks. Remote assets and external SVG images/fonts are not loaded. A theme can declare up to eight fonts. Each font source is limited to 2 MiB, each decoded font to 4 MiB, and retained font data to 8 MiB per theme. Font sizes must be percentages from 1% to 400%.

Font files are loaded once on background workers when selecting a theme. Individual glyphs and image icons are rasterized only as needed into the shared 128-entry cache, with at most four concurrent jobs. Selecting another theme releases the old font data after pending jobs finish and clears the image cache. Imported fonts are never installed system-wide. Project files are never opened to determine their icons.

## Configuration

The picker updates only the `[icons]` section of `config.toml`, preserving other settings and comments. Built-in selections are:

```toml
[icons]
theme = "material" # or "none"
high_contrast = false
```

Normally the theme's light or dark associations follow the window appearance. Set `high_contrast = true` to use its `highContrast` associations instead. Manual configuration edits take effect after restarting; picker changes apply immediately.

Imported themes are registered automatically. They can also be configured with paths relative to `config.toml`:

```toml
[icons]
theme = "icon-themes/example/themes/icons.json"

[[icons.imports]]
name = "Example"
path = "icon-themes/example/themes/icons.json"
root = "icon-themes/example"
```

`root` is the directory containing the theme and its assets. Selecting an extension folder establishes this automatically, allowing theme files in `themes/` to reference `../icons/`.

The bundled Material artwork retains its [MIT notice](../THIRD_PARTY_NOTICES.md). Imported artwork stays in its original directory with its own licence files.
