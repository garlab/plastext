//! The `plastext` launcher (spec CLI-1, CLI-5).
//!
//! For now it validates the arguments and starts the app binary installed next
//! to it. In M0 it becomes the client of the single-instance socket.

use std::{
    env,
    process::{Command, ExitCode, Stdio},
};

use clap::Parser as _;
use plastext_cli::Cli;

fn main() -> ExitCode {
    // Handles `--help` and `--version`, and rejects bad arguments before the app starts.
    Cli::parse();

    let app = match env::current_exe() {
        Ok(exe) => exe.with_file_name(format!("plastext-app{}", env::consts::EXE_SUFFIX)),
        Err(err) => {
            eprintln!("plastext: cannot locate the app binary: {err}");
            return ExitCode::FAILURE;
        }
    };
    match Command::new(&app)
        .args(env::args_os().skip(1))
        .stdin(Stdio::null())
        .spawn()
    {
        Ok(_) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("plastext: cannot start {}: {err}", app.display());
            ExitCode::FAILURE
        }
    }
}
