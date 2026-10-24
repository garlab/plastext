# M1 performance check — 2026-10-24

**M1 is not ready for performance sign-off.** The optimization pass makes a 10 MB file's initial tree-highlighted viewport available in 25.31 ms instead of 1,133.02 ms. Million-path Quick Open streams fresh results at 2.28 ms p99, while completing its full ranking improves from 85.63 to 28.68 ms p99 and still misses 16 ms. Cold launch, the reference-machine run and the remaining end-to-end checks are still outstanding.

## Environment and fixtures

- Apple M2 Pro, 16 GiB RAM; macOS 26.4.1 (25E253). The spec's reference is an M1 MacBook Air with 8 GB RAM, so these numbers are not reference-machine certification.
- Rust 1.99.0 (`b940084d7`), release builds with the workspace's thin LTO settings; default plastext settings.
- [Linux 6.12 source archive](https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-6.12.tar.xz), 86,280 files visible to the production ignore-aware walker, approximately 1.6 GB extracted. This fixture contains source files, not git history. SHA-256: `b1a2562be56e42afb3f8489d4c2a7ac472ac23098f1ef1c1e40da601f54625eb`.
- The million-path fixture repeats the real relative paths under distinct `copy-N/` prefixes. It measures matching a million entries, not walking a million on-disk files.
- The large source fixture repeats complete copies of `kernel/sched/core.c` to 10,016,064 bytes. It exercises the full C parser without truncating a declaration.

## Baseline at `98fc598`

Original core measurements and their individual samples are saved in [m1-2026-10-24-core.json](benchmarks/m1-2026-10-24-core.json). These are retained for comparison with the optimization pass below.

| Measurement | Result | Spec budget | Outcome |
|---|---:|---:|---|
| Kernel Quick Open: first completed snapshot from walk start, 5 runs | max 4.41 ms | 100 ms | Core passes |
| Kernel Quick Open: completed update per keystroke, 280 samples | p99 6.51 ms; max 9.23 ms | 16 ms | Core passes |
| 1M synthetic paths: completed update per keystroke, 280 samples | p99 85.63 ms; max 89.13 ms | 16 ms | **Fails** |
| Warm kernel project search: first result, 5 runs | max 1.34 ms | 50 ms | Core passes |
| Kernel file: first paint into an existing window | 17.66 ms | 50 ms | Passes |
| Kernel file: tree-based syntax first paint | 50.90 ms | 100 ms | Passes |
| Kernel file: 1,000 native scroll frames, draw + submit wall time | p99 1.95 ms; max 2.36 ms | 8 ms | Measured portion passes |
| 10 MB C file: first paint into an existing window | 15.45 ms | 50 ms | Passes |
| 10 MB C file: tree-based syntax first paint | 1,133.02 ms | 100 ms | **Fails** |
| 10 MB C file: 300 native scroll frames, draw + submit wall time | p99 2.26 ms; max 2.35 ms | 8 ms | Measured portion passes |
| Launch: process main to first frame, first run after rebuild | 440.5 ms | 200 ms | **Fails** |
| Launch: following 9 runs | 154.1–193.2 ms | 200 ms | Warm runs pass |

The median across all 10 launches was 169.6 ms. The initial outlier is retained; this run did not isolate how much of it came from runtime Metal shader compilation. Startup timing begins inside `main`, so it also excludes process creation and dynamic loading.

Informational core timings: the entire kernel file walk and initial matching took 81.84 ms median / 141.04 ms maximum. Opening `kernel/sched/core.c`, parsing its whole syntax tree and highlighting its first 60 rows took at most 24.97 ms. Computing its syntax folds in the background took at most 5.20 ms.

## Optimization pass

The matcher now retains the best 256 rows using parallel bounded heaps instead of sorting every match. It keeps nucleo's scoring, smart case and Unicode normalization, including the recent-file ranking. A conservative character-presence filter skips impossible ASCII candidates. Completed queries are cached (32 entries), stale jobs are cancelled, and indexing notifications are batched. For large indexes, a prefix scan publishes provisional results while the full ranking continues; results can move as better matches arrive. Quick Open wakes on notifications instead of polling every 16 ms.

The editor parses a prefix of at most 64 KiB first, compiles its highlight query in the background, and publishes that tree before parsing the rest. The full tree is then published before symbol/fold extraction. Scrolling outside the prefix uses lexical highlighting until the full tree arrives. Initial colors are provisional where syntax crosses the prefix boundary; structural features always use the full tree.

Measurements and samples are saved in [optimized core results](benchmarks/m1-2026-10-24-optimized-core.json) and [optimized native results](benchmarks/m1-2026-10-24-optimized-native.json).

| Measurement | Optimized result | Spec budget | Outcome |
|---|---:|---:|---|
| Kernel Quick Open: first snapshot from walk start, 5 runs | max 3.93 ms | 100 ms | Core passes |
| Kernel Quick Open: first fresh update, 280 samples | p99 0.80 ms; max 1.35 ms | 16 ms | Core passes |
| Kernel Quick Open: completed ranking, 280 samples | p99 2.64 ms; max 2.84 ms | 16 ms | Core passes |
| 1M synthetic paths: first fresh update, 280 samples | p99 2.28 ms; max 2.51 ms | 16 ms | Core passes |
| 1M synthetic paths: completed ranking, 280 samples | p99 28.68 ms; max 35.21 ms | 16 ms | **Still fails** |
| Warm kernel project search: first result, 5 runs | max 0.94 ms | 50 ms | Core passes |
| Kernel file: first paint / initial tree-highlighted viewport | 16.54 / 33.52 ms | 50 / 100 ms | Timings pass; scroll run incomplete |
| Kernel file: full-tree paint | 49.28 ms | Informational | Does not wait for symbols/folds |
| Kernel file: 7,487 of 10,000 requested native frames | p99 7.45 ms; max 9.32 ms | 8 ms p99 | **Incomplete run** |
| 10 MB C file: first paint / initial tree-highlighted viewport | 16.98 / 25.31 ms | 50 / 100 ms | Passes on this fixture |
| 10 MB C file: full-tree paint | 800.31 ms | Informational | Full parse continues in background |
| 10 MB C file: 300 native scroll frames, draw + submit | p99 2.25 ms; max 2.37 ms | 8 ms | Measured portion passes |

The whole kernel walk plus matching took 99.04 ms median / 141.79 ms maximum; this did not materially improve. Repeated and backspaced queries hit the cache, producing sub-0.01 ms medians; the first round still includes uncached queries. Both first-result and complete-ranking times are recorded, and the harness **continues to fail** if complete ranking exceeds 16 ms. A quick provisional result is not counted as completion of the million-entry ranking.

The native table uses `read --foreground`. An earlier completed background-window 300-frame run had a 32.47 ms initial syntax paint but 16.28 ms p99 draw/submit time; that failed run is also retained in the native results. GPUI's timer measures wall time inside drawing and submission, including any driver waits, rather than isolated CPU work or GPU completion. Foreground versus background presentation affects these timings. A covered window can also stop GPUI's frame callbacks entirely on macOS.

The latest 10,000-frame run stopped after 7,487 frames with the explicit `reading: last window closed` marker. It exited with status 0, but the benchmark rejected it because the completion marker was absent. The partial samples are retained, not accepted as a successful long run. This replaces the earlier unexplained 826-frame attempt as the latest evidence; a completed 10,000-frame check is still required.

Shader precompilation is now selectable with `cargo build --release -p plastext-app --no-default-features`. The default remains compatible with Command Line Tools through the `runtime-shaders` feature. The no-default-feature dependency graph was checked, but the precompiled build and its cold-launch effect could not be tested here: `xcrun --find metal` reports that the Metal compiler is unavailable. The baseline launch failure therefore remains unresolved; no startup improvement is claimed.

## Measurement boundaries and follow-up

The core runner uses the same `FileMatcher`, file walker and content search as the UI. Query samples include typing and deleting every prefix of `sched`, `net`, `drivers`, `include` and `core.c`, across five rounds. The baseline recorded completed snapshots only. The optimized runner records both the first fresh nonempty snapshot (or completed empty result) and the completed ranking, including recency. Cached results count. These and content-search measurements end at the core API; UI scheduling and painting add latency. The former 16 ms UI polling interval was excluded from the baseline and has now been removed.

Native file-open timing begins with an open request after the window has drawn. Paint timing is reported at the next frame callback, an upper bound within one display refresh after submission. Scrolling measures GPUI's whole frame drawing and submission, not GPU completion or physical display presentation. Files have lexical colors before a tree is available; the optimized syntax delay reports the initial prefix tree, with full-tree paint recorded separately. Scrolling starts immediately after initial tree highlighting, so it includes the background full parse and structural extraction.

Remaining performance work includes reducing complete million-path ranking and verifying cold startup with precompiled shaders. A performance sign-off also needs the M1 Air run, a complete 10,000-frame scroll check, native Quick Open input-to-results latency, a real million-file walk, idle RSS/CPU, large-file memory, and launcher handoff timing. Markdown/edit/git budgets belong to later milestones and were not tested here.

## Reproduce

```sh
cargo build --release -p plastext-app -p plastext-bench
curl -L --fail -o /tmp/linux-6.12.tar.xz https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-6.12.tar.xz
tar -xJf /tmp/linux-6.12.tar.xz -C /tmp

target/release/plastext-bench m1 /tmp/linux-6.12 --million --output /tmp/m1-core.json
target/release/plastext-bench launch --runs 10 -- /tmp/linux-6.12
target/release/plastext-bench read /tmp/linux-6.12/kernel/sched/core.c --root /tmp/linux-6.12 --frames 10000 --foreground

python3 - <<'PY'
from pathlib import Path
source = Path('/tmp/linux-6.12/kernel/sched/core.c').read_bytes()
Path('/tmp/plastext-m1-10mb.c').write_bytes(source * (10_000_000 // len(source) + 1))
PY
target/release/plastext-bench read /tmp/plastext-m1-10mb.c --frames 300 --foreground
```

`m1` and `read` return nonzero for a measured budget failure, missing results or an incomplete native run. `read` prints the path of its raw frame log and has a timeout; it uses default settings. `--foreground` activates the benchmark window; keep it visible and open until it exits itself. Without this flag it leaves keyboard focus alone, but occlusion can suspend the run. `launch` prints its measurements for comparison with the table. Run performance commands serially, with compilation and other heavy tasks finished.
