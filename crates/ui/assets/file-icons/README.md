# File icons

Unmodified SVGs from [Material Icon Theme](https://github.com/material-extensions/vscode-material-icon-theme), revision [`23e86c06d09e92571ef027184c00cad78e8c827f`](https://github.com/material-extensions/vscode-material-icon-theme/tree/23e86c06d09e92571ef027184c00cad78e8c827f/icons).

The artwork is MIT-licensed; the upstream [LICENSE](LICENSE) is retained here. The notice is also included in the repository's `THIRD_PARTY_NOTICES.md` and embedded in both binaries, available with `plastext --notices` or `plastext-app --notices`.

[material-icon-theme.json](material-icon-theme.json) contains plastext's filename and extension associations in VS Code's file-icon theme format. The same resolver loads bundled and imported themes. [file_icons.rs](../../src/file_icons.rs) embeds the artwork; the file-tree cache loads it on workers without reading project files or using the network. `toml_light.svg` is the upstream variant for light backgrounds.

To update an icon, copy its original SVG from a pinned upstream revision and update this reference and the notices if needed. Keep the original filenames and licence.
