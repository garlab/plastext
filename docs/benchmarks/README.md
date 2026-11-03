# Benchmark artifacts

Only compact `*.summary.json` reports belong here. They retain fixture labels,
sample counts, aggregate timings, budgets and completion/pass/fail status.
Raw samples, logs and absolute local paths are excluded.

The [large-workload validation guide](../m2-validation.md) describes the Markdown,
CSV, PDF and TeX fixtures, native scrolling checks and reference-hardware procedure.

`plastext-bench` stores full JSON and native stderr in a unique directory under
`target/benchmarks/` and prints that directory. `--output` writes a summary only:

```sh
target/release/plastext-bench read /tmp/plastext-m2/large.md --frames 10000 --foreground --keep-visible --output docs/benchmarks/m2-native.summary.json
python3 scripts/math-corpus.py --summary docs/benchmarks/math-corpus.summary.json
```

The math runner stores detailed results, TeX and SVG files under
`target/benchmarks/math-corpus/` by default. Its `--output` option changes that
local directory; `--summary` writes a report suitable for Git. Criterion results
also stay under ignored `target/`.

`.gitignore` excludes raw artifacts. CI checks staged summaries for raw data,
absolute paths and files exceeding 16 KiB. Run the same check after staging:

```sh
python3 scripts/check-benchmark-artifacts.py
```
