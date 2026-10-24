//! `plastext-bench`: writes the extreme-case fixtures of spec §6.2 and times
//! app launches (spec §9).

use std::{
    env,
    fs::File,
    io::{self, BufWriter, Write as _},
    path::PathBuf,
    process::{Command, ExitCode},
    time::Instant,
};

use clap::{Parser, Subcommand};
use plastext_settings::ByteSize;

#[derive(Parser)]
#[command(
    name = "plastext-bench",
    about = "Fixtures and benchmarks for plastext"
)]
struct Args {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Write server-log lines up to SIZE (e.g. "10GB").
    Log { size: ByteSize, path: PathBuf },
    /// Write one line of minified JSON of SIZE, without a newline.
    LongLine { size: ByteSize, path: PathBuf },
    /// Create COUNT empty files in DIR.
    WideDir { count: u64, dir: PathBuf },
    /// Launch the app RUNS times and report how long its first frame takes.
    ///
    /// Build the app first, e.g. `cargo build --release -p plastext-app`.
    Launch {
        #[arg(long, default_value_t = 10)]
        runs: usize,
        /// The app binary; defaults to `plastext-app` next to this binary.
        #[arg(long)]
        app: Option<PathBuf>,
        /// Arguments for the app.
        #[arg(last = true)]
        args: Vec<String>,
    },
}

fn main() -> ExitCode {
    let result = match Args::parse().command {
        Commands::Log { size, path } => {
            write_file(&path, |out| plastext_bench::write_log(out, size.0))
        }
        Commands::LongLine { size, path } => {
            write_file(&path, |out| plastext_bench::write_long_line(out, size.0))
        }
        Commands::WideDir { count, dir } => plastext_bench::create_wide_dir(&dir, count),
        Commands::Launch { runs, app, args } => launch(runs, app, &args),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("plastext-bench: {err}");
            ExitCode::FAILURE
        }
    }
}

fn write_file(
    path: &PathBuf,
    write: impl FnOnce(&mut BufWriter<File>) -> io::Result<()>,
) -> io::Result<()> {
    let mut out = BufWriter::with_capacity(1 << 20, File::create(path)?);
    write(&mut out)?;
    out.flush()
}

/// Runs the app with `PLASTEXT_PROFILE_STARTUP`, which makes it report its
/// first frame and quit, and prints the median of each measurement.
fn launch(runs: usize, app: Option<PathBuf>, args: &[String]) -> io::Result<()> {
    let app = match app {
        Some(app) => app,
        None => {
            env::current_exe()?.with_file_name(format!("plastext-app{}", env::consts::EXE_SUFFIX))
        }
    };
    // A socket of its own, so a running plastext doesn't take over the launches.
    let socket = env::temp_dir().join("plastext-bench.sock");
    let mut wall = Vec::new();
    let mut window = Vec::new();
    let mut first_frame = Vec::new();
    for run in 1..=runs.max(1) {
        let start = Instant::now();
        let output = Command::new(&app)
            .args(args)
            .env("PLASTEXT_PROFILE_STARTUP", "1")
            .env("PLASTEXT_SOCKET", &socket)
            .output()?;
        let elapsed = start.elapsed().as_secs_f64() * 1000.;
        let stderr = String::from_utf8_lossy(&output.stderr);
        let Some((opened, drawn)) = stderr.lines().find_map(parse_profile) else {
            return Err(io::Error::other(format!(
                "{} reported no startup profile:\n{stderr}",
                app.display()
            )));
        };
        println!(
            "run {run:>2}: window opened in {opened:6.1} ms, first frame by {drawn:6.1} ms, process exited after {elapsed:6.1} ms"
        );
        wall.push(elapsed);
        window.push(opened);
        first_frame.push(drawn);
    }
    println!(
        "median: window opened in {:.1} ms, first frame by {:.1} ms, process exited after {:.1} ms",
        median(&mut window),
        median(&mut first_frame),
        median(&mut wall)
    );
    Ok(())
}

/// Parses the app's "startup: window opened in X ms, first frame drawn by Y ms" line.
fn parse_profile(line: &str) -> Option<(f64, f64)> {
    let rest = line.strip_prefix("startup: window opened in ")?;
    let (opened, rest) = rest.split_once(" ms, first frame drawn by ")?;
    let drawn = rest.strip_suffix(" ms")?;
    Some((opened.parse().ok()?, drawn.parse().ok()?))
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_startup_profile() {
        assert_eq!(
            parse_profile("startup: window opened in 71.9 ms, first frame drawn by 365.3 ms"),
            Some((71.9, 365.3))
        );
        assert_eq!(parse_profile("plastext: something else"), None);
    }
}
