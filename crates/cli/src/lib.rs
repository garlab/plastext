//! The `plastext` launcher and the command-line surface it shares with the app (spec §5.9).

mod location;

use clap::Parser;

pub use location::Location;

/// Copyright and licence notices shipped with both standalone binaries.
pub const BUNDLED_ASSET_NOTICES: &str = include_str!("../../../THIRD_PARTY_NOTICES.md");

/// Arguments accepted by both the `plastext` launcher and the app binary.
#[derive(Debug, Parser)]
#[command(
    name = "plastext",
    version,
    about = "A fast, native, read-first viewer and light editor for code, Markdown and assets."
)]
pub struct Cli {
    /// Print notices for bundled third-party assets and exit.
    #[arg(long, exclusive = true)]
    pub notices: bool,

    /// Files or folders to open. A file can carry a 1-based line and column.
    #[arg(value_name = "PATH[:LINE[:COL]]")]
    pub locations: Vec<Location>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notices_are_available_without_opening_a_file() {
        assert!(
            Cli::try_parse_from(["plastext", "--notices"])
                .unwrap()
                .notices
        );
        assert!(Cli::try_parse_from(["plastext", "--notices", "file.md"]).is_err());
        assert!(
            BUNDLED_ASSET_NOTICES
                .contains(include_str!("../../ui/assets/file-icons/LICENSE").trim())
        );
    }
}
