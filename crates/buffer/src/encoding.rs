//! Lossless originals plus a bounded UTF-8 projection for non-UTF-8 editing.
//! Checkpoints describe blocks, not characters; save maps surviving pieces back
//! to their original encoded units instead of roundtripping through an encoder.
use std::{
    io::{self, Write},
    ops::Range,
    sync::{Arc, atomic::AtomicBool},
};

use crate::{
    ByteSource, FileSource, LineEnding, OpenOptions, edit::check_cancel, line_summary::LineSummary,
    piece_tree::LEAF_BYTES, unicode_summary::UnicodeSummary,
};
use encoding_rs::{DecoderResult, Encoding};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextEncoding(&'static Encoding);

impl TextEncoding {
    pub const UTF8: Self = Self(encoding_rs::UTF_8);
    pub const UTF16_LE: Self = Self(encoding_rs::UTF_16LE);
    pub const UTF16_BE: Self = Self(encoding_rs::UTF_16BE);

    /// WHATWG labels (e.g. windows-1252, Shift_JIS). Stateful ISO-2022-JP and
    /// replacement-only encodings cannot provide independently editable pieces.
    pub fn for_label(label: &str) -> Option<Self> {
        Encoding::for_label_no_replacement(label.as_bytes())
            .filter(|encoding| *encoding != encoding_rs::ISO_2022_JP)
            .map(Self)
    }
    pub fn name(self) -> &'static str {
        self.0.name()
    }
    pub(super) fn utf16(self) -> bool {
        self == Self::UTF16_LE || self == Self::UTF16_BE
    }
    pub(super) fn bom(self) -> &'static [u8] {
        if self == Self::UTF8 {
            b"\xef\xbb\xbf"
        } else if self == Self::UTF16_LE {
            b"\xff\xfe"
        } else if self == Self::UTF16_BE {
            b"\xfe\xff"
        } else {
            b""
        }
    }
    pub(super) fn codec(self) -> &'static Encoding {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EncodingHint {
    #[default]
    Auto,
    Explicit(TextEncoding),
}

/// Output settings are versioned with the text. Newline conversion is a save
/// policy, so changing it does not rewrite the tree or move pane selections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextFormat {
    encoding: TextEncoding,
    bom: bool,
    line_ending: Option<LineEnding>,
}
impl Default for TextFormat {
    fn default() -> Self {
        Self {
            encoding: TextEncoding::UTF8,
            bom: false,
            line_ending: None,
        }
    }
}
impl TextFormat {
    pub fn new(encoding: TextEncoding, bom: bool) -> io::Result<Self> {
        if bom && encoding.bom().is_empty() {
            return Err(invalid("this encoding has no byte-order mark"));
        }
        Ok(Self {
            encoding,
            bom,
            line_ending: None,
        })
    }
    pub fn encoding(self) -> TextEncoding {
        self.encoding
    }
    pub fn has_bom(self) -> bool {
        self.bom
    }
    pub fn line_ending(self) -> Option<LineEnding> {
        self.line_ending
    }
    pub fn with_line_ending(mut self, ending: Option<LineEnding>) -> Self {
        self.line_ending = ending;
        self
    }
}

pub(super) fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub(super) fn detect(source: &FileSource, hint: EncodingHint) -> io::Result<(TextFormat, u64)> {
    let mut sample = vec![0; source.len().min(128 * 1024) as usize];
    source.read_uncached(0..sample.len() as u64, &mut sample)?;
    if (sample.starts_with(b"\xff\xfe\0\0") || sample.starts_with(b"\0\0\xfe\xff"))
        && !matches!(hint, EncodingHint::Explicit(encoding) if encoding.utf16())
    {
        return Err(invalid("UTF-32 is not supported for editing"));
    }
    let bom = [
        TextEncoding::UTF8,
        TextEncoding::UTF16_LE,
        TextEncoding::UTF16_BE,
    ]
    .into_iter()
    .find(|encoding| sample.starts_with(encoding.bom()));
    let encoding = match (bom, hint) {
        (Some(bom), EncodingHint::Explicit(requested)) if bom != requested => {
            return Err(invalid(
                "the selected encoding conflicts with the file's byte-order mark",
            ));
        }
        (Some(bom), _) => bom,
        (None, EncodingHint::Explicit(requested)) => requested,
        (None, EncodingHint::Auto) => {
            // An incomplete scalar at the sample boundary is not invalid UTF-8.
            let valid_utf8 = match std::str::from_utf8(&sample) {
                Ok(_) => true,
                Err(error) => error.error_len().is_none() && (sample.len() as u64) < source.len(),
            };
            if valid_utf8 {
                TextEncoding::UTF8
            } else {
                let mut detector =
                    chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
                detector.feed(&sample, sample.len() as u64 == source.len());
                TextEncoding(detector.guess(None, chardetng::Utf8Detection::Allow))
            }
        }
    };
    Ok((
        TextFormat::new(encoding, bom.is_some())?,
        bom.map_or(0, |encoding| encoding.bom().len() as u64),
    ))
}

pub(super) struct Atom {
    pub raw_len: usize,
    pub text: [u8; 8],
    pub text_len: usize,
    pub invalid: bool,
}
impl Atom {
    fn scalar(raw_len: usize, value: char, invalid: bool) -> Self {
        let mut atom = Self {
            raw_len,
            text: [0; 8],
            text_len: 0,
            invalid,
        };
        atom.text_len = value.encode_utf8(&mut atom.text).len();
        atom
    }
    pub fn text(&self) -> &[u8] {
        &self.text[..self.text_len]
    }
}

/// Decode one stateless source unit. Invalid bytes are independent units;
/// UTF-16 preserves an invalid code unit (or final odd byte) as one replacement.
pub(super) fn atom(encoding: TextEncoding, bytes: &[u8]) -> Atom {
    assert!(!bytes.is_empty());
    if encoding.utf16() {
        let word = |at| {
            if encoding == TextEncoding::UTF16_LE {
                u16::from_le_bytes([bytes[at], bytes[at + 1]])
            } else {
                u16::from_be_bytes([bytes[at], bytes[at + 1]])
            }
        };
        if bytes.len() < 2 {
            return Atom::scalar(1, '\u{fffd}', true);
        }
        let first = word(0);
        if (0xd800..=0xdbff).contains(&first) && bytes.len() >= 4 {
            let second = word(2);
            if (0xdc00..=0xdfff).contains(&second) {
                let scalar =
                    0x10000 + ((u32::from(first) - 0xd800) << 10) + u32::from(second) - 0xdc00;
                return Atom::scalar(4, char::from_u32(scalar).unwrap(), false);
            }
        }
        return char::from_u32(u32::from(first)).map_or_else(
            || Atom::scalar(2, '\u{fffd}', true),
            |value| Atom::scalar(2, value, false),
        );
    }
    if bytes[0] < 0x80 {
        return Atom::scalar(1, char::from(bytes[0]), false);
    }
    if encoding == TextEncoding::UTF8 {
        let (value, len) = crate::unicode_summary::scalar(bytes);
        return Atom::scalar(len, value, len == 1 && bytes[0] >= 0x80);
    }
    // Some Big5 units expand to two Unicode scalars; an atom is not necessarily
    // one character. No supported stateless codec consumes more than four bytes.
    for len in 1..=bytes.len().min(4) {
        let mut output = [0; 16];
        let (result, read, written) = encoding
            .0
            .new_decoder_without_bom_handling()
            .decode_to_utf8_without_replacement(&bytes[..len], &mut output, true);
        if result == DecoderResult::InputEmpty && read == len && written != 0 {
            let mut atom = Atom {
                raw_len: len,
                text: [0; 8],
                text_len: written,
                invalid: false,
            };
            atom.text[..written].copy_from_slice(&output[..written]);
            return atom;
        }
    }
    Atom::scalar(1, '\u{fffd}', true)
}

pub(super) type LeafSummary = (u64, LineSummary, UnicodeSummary);
struct Checkpoint {
    text: u64,
    raw: u64,
}
pub(super) struct DecodedSource {
    pub text: Arc<FileSource>,
    raw: Arc<FileSource>,
    pub encoding: TextEncoding,
    checkpoints: Vec<Checkpoint>,
}

/// Bounded positional input with enough lookahead for any source unit.
struct Reader<'a> {
    source: &'a FileSource,
    offset: u64,
    buffer: Vec<u8>,
    start: usize,
    end: usize,
}
impl<'a> Reader<'a> {
    fn new(source: &'a FileSource, offset: u64) -> Self {
        Self {
            source,
            offset,
            buffer: vec![0; LEAF_BYTES + 4],
            start: 0,
            end: 0,
        }
    }
    fn peek(&mut self) -> io::Result<&[u8]> {
        if self.end - self.start < 4
            && self.offset + ((self.end - self.start) as u64) < self.source.len()
        {
            let len = (self.source.len() - self.offset).min(self.buffer.len() as u64) as usize;
            self.source.read_uncached(
                self.offset..self.offset + len as u64,
                &mut self.buffer[..len],
            )?;
            self.start = 0;
            self.end = len;
        }
        Ok(&self.buffer[self.start..self.end])
    }
    fn advance(&mut self, len: usize) {
        self.start += len;
        self.offset += len as u64;
    }
}

impl DecodedSource {
    pub fn prepare(
        raw: Arc<FileSource>,
        encoding: TextEncoding,
        skip: u64,
        options: OpenOptions,
        cancel: &AtomicBool,
        progress: &mut impl FnMut(u64, u64),
    ) -> io::Result<(Arc<Self>, Vec<LeafSummary>)> {
        let mut output = tempfile::tempfile()?;
        let mut reader = Reader::new(&raw, skip);
        let mut block = Vec::with_capacity(LEAF_BYTES);
        let mut leaves = Vec::new();
        let mut checkpoints = vec![Checkpoint { text: 0, raw: skip }];
        let mut text_len = 0;
        loop {
            let bytes = reader.peek()?;
            if bytes.is_empty() {
                break;
            }
            let unit = atom(encoding, bytes);
            if block.len() + unit.text_len > LEAF_BYTES {
                check_cancel(cancel)?;
                output.write_all(&block)?;
                leaves.push((
                    block.len() as u64,
                    LineSummary::new(&block),
                    UnicodeSummary::new(&block),
                ));
                text_len += block.len() as u64;
                checkpoints.push(Checkpoint {
                    text: text_len,
                    raw: reader.offset,
                });
                block.clear();
                progress(reader.offset, raw.len());
            }
            block.extend_from_slice(unit.text());
            reader.advance(unit.raw_len);
        }
        check_cancel(cancel)?;
        if !block.is_empty() {
            output.write_all(&block)?;
            leaves.push((
                block.len() as u64,
                LineSummary::new(&block),
                UnicodeSummary::new(&block),
            ));
        }
        progress(raw.len(), raw.len());
        let text = Arc::new(FileSource::from_file(output, options)?);
        Ok((
            Arc::new(Self {
                text,
                raw,
                encoding,
                checkpoints,
            }),
            leaves,
        ))
    }

    /// Visit source units overlapping a surviving UTF-8 piece. At most one
    /// checkpoint block is scanned before the requested range. Raw bytes stay
    /// private even when the user's original file is rewritten elsewhere.
    pub fn visit(
        &self,
        range: Range<u64>,
        mut visitor: impl FnMut(&Atom, &[u8], &[u8]) -> io::Result<()>,
    ) -> io::Result<()> {
        if range.is_empty() {
            return Ok(());
        }
        let checkpoint = &self.checkpoints[self
            .checkpoints
            .partition_point(|point| point.text <= range.start)
            - 1];
        let mut reader = Reader::new(&self.raw, checkpoint.raw);
        let mut offset = checkpoint.text;
        while offset < range.end {
            let bytes = reader.peek()?;
            let unit = atom(self.encoding, bytes);
            let end = offset + unit.text_len as u64;
            if end > range.start {
                let start = range.start.saturating_sub(offset) as usize;
                let take = (range.end.min(end) - offset) as usize;
                let text = &unit.text()[start..take];
                std::str::from_utf8(text)
                    .map_err(|_| invalid("encoded piece ends inside a Unicode scalar"))?;
                visitor(&unit, &bytes[..unit.raw_len], text)?;
            }
            offset = end;
            reader.advance(unit.raw_len);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
