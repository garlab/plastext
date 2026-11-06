# Local development

See [Building](../README.md#building) for prerequisites. Run the commands below from the repository root unless noted otherwise.

## Try another folder

Run `cargo run -p plastext-app` without a path for the welcome screen, with
**Open Folder**, **Open File** and up to ten recent folders. Opening a folder
reuses an empty welcome window. Recent folders are stored locally in
`recent-folders.json` beside `config.toml`; **Clear Recent** clears this history.

Pass the folder to the app:

```sh
cargo run -p plastext-app -- ../another-project
```

Replace `../another-project` with the folder you want to open. Relative paths are resolved from your current directory.

On macOS, the running app sets its Dock and app-switcher icon automatically, including with `cargo run`. Quit any older running instance before trying an updated build. The editable SVG and regeneration instructions are in [app icon assets](../crates/app/assets/README.md).

## Install for your user

Install the app and launcher together:

```sh
cargo install --path crates/app --locked
cargo install --path crates/cli --locked
```

Cargo installs release builds into `~/.cargo/bin` by default, or `$CARGO_HOME/bin` if configured. Keep both binaries in the same directory: the `plastext` launcher starts the neighboring `plastext-app` binary.

Make sure Cargo's bin directory is on your `PATH`:

```sh
export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
```

Then, from any project directory:

```sh
plastext .
```

The installed binaries are independent copies. Editing the checkout or rebuilding it does not update them. Rerun both install commands to update, then quit and relaunch the app to use the new build.

## Run development independently

By default, launches share the running app instance, including across installed and development builds. Quit any existing development instance after installing so the next `plastext` launch starts the installed build.

To run a development instance alongside the installed app, give it a separate socket and settings directory:

```sh
PLASTEXT_SOCKET="${TMPDIR:-/tmp}/plastext-dev.sock" \
XDG_CONFIG_HOME="$PWD/target/dev-config" \
cargo run -p plastext-app -- .
```

Use this command from the repository root for subsequent development launches. Development settings are read from `target/dev-config/plastext/config.toml`; a missing file uses the defaults. This directory is ignored by Git. The installed app continues to use its normal settings, by default `~/.config/plastext/config.toml`.

## Native startup check

GPUI unit tests use a mock text renderer. After changing startup UI or text layout,
also run `cargo build -p plastext-app` and `python3 scripts/check-native-startup.py`.
The check briefly opens native windows with empty history and Unicode recent-folder
names, using temporary settings. Each window exits after its first frame. Pass a
binary path to check a release build instead.

## Shared rendering libraries

Math uses the published `katex-native` crate. Mermaid/SVG/SGF/chess rendering, image decoding and worker
supervision use the [document-graphics](https://crates.io/crates/document-graphics)
crate, maintained in [its own repository](https://github.com/garlab/document-graphics).
Plastext retains its own queue, cache and UI code; shared renderer and protocol
tests belong in that library.

Plastext pins published `document-graphics` 0.1.7 with `sgf`, `chess` and
`image-native`. The Mermaid backend and math dependencies also resolve from
crates.io. Normal builds need no local dependency overrides.

To work on document-graphics alongside Plastext, place its checkout beside
Plastext and add the following to an untracked `.cargo/config.toml` in the
Plastext checkout. Merge it into any existing `[patch.crates-io]` table rather
than replacing other local settings:

```toml
[patch.crates-io]
document-graphics = { path = "../document-graphics" }
```

Adapt the relative path if the checkouts are elsewhere. Keep this checkout
outside `target/` so `cargo clean` cannot remove the dependency. Run Cargo from
the Plastext root so it reads the local override. Before validating a published
release, remove the override and run `cargo check -p plastext-app` to restore the
registry source and checksum in `Cargo.lock` using the version pinned in
`Cargo.toml`. See
[shared rendering release checks](shared-rendering-release.md) for review and
validation commands.

For code-only changes with unchanged package versions and dependency graphs,
Cargo's `paths` override keeps `Cargo.lock` on its published versions. To test
both the shared library and its Mermaid backend, use this top-level setting
instead of the patch table above:

```toml
paths = ["../document-graphics", "../document-graphics-mermaid"]
```

The backend checkout's package name is `document-graphics-mermaid`. Keep these
paths in the ignored `.cargo/config.toml`. This override does not support adding
or changing dependencies; use `[patch.crates-io]` in that case. Remove the local
overrides when verifying a published release.

Each app selects raster fitting, cache size and worker lifetime; shared renderer
changes should preserve those policies unless explicitly intended.
