# Shared rendering release checks

`document-graphics` is maintained in its own repository and published on
crates.io from version tags. Plastext pins published 0.1.5; `katex-native` also
comes from the registry. Use a
[local override](local-development.md#shared-rendering-libraries) only when
reviewing changes before a library release.

## Review and automated checks

Review the shared crate's public API and README, both app adapters, and the
package file list. Rendering and process supervision belong to the library;
queue, cache, fitting and idle policies belong to each app.

Run these commands from the **document-graphics root**. Running Cargo from Plastext with
`--manifest-path` still reads Plastext's local overrides and can change package
resolution.

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked --no-deps -- -D warnings
cargo test --all-features --locked
cargo +1.90.0 check --all-targets --all-features --locked
cargo package --list --locked
cargo package --all-features --locked
```

Install Rust 1.90.0 with rustup if needed for the minimum-version check. Run worker
tests in a normal terminal: on macOS they install their own process sandbox and
cannot run inside an enclosing sandbox. The package command verifies the crate without uploading it. Package from a clean checkout; the include list
limits the archive to library source, tests, metadata, README and license.

From the **mdviewer root**, with its own local override pointing to the same
standalone library checkout:

```sh
cargo test --locked
cargo build --locked
python3 tests/terminal_smoke.py target/debug/mdviewer
python3 tests/pager_smoke.py target/debug/mdviewer
```

From the **Plastext root**, with the local override active:

```sh
cargo check --locked -p plastext-app
cargo test --locked -p plastext-assets -p plastext-math -p plastext-platform -p plastext-ui
```

Keep generated PNGs, timing samples and other review artifacts in ignored
`target/` directories. Existing performance baselines remain fixed.

## Manual checks

In Plastext, open `docs/fixtures/mermaid.md` and `docs/fixtures/m2.md` using the
isolated development launch described in [local development](local-development.md#run-development-independently).

- Check diagrams and math in light and dark appearances, including labels,
  arrows, transparency and sharpness at different zoom levels.
- Split the view, resize panes and scroll away from unfinished diagrams. Check
  responsiveness and that returning to a cached diagram preserves its appearance.
- Toggle Source/Preview, use find, and check that unsupported diagrams retain
  readable source. Close tabs while rendering and reopen them.
- Open an ordinary Markdown file without diagrams and check navigation and
  scrolling against normal behavior.

In a Kitty graphics-compatible terminal, run these from the **mdviewer root**:

```sh
cargo run --release -- tests/fixtures/mermaid.md --theme dark
cargo run --release -- tests/fixtures/mermaid.md --theme light --show-mermaid-source
cargo run --release -- tests/fixtures/mermaid.md --mermaid off
```

Check narrow terminal widths, source fallback and local SVG/math fixtures too.
Automated pixel comparisons and worker tests do not replace these interaction
checks. OS sandbox and memory enforcement outside macOS remain documented limits
in the shared crate's README.

## Updating to a new release

document-graphics publishes from version tags through its release workflow; see
its [release checks](https://github.com/garlab/document-graphics/blob/main/docs/releasing.md).
After a release, remove local `document-graphics` and `document-graphics-mermaid`
overrides from Plastext's `paths` or `[patch.crates-io]` configuration if present,
set the new version pin in the root `Cargo.toml`, then from the **Plastext root**:

```sh
cargo check -p plastext-app
cargo test --locked -p plastext-platform --test mermaid_preview
cargo test --locked -p plastext-platform --test image_preview
cargo test --locked -p plastext-ui markdown_view::mermaid
```

The first check allows Cargo to replace the removed path override in `Cargo.lock`
with the pinned registry release. Subsequent checks use `--locked`.

Review and commit the `Cargo.lock` registry source and checksum. Do the same in
mdviewer and rerun its integration tests. Both apps pin the library's version
explicitly; it has its own version and release cycle.
