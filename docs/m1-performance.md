# M1 performance check — 2026-10-24

**M1 is not ready for performance sign-off.** Reading the Linux kernel passes the measured core search and small-file rendering budgets on this machine. Million-path matching, tree-based highlighting of a 10 MB file, and the first process launch exceed their targets. The reference-machine and remaining end-to-end checks are still required.

## Environment and fixtures

- Apple M2 Pro, 16 GiB RAM; macOS 26.4.1 (25E253). The spec's reference is an M1 MacBook Air with 8 GB RAM, so these numbers are not reference-machine certification.
- Rust 1.99.0 (`b940084d7`), release builds with the workspace's thin LTO settings; default plastext settings.
- [Linux 6.12 source archive](https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-6.12.tar.xz), 86,280 files visible to the production ignore-aware walker, approximately 1.6 GB extracted. This fixture contains source files, not git history. SHA-256: `b1a2562be56e42afb3f8489d4c2a7ac472ac23098f1ef1c1e40da601f54625eb`.
- The million-path fixture repeats the real relative paths under distinct `copy-N/` prefixes. It measures matching a million entries, not walking a million on-disk files.
- The large source fixture repeats complete copies of `kernel/sched/core.c` to 10,016,064 bytes. It exercises the full C parser without truncating a declaration.

## Results

Core measurements and their individual samples are saved in [m1-2026-10-24-core.json](benchmarks/m1-2026-10-24-core.json).

| Measurement | Result | Spec budget | Outcome |
|---|---:|---:|---|
| Kernel Quick Open: first completed snapshot from walk start, 5 runs | max 4.41 ms | 100 ms | Core passes |
| Kernel Quick Open: completed update per keystroke, 280 samples | p99 6.51 ms; max 9.23 ms | 16 ms | Core passes |
| 1M synthetic paths: completed update per keystroke, 280 samples | p99 85.63 ms; max 89.13 ms | 16 ms | **Fails** |
| Warm kernel project search: first result, 5 runs | max 1.34 ms | 50 ms | Core passes |
| Kernel file: first paint into an existing window | 17.66 ms | 50 ms | Passes |
| Kernel file: tree-based syntax first paint | 50.90 ms | 100 ms | Passes |
| Kernel file: 1,000 native scroll frames, CPU draw + submit | p99 1.95 ms; max 2.36 ms | 8 ms | Measured CPU portion passes |
| 10 MB C file: first paint into an existing window | 15.45 ms | 50 ms | Passes |
| 10 MB C file: tree-based syntax first paint | 1,133.02 ms | 100 ms | **Fails** |
| 10 MB C file: 300 native scroll frames, CPU draw + submit | p99 2.26 ms; max 2.35 ms | 8 ms | Measured CPU portion passes |
| Launch: process main to first frame, first run after rebuild | 440.5 ms | 200 ms | **Fails** |
| Launch: following 9 runs | 154.1–193.2 ms | 200 ms | Warm runs pass |

The median across all 10 launches was 169.6 ms. The initial outlier is retained; this run did not isolate how much of it came from runtime Metal shader compilation. Startup timing begins inside `main`, so it also excludes process creation and dynamic loading.

Informational core timings: the entire kernel file walk and initial matching took 81.84 ms median / 141.04 ms maximum. Opening `kernel/sched/core.c`, parsing its whole syntax tree and highlighting its first 60 rows took at most 24.97 ms. Computing its syntax folds in the background took at most 5.20 ms.

## Measurement boundaries and follow-up

The core runner uses the same `FileMatcher`, file walker and content search as the UI. Query samples include typing and deleting every prefix of `sched`, `net`, `drivers`, `include` and `core.c`, across five rounds. The clock stops when a fresh completed snapshot is available, including recent-file ranking. The first-result and content-search measurements end at the core API; UI scheduling and painting add latency. In particular, Quick Open's current 16 ms refresh timer is not included in the core result.

Native file-open timing begins with an open request after the window has drawn. Paint timing is reported at the next frame callback, an upper bound within one display refresh after submission. Scrolling measures GPUI's whole frame drawing and submission, not GPU completion or physical display presentation. The 10 MB file has lexical colors during the initial paint while its full tree is built; the reported syntax delay is the time until tree-based coloring is painted.

The long scroll harness checks for both its completion marker and at least the requested frame count. The latest 10,000-frame attempt exited with status 0 after 826 measured frames, without its completion marker; the cause of the early exit was not isolated. The harness rejected it. Only the completed short runs appear in the results table, and a complete 10,000-frame run remains unverified.

The observed failures require work on broad-query matching at one million paths, making a fully parsed viewport available before the whole large file's parse/symbol/fold pipeline completes, and cold startup. Release shader precompilation remains part of M5. A performance sign-off also needs the M1 Air run, a complete 10,000-frame scroll check, native Quick Open input-to-results latency, a real million-file walk, idle RSS/CPU, large-file memory, and launcher handoff timing. Markdown/edit/git budgets belong to later milestones and were not tested here.

## Reproduce

```sh
cargo build --release -p plastext-app -p plastext-bench
curl -L --fail -o /tmp/linux-6.12.tar.xz https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-6.12.tar.xz
tar -xJf /tmp/linux-6.12.tar.xz -C /tmp

target/release/plastext-bench m1 /tmp/linux-6.12 --million --output /tmp/m1-core.json
target/release/plastext-bench launch --runs 10 -- /tmp/linux-6.12
target/release/plastext-bench read /tmp/linux-6.12/kernel/sched/core.c --root /tmp/linux-6.12 --frames 10000

python3 - <<'PY'
from pathlib import Path
source = Path('/tmp/linux-6.12/kernel/sched/core.c').read_bytes()
Path('/tmp/plastext-m1-10mb.c').write_bytes(source * (10_000_000 // len(source) + 1))
PY
target/release/plastext-bench read /tmp/plastext-m1-10mb.c --frames 300
```

`m1` and `read` return nonzero for a measured budget failure, missing results or an incomplete native run. `read` prints the path of its raw frame log and has a timeout; it uses default settings and does not activate the benchmark window. `launch` prints its measurements for comparison with the table. Run performance commands serially, with compilation and other heavy tasks finished.
