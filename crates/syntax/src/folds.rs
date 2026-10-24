use std::{collections::BTreeMap, ops::Range};

use tree_sitter::{QueryCursor, StreamingIterator as _};

use crate::SyntaxTree;

/// A fold's visible header and the complete lines hidden below it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fold {
    pub header: u64,
    pub hidden: Range<u64>,
}

/// Structural nodes, including multiline literals and comments. Never fold
/// the document root or infer a block from whitespace inside a parsed file.
pub(crate) fn query(language: &str) -> Option<&'static str> {
    Some(match language {
        "Bash" => {
            "[(function_definition) (if_statement) (for_statement) (while_statement) (case_statement) (compound_statement)] @fold"
        }
        "C" | "C++" => {
            "[(compound_statement) (field_declaration_list) (enumerator_list) (initializer_list) (comment) (preproc_if) (preproc_ifdef)] @fold"
        }
        "CSS" => "[(block) (comment)] @fold",
        "Go" => "[(block) (field_declaration_list) (literal_value) (comment)] @fold",
        "HTML" => "[(element) (script_element) (style_element) (comment)] @fold",
        "JavaScript" | "TypeScript" | "TSX" => {
            "[(statement_block) (class_body) (object) (array) (template_string) (comment)] @fold"
        }
        "JSON" => "[(object) (array)] @fold",
        "Markdown" => "[(section) (fenced_code_block) (list_item) (block_quote)] @fold",
        "Python" => {
            "[(function_definition) (class_definition) (if_statement) (for_statement) (while_statement) (try_statement) (with_statement) (list) (dictionary) (set) (tuple) (string)] @fold"
        }
        "Rust" => {
            "[(block) (declaration_list) (field_declaration_list) (enum_variant_list) (match_block) (use_list) (array_expression) (block_comment)] @fold"
        }
        "TOML" => "[(table) (table_array_element) (array) (inline_table)] @fold",
        "YAML" => {
            "[(block_mapping_pair) (block_sequence_item) (flow_mapping) (flow_sequence)] @fold"
        }
        _ => return None,
    })
}

impl SyntaxTree {
    /// Folds for the text that was parsed. Run alongside parsing, off the UI
    /// thread. `None` means indentation should be used as a fallback.
    pub fn folds(&self, text: &[u8]) -> Option<Vec<Fold>> {
        let query = self.language().fold_query()?;
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.captures(query, self.tree().root_node(), text);
        let mut folds = BTreeMap::<u64, Range<u64>>::new();
        while let Some((found, index)) = matches.next() {
            let node = found.captures()[*index].node;
            if node.is_error()
                || node.is_missing()
                || node.start_position().row == node.end_position().row
            {
                continue;
            }
            let start = node.start_byte();
            let end = node.end_byte().min(text.len());
            let header = text[..start]
                .iter()
                .rposition(|&b| b == b'\n')
                .map_or(0, |i| i + 1);
            let Some(newline) = text[start..end].iter().position(|&b| b == b'\n') else {
                continue;
            };
            let hidden_start = start + newline + 1;
            // Include the node's last line only when it contains no following
            // code. In `} else {`, the else clause must remain visible.
            let tail_end = end
                + text[end..]
                    .iter()
                    .position(|&b| b == b'\n')
                    .map_or(text.len() - end, |i| i + 1);
            let hidden_end = if text[end..tail_end].iter().all(u8::is_ascii_whitespace) {
                tail_end
            } else {
                text[..end]
                    .iter()
                    .rposition(|&b| b == b'\n')
                    .map_or(0, |i| i + 1)
            };
            if hidden_end > hidden_start {
                folds
                    .entry(header as u64)
                    .and_modify(|range| range.end = range.end.max(hidden_end as u64))
                    .or_insert(hidden_start as u64..hidden_end as u64);
            }
        }
        Some(
            folds
                .into_iter()
                .map(|(header, hidden)| Fold { header, hidden })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn folds(path: &str, text: &str) -> Vec<Fold> {
        SyntaxTree::parse(
            crate::detect(Path::new(path), b"").unwrap(),
            text.as_bytes(),
            || false,
        )
        .unwrap()
        .folds(text.as_bytes())
        .unwrap()
    }

    #[test]
    fn folds_unindented_code_and_nested_blocks() {
        let text = "fn main() {\nif true {\ngo();\n}\n}\nnext();\n";
        let result = folds("main.rs", text);
        assert_eq!(result.len(), 2);
        assert_eq!(
            &text[result[0].hidden.start as usize..result[0].hidden.end as usize],
            "if true {\ngo();\n}\n}\n"
        );
        assert_eq!(result[1].header, 12);
    }

    #[test]
    fn does_not_fold_an_indented_continuation_or_one_line_block() {
        assert!(folds("main.c", "int x =\n    42;\nint f() { return x; }\n").is_empty());
    }

    #[test]
    fn keeps_code_after_a_closing_delimiter_visible() {
        let text = "if (x) {\nfoo();\n} else {\nbar();\n}\n";
        let result = folds("main.c", text);
        assert_eq!(
            &text[result[0].hidden.start as usize..result[0].hidden.end as usize],
            "foo();\n"
        );
    }

    #[test]
    fn folds_comments_literals_crlf_and_final_line_without_newline() {
        for (path, text) in [
            ("main.c", "/* comment\ncontinued\n*/"),
            ("main.py", "def café():\r\n    return 42"),
            ("data.json", "{\n\"a\": 1\n}"),
        ] {
            let result = folds(path, text);
            assert_eq!(result.len(), 1, "{path}");
            assert_eq!(result[0].header, 0);
            assert_eq!(result[0].hidden.end, text.len() as u64);
        }
    }
}
