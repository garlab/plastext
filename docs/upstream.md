# Upstream work

Fixes to third-party code go upstream, not into plastext. plastext keeps no modified copies of other projects' files. A dependency may point at our fork only while its pull request is open; each such override is listed here with the condition for removing it.

## Shared libraries

`katex-native` and `document-graphics` are consumed from crates.io at version
0.1.0. `document-graphics` owns shared Mermaid/SVG rendering and worker
supervision; its source lives in the standalone document-graphics repository.
Plastext uses versioned dependencies and keeps GUI scheduling, cache budgets and
presentation locally. Development overrides belong in untracked Cargo
configuration ([local override](local-development.md#shared-rendering-libraries)).

## Open

| Dependency | Change | Where | Status | Remove the override when |
|---|---|---|---|---|
| `katex-rs` | `\mapsfrom` and `\reflectbox`, which KaTeX added after the version katex-rs 0.3 tracks | — | Not started | — (no override needed) |

## Rules

- Open an issue upstream before large changes.
- While a pull request is open, pin the fork's exact commit with `[patch.crates-io]`, and list it above.
- For local development against a checkout, use an untracked Cargo config override, not a committed path.
- Tests for an upstream fix belong upstream; plastext's own checks only cover integration.
