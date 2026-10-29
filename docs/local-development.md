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
