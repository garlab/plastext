# Contributing

Issues and pull requests are welcome. The [specification](docs/specs.md)
describes the product and its milestones, and the [roadmap](docs/roadmap.md) the
remaining work. Unless you state otherwise, contributions are dual licensed under
Apache-2.0 and MIT, as described in the [README](README.md#license).

## Building and running

You need macOS on Apple Silicon and a stable Rust toolchain (1.99 or later). The
Xcode Command Line Tools are enough; full Xcode isn't required.

```sh
cargo run -p plastext-app        # welcome screen: open a folder, file or recent folder
cargo run -p plastext-app -- .   # open the current folder
cargo test --workspace
cargo build --release            # builds target/release/plastext and plastext-app
```

Default builds use the `runtime-shaders` feature so development works with the
Command Line Tools alone. With full Xcode and its Metal compiler installed,
precompile the shaders to avoid runtime shader compilation on a cold launch:

```sh
cargo build --release -p plastext-app --no-default-features
cargo build --release -p plastext-cli
```

Use the package-specific commands: building the whole workspace enables the UI
crates' default features again.

[Local development](docs/local-development.md) explains how to open other
folders, run a development instance alongside the installed app with separate
settings, check native startup, and work on the shared rendering libraries
([document-graphics](https://crates.io/crates/document-graphics) and
[katex-native](https://crates.io/crates/katex-native)) with a local override.

## Checks

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
bash scripts/check-gpui-boundary.sh
python3 scripts/check-benchmark-artifacts.py
```

After changing startup UI or text layout, also run `cargo build -p plastext-app`
and `python3 scripts/check-native-startup.py`.

## Workspace layout

The workspace follows spec §7.3, with one crate per concern under `crates/`. Only
`app`, `ui` and `editor` may depend on GPUI. Every other crate is UI-agnostic and
must build and pass its tests on macOS, Linux and Windows. CI enforces both
rules; the first is checked by `scripts/check-gpui-boundary.sh`.

## Benchmarks

```sh
cargo bench -p plastext-bench --bench text              # text engine micro-benchmarks
cargo build --release -p plastext-app -p plastext-bench
target/release/plastext-bench launch -- .               # time to first frame, 10 launches
target/release/plastext-bench log 10GB /path/big.log    # spec §6.2 fixtures; also long-line, wide-dir
target/release/plastext-bench m1 /path/linux-6.12 --million --output /tmp/m1.json
target/release/plastext-bench read /path/linux-6.12/kernel/sched/core.c --root /path/linux-6.12 --foreground
python3 scripts/m2-fixtures.py /tmp/plastext-m2          # 100 MB Markdown + 20M CSV records
target/release/plastext-bench m2 /tmp/plastext-m2/large.md --output /tmp/m2-markdown.json
target/release/plastext-bench csv /tmp/plastext-m2/large.csv
target/release/plastext-bench read /tmp/plastext-m2/large.md --frames 10000 --foreground --keep-visible --output /tmp/m2-native.json
```

`m1` measures first streamed Quick Open results and completed query rankings
separately (including cached backspacing), plus warm project search. `--million`
also tests one million synthetic paths derived from the real tree. `read` drives
10,000 native scroll frames and measures first paint, initial viewport
highlighting, full-tree availability and GPUI frame drawing and submission.
`--foreground` activates its window; leave it visible because macOS suspends
drawing for covered windows. Both runners return a nonzero exit code when a
measured budget fails. These checks do not substitute for the reference M1 Air
run or measure GPU completion, idle memory, or features from later milestones.
See [the M1 performance report](docs/m1-performance.md) for results and
remaining gaps.

The `m2` command measures the Markdown index, bounded page cache, rendered search
and memory; `csv` measures core indexing. Native scrolling and milestone sign-off
require separate checks. Open [the M2 demo](docs/fixtures/m2.md) to exercise the
Markdown views.

`read` also profiles Markdown, including initial preview paint, completion of
the full model, and scrolling through distant sections. `--keep-visible` uses a
temporary popup window above other apps to prevent occlusion; it requires
`--foreground` and changes only benchmark windows. The runner saves failed and
incomplete results with `--output`, and aborts after 30 seconds without frame
progress.

Benchmark `--output` files contain summaries only. Full sample arrays and native
logs stay in ignored `target/benchmarks/`; the math corpus runner has a separate
`--summary` option. Only `*.summary.json` reports go into
[docs/benchmarks](docs/benchmarks/README.md), with a CI check against raw data and
machine-specific paths.
