# Image previews

Images open centered at their natural size, shrinking to fit both dimensions of
the pane when necessary. **Fit** restores that view; **1:1** uses one logical
screen point per source pixel. Use the toolbar or `⌘+` / `⌘−` to zoom, pinch on a
Mac trackpad, or hold Command/Control while scrolling. Drag or scroll to pan.
Zooming with the pointer keeps the source point beneath it fixed.

Transparent pixels have a checkerboard background. A pixel grid appears at 8×
zoom. The status bar shows dimensions, encoded file size, format and color
profile/model. An expanded EXIF inspector is not implemented.

GIF and WebP animations play automatically, respect frame delays and finite
loop counts, and provide **Play/Pause** (`Space`). A complete next frame replaces
the current one when ready. Hiding the tab, switching to source, or closing it
stops decoding and releases its image cache. Split panes navigate independently.

**Source** (`⇧⌘V`) shows SVG XML with tag/attribute highlighting. Raster images
provide **Binary** instead. Both modes preserve the preview's zoom and pan.
Source remains read-only until M3 editing.

## Formats and platform support

On macOS, ImageIO handles PNG, JPEG, GIF, WebP, BMP, ICO, TIFF, AVIF and HEIC/HEIF.
Other installed ImageIO codecs can handle ICNS, PSD, TGA and JPEG XL. QOI and PNM
also have Rust decoders. SVG uses resvg. No external image tools, codec downloads
or network access are needed at runtime.

AVIF, HEIC and other native formats depend on the codecs supplied by the installed
macOS version. An unavailable codec produces a preview error with Binary still
accessible. EXIF orientation is applied before displaying image dimensions or
mapping tiles. Native crops preserve codec block alignment, and PNG tiles use
the source color profile so their colors match the overview.

The application is currently supported on macOS. The portable decoder provides
bounded still-image decoding for future Unix ports; native animation, HEIC/AVIF,
pinch input and worker restrictions need platform-specific implementations there.

## Large images and resource bounds

The viewer keeps a small overview and requests physical-pixel tiles near the
viewport. SVG is rasterized at the requested scale. PNG tiles stream through
filter rows, retaining only requested pixels; this also handles Adam7 images.
A 40,000 × 40,000 PNG does not require a full decoded allocation. Other raster
formats use ImageIO's overview and region decoding. If a codec requires too much
memory to produce a region, the overview remains visible with a limit message.

Decoding runs in a disposable helper process. Changes to zoom, pan or pane size
cancel obsolete requests; a 60 ms settling interval coalesces continuous input.
Existing pixels stay visible while replacement tiles arrive. Animations retain
the displayed frame and at most one prepared replacement, rather than every
decoded frame.

| Resource | Bound |
| --- | --- |
| Encoded image | 256 MiB; SVG and data URI payloads: 32 MiB |
| Source dimensions | 100,000 pixels per side; 2 billion total pixels |
| Animation frame count | 10,000 |
| One render request | 16 regions, at most 4,194,304 output pixels |
| Tile edge | 512 physical pixels |
| Image-tab cache | 64 MiB shared across panes, charging both CPU pixels and a GPU copy |
| Worker | One decoder process; 10-second request deadline; 256 MiB resident-memory kill threshold on macOS |
| File snapshots | 1 GiB total on disk; copied in 256 KiB chunks, unlinked immediately |

Each visible image pane receives a share of the cache. Under pressure, raster
resolution falls while image geometry stays fixed. Very small shares use only
the overview. ImageIO frame caches are discarded as playback advances; the idle
worker exits after three seconds. Memory checks run every 20 ms, so a decoder
can briefly overshoot the kill threshold. GPU atlas overhead and allocations in
OS codec services are outside the cache's byte accounting.

The worker inherits an immutable snapshot descriptor, and on macOS denies
networking, writes and child execution. System codec/font reads remain allowed.
SVG external and embedded image references are disabled. Markdown's local and
data-URI images use the same worker for static overviews; remote images remain
blocked. Its cache is limited to 64 MiB/128 entries per visible document, with
at most four pending requests; hiding a document releases those pixels.

## Verification

The integration suite exercises the actual worker protocol and sandbox, native
formats, orientation, color profiles, region boundaries, GIF/WebP playback,
worker recycling and immutable snapshots. Unit tests cover Adam7 decoding,
pixel/request limits, cancellation, memory/deadline guards, fit/zoom geometry,
finite playback and cache shares across split panes.

```sh
cargo test -p plastext-assets --lib
cargo test -p plastext-platform --lib images
cargo test -p plastext-platform --test image_preview
cargo test -p plastext-ui --lib image_view
```

Generate the large fixture and run the optional decoder benchmark from the
repository root:

```sh
python3 scripts/make-large-image.py target/image-qa/huge.png
PLASTEXT_TEST_HUGE_IMAGE="$PWD/target/image-qa/huge.png" \
  cargo test --release -p plastext-platform --test image_preview
```

The generated PNG is 40,000 × 40,000 grayscale pixels with a repeating row
pattern, about 7.1 MB encoded. On an M2 Pro with 16 GiB RAM and macOS 26.4.1, a
release run took **1.24 seconds** to open and render the overview and **137 ms**
for a 512 × 512 tile at the bottom-right corner. Together those output buffers
retain **10.7 MiB**. The worker stayed within its memory/time guards. These are
decoder measurements on a compressible fixture, not a reference-M1 UI frame-time
certification or a guarantee for every codec. Large fixtures and raw logs remain
local under ignored `target/`; only summaries belong in Git.
