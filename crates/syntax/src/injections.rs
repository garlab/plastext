//! Embedded HTML languages, discovered and parsed on the background parse job.

use crate::{Language, detect, highlight::parse_tree};
use std::{
    ops::{ControlFlow, Range},
    path::Path,
    sync::OnceLock,
};
use tree_sitter::{Node, Query, QueryCursor, QueryCursorOptions, StreamingIterator as _, Tree};

pub(crate) struct Injection {
    pub range: Range<usize>,
    pub language: &'static Language,
    pub tree: Tree,
}

// The initial bounded parse can end before a closing tag. The HTML grammar
// keeps the start tag and raw content under ERROR instead of script_element.
const INCOMPLETE_HTML: &str = r#"
((ERROR (start_tag (tag_name) @_script) . (raw_text) @injection.content)
 (#match? @_script "(?i)^script$")
 (#set! injection.language "javascript"))
((ERROR (start_tag (tag_name) @_style) . (raw_text) @injection.content)
 (#match? @_style "(?i)^style$")
 (#set! injection.language "css"))
"#;

pub(crate) fn parse(
    language: &'static Language,
    tree: &Tree,
    text: &[u8],
    cancelled: &dyn Fn() -> bool,
) -> Option<Vec<Injection>> {
    if language.name != "HTML" {
        return Some(Vec::new());
    }
    static QUERY: OnceLock<Query> = OnceLock::new();
    let query = QUERY.get_or_init(|| {
        Query::new(
            &tree_sitter_html::LANGUAGE.into(),
            &[tree_sitter_html::INJECTIONS_QUERY, INCOMPLETE_HTML].concat(),
        )
        .expect("bundled HTML injection query must compile")
    });
    let content = query.capture_index_for_name("injection.content")?;
    let mut cursor = QueryCursor::new();
    let mut progress = |_: &tree_sitter::QueryCursorState| {
        if cancelled() {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    let mut matches = cursor.matches_with_options(
        query,
        tree.root_node(),
        text,
        QueryCursorOptions::new().progress_callback(&mut progress),
    );
    let mut injections = Vec::new();
    while let Some(found) = matches.next() {
        if cancelled() {
            return None;
        }
        let default = query
            .property_settings(found.pattern_index)
            .iter()
            .find(|property| property.key.as_ref() == "injection.language")
            .and_then(|property| property.value.as_deref());
        for capture in found
            .captures()
            .iter()
            .filter(|capture| capture.index == content)
        {
            let range = capture.node.byte_range();
            if range.is_empty() {
                continue;
            }
            let Some(language) = content_language(capture.node, text, default) else {
                continue;
            };
            // Each block has its own tree: an unfinished comment/string in one
            // script must not change the interpretation of another script.
            let tree = parse_tree(language, &text[range.clone()], cancelled)?;
            injections.push(Injection {
                range,
                language,
                tree,
            });
        }
    }
    if cancelled() {
        return None;
    }
    injections.sort_unstable_by_key(|injection| injection.range.start);
    Some(injections)
}

fn content_language(
    node: Node<'_>,
    text: &[u8],
    default: Option<&str>,
) -> Option<&'static Language> {
    let parent = node.parent()?;
    let tag = node
        .prev_named_sibling()
        .filter(|node| node.kind() == "start_tag")
        .or_else(|| {
            parent
                .named_child(0)
                .filter(|node| node.kind() == "start_tag")
        })?;
    let mut cursor = tag.walk();
    let mut kind = "";
    for attribute in tag
        .named_children(&mut cursor)
        .filter(|node| node.kind() == "attribute")
    {
        let name = attribute.named_child(0)?;
        if name.utf8_text(text).ok()?.eq_ignore_ascii_case("type") {
            kind = attribute
                .named_child(1)
                .map(|value| value.utf8_text(text).unwrap_or("?"))
                .unwrap_or("")
                .trim_matches(['\'', '"'])
                .trim();
            break;
        }
    }
    let kind = kind.to_ascii_lowercase();
    // MIME parameters do not change which syntax is used for display.
    let mime = kind.split(';').next().unwrap_or("").trim();
    let extension = match default {
        Some("css") if matches!(mime, "" | "text/css") => "css",
        Some("javascript")
            if matches!(
                mime,
                "" | "module"
                    | "text/javascript"
                    | "application/javascript"
                    | "text/ecmascript"
                    | "application/ecmascript"
                    | "application/x-javascript"
                    | "application/x-ecmascript"
                    | "text/x-javascript"
                    | "text/x-ecmascript"
                    | "text/jscript"
                    | "text/livescript"
                    | "text/javascript1.0"
                    | "text/javascript1.1"
                    | "text/javascript1.2"
                    | "text/javascript1.3"
                    | "text/javascript1.4"
                    | "text/javascript1.5"
            ) =>
        {
            "js"
        }
        Some("javascript")
            if matches!(
                mime,
                "application/json" | "text/json" | "importmap" | "speculationrules"
            ) || mime.ends_with("+json") =>
        {
            "json"
        }
        _ => return None,
    };
    detect(Path::new(&format!("embedded.{extension}")), b"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Highlight, SyntaxTree};
    use std::cell::Cell;

    fn html(text: &str) -> SyntaxTree {
        SyntaxTree::parse(
            detect(Path::new("page.html"), b"").unwrap(),
            text.as_bytes(),
            || false,
        )
        .unwrap()
    }

    fn assert_block(tree: &SyntaxTree, document: &str, content: &str, file: &str) {
        let start = document.find(content).unwrap();
        let language = detect(Path::new(file), b"").unwrap();
        let standalone = SyntaxTree::parse(language, content.as_bytes(), || false).unwrap();
        let expected = standalone.highlights(content.as_bytes(), 0..content.len());
        assert!(!expected.is_empty());
        let actual = tree
            .highlights(document.as_bytes(), start..start + content.len())
            .into_iter()
            .map(|(range, kind)| (range.start - start..range.end - start, kind))
            .collect::<Vec<_>>();
        assert_eq!(actual, expected, "{file}");
    }

    #[test]
    fn html_scripts_and_styles_use_their_own_grammars_and_preserve_markup() {
        let script = "\r\nconst café = 'hello';\r\nfunction read() { return café; }\r\n";
        let css = "\n/* color */\nbody { color: #abc; margin: 4px; }\n";
        let document = format!(
            "<p>中文</p><script type='text/javascript'>{script}</script><style type=\"text/css\">{css}</style><p>tail</p>"
        );
        let tree = html(&document);
        assert_block(&tree, &document, script, "inline.js");
        assert_block(&tree, &document, css, "inline.css");
        let spans = tree.highlights(document.as_bytes(), 0..document.len());
        assert!(
            spans
                .iter()
                .any(|(r, h)| &document[r.clone()] == "const" && *h == Highlight::Keyword)
        );
        assert!(
            spans
                .iter()
                .any(|(r, h)| &document[r.clone()] == "script" && *h == Highlight::Tag)
        );
        assert!(
            spans
                .windows(2)
                .all(|pair| pair[0].0.end <= pair[1].0.start)
        );
    }

    #[test]
    fn script_types_select_javascript_or_json_and_leave_unknown_data_plain() {
        for attributes in [
            "",
            "type",
            "type=''",
            "type=module",
            "TYPE='TEXT/JAVASCRIPT'",
            "type='application/javascript; charset=utf-8'",
        ] {
            let code = "const answer = 42;";
            let text = format!("<script {attributes}>{code}</script>");
            assert_block(&html(&text), &text, code, "inline.js");
        }
        for kind in [
            "application/json",
            "application/ld+json",
            "importmap",
            "speculationrules",
        ] {
            let code = r#"{"enabled": true, "count": 3}"#;
            let text = format!("<script type='{kind}'>{code}</script>");
            assert_block(&html(&text), &text, code, "inline.json");
        }
        for (tag, kind) in [
            ("script", "text/plain"),
            ("script", "text/template"),
            ("style", "text/unknown"),
        ] {
            let code = "const answer = 42;";
            let text = format!("<{tag} type='{kind}'>{code}</{tag}>");
            let start = text.find(code).unwrap();
            assert!(
                html(&text)
                    .highlights(text.as_bytes(), start..start + code.len())
                    .is_empty()
            );
        }
    }

    #[test]
    fn block_boundaries_and_viewport_clipping_keep_correct_offsets() {
        let script = "const name = '世界'; /* multi\nline comment */";
        let text = format!(
            "<!-- <script>not code</script> --><script>/* unfinished</script><p>plain</p><script>{script}</script>"
        );
        let tree = html(&text);
        assert_block(&tree, &text, script, "inline.js");
        let start = text.find("line comment").unwrap();
        let range = start + 1..start + 5;
        assert_eq!(
            tree.highlights(text.as_bytes(), range.clone()),
            vec![(range, Highlight::Comment)]
        );
        let start = text.find("plain").unwrap();
        assert!(
            tree.highlights(text.as_bytes(), start..start + 5)
                .is_empty()
        );
    }

    #[test]
    fn initial_viewport_inside_a_long_unclosed_script_matches_full_parse() {
        for (tag, unit, expected) in [
            ("script", "const answer = 42;\n", Highlight::Keyword),
            ("style", "body { color: blue; }\n", Highlight::Property),
        ] {
            let text = format!(
                "<html><head><{tag}>\n{}\n</{tag}></head></html>",
                unit.repeat(6000)
            );
            let language = detect(Path::new("page.html"), b"").unwrap();
            let initial = SyntaxTree::parse_initial(language, text.as_bytes(), || false).unwrap();
            let full = html(&text);
            assert!(initial.parsed_len() <= 64 * 1024);
            let viewport = 0..300;
            let spans = initial.highlights(text.as_bytes(), viewport.clone());
            assert!(
                spans.iter().any(|(_, h)| *h == expected),
                "{}",
                initial.tree().root_node().to_sexp()
            );
            assert_eq!(spans, full.highlights(text.as_bytes(), viewport));
        }
    }

    #[test]
    fn cancellation_during_embedded_parsing_discards_partial_work() {
        let text = "<script>const x = 1;</script>".repeat(30);
        let language = detect(Path::new("page.html"), b"").unwrap();
        let tree = parse_tree(language, text.as_bytes(), &|| false).unwrap();
        let checks = Cell::new(0);
        let cancelled = || {
            checks.set(checks.get() + 1);
            checks.get() > 6
        };
        assert!(parse(language, &tree, text.as_bytes(), &cancelled).is_none());
        assert!(checks.get() > 6);
    }
}
