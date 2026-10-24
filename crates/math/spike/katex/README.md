# KaTeX screenshot corpus

`cases.json` is a lossless JSON projection of all 128 top-level cases in [KaTeX's screenshot fixtures](https://github.com/KaTeX/KaTeX/blob/8000fbd4e29cca2c0cb1e88b745af3ed2f436b66/test/screenshotter/ss_data.yaml), pinned to commit `8000fbd4e29cca2c0cb1e88b745af3ed2f436b66`. The upstream MIT license is included in `LICENSE`.

Each entry retains its name, TeX, display flag, and other options. String shorthand becomes `display: false` and an empty options object. No cases are excluded. `scripts/math-corpus.py` runs the opt-in native spike in a fresh process with a five-second timeout per expression. Macro options become declarations; HTML wrappers, CSS, and KaTeX's error-color/no-throw behavior remain recorded but are not implemented by the probe.

Passing compilation does not establish layout equivalence. This is a reproducible coverage gate for engine selection, not the full KaTeX unit suite or M2's visual-corpus sign-off. `UnsupportedCmds` deliberately contains invalid commands; it remains in the denominator and should eventually test the specified error fallback.
