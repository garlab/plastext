//! The `plastext` launcher and the command-line surface it shares with the app (spec §5.9).

mod location;

use clap::Parser;

pub use location::Location;

/// Arguments accepted by both the `plastext` launcher and the app binary.
#[derive(Debug, Parser)]
#[command(
    name = "plastext",
    version,
    about = "A fast, native, read-first viewer and light editor for code, Markdown and assets."
)]
pub struct Cli {
    /// Files or folders to open. A file can carry a 1-based line and column.
    #[arg(value_name = "PATH[:LINE[:COL]]")]
    pub locations: Vec<Location>,
}
