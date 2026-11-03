# Reading Markdown

Markdown opens in Preview. Use **⇧⌘V** or **Source** to switch views at the
corresponding document position. Each split pane has its own view and selection.

## Select and copy

- Drag across text, headings, table cells or code to select it. Drag beyond the
  top or bottom of the pane to scroll while extending the selection.
- Double-click a word or inline formula; triple-click a paragraph or table cell.
  Click a display formula to select its TeX. Shift-click extends the selection.
- **⌘A** selects the document, **⌘C** copies, and **Escape** or clicking empty
  space clears the selection. Clicking a link follows it; dragging selects text.

Copy produces plain text: Markdown styling and link destinations are omitted,
table cells are separated by tabs, and formulas copy their original TeX without
the surrounding dollar delimiters. Code and diagram source remain text.
Collapsed front matter and HTML disclosures are omitted; expand them to include
their contents.

Selection survives scrolling, zoom and Source/Preview toggles. Copying waits for
the document index to finish, then reads one parsed page at a time in the
background without filling the preview cache. A changed selection, a new copy,
switching to Source or closing the view cancels pending work. The clipboard is
updated only on success; a changed file or a selection above 64 MB reports an
error without copying partial text.

## Math and document macros

Inline math works in prose, headings, links and table cells. Heading formulas
follow the heading's size and zoom. Display math supports equation numbering.
Find searches the TeX behind formulas and highlights the complete formula when
it matches; copying uses the TeX, not the raster or its layout placeholders.

Define macros in math using `\newcommand` or `\def`, for example:

```markdown
$$\newcommand{\sq}[1]{#1^2}\def\N{\mathbb{N}}$$

# A function $f: \N \to \N$

The square is $\sq{x}$.
```

Definitions apply to subsequent math in the same document, including later
formulas in the same paragraph. KaTeX's grouping and redefinition rules apply.
Scrolling backwards or opening a split does not change which definitions a
formula sees. Definitions never leak to other documents. Code fences and inline
code do not define macros. An invalid defining formula does not change the
namespace of later formulas; unsupported math remains readable as TeX.

The compact index retains at most 256 defining formulas and 128 KiB of their
source, shared by all formula cache entries. Replaying definitions uses a fresh
KaTeX namespace for each render, with a 1,000-expansion limit per formula.
Exceeding the document history limit leaves subsequent formulas as TeX. Normal
paragraphs and formulas without definitions do not add to this history.

Try the [reading fixture](fixtures/markdown-reading.md) and
[heading/zoom fixture](fixtures/math-headings.md).
Rich-text/HTML clipboard output remains future work. Editing and synchronized
source/preview are M3 work.

## Safe native HTML

HTML inside Markdown uses the same native blocks, text selection, find and image
workers as Markdown. Supported elements are:

- `<details>` and `<summary>`, including nesting and the `open` attribute.
  Click the disclosure triangle or summary to toggle it. Find and source
  navigation reveal enclosing disclosures; collapsed contents are excluded from
  layout and copying. Each split pane has independent expansion state.
- `<b>`, `<strong>`, `<i>`, `<em>`, `<code>`, `<kbd>`, `<mark>`, `<sub>` and `<sup>`.
  Subscripts and superscripts use smaller native glyphs; copying retains their
  original text. `<a href>` follows the same link policy as Markdown links.
- `<p align>` and `<div align>` with left, center or right alignment, `<br>` and
  `<hr>`. Comments are hidden, and HTML entities are decoded.
- `<img src alt title width height align>`. Width and height accept positive
  pixel values up to 16,384; the display box remains within the reading column.
  `<picture>` selects the first applicable `<source srcset>` for the window's
  light/dark appearance, falling back to `<img>`. Only empty media conditions
  and `(prefers-color-scheme: light|dark)` are supported; the first srcset
  candidate is used, without responsive density/width selection.

Unsupported elements, including scripts, styles, forms and iframes, remain
inert, dimmed source. Event handlers, CSS, navigation attributes and browser
features are never evaluated. The tokenizer has no resource loader or execution
engine. Local/data images use the existing bounded decoder; remote resources,
including localhost URLs, are always blocked. Links leave the app only on a
user click. Standalone HTML files still open in Source.

HTML nesting is limited to 128 levels. Each tokenizer input is at most 256 KiB
with at most 32,768 tokens; attribute values are limited to 16 KiB. Open-element
context is bounded to approximately 64 KiB and charged to the existing index
budget. Oversized constructs fall back to source. Text layout items remain
bounded to 16 KiB, and parsed pages retain their 32 MiB cache limit. No full DOM
is retained.

Try the [safe HTML fixture](fixtures/safe-html.md).
