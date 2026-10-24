# MiTeX spike helpers

`prelude.typ` and `standard.typ` are from [MiTeX](https://github.com/mitex-rs/mitex/tree/f69eb43b52372e141b35661c049ce522567adbf0/packages/mitex/specs), commit `f69eb43b52372e141b35661c049ce522567adbf0`, matching the `mitex` 0.2.4 crate. Their Apache-2.0 license is retained in `LICENSE`.

Changes in `standard.typ`:

- Resolve `prelude.typ` from the same static source directory.
- Replace the `@preview/xarrow:0.2.0` import with an explicit unsupported-operation error. That package has a different license and is not included here.
- Replace the unavailable `kai` symbol in the KaTeX logo with `K`, so the helper dictionary loads with Typst 0.15.1.

These files are used only by the opt-in `spike` example, not by the app. The example has static sources and bundled fonts; it has no filesystem or network resolver. Successful compilation is not a visual-equivalence test.
