# Upstream work

Fixes to third-party code go upstream, not into plastext. plastext keeps no modified copies of other projects' files. A dependency may point at our fork only while its pull request is open; each such override is listed here with the condition for removing it.

## Open

| Dependency | Change | Where | Status | Remove the override when |
|---|---|---|---|---|
| `katex-native` (ours) | Native layout for katex-rs output: KaTeX's CSS applied with its fonts, as a display list and rasters | Local repository `../katex-native`, used by path on branch `math-katex` only | Not published. Offer it to katex-rs as an optional native layout module first; publish it separately if declined | It is on crates.io or a public git repository. The path dependency must not reach `main`, because CI can't see a sibling folder |
| `katex-rs` | `\mapsfrom` and `\reflectbox`, which KaTeX added after the version katex-rs 0.3 tracks | — | Not started | — (no override needed) |

## Rules

- Open an issue upstream before large changes.
- While a pull request is open, pin the fork's exact commit with `[patch.crates-io]`, and list it above.
- For local development against a checkout, use an untracked Cargo config override, not a committed path.
- Tests for an upstream fix belong upstream; plastext's own checks only cover integration.
