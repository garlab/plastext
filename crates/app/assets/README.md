# App icon

[plastext.svg](plastext.svg) is the editable source: a folded-page P with text lines on a midnight-blue tile. The artwork is original to Plastext and uses the repository's MIT OR Apache-2.0 licence.

The checked-in derivatives are a 1024×1024 [PNG](plastext.png) and a multi-resolution [macOS icon](plastext.icns). On macOS, regenerate them from the repository root:

```sh
cargo run -p plastext-assets --example generate_app_icon
```

The generator renders each size directly from the SVG and uses macOS `iconutil` to create the ICNS. Intermediate PNGs stay in the ignored `target/app-icon/` directory. Normal builds use the checked-in icon; they do not need to regenerate it.

The app embeds the ICNS and sets its running Dock and app-switcher icon through AppKit, so `cargo run` and Cargo-installed executables show the icon without an `.app` bundle. When packaging a bundle, copy the ICNS into `Contents/Resources/` and set `CFBundleIconFile` to `plastext.icns` in its `Info.plist` to give the bundle a Finder icon too.
