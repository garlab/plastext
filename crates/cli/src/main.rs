//! The `plastext` launcher (spec CLI-1, CLI-5).
//!
//! It hands its arguments to the running app if there is one, and otherwise
//! starts the app binary installed next to it.

use std::{
    env,
    process::{Command, ExitCode, Stdio},
};

use clap::Parser as _;
use plastext_cli::Cli;
use plastext_platform::instance::{self, OpenRequest};

fn main() -> ExitCode {
    // Handles `--help` and `--version`, and rejects bad arguments before the app sees them.
    Cli::parse();

    if let Ok(request) = OpenRequest::from_env()
        && instance::send(&request).is_ok()
    {
        return ExitCode::SUCCESS;
    }

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
