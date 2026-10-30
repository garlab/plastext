# Local development

See [Building](../README.md#building) for prerequisites. Run the commands below from the repository root unless noted otherwise.

## Try another folder

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

## Shared rendering libraries

Math uses the published `katex-native` crate. Mermaid/SVG rendering and worker
supervision use one shared library, [document-graphics](https://github.com/garlab/document-graphics),
maintained in its own repository. Plastext retains its own queue, cache and UI code; shared renderer
and protocol tests belong in that library.

`document-graphics` 0.1.0 is awaiting its first crates.io release. For now, place the
document-graphics checkout beside Plastext and add the following to an untracked
`.cargo/config.toml` in the Plastext checkout. Merge it into any existing
`[patch.crates-io]` table rather than replacing other local settings:

```toml
[patch.crates-io]
document-graphics = { path = "../document-graphics" }
```

Adapt the relative path if the checkouts are elsewhere. Keep this checkout
outside `target/` so `cargo clean` cannot remove the dependency. Run Cargo from
the Plastext root so it reads the local override.

The current lockfile resolves this unpublished crate locally. After publication,
remove only its override and update the lockfile to record the registry source
and checksum. See [shared rendering release checks](shared-rendering-release.md)
for review, validation and publication commands.

The same override is useful for later library development. Once the registry
version is in use, keep temporary path-resolution changes to `Cargo.lock` out of
commits. Each app selects raster fitting, cache size and worker lifetime; shared
renderer changes should preserve those policies unless explicitly intended.
