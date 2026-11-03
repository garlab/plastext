---
title: Markdown reading
description: Expand this section to select and copy metadata values.
---

$$\newcommand{\sq}[1]{#1^2}\def\N{\mathbb{N}}$$

# Reading $f: \N \to \N$

Select **styled text**, *café*, 中文, 👩‍💻 and `inline code` together.
Click [the heading fixture](math-headings.md) to follow the link, or drag across
it to copy its label. Selection should survive zooming and Source/Preview.

## A heading with $\sq{x}$

An inline fraction $\frac{a+b}{c}$ should copy its TeX. Double-click the formula
or select it along with this paragraph. Find `frac` to highlight the formula.

$$
\begin{equation}
\sq{x} + \sq{y} = \sq{z}
\end{equation}
$$

| Left | Center | Right |
| :--- | :---: | ---: |
| café | $\sq{x}$ | 12345 |
| long text wrapping in a narrow pane | $\frac{a+b}{c}$ | right-aligned words |
| | empty neighbor | |

```rust
// Select a word, a line, or all of this fence.
let greeting = "café 中文";
```

The next definition affects only subsequent formulas: $\def\N{\mathbf{N}}$ then
$\N$. Scroll back up: the heading still uses blackboard bold.

$$
\begin{equation}
x \in \N
\end{equation}
$$

## Invalid math

This formula stays as source: $\notACommand{x}$.
Open a different document containing `\N` without a definition: this document's
macro must not appear there.
