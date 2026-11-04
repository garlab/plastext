//! Streaming encoding and newline conversion. Malformed original units survive
//! in their own encoding; conversions fail rather than substituting bytes.
use crate::{
    LineEnding, TextEncoding, TextFormat,
    encoding::{DecodedSource, atom, invalid},
};
use std::{
    collections::VecDeque,
    io::{self, BufWriter, Write},
    ops::Range,
};

struct EncodedAtom {
    bytes: [u8; 32],
    len: usize,
    invalid: bool,
}

pub(super) struct Serializer<W: Write> {
    output: BufWriter<W>,
    format: TextFormat,
    pending_cr: bool,
    utf8_tail: Vec<u8>,
    pending: VecDeque<EncodedAtom>,
}

impl<W: Write> Serializer<W> {
    pub fn new(output: W, format: TextFormat) -> io::Result<Self> {
        let mut output = BufWriter::with_capacity(64 * 1024, output);
        if format.has_bom() {
            output.write_all(format.encoding().bom())?;
        }
        Ok(Self {
            output,
            format,
            pending_cr: false,
            utf8_tail: Vec::new(),
            pending: VecDeque::new(),
        })
    }

    pub fn utf8(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.format.encoding() == TextEncoding::UTF8 {
            if self.format.line_ending().is_none() {
                return self.output.write_all(bytes);
            }
            // Preserve malformed UTF-8 too: only CR/LF bytes are transformed.
            let mut start = 0;
            for at in memchr::memchr2_iter(b'\r', b'\n', bytes) {
                if at > start {
                    self.flush_cr()?;
                    self.output.write_all(&bytes[start..at])?;
                }
                self.newline(bytes[at])?;
                start = at + 1;
            }
            if start < bytes.len() {
                self.flush_cr()?;
                self.output.write_all(&bytes[start..])?;
            }
            return Ok(());
        }
        // Original and added UTF-8 leaves can split a scalar. Keep only an
        // incomplete suffix between calls, not an entire document projection.
        let mut offset = 0;
        if !self.utf8_tail.is_empty() {
            while offset < bytes.len() && self.utf8_tail.len() < 4 {
                self.utf8_tail.push(bytes[offset]);
                offset += 1;
                match std::str::from_utf8(&self.utf8_tail) {
                    Ok(_) => break,
                    Err(error) if error.error_len().is_some() => {
                        return Err(invalid("cannot convert malformed UTF-8"));
                    }
                    Err(_) => {}
                }
            }
            if let Ok(text) = std::str::from_utf8(&self.utf8_tail) {
                let mut small = [0; 4];
                let len = text.len();
                small[..len].copy_from_slice(text.as_bytes());
                self.utf8_tail.clear();
                self.text(std::str::from_utf8(&small[..len]).unwrap())?;
            } else {
                return Ok(());
            }
        }
        let rest = &bytes[offset..];
        match std::str::from_utf8(rest) {
            Ok(text) => self.text(text),
            Err(error) if error.error_len().is_none() => {
                self.text(std::str::from_utf8(&rest[..error.valid_up_to()]).unwrap())?;
                self.utf8_tail
                    .extend_from_slice(&rest[error.valid_up_to()..]);
                Ok(())
            }
            Err(_) => Err(invalid("cannot convert malformed UTF-8")),
        }
    }

    pub fn decoded(&mut self, source: &DecodedSource, range: Range<u64>) -> io::Result<()> {
        if !self.utf8_tail.is_empty() {
            return Err(invalid("encoded piece follows incomplete UTF-8"));
        }
        source.visit(range, |unit, raw, selected| {
            if self.format.line_ending().is_some() && (selected == b"\r" || selected == b"\n") {
                return self.newline(selected[0]);
            }
            self.flush_cr()?;
            if source.encoding == self.format.encoding() && selected.len() == unit.text_len {
                self.raw(raw, unit.invalid)
            } else if unit.invalid {
                Err(invalid("cannot convert malformed source bytes; replace or remove them explicitly first"))
            } else {
                self.encode(std::str::from_utf8(selected).unwrap())
            }
        })
    }

    fn text(&mut self, text: &str) -> io::Result<()> {
        if self.format.line_ending().is_none() {
            return self.encode(text);
        }
        let mut start = 0;
        for at in memchr::memchr2_iter(b'\r', b'\n', text.as_bytes()) {
            if at > start {
                self.flush_cr()?;
                self.encode(&text[start..at])?;
            }
            self.newline(text.as_bytes()[at])?;
            start = at + 1;
        }
        if start < text.len() {
            self.flush_cr()?;
            self.encode(&text[start..])?;
        }
        Ok(())
    }

    fn newline(&mut self, byte: u8) -> io::Result<()> {
        if self.pending_cr {
            self.flush_cr()?;
            if byte == b'\n' {
                return Ok(());
            }
        }
        if byte == b'\r' {
            self.pending_cr = true;
            Ok(())
        } else {
            self.ending()
        }
    }
    fn flush_cr(&mut self) -> io::Result<()> {
        if self.pending_cr {
            self.pending_cr = false;
            self.ending()?;
        }
        Ok(())
    }
    fn ending(&mut self) -> io::Result<()> {
        self.encode(self.format.line_ending().unwrap_or(LineEnding::Lf).as_str())
    }

    fn encode(&mut self, text: &str) -> io::Result<()> {
        let encoding = self.format.encoding();
        if encoding == TextEncoding::UTF8 {
            return self.output.write_all(text.as_bytes());
        }
        for value in text.chars() {
            let mut bytes = [0; 32];
            let len = if encoding.utf16() {
                let mut words = [0; 2];
                let words = value.encode_utf16(&mut words);
                for (i, &word) in words.iter().enumerate() {
                    let encoded = if encoding == TextEncoding::UTF16_LE {
                        word.to_le_bytes()
                    } else {
                        word.to_be_bytes()
                    };
                    bytes[i * 2..i * 2 + 2].copy_from_slice(&encoded);
                }
                words.len() * 2
            } else if value.is_ascii() {
                bytes[0] = value as u8;
                1
            } else {
                let mut utf8 = [0; 4];
                let text = value.encode_utf8(&mut utf8);
                let (result, read, written) = encoding
                    .codec()
                    .new_encoder()
                    .encode_from_utf8_without_replacement(text, &mut bytes, true);
                if result != encoding_rs::EncoderResult::InputEmpty || read != text.len() {
                    return Err(invalid(&format!(
                        "{} cannot represent {value:?}",
                        encoding.name()
                    )));
                }
                // A few legacy encoders accept compatibility aliases (e.g.
                // Shift_JIS yen -> backslash). Accepting those silently would
                // change the user's text even though no replacement was used.
                let decoded = atom(encoding, &bytes[..written]);
                if decoded.invalid
                    || decoded.text() != text.as_bytes()
                    || decoded.raw_len != written
                {
                    return Err(invalid(&format!(
                        "{} cannot represent {value:?} losslessly",
                        encoding.name()
                    )));
                }
                written
            };
            self.raw(&bytes[..len], false)?;
        }
        Ok(())
    }

    fn raw(&mut self, bytes: &[u8], malformed: bool) -> io::Result<()> {
        if self.pending.is_empty() && !malformed {
            return self.output.write_all(bytes);
        }
        let mut unit = EncodedAtom {
            bytes: [0; 32],
            len: bytes.len(),
            invalid: malformed,
        };
        unit.bytes[..bytes.len()].copy_from_slice(bytes);
        self.pending.push_back(unit);
        self.drain(false)
    }

    // Retain up to four bytes of lookahead after a malformed unit. Joining two
    // surviving pieces must not turn a malformed surrogate/lead byte into a new
    // character, or shift every UTF-16 word after an odd trailing byte.
    fn drain(&mut self, last: bool) -> io::Result<()> {
        while let Some(front) = self.pending.front() {
            if front.invalid {
                let mut lookahead = [0; 4];
                let mut len = 0;
                for unit in &self.pending {
                    let take = unit.len.min(4 - len);
                    lookahead[len..len + take].copy_from_slice(&unit.bytes[..take]);
                    len += take;
                    if len == 4 {
                        break;
                    }
                }
                if len < 4 && !last {
                    break;
                }
                let decoded = atom(self.format.encoding(), &lookahead[..len]);
                if !decoded.invalid || decoded.raw_len != front.len {
                    return Err(invalid(
                        "this edit would change the interpretation of preserved malformed bytes",
                    ));
                }
            }
            let unit = self.pending.pop_front().unwrap();
            self.output.write_all(&unit.bytes[..unit.len])?;
        }
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<()> {
        if !self.utf8_tail.is_empty() {
            return Err(invalid("cannot convert incomplete UTF-8"));
        }
        self.flush_cr()?;
        self.drain(true)?;
        self.output.flush()
    }
}
