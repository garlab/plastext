//! A fast, line-local highlighter for files too large to parse, and for
//! languages without a grammar (spec CODE-2): comments, strings, numbers and
//! known words, one line at a time.

use std::ops::Range;

use crate::Highlight;

/// How to highlight a language one line at a time.
pub struct Lexical {
    /// Starts of comments that run to the end of the line.
    pub line_comments: &'static [&'static str],
    /// Characters that open and close single-line strings.
    pub quotes: &'static [u8],
    /// Words and how to highlight them.
    pub words: &'static [(&'static str, Highlight)],
}

/// Builds a `&[(word, Highlight)]` table: `words!(Keyword: ["fn"], Constant: ["true"])`.
macro_rules! words {
    ($($highlight:ident: [$($word:literal),* $(,)?]),* $(,)?) => {
        &[$($(($word, $crate::Highlight::$highlight)),*),*]
    };
}

impl Lexical {
    pub const NONE: Self = Self {
        line_comments: &[],
        quotes: &[],
        words: &[],
    };

    /// The highlighted spans of one line, sorted and without overlaps. A
    /// string or comment left open ends with the line.
    pub fn highlights(&self, line: &[u8]) -> Vec<(Range<usize>, Highlight)> {
        let mut spans = Vec::new();
        let is_word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
        let mut i = 0;
        while i < line.len() {
            let rest = &line[i..];
            if self
                .line_comments
                .iter()
                .any(|start| rest.starts_with(start.as_bytes()))
            {
                spans.push((i..line.len(), Highlight::Comment));
                break;
            }
            let byte = line[i];
            if self.quotes.contains(&byte) {
                let mut end = i + 1;
                while end < line.len() && line[end] != byte {
                    end += if line[end] == b'\\' { 2 } else { 1 };
                }
                let end = (end + 1).min(line.len());
                spans.push((i..end, Highlight::String));
                i = end;
                continue;
            }
            if byte.is_ascii_digit() {
                let end = i + rest
                    .iter()
                    .position(|&b| !is_word(b) && b != b'.')
                    .unwrap_or(rest.len());
                spans.push((i..end, Highlight::Number));
                i = end;
                continue;
            }
            if is_word(byte) {
                let end = i + rest.iter().position(|&b| !is_word(b)).unwrap_or(rest.len());
                let word = &line[i..end];
                let known = self
                    .words
                    .iter()
                    .find(|(known, _)| known.as_bytes() == word);
                if let Some((_, highlight)) = known {
                    spans.push((i..end, *highlight));
                }
                i = end;
                continue;
            }
            i += 1;
        }
        spans
    }
}

pub(crate) const BASH: Lexical = Lexical {
    line_comments: &["#"],
    quotes: b"\"'",
    words: words!(Keyword: [
        "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac",
        "function", "in", "return", "local", "export", "readonly", "declare", "source",
    ]),
};

pub(crate) const C: Lexical = Lexical {
    line_comments: &["//"],
    quotes: b"\"'",
    words: words!(
        Keyword: [
            "auto", "break", "case", "char", "const", "continue", "default", "do", "double", "else",
            "enum", "extern", "float", "for", "goto", "if", "inline", "int", "long", "register",
            "return", "short", "signed", "sizeof", "static", "struct", "switch", "typedef", "union",
            "unsigned", "void", "volatile", "while", "class", "namespace", "template", "typename",
            "public", "private", "protected", "virtual", "override", "new", "delete", "this",
            "using", "try", "catch", "throw", "constexpr", "noexcept",
        ],
        Constant: ["NULL", "nullptr", "true", "false"],
    ),
};

pub(crate) const GO: Lexical = Lexical {
    line_comments: &["//"],
    quotes: b"\"'`",
    words: words!(
        Keyword: [
            "break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough",
            "for", "func", "go", "goto", "if", "import", "interface", "map", "package", "range",
            "return", "select", "struct", "switch", "type", "var",
        ],
        Constant: ["nil", "true", "false", "iota"],
    ),
};

pub(crate) const JAVASCRIPT: Lexical = Lexical {
    line_comments: &["//"],
    quotes: b"\"'`",
    words: words!(
        Keyword: [
            "async", "await", "break", "case", "catch", "class", "const", "continue", "debugger",
            "default", "delete", "do", "else", "export", "extends", "finally", "for", "from",
            "function", "if", "import", "in", "instanceof", "let", "new", "of", "return", "static",
            "super", "switch", "this", "throw", "try", "typeof", "var", "void", "while", "with",
            "yield", "interface", "type", "enum", "implements", "namespace", "declare", "readonly",
            "abstract", "public", "private", "protected", "as",
        ],
        Constant: ["null", "undefined", "true", "false"],
    ),
};

pub(crate) const DATA: Lexical = Lexical {
    line_comments: &["#"],
    quotes: b"\"'",
    words: words!(Constant: ["true", "false", "null", "yes", "no"]),
};

pub(crate) const JSON: Lexical = Lexical {
    line_comments: &[],
    quotes: b"\"",
    words: words!(Constant: ["true", "false", "null"]),
};

pub(crate) const PYTHON: Lexical = Lexical {
    line_comments: &["#"],
    quotes: b"\"'",
    words: words!(
        Keyword: [
            "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
            "elif", "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is",
            "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while", "with",
            "yield", "match", "case",
        ],
        Constant: ["None", "True", "False", "self"],
    ),
};

pub(crate) const RUST: Lexical = Lexical {
    line_comments: &["//"],
    // Not `'`, which also starts lifetimes.
    quotes: b"\"",
    words: words!(
        Keyword: [
            "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
            "extern", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut",
            "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "type",
            "unsafe", "use", "where", "while",
        ],
        Constant: ["true", "false", "None", "Some", "Ok", "Err"],
    ),
};

pub(crate) const LOG: Lexical = Lexical {
    line_comments: &[],
    quotes: b"\"",
    words: words!(
        Keyword: ["ERROR", "error", "FATAL", "fatal", "CRITICAL", "PANIC", "panic"],
        Type: ["WARN", "warn", "WARNING", "warning"],
        Constant: ["INFO", "info", "NOTICE"],
        Comment: ["DEBUG", "debug", "TRACE", "trace"],
    ),
};

#[cfg(test)]
mod tests {
    use super::*;

    fn spans<'a>(rules: &Lexical, line: &'a str) -> Vec<(&'a str, Highlight)> {
        rules
            .highlights(line.as_bytes())
            .into_iter()
            .map(|(range, highlight)| (&line[range], highlight))
            .collect()
    }

    #[test]
    fn highlights_comments_strings_numbers_and_words() {
        use Highlight::*;
        assert_eq!(
            spans(&RUST, r#"let x = "a \"b\" // no" + 42; // done"#),
            [
                ("let", Keyword),
                (r#""a \"b\" // no""#, String),
                ("42", Number),
                ("// done", Comment)
            ]
        );
        assert_eq!(spans(&RUST, "fn f<'a>(x: &'a str)"), [("fn", Keyword)]);
        assert_eq!(spans(&PYTHON, "print('open"), [("'open", String)]);
        assert_eq!(
            spans(&JSON, r#"{"a": [1.5, true, null]}"#),
            [
                (r#""a""#, String),
                ("1.5", Number),
                ("true", Constant),
                ("null", Constant)
            ]
        );
    }

    #[test]
    fn colors_log_levels() {
        use Highlight::*;
        assert_eq!(
            spans(&LOG, r#"2026-10-24 ERROR db timeout="30s""#),
            [
                ("2026", Number),
                ("10", Number),
                ("24", Number),
                ("ERROR", Keyword),
                (r#""30s""#, String)
            ]
        );
        assert_eq!(
            spans(&LOG, "WARNINGS are not WARNING")[0],
            ("WARNING", Type)
        );
    }
}
