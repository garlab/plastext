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

#[cfg(test)]
mod tests {
    use super::*;

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
