# plastext

A fast, native, read-first viewer and light editor for code, Markdown and assets. It's built for people who write code through LLM agents in the terminal and mostly need to read, review and tweak it.

> **Status:** early scaffolding. It opens a window and nothing more yet. The full specification is in [docs/specs.md](docs/specs.md).

## Building

You need macOS on Apple Silicon and a stable Rust toolchain (1.99 or later). The Xcode Command Line Tools are enough; full Xcode isn't required.

```sh
cargo run -p plastext-app -- .   # open the current folder
cargo test --workspace
cargo build --release            # builds target/release/plastext and plastext-app
```

`plastext` is the command-line launcher. It starts the `plastext-app` binary sitting next to it, so keep the two in the same directory.

## Layout

The workspace follows spec §7.3, with one crate per concern under `crates/`. Only `app`, `ui` and `editor` may depend on GPUI. Every other crate is UI-agnostic and must build and pass its tests on macOS, Linux and Windows. CI enforces both rules; the first is checked by `scripts/check-gpui-boundary.sh`.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in plastext by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
