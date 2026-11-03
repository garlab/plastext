# Image fixtures

These are generated 32 × 24 images containing two colored rectangles, under the
repository's MIT/Apache-2.0 licenses. No photographs or third-party artwork are
included.

- `sample.avif`: AVIF encoded with Pillow/libavif.
- `sample.heic`: HEIC encoded with macOS ImageIO.
- `oriented.jpg`: ImageIO JPEG with EXIF orientation 6 (90° clockwise).
- `profile.png`: ImageIO PNG with a Display P3 color profile.

The decoder integration test compares cropped pixels with the overview, checks
orientation, and exercises the installed macOS codecs. Animation fixtures are
generated in a temporary directory during the test. Huge generated images and
raw benchmark logs stay in ignored `target/` directories.
