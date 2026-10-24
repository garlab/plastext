use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, de};

/// A number of bytes, written as an integer or as a string such as `"64MB"`
/// or `"1.5 GiB"`. KB, MB, GB and TB are powers of 1000; KiB, MiB, GiB and
/// TiB powers of 1024.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ByteSize(pub u64);

#[derive(Debug, PartialEq, Eq)]
pub struct ParseByteSizeError(String);

impl fmt::Display for ParseByteSizeError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "`{}` isn't a size such as \"64MB\" or \"1.5 GiB\"",
            self.0
        )
    }
}

impl std::error::Error for ParseByteSizeError {}

impl FromStr for ByteSize {
    type Err = ParseByteSizeError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let error = || ParseByteSizeError(text.to_owned());
        let trimmed = text.trim();
        let split = trimmed
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(trimmed.len());
        let (number, unit) = trimmed.split_at(split);
        let number: f64 = number.parse().map_err(|_| error())?;
        let multiplier: u64 = match unit.trim().to_ascii_lowercase().as_str() {
            "" | "b" => 1,
            "kb" => 1000,
            "mb" => 1000_u64.pow(2),
            "gb" => 1000_u64.pow(3),
            "tb" => 1000_u64.pow(4),
            "kib" => 1 << 10,
            "mib" => 1 << 20,
            "gib" => 1 << 30,
            "tib" => 1 << 40,
            _ => return Err(error()),
        };
        Ok(Self((number * multiplier as f64) as u64))
    }
}

impl<'de> Deserialize<'de> for ByteSize {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Bytes(u64),
            Text(String),
        }
        match Raw::deserialize(deserializer)? {
            Raw::Bytes(bytes) => Ok(Self(bytes)),
            Raw::Text(text) => text.parse().map_err(de::Error::custom),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sizes_with_units() {
        let cases = [
            ("512", 512),
            ("64MB", 64_000_000),
            ("64 mb", 64_000_000),
            ("1.5 GiB", 3 << 29),
            ("256KiB", 256 * 1024),
            ("2TB", 2_000_000_000_000),
        ];
        for (text, bytes) in cases {
            assert_eq!(text.parse(), Ok(ByteSize(bytes)), "{text}");
        }
        for text in ["", "MB", "12 parsecs", "1.2.3GB"] {
            assert!(text.parse::<ByteSize>().is_err(), "{text}");
        }
    }
}
