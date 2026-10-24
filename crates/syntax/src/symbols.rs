use std::ops::Range;

use tree_sitter::{QueryCursor, StreamingIterator as _};

use crate::SyntaxTree;

/// Headings, for the Markdown outline.
pub(crate) const MARKDOWN_TAGS: &str = "
(atx_heading heading_content: (_) @name) @definition.heading
(setext_heading heading_content: (_) @name) @definition.heading
";

/// A named definition in a file, for the outline and "go to symbol" (spec CODE-6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    /// What it defines, from the tags query: `function`, `class`, `heading`…
    pub kind: String,
    /// The whole definition.
    pub range: Range<usize>,
    /// How many other symbols contain it.
    pub depth: usize,
}

impl SyntaxTree {
    /// The definitions in `text`, which must be the text that was parsed, in
    /// order. Languages without a tags query have none.
    pub fn symbols(&self, text: &[u8]) -> Vec<Symbol> {
        let Some(query) = self.language().tags_query() else {
            return Vec::new();
        };
        let names = query.capture_names();
        let mut symbols = Vec::new();
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(query, self.tree().root_node(), text);
        while let Some(found) = matches.next() {
            let mut name = None;
            let mut definition = None;
            for capture in found.captures() {
                let capture_name = names[capture.index as usize];
                if capture_name == "name" {
                    name = Some(capture.node.byte_range());
                } else if let Some(kind) = capture_name.strip_prefix("definition.") {
                    definition = Some((kind, capture.node.byte_range()));
                }
            }
            let (Some(name), Some((kind, range))) = (name, definition) else {
                continue;
            };
            let name = String::from_utf8_lossy(&text[name])
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            symbols.push(Symbol {
                name,
                kind: kind.to_owned(),
                range,
                depth: 0,
            });
        }
        symbols.sort_by_key(|symbol| (symbol.range.start, std::cmp::Reverse(symbol.range.end)));
        symbols.dedup_by(|a, b| a.range == b.range && a.name == b.name);

        // Depth is the number of enclosing symbols.
        let mut open: Vec<usize> = Vec::new();
        for symbol in &mut symbols {
            while open.last().is_some_and(|&end| end <= symbol.range.start) {
                open.pop();
            }
            symbol.depth = open.len();
            open.push(symbol.range.end);
        }
        symbols
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::detect;

    fn symbols(path: &str, text: &str) -> Vec<(String, String, usize)> {
        let language = detect(Path::new(path), b"").unwrap();
        let tree = SyntaxTree::parse(language, text.as_bytes(), || false).unwrap();
        tree.symbols(text.as_bytes())
            .into_iter()
            .map(|symbol| (symbol.name, symbol.kind, symbol.depth))
            .collect()
    }

    fn named(items: &[(&str, &str, usize)]) -> Vec<(String, String, usize)> {
        items
            .iter()
            .map(|&(name, kind, depth)| (name.to_owned(), kind.to_owned(), depth))
            .collect()
    }

    #[test]
    fn lists_rust_definitions_with_nesting() {
        let text = "struct Point { x: i32 }\nimpl Point {\n    fn len(&self) -> i32 { 0 }\n}\nfn main() {}\n";
        assert_eq!(
            symbols("lib.rs", text),
            named(&[
                ("Point", "class", 0),
                ("len", "method", 0),
                ("main", "function", 0)
            ])
        );
        let text = "fn outer() {\n    fn inner() {}\n}\n";
        assert_eq!(
            symbols("lib.rs", text),
            named(&[("outer", "function", 0), ("inner", "function", 1)])
        );
    }

    #[test]
    fn lists_markdown_headings() {
        let text = "# Title\n\nText.\n\n## Section one\n\nSetext\n------\n";
        assert_eq!(
            symbols("README.md", text),
            named(&[
                ("Title", "heading", 0),
                ("Section one", "heading", 0),
                ("Setext", "heading", 0)
            ])
        );
    }

    #[test]
    fn languages_without_tags_have_no_symbols() {
        assert!(symbols("data.json", "{\"a\": 1}").is_empty());
    }
}
