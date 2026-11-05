use std::ops::{ControlFlow, Range};

use tree_sitter::{
    InputEdit, ParseOptions, Parser, Point, QueryCursor, StreamingIterator as _, Tree,
};

use crate::{
    Language,
    injections::{self, Injection},
};

/// What a span of code is, as far as coloring goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Highlight {
    Keyword,
    Function,
    Type,
    String,
    Escape,
    Number,
    Constant,
    Comment,
    Property,
    Tag,
    Attribute,
    Label,
    Operator,
    Punctuation,
    Heading,
    Link,
}

/// How a query capture affects coloring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaptureKind {
    /// Colors its node.
    Highlight(Highlight),
    /// Shows its node as plain text, even inside a highlighted node.
    Plain,
    /// Changes nothing, like `@spell` or captures used only by predicates.
    Ignore,
}

impl CaptureKind {
    /// Maps a capture name such as `function.method.builtin`.
    pub(crate) fn from_name(name: &str) -> Self {
        use Highlight::*;
        let mut parts = name.split('.');
        let first = parts.next().unwrap_or_default();
        let second = parts.next();
        let highlight = match (first, second) {
            ("comment", _) => Comment,
            ("string", Some("escape" | "regexp" | "regex")) | ("escape", _) => Escape,
            ("string" | "character", _) => String,
            ("number" | "float", _) => Number,
            ("boolean" | "constant", _) => Constant,
            (
                "keyword" | "conditional" | "repeat" | "include" | "exception" | "storageclass",
                _,
            ) => Keyword,
            ("variable", Some("builtin")) => Keyword,
            ("function" | "method" | "constructor", _) => Function,
            ("type" | "module" | "namespace", _) => Type,
            ("property" | "field", _) => Property,
            ("tag", _) => Tag,
            ("attribute", _) => Attribute,
            ("label", _) => Label,
            ("operator", _) => Operator,
            ("punctuation", _) => Punctuation,
            ("text" | "markup", Some("title" | "heading")) => Heading,
            ("text" | "markup", Some("uri" | "link" | "reference")) => Link,
            ("text" | "markup", Some("literal" | "raw")) => String,
            ("variable" | "embedded" | "none" | "text" | "markup", _) => return Self::Plain,
            _ => return Self::Ignore,
        };
        Self::Highlight(highlight)
    }
}

/// A parsed document, for highlighting.
pub struct SyntaxTree {
    tree: Tree,
    language: &'static Language,
    parsed_len: usize,
    injections: Vec<Injection>,
}

impl SyntaxTree {
    /// Parse a bounded prefix first, preserving lexical context from byte zero.
    /// It covers the initial viewport plus a margin; the full parse replaces
    /// it later. Structural features must wait for the complete document.
    pub fn parse_initial(
        language: &'static Language,
        text: &[u8],
        cancelled: impl Fn() -> bool,
    ) -> Option<Self> {
        const INITIAL_BYTES: usize = 64 * 1024;
        let end = if text.len() <= INITIAL_BYTES {
            text.len()
        } else {
            text[..INITIAL_BYTES]
                .iter()
                .rposition(|&byte| byte == b'\n')
                .map_or(INITIAL_BYTES, |i| i + 1)
        };
        Self::parse(language, &text[..end], cancelled)
    }

    pub fn parsed_len(&self) -> usize {
        self.parsed_len
    }

    /// Parses `text`, giving up if `cancelled` starts returning true. Blocks:
    /// call it off the main thread.
    pub fn parse(
        language: &'static Language,
        text: &[u8],
        cancelled: impl Fn() -> bool,
    ) -> Option<Self> {
        let tree = parse_tree(language, text, &cancelled)?;
        let injections = injections::parse(language, &tree, text, &cancelled)?;
        Some(Self {
            tree,
            language,
            parsed_len: text.len(),
            injections,
        })
    }

    /// Reparse an edited version on a worker, reusing unchanged tree nodes.
    /// `before` must be the complete byte slice used for this tree.
    pub fn reparse(
        &self,
        before: &[u8],
        after: &[u8],
        cancelled: impl Fn() -> bool,
    ) -> Option<Self> {
        if before.len() != self.parsed_len {
            return Self::parse(self.language, after, cancelled);
        }
        if cancelled() {
            return None;
        }
        // A single enclosing replacement also handles coalesced edits and undo.
        let mut start = before.iter().zip(after).take_while(|(a, b)| a == b).count();
        let continuation =
            |bytes: &[u8], at: usize| bytes.get(at).is_some_and(|byte| byte & 0xc0 == 0x80);
        while start > 0 && (continuation(before, start) || continuation(after, start)) {
            start -= 1;
        }
        let mut suffix = before[start..]
            .iter()
            .rev()
            .zip(after[start..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        if cancelled() {
            return None;
        }
        while suffix > 0
            && (continuation(before, before.len() - suffix)
                || continuation(after, after.len() - suffix))
        {
            suffix -= 1;
        }
        let old_end = before.len() - suffix;
        let new_end = after.len() - suffix;
        let start_position = advance(Point::new(0, 0), &before[..start]);
        let mut old_tree = self.tree.clone();
        old_tree.edit(&InputEdit {
            start_byte: start,
            old_end_byte: old_end,
            new_end_byte: new_end,
            start_position,
            old_end_position: advance(start_position, &before[start..old_end]),
            new_end_position: advance(start_position, &after[start..new_end]),
        });
        let tree = parse_tree_with(self.language, after, Some(&old_tree), &cancelled)?;
        // Injection boundaries can change after edits to an HTML tag or type.
        let injections = injections::parse(self.language, &tree, after, &cancelled)?;
        Some(Self {
            tree,
            language: self.language,
            parsed_len: after.len(),
            injections,
        })
    }

    pub fn language(&self) -> &'static Language {
        self.language
    }

    pub(crate) fn tree(&self) -> &Tree {
        &self.tree
    }

    /// The highlighted spans within `range` of `text`, which must be the text
    /// that was parsed. Spans are sorted, don't overlap, and lie within `range`.
    ///
    /// When several captures cover a byte, the innermost node wins, and for
    /// the same node the earliest pattern in the query wins, as in
    /// tree-sitter's own highlighter. Embedded languages overlay their content
    /// without changing the surrounding markup.
    pub fn highlights(&self, text: &[u8], range: Range<usize>) -> Vec<(Range<usize>, Highlight)> {
        let len = text.len().min(self.parsed_len);
        let range = range.start.min(len)..range.end.min(len);
        if range.is_empty() {
            return Vec::new();
        }
        let mut painted = vec![None; range.len()];
        paint(&self.tree, self.language, text, range.clone(), &mut painted);
        // Injection ranges are disjoint and sorted. A viewport lookup never
        // walks all of a long document's script/style blocks.
        let first = self
            .injections
            .partition_point(|injection| injection.range.end <= range.start);
        for injection in self.injections[first..]
            .iter()
            .take_while(|i| i.range.start < range.end)
        {
            let start = range.start.max(injection.range.start);
            let end = range.end.min(injection.range.end);
            let visible = &mut painted[start - range.start..end - range.start];
            visible.fill(None);
            paint(
                &injection.tree,
                injection.language,
                &text[injection.range.clone()],
                start - injection.range.start..end - injection.range.start,
                visible,
            );
        }

        let mut spans: Vec<(Range<usize>, Highlight)> = Vec::new();
        for (i, highlight) in painted.into_iter().enumerate() {
            let Some(highlight) = highlight else { continue };
            let offset = range.start + i;
            match spans.last_mut() {
                Some((span, last)) if span.end == offset && *last == highlight => span.end += 1,
                _ => spans.push((offset..offset + 1, highlight)),
            }
        }
        spans
    }
}

pub(crate) fn parse_tree(
    language: &'static Language,
    text: &[u8],
    cancelled: &dyn Fn() -> bool,
) -> Option<Tree> {
    parse_tree_with(language, text, None, cancelled)
}

fn advance(start: Point, bytes: &[u8]) -> Point {
    let rows = bytes.iter().filter(|&&byte| byte == b'\n').count();
    let column = bytes
        .iter()
        .rposition(|&byte| byte == b'\n')
        .map_or(start.column + bytes.len(), |at| bytes.len() - at - 1);
    Point::new(start.row + rows, column)
}

fn parse_tree_with(
    language: &'static Language,
    text: &[u8],
    old_tree: Option<&Tree>,
    cancelled: &dyn Fn() -> bool,
) -> Option<Tree> {
    if cancelled() {
        return None;
    }
    let mut parser = Parser::new();
    parser.set_language(&language.grammar()?).ok()?;
    let mut progress = |_: &tree_sitter::ParseState| {
        if cancelled() {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    let options = ParseOptions::new().progress_callback(&mut progress);
    let mut read = |offset: usize, _| &text[offset.min(text.len())..];
    let tree = parser.parse_with_options(&mut read, old_tree, Some(options))?;
    if cancelled() {
        return None;
    }
    // Compilation belongs to the background parse, not the first frame.
    language.highlight_query();
    Some(tree)
}

fn paint(
    tree: &Tree,
    language: &Language,
    text: &[u8],
    range: Range<usize>,
    painted: &mut [Option<Highlight>],
) {
    let Some(query) = language.highlight_query() else {
        return;
    };
    let mut captures = Vec::new();
    let mut cursor = QueryCursor::new();
    cursor.set_byte_range(range.clone());
    let mut matches = cursor.captures(&query.query, tree.root_node(), text);
    while let Some((found, index)) = matches.next() {
        let capture = found.captures()[*index];
        let kind = query.kinds[capture.index as usize];
        if kind != CaptureKind::Ignore {
            captures.push((capture.node.byte_range(), found.pattern_index, kind));
        }
    }
    // Outer nodes paint first so inner ones override them; for the same
    // node the earliest pattern paints last.
    captures.sort_by(|(a, a_pattern, _), (b, b_pattern, _)| {
        a.start
            .cmp(&b.start)
            .then(b.end.cmp(&a.end))
            .then(b_pattern.cmp(a_pattern))
    });

    for (node, _, kind) in captures {
        let start = node.start.clamp(range.start, range.end) - range.start;
        let end = node.end.clamp(range.start, range.end) - range.start;
        let highlight = match kind {
            CaptureKind::Highlight(highlight) => Some(highlight),
            _ => None,
        };
        painted[start..end].fill(highlight);
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::detect;

    fn highlighted<'a>(path: &str, text: &'a str) -> Vec<(&'a str, Highlight)> {
        let language = detect(Path::new(path), text.as_bytes()).unwrap();
        let tree = SyntaxTree::parse(language, text.as_bytes(), || false).unwrap();
        tree.highlights(text.as_bytes(), 0..text.len())
            .into_iter()
            .map(|(range, highlight)| (&text[range], highlight))
            .collect()
    }

    #[test]
    fn incremental_reparse_matches_fresh_parse_with_unicode_and_injections() {
        for (name, original, replacements) in [
            (
                "a.md",
                "# Heading\r\n\r\nparagraph\r\n",
                vec!["## Changed\n\n> quote\n", "", "# é\n", "# ê\n", "# Ω\n"],
            ),
            (
                "a.rs",
                "fn main() { let café = 1; }\n",
                vec![
                    "/* comment\n*/fn main() { let café = 2; }",
                    "fn renamed() {}\n",
                    "",
                ],
            ),
            (
                "a.html",
                "<script>let a = 1;</script>",
                vec![
                    "<script type='application/json'>{\"a\": true}</script>",
                    "<style>body { color: red; }</style>",
                    "<p>é</p>",
                ],
            ),
        ] {
            let language = detect(Path::new(name), b"").unwrap();
            let mut before = original;
            let mut tree = SyntaxTree::parse(language, before.as_bytes(), || false).unwrap();
            for after in replacements.into_iter().chain(std::iter::once(original)) {
                let next = tree
                    .reparse(before.as_bytes(), after.as_bytes(), || false)
                    .unwrap();
                let fresh = SyntaxTree::parse(language, after.as_bytes(), || false).unwrap();
                assert_eq!(
                    next.highlights(after.as_bytes(), 0..after.len()),
                    fresh.highlights(after.as_bytes(), 0..after.len()),
                    "{name}: {after}"
                );
                assert_eq!(
                    next.symbols(after.as_bytes()),
                    fresh.symbols(after.as_bytes())
                );
                assert_eq!(next.folds(after.as_bytes()), fresh.folds(after.as_bytes()));
                assert!(
                    tree.reparse(before.as_bytes(), after.as_bytes(), || true)
                        .is_none()
                );
                tree = next;
                before = after;
            }
        }
    }

    #[test]
    fn highlights_rust() {
        let spans = highlighted("main.rs", "// hi\nfn main() { let x = \"s\"; }\n");
        assert!(spans.contains(&("// hi", Highlight::Comment)), "{spans:?}");
        assert!(spans.contains(&("fn", Highlight::Keyword)), "{spans:?}");
        assert!(spans.contains(&("main", Highlight::Function)), "{spans:?}");
        assert!(spans.contains(&("\"s\"", Highlight::String)), "{spans:?}");
    }

    #[test]
    fn spans_stay_within_the_requested_range() {
        let text = "def f():\n    return 'a long string'\n";
        let language = detect(Path::new("f.py"), text.as_bytes()).unwrap();
        let tree = SyntaxTree::parse(language, text.as_bytes(), || false).unwrap();
        let range = 20..28;
        let spans = tree.highlights(text.as_bytes(), range.clone());
        assert!(!spans.is_empty());
        assert!(
            spans
                .iter()
                .all(|(span, _)| range.start <= span.start && span.end <= range.end)
        );
        assert!(
            spans
                .windows(2)
                .all(|pair| pair[0].0.end <= pair[1].0.start)
        );
    }

    #[test]
    fn cancelled_parses_give_up() {
        let text = "x = 1\n".repeat(100_000);
        let language = detect(Path::new("big.py"), b"").unwrap();
        assert!(SyntaxTree::parse(language, text.as_bytes(), || true).is_none());
    }

    #[test]
    fn initial_parse_matches_full_viewport_and_is_bounded() {
        for (file, unit) in [
            (
                "code.c",
                "/* a comment */\nint f(int x) {\nreturn x + 1;\n}\n",
            ),
            ("code.rs", "// café\nfn f() {\n    let s = \"hello\";\n}\n"),
            ("code.py", "def f():\n    # café\n    return 'hello'\n"),
            ("code.js", "function f() {\n    return 'hello';\n}\n"),
            (
                "code.html",
                "<p>Café</p><script>function f() { return 'hello'; }</script>\n",
            ),
        ] {
            let text = unit.repeat(4000);
            let language = detect(Path::new(file), b"").unwrap();
            let initial = SyntaxTree::parse_initial(language, text.as_bytes(), || false).unwrap();
            let full = SyntaxTree::parse(language, text.as_bytes(), || false).unwrap();
            assert!(initial.parsed_len() <= 64 * 1024);
            assert!(initial.parsed_len() < full.parsed_len());
            assert_eq!(
                initial.highlights(text.as_bytes(), 0..unit.len() * 2),
                full.highlights(text.as_bytes(), 0..unit.len() * 2),
                "{file}"
            );
            assert!(
                initial
                    .highlights(text.as_bytes(), text.len() - 100..text.len())
                    .is_empty()
            );
        }
    }

    #[test]
    fn capture_names_map_to_highlights() {
        assert_eq!(
            CaptureKind::from_name("function.method.builtin"),
            CaptureKind::Highlight(Highlight::Function)
        );
        assert_eq!(
            CaptureKind::from_name("string.escape"),
            CaptureKind::Highlight(Highlight::Escape)
        );
        assert_eq!(
            CaptureKind::from_name("variable.parameter"),
            CaptureKind::Plain
        );
        assert_eq!(CaptureKind::from_name("spell"), CaptureKind::Ignore);
        assert_eq!(CaptureKind::from_name("_name"), CaptureKind::Ignore);
    }
}
