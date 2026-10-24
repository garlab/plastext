//! Turning a row's bytes into the text shown on screen.

use std::ops::Range;

use plastext_syntax::Highlight;

/// The on-screen part of a row.
#[derive(Debug, PartialEq)]
pub(crate) struct VisibleText {
    pub text: String,
    /// Consecutive runs covering `text`: a length in bytes of `text` and its highlight.
    pub runs: Vec<(usize, Option<Highlight>)>,
    /// The row's full width in columns.
    pub width: usize,
}

/// The `columns` of a row's text, with tabs expanded and control characters
/// made visible. `spans` are sorted highlights, with offsets into `bytes`.
///
/// Each character counts as one column, which is exact for monospace text
/// except for wide (e.g. CJK) characters.
pub(crate) fn visible_text(
    bytes: &[u8],
    tab_size: usize,
    columns: Range<usize>,
    spans: &[(Range<usize>, Highlight)],
) -> VisibleText {
    let tab_size = tab_size.max(1);
    let mut visible = VisibleText {
        text: String::new(),
        runs: Vec::new(),
        width: 0,
    };
    let mut spans = spans.iter().peekable();
    let mut push = |ch: char, offset: usize, visible: &mut VisibleText| {
        let (shown, width) = match ch {
            '\t' => (' ', tab_size - visible.width % tab_size),
            '\u{7f}' => ('\u{2421}', 1),
            ch if (ch as u32) < 0x20 => {
                (char::from_u32(0x2400 + ch as u32).unwrap_or('\u{fffd}'), 1)
            }
            ch if ch.is_control() => ('\u{fffd}', 1),
            ch => (ch, 1),
        };
        let shown_columns = (visible.width..visible.width + width)
            .filter(|column| columns.contains(column))
            .count();
        visible.width += width;
        if shown_columns == 0 {
            return;
        }
        while spans.next_if(|(span, _)| span.end <= offset).is_some() {}
        let highlight = spans
            .peek()
            .filter(|(span, _)| span.contains(&offset))
            .map(|(_, highlight)| *highlight);
        let len = shown.len_utf8() * shown_columns;
        visible
            .text
            .extend(std::iter::repeat_n(shown, shown_columns));
        match visible.runs.last_mut() {
            Some((run_len, run_highlight)) if *run_highlight == highlight => *run_len += len,
            _ => visible.runs.push((len, highlight)),
        }
    };

    let mut offset = 0;
    for chunk in bytes.utf8_chunks() {
        for (i, ch) in chunk.valid().char_indices() {
            push(ch, offset + i, &mut visible);
        }
        offset += chunk.valid().len();
        if !chunk.invalid().is_empty() {
            push('\u{fffd}', offset, &mut visible);
            offset += chunk.invalid().len();
        }
    }
    visible
}

/// Each character of a row with its byte offset, starting column and width
/// in columns. Invalid UTF-8 shows as one replacement character per run.
fn characters(
    bytes: &[u8],
    tab_size: usize,
) -> impl Iterator<Item = (usize, usize, usize, usize)> + '_ {
    let tab_size = tab_size.max(1);
    let mut column = 0;
    let mut offset = 0;
    bytes.utf8_chunks().flat_map(move |chunk| {
        let start = offset;
        offset += chunk.valid().len() + chunk.invalid().len();
        let invalid = (!chunk.invalid().is_empty()).then(|| {
            (
                start + chunk.valid().len(),
                '\u{fffd}',
                chunk.invalid().len(),
            )
        });
        chunk
            .valid()
            .char_indices()
            .map(move |(i, ch)| (start + i, ch, ch.len_utf8()))
            .chain(invalid)
            .map(move |(offset, ch, len)| {
                let width = if ch == '\t' {
                    tab_size - column % tab_size
                } else {
                    1
                };
                let character = (offset, len, column, width);
                column += width;
                character
            })
            .collect::<Vec<_>>()
    })
}

/// The column where the character at byte `offset` of a row starts, or the
/// row's width if `offset` is at or past its end.
pub(crate) fn column_at(bytes: &[u8], tab_size: usize, offset: usize) -> usize {
    let mut width = 0;
    for (start, _, column, char_width) in characters(bytes, tab_size) {
        if start >= offset {
            return column;
        }
        width = column + char_width;
    }
    width
}

/// The character boundary of a row nearest to the boundary before `column`.
pub(crate) fn offset_at(bytes: &[u8], tab_size: usize, column: usize) -> usize {
    for (offset, len, start, width) in characters(bytes, tab_size) {
        if column <= start {
            return offset;
        }
        if column < start + width {
            return if column - start <= width / 2 {
                offset
            } else {
                offset + len
            };
        }
    }
    bytes.len()
}

/// The word, run of spaces or single symbol around byte `offset` of a row.
pub(crate) fn word_at(bytes: &[u8], offset: usize) -> std::ops::Range<usize> {
    #[derive(PartialEq)]
    enum Class {
        Word,
        Space,
        Other,
    }
    let class = |ch: char| {
        if ch.is_alphanumeric() || ch == '_' {
            Class::Word
        } else if ch.is_whitespace() {
            Class::Space
        } else {
            Class::Other
        }
    };
    let text = String::from_utf8_lossy(bytes);
    if text.len() != bytes.len() || offset >= bytes.len() {
        // Invalid UTF-8 would shift offsets; fall back to a single byte.
        return offset.min(bytes.len())..(offset + 1).min(bytes.len());
    }
    let Some(ch) = text[offset..].chars().next() else {
        return offset..offset;
    };
    let target = class(ch);
    if target == Class::Other {
        return offset..offset + ch.len_utf8();
    }
    let start = text[..offset]
        .char_indices()
        .rev()
        .take_while(|&(_, ch)| class(ch) == target)
        .last()
        .map_or(offset, |(i, _)| i);
    let end = text[offset..]
        .char_indices()
        .find(|&(_, ch)| class(ch) != target)
        .map_or(text.len(), |(i, _)| offset + i);
    start..end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_between_offsets_and_columns() {
        let row = "a\tbé c".as_bytes();
        // a:0, tab:1..4, b:4, é:5 (2 bytes), space:6, c:7
        assert_eq!(column_at(row, 4, 0), 0);
        assert_eq!(column_at(row, 4, 2), 4);
        assert_eq!(column_at(row, 4, 3), 5);
        assert_eq!(column_at(row, 4, 6), 7);
        assert_eq!(column_at(row, 4, 99), 8);
        assert_eq!(offset_at(row, 4, 0), 0);
        assert_eq!(offset_at(row, 4, 2), 1, "nearer the tab's start");
        assert_eq!(offset_at(row, 4, 3), 2, "nearer the tab's end");
        assert_eq!(offset_at(row, 4, 6), 5);
        assert_eq!(offset_at(row, 4, 50), row.len());
    }

    #[test]
    fn finds_words_spaces_and_symbols() {
        let row = b"let foo_bar = x;  ";
        assert_eq!(word_at(row, 5), 4..11);
        assert_eq!(word_at(row, 12), 12..13);
        assert_eq!(word_at(row, 16), 16..18);
        assert_eq!(word_at(row, 0), 0..3);
        assert_eq!(word_at(row, 99), 18..18);
    }

    fn text(bytes: &[u8], columns: Range<usize>) -> (String, usize) {
        let visible = visible_text(bytes, 4, columns, &[]);
        (visible.text, visible.width)
    }

    #[test]
    fn expands_tabs_and_shows_control_characters() {
        assert_eq!(text(b"a\tb", 0..80), ("a   b".to_owned(), 5));
        assert_eq!(
            text(b"\x00x\x7f", 0..80),
            ("\u{2400}x\u{2421}".to_owned(), 3)
        );
        assert_eq!(text("é\u{fffd}".as_bytes(), 0..80).1, 2);
        assert_eq!(text(b"\xffz", 0..80), ("\u{fffd}z".to_owned(), 2));
    }

    #[test]
    fn keeps_only_the_visible_columns() {
        assert_eq!(text(b"0123456789", 3..7), ("3456".to_owned(), 10));
        assert_eq!(text(b"\tab", 2..5), ("  a".to_owned(), 6));
        assert_eq!(text(b"short", 10..15), (String::new(), 5));
    }

    #[test]
    fn splits_text_into_highlighted_runs() {
        use Highlight::*;
        let spans = [(0..2, Keyword), (3..7, Function)];
        let visible = visible_text(b"fn main()", 4, 0..80, &spans);
        assert_eq!(visible.text, "fn main()");
        assert_eq!(
            visible.runs,
            [
                (2, Some(Keyword)),
                (1, None),
                (4, Some(Function)),
                (2, None)
            ]
        );

        // Runs follow the visible columns, in bytes of the shown text.
        let visible = visible_text(
            "fn é\tx".as_bytes(),
            4,
            1..6,
            &[(0..2, Keyword), (3..5, String)],
        );
        assert_eq!(visible.text, "n é  ");
        assert_eq!(
            visible.runs,
            [(1, Some(Keyword)), (1, None), (2, Some(String)), (2, None)]
        );
    }
}
