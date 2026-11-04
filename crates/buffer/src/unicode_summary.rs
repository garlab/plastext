//! A composable UTF-8 measure, including byte-preserving invalid input. Invalid
//! bytes each count as one replacement scalar/unit. A join can complete at most
//! one scalar across its boundary, which only needs three bytes on either side.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct UnicodeSummary {
    pub scalars: u64,
    pub utf16: u64,
    pub head: [u8; 3],
    pub tail: [u8; 3],
}

#[derive(Clone, Copy)]
pub(super) struct Crossing {
    pub left: u64,
    pub right: u64,
    pub utf16: u64,
}

/// Decode one scalar, or preserve one malformed byte as one replacement unit.
pub(super) fn scalar(bytes: &[u8]) -> (char, usize) {
    let first = bytes[0];
    let len = match first {
        0..=0x7f => return (first as char, 1),
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return ('\u{fffd}', 1),
    };
    bytes
        .get(..len)
        .and_then(|b| std::str::from_utf8(b).ok())
        .map_or(('\u{fffd}', 1), |s| (s.chars().next().unwrap(), len))
}

impl UnicodeSummary {
    pub fn new(bytes: &[u8]) -> Self {
        let mut result = Self::edges(bytes);
        if bytes.is_ascii() {
            result.scalars = bytes.len() as u64;
            result.utf16 = result.scalars;
        } else {
            for chunk in bytes.utf8_chunks() {
                let chars = chunk.valid().chars().count() + chunk.invalid().len();
                result.scalars += chars as u64;
                result.utf16 +=
                    chars as u64 + chunk.valid().bytes().filter(|&b| b >= 0xf0).count() as u64;
            }
        }
        result
    }

    fn edges(bytes: &[u8]) -> Self {
        let mut result = Self::default();
        let edge = bytes.len().min(3);
        result.head[..edge].copy_from_slice(&bytes[..edge]);
        result.tail[..edge].copy_from_slice(&bytes[bytes.len() - edge..]);
        result
    }

    /// Scan only the shorter side of a split. The other count follows from the
    /// original measure, correcting for a scalar bisected by the split.
    pub fn split(self, bytes: &[u8], at: usize) -> (Self, Self) {
        let (prefix, suffix) = bytes.split_at(at);
        let scan_left = prefix.len() <= suffix.len();
        let mut left = if scan_left {
            Self::new(prefix)
        } else {
            Self::edges(prefix)
        };
        let mut right = if scan_left {
            Self::edges(suffix)
        } else {
            Self::new(suffix)
        };
        let crossing = left.crossing(prefix.len() as u64, right, suffix.len() as u64);
        let scalars = self.scalars + crossing.map_or(0, |c| c.left + c.right - 1);
        let utf16 = self.utf16 + crossing.map_or(0, |c| c.left + c.right - c.utf16);
        if scan_left {
            right.scalars = scalars - left.scalars;
            right.utf16 = utf16 - left.utf16;
        } else {
            left.scalars = scalars - right.scalars;
            left.utf16 = utf16 - right.utf16;
        }
        (left, right)
    }

    pub fn crossing(self, left_len: u64, right: Self, right_len: u64) -> Option<Crossing> {
        if left_len == 0 || right_len == 0 || right.head[0] & 0xc0 != 0x80 {
            return None;
        }
        let left = left_len.min(3) as usize;
        let right_count = right_len.min(3) as usize;
        let mut bytes = [0; 6];
        bytes[..left].copy_from_slice(&self.tail[..left]);
        bytes[left..left + right_count].copy_from_slice(&right.head[..right_count]);
        for start in 0..left {
            let (ch, len) = scalar(&bytes[start..left + right_count]);
            if start + len > left {
                return Some(Crossing {
                    left: (left - start) as u64,
                    right: (start + len - left) as u64,
                    utf16: ch.len_utf16() as u64,
                });
            }
        }
        None
    }

    pub fn join(self, left_len: u64, right: Self, right_len: u64) -> Self {
        if left_len == 0 {
            return right;
        }
        if right_len == 0 {
            return self;
        }
        let mut result = Self {
            scalars: self.scalars + right.scalars,
            utf16: self.utf16 + right.utf16,
            ..Self::default()
        };
        if let Some(crossing) = self.crossing(left_len, right, right_len) {
            result.scalars -= crossing.left + crossing.right - 1;
            result.utf16 -= crossing.left + crossing.right - crossing.utf16;
        }
        let left = left_len.min(3) as usize;
        let right_count = right_len.min(3) as usize;
        let total = (left_len + right_len).min(3) as usize;
        result.head[..left].copy_from_slice(&self.head[..left]);
        result.head[left..total].copy_from_slice(&right.head[..total - left]);
        let before = total - right_count;
        result.tail[..before].copy_from_slice(&self.tail[left - before..left]);
        result.tail[before..total].copy_from_slice(&right.tail[..right_count]);
        result
    }
}
