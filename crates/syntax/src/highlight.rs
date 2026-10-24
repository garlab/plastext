use std::ops::{ControlFlow, Range};

use tree_sitter::{ParseOptions, Parser, QueryCursor, StreamingIterator as _, Tree};

use crate::Language;

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
}

impl SyntaxTree {
    /// Parses `text`, giving up if `cancelled` starts returning true. Blocks:
    /// call it off the main thread.
    pub fn parse(
        language: &'static Language,
        text: &[u8],
        cancelled: impl Fn() -> bool,
    ) -> Option<Self> {
        let mut parser = Parser::new();
        parser.set_language(&language.grammar()).ok()?;
        let mut progress = |_: &tree_sitter::ParseState| {
            if cancelled() {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        };
        let options = ParseOptions::new().progress_callback(&mut progress);
        let mut read = |offset: usize, _| &text[offset.min(text.len())..];
        let tree = parser.parse_with_options(&mut read, None, Some(options))?;
        Some(Self { tree, language })
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
    /// tree-sitter's own highlighter.
    pub fn highlights(&self, text: &[u8], range: Range<usize>) -> Vec<(Range<usize>, Highlight)> {
        let Some(query) = self.language.highlight_query() else {
            return Vec::new();
        };
        let range = range.start.min(text.len())..range.end.min(text.len());
        let mut captures = Vec::new();
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(range.clone());
        let mut matches = cursor.captures(&query.query, self.tree.root_node(), text);
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

        let mut painted = vec![None; range.len()];
        for (node, _, kind) in captures {
            let start = node.start.clamp(range.start, range.end) - range.start;
            let end = node.end.clamp(range.start, range.end) - range.start;
            let highlight = match kind {
                CaptureKind::Highlight(highlight) => Some(highlight),
                _ => None,
            };
            painted[start..end].fill(highlight);
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
