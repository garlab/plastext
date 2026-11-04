//! Composable CR, LF and CRLF counts. A CRLF may straddle any tree boundary.
use memchr::memchr2_iter;

/// An explicit output convention; absent a conversion, original bytes survive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
    Cr,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::CrLf => "\r\n",
            Self::Cr => "\r",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineEndings {
    pub lf: u64,
    pub crlf: u64,
    pub cr: u64,
}

impl LineEndings {
    pub fn count(self) -> u64 {
        self.lf + self.crlf + self.cr
    }

    pub fn is_mixed(self) -> bool {
        u8::from(self.lf > 0) + u8::from(self.crlf > 0) + u8::from(self.cr > 0) > 1
    }

    /// Dominant convention; ties prefer LF, then CRLF. Empty files use LF.
    pub fn preferred(self) -> LineEnding {
        if self.lf >= self.crlf && self.lf >= self.cr {
            LineEnding::Lf
        } else if self.crlf >= self.cr {
            LineEnding::CrLf
        } else {
            LineEnding::Cr
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LineSummary {
    pub counts: LineEndings,
    pub starts_lf: bool,
    pub ends_cr: bool,
    empty: bool,
}

impl LineSummary {
    pub fn new(bytes: &[u8]) -> Self {
        let mut counts = LineEndings::default();
        for at in memchr2_iter(b'\r', b'\n', bytes) {
            match bytes[at] {
                b'\r' if bytes.get(at + 1) == Some(&b'\n') => counts.crlf += 1,
                b'\r' => counts.cr += 1,
                _ if at == 0 || bytes[at - 1] != b'\r' => counts.lf += 1,
                _ => {}
            }
        }
        Self {
            counts,
            starts_lf: bytes.first() == Some(&b'\n'),
            ends_cr: bytes.last() == Some(&b'\r'),
            empty: bytes.is_empty(),
        }
    }

    pub fn join(self, other: Self) -> Self {
        if self.empty {
            return other;
        }
        if other.empty {
            return self;
        }
        let crossing = u64::from(self.ends_cr && other.starts_lf);
        Self {
            counts: LineEndings {
                lf: self.counts.lf + other.counts.lf - crossing,
                crlf: self.counts.crlf + other.counts.crlf + crossing,
                cr: self.counts.cr + other.counts.cr - crossing,
            },
            starts_lf: self.starts_lf,
            ends_cr: other.ends_cr,
            empty: false,
        }
    }

    /// Scan only the shorter side of an edited leaf.
    pub fn split(self, bytes: &[u8], at: usize) -> (Self, Self) {
        let crossing =
            u64::from(at > 0 && bytes.get(at - 1) == Some(&b'\r') && bytes.get(at) == Some(&b'\n'));
        let remainder = |known: Self, rest: &[u8]| Self {
            counts: LineEndings {
                lf: self.counts.lf + crossing - known.counts.lf,
                crlf: self.counts.crlf - crossing - known.counts.crlf,
                cr: self.counts.cr + crossing - known.counts.cr,
            },
            starts_lf: rest.first() == Some(&b'\n'),
            ends_cr: rest.last() == Some(&b'\r'),
            empty: rest.is_empty(),
        };
        if at <= bytes.len() - at {
            let left = Self::new(&bytes[..at]);
            (left, remainder(left, &bytes[at..]))
        } else {
            let right = Self::new(&bytes[at..]);
            (remainder(right, &bytes[..at]), right)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_split_and_join_of_newline_combinations() {
        for n in 0..7 {
            for mut pattern in 0..3usize.pow(n) {
                let bytes: Vec<_> = (0..n)
                    .map(|_| {
                        let byte = b"x\r\n"[pattern % 3];
                        pattern /= 3;
                        byte
                    })
                    .collect();
                let summary = LineSummary::new(&bytes);
                for at in 0..=bytes.len() {
                    let (a, b) = summary.split(&bytes, at);
                    assert_eq!(a, LineSummary::new(&bytes[..at]));
                    assert_eq!(b, LineSummary::new(&bytes[at..]));
                    assert_eq!(a.join(b), summary);
                }
            }
        }
    }
}
