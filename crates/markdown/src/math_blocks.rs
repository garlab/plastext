//! Let display math contain blank lines without changing source byte offsets.
use crate::parser::options;
use pulldown_cmark::{Event, Parser, Tag};
use std::{borrow::Cow, ops::Range};

/// Hide paragraph-breaking line endings inside `$$` from Markdown; math events
/// are read back from the original source. Boundary discovery also keeps an
/// unfinished formula together until the next bounded window sees its end.
pub(crate) fn prepare(text: &str, metadata: bool, keep_open: bool) -> Cow<'_, str> {
    if !text.contains("$$") {
        return Cow::Borrowed(text);
    }
    let mut candidates: Vec<(usize, Vec<Range<usize>>)> = Vec::new();
    let mut opening = None;
    let mut protected: Option<Vec<Range<usize>>> = None;
    let mut blanks = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if let Some(start) = opening {
            if trimmed.trim().is_empty() {
                blanks.push(offset + trimmed.len()..offset + line.len());
            }
            if closing(trimmed) {
                if !blanks.is_empty() {
                    candidates.push((start, std::mem::take(&mut blanks)));
                }
                opening = None;
            }
        } else {
            let body = trimmed.trim_start_matches(' ');
            let indent = trimmed.len() - body.len();
            if indent <= 3
                && body.starts_with("$$")
                && !body.starts_with("$$$")
                && !closing(&body[2..])
            {
                // Defer this extra parse until there is a multiline opener.
                // Literal examples must not consume a later real delimiter.
                let protected = protected.get_or_insert_with(|| {
                    Parser::new_ext(text, options(metadata))
                        .into_offset_iter()
                        .filter_map(|(event, range)| {
                            matches!(
                                event,
                                Event::Start(
                                    Tag::CodeBlock(_) | Tag::HtmlBlock | Tag::MetadataBlock(_)
                                ) | Event::Code(_)
                            )
                            .then_some(range)
                        })
                        .collect()
                });
                let start = offset + indent;
                let i = protected.partition_point(|range| range.end <= start);
                if !protected.get(i).is_some_and(|range| range.contains(&start)) {
                    opening = Some(start);
                }
            }
        }
        offset += line.len();
    }
    if keep_open
        && let Some(start) = opening
        && !blanks.is_empty()
    {
        candidates.push((start, blanks));
    }
    if candidates.is_empty() {
        return Cow::Borrowed(text);
    }

    let mut output = None;
    for (_, blanks) in candidates {
        let bytes = output.get_or_insert_with(|| text.as_bytes().to_vec());
        for blank in blanks {
            bytes[blank].fill(b' ');
        }
    }
    output.map_or(Cow::Borrowed(text), |bytes| {
        Cow::Owned(String::from_utf8(bytes).expect("only ASCII line endings were replaced"))
    })
}

fn closing(line: &str) -> bool {
    let bytes = line.as_bytes();
    line.match_indices("$$").any(|(at, _)| {
        at.checked_sub(1).is_none_or(|i| bytes[i] != b'$')
            && bytes.get(at + 2) != Some(&b'$')
            && bytes[..at]
                .iter()
                .rev()
                .take_while(|&&b| b == b'\\')
                .count()
                .is_multiple_of(2)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlockKind, parse};

    #[test]
    fn blank_lines_preserve_tex_and_source_offsets_in_lf_and_crlf() {
        for newline in ["\n", "\r\n"] {
            let text =
                include_str!("../../../docs/fixtures/math-headings.md").replace('\n', newline);
            let document = parse(&text);
            let math: Vec<_> = document
                .blocks
                .iter()
                .filter(|b| b.kind == BlockKind::Math)
                .collect();
            assert_eq!(math.len(), 1);
            assert!(math[0].text.contains(&format!("{newline}{newline}")));
            assert_eq!(
                &text[math[0].source.clone()],
                format!("$${}$$", math[0].text)
            );
            assert!(document.blocks[0].spans.iter().any(|span| span.style.math));
            assert_eq!(
                document.headings.last().unwrap().source.start,
                text.find("#### A smaller").unwrap()
            );
            assert_eq!(prepare(&text, true, false).len(), text.len());
        }
    }

    #[test]
    fn literal_regions_and_unclosed_delimiters_are_unchanged() {
        for text in [
            "```tex\n$$\na\n\nb\n$$\n```\n",
            "~~~\n$$\na\n\nb\n$$\n~~~\n",
            "    $$\n    a\n\n    b\n    $$\n",
            "`literal\n$$\na\nb\n$$\nend`\n",
            "<!--\n$$\na\n\nb\n$$\n-->\n",
            "---\nvalue: |\n  $$\n  a\n\n  b\n  $$\n---\n",
            "$$\na\n\nb\n",
            "$$$\na\n\nb\n$$$\n",
            "\\$$\na\n\nb\n\\$$\n",
        ] {
            assert_eq!(prepare(text, true, false), text, "{text}");
        }
        assert!(matches!(
            prepare("$$x^2$$\n\ntext", true, false),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn an_unpaired_literal_delimiter_cannot_consume_real_math() {
        let text = "```\n$$\n```\n\n$$\na\n\nb\n$$\n";
        let document = parse(text);
        assert_eq!(document.blocks.len(), 2);
        assert_eq!(document.blocks[0].text, "$$\n");
        assert_eq!(document.blocks[1].kind, BlockKind::Math);
        assert!(document.blocks[1].text.contains("a\n\nb"));
    }

    #[test]
    fn ordinary_math_in_containers_keeps_markdown_prefixes_out_of_tex() {
        let document = parse("> $$\n> a + b\n> $$\n\n$$\nc\n\nd\n$$\n");
        let math: Vec<_> = document
            .blocks
            .iter()
            .filter(|b| b.kind == BlockKind::Math)
            .collect();
        assert_eq!(math.len(), 2);
        assert!(!math[0].text.contains('>'));
        assert!(math[1].text.contains("c\n\nd"));
    }

    #[test]
    fn text_after_a_closing_delimiter_keeps_its_paragraph_breaks() {
        let text = "$$a\n\nb$$ and text\n\nnext\n";
        let document = parse(text);
        assert_eq!(document.blocks[0].kind, BlockKind::Math);
        assert_eq!(document.blocks[0].text, "a\n\nb");
        assert_eq!(document.blocks.last().unwrap().text, "next");
    }
}
