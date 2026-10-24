use std::{convert::Infallible, path::PathBuf, str::FromStr};

/// A path with an optional 1-based line and column, written `path[:line[:col]]` (spec CLI-1).
///
/// A suffix only counts as a position when it is a positive integer, so
/// `C:\src\main.rs:3` and `notes:draft.md` parse as expected. One trailing colon
/// is ignored, so ripgrep output such as `src/lib.rs:42:7:` can be pasted as is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

impl FromStr for Location {
    type Err = Infallible;

    fn from_str(arg: &str) -> Result<Self, Self::Err> {
        let arg = arg.strip_suffix(':').unwrap_or(arg);
        let (path, line, column) = match split_number(arg) {
            (rest, Some(last)) => match split_number(rest) {
                (path, Some(line)) => (path, Some(line), Some(last)),
                (_, None) => (rest, Some(last), None),
            },
            (_, None) => (arg, None, None),
        };
        Ok(Self {
            path: path.into(),
            line,
            column,
        })
    }
}

/// Splits off a positive number after the last `:`, provided something precedes it.
fn split_number(text: &str) -> (&str, Option<u32>) {
    if let Some((head, tail)) = text.rsplit_once(':')
        && !head.is_empty()
        && let Ok(number @ 1..) = tail.parse()
    {
        return (head, Some(number));
    }
    (text, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_optional_line_and_column() {
        let cases = [
            ("src/lib.rs", "src/lib.rs", None, None),
            ("src/lib.rs:42", "src/lib.rs", Some(42), None),
            ("src/lib.rs:42:7", "src/lib.rs", Some(42), Some(7)),
            ("src/lib.rs:42:7:", "src/lib.rs", Some(42), Some(7)),
            ("notes:draft.md", "notes:draft.md", None, None),
            ("notes:draft.md:3", "notes:draft.md", Some(3), None),
            (r"C:\src\main.rs:3", r"C:\src\main.rs", Some(3), None),
            ("a.rs:0", "a.rs:0", None, None),
            ("a.rs:-1", "a.rs:-1", None, None),
            (":12", ":12", None, None),
        ];
        for (arg, path, line, column) in cases {
            let expected = Location {
                path: path.into(),
                line,
                column,
            };
            assert_eq!(arg.parse(), Ok(expected), "parsing {arg:?}");
        }
    }
}
