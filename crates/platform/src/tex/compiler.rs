use super::snapshot::{Budget, Snapshot};
use super::{Cancel, Config, Engine, check};
use crate::pdf::{Document, PageInfo};
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::Path,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

static COMPILATION: Mutex<()> = Mutex::new(());
const OUTPUT_LIMIT: u64 = 256 * 1024 * 1024;
const LOG_LIMIT: u64 = 1024 * 1024;
const BUILD_TIMEOUT: Duration = Duration::from_secs(60);

pub struct Compiled {
    pub document: Arc<Document>,
    pub info: PageInfo,
}

impl Engine {
    pub fn discover() -> Result<Self, String> {
        // Discovery is lazy and never runs an executable from the project/PATH.
        Self::at(Path::new("/Library/TeX/texbin/pdflatex"))
    }
    pub fn at(path: &Path) -> Result<Self, String> {
        let executable = path.canonicalize().map_err(
            |_| "Choose pdfLaTeX from an installed TeX Live distribution (MacTeX or BasicTeX)",
        )?;
        let installation = executable
            .ancestors()
            .nth(3)
            .ok_or("Invalid TeX Live executable")?
            .to_owned();
        if executable.file_name().is_none_or(|name| name != "pdftex")
            || !installation.join("texmf-dist/web2c/texmf.cnf").is_file()
        {
            return Err(
                "This increment supports TeX Live pdfLaTeX; choose its pdflatex executable".into(),
            );
        }
        // MacTeX keeps generated fonts outside the installation. Reuse only its
        // existing fonts directory, read-only; never expose HOME or follow a
        // redirected cache directory to unrelated files.
        let font_cache = std::env::var_os("HOME")
            .and_then(|home| Path::new(&home).canonicalize().ok())
            .and_then(|home| {
                let candidate = home
                    .join("Library/texlive")
                    .join(installation.file_name()?)
                    .join("texmf-var/fonts");
                candidate
                    .canonicalize()
                    .ok()
                    .filter(|path| path == &candidate && path.is_dir())
            });
        Ok(Self {
            executable,
            installation,
            font_cache,
        })
    }
}

pub fn compile(config: &Config, cancel: &Cancel) -> Result<Compiled, String> {
    if !super::supported() {
        return Err("Local TeX preview is currently available on macOS".into());
    }
    let _slot = loop {
        check(cancel)?;
        match COMPILATION.try_lock() {
            Ok(slot) => break slot,
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err("TeX compiler service stopped; reopen the application".into());
            }
            Err(std::sync::TryLockError::WouldBlock) => {}
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let workspace = tempfile::tempdir().map_err(|e| e.to_string())?;
    let private = workspace.path().canonicalize().map_err(|e| e.to_string())?;
    compile_in(config, cancel, &private)
}

fn compile_in(config: &Config, cancel: &Cancel, private: &Path) -> Result<Compiled, String> {
    let source = private.join("source");
    let output = private.join("output");
    fs::create_dir(&output).map_err(|e| e.to_string())?;
    let mut budget = Budget::default();
    let mut snapshots = Vec::new();
    for root in &config.input_folders {
        let destination = source.join(
            root.strip_prefix(&config.base)
                .map_err(|_| "Invalid input folder")?,
        );
        snapshots.push(Snapshot::capture(root, &destination, cancel, &mut budget)?);
    }
    let working = source.join(
        config
            .root
            .strip_prefix(&config.base)
            .map_err(|_| "Invalid document folder")?,
    );
    let deadline = Instant::now() + BUILD_TIMEOUT;
    let input = config
        .input
        .strip_prefix(&config.root)
        .map_err(|_| "TeX root must be inside the enabled folder")?;
    let job = input
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or("TeX root needs a UTF-8 filename")?;
    let artifact = |extension: &str| output.join(format!("{job}.{extension}"));
    let engine = &config.engine;
    let profile = private.join("sandbox.sb");
    fs::write(&profile, sandbox_profile(&source, &output, engine)).map_err(|e| e.to_string())?;
    let mut bibliography = false;
    for pass in 0..3 {
        let mut latex = command(engine, &profile, &source, &output, &engine.executable);
        latex.current_dir(&working);
        latex
            .args([
                "-progname=pdflatex",
                "-fmt=pdflatex",
                "-no-shell-escape",
                "-no-parse-first-line",
                "-interaction=nonstopmode",
                "-halt-on-error",
                "-file-line-error",
                "-recorder",
            ])
            .arg(format!("-jobname={job}"))
            .arg(format!("-output-directory={}", output.display()))
            .arg(Path::new(".").join(input));
        run(latex, &output, cancel, deadline)?;
        if pass == 0 {
            if artifact("bcf").exists() {
                return Err(
                    "This document requires Biber, which is not supported by local preview yet"
                        .into(),
                );
            }
            let aux = bounded_text(&artifact("aux"), LOG_LIMIT)?;
            if aux.contains("\\bibdata{") {
                let bibtex = engine
                    .executable
                    .parent()
                    .unwrap()
                    .join("bibtex")
                    .canonicalize()
                    .map_err(|_| "This document needs BibTeX, but it is not installed")?;
                if !bibtex.starts_with(&engine.installation) {
                    return Err("BibTeX must belong to the selected TeX Live installation".into());
                }
                let mut command = command(engine, &profile, &source, &output, &bibtex);
                command
                    .current_dir(&output)
                    .arg(Path::new(".").join(job))
                    .env(
                        "BIBINPUTS",
                        format!("{}:{}//:", working.display(), source.display()),
                    )
                    .env(
                        "BSTINPUTS",
                        format!("{}:{}//:", working.display(), source.display()),
                    );
                run(command, &output, cancel, deadline)?;
                bibliography = true;
            }
        }
        // Two passes resolve ordinary labels; bibliography requires the third.
        if pass == 1 && !bibliography {
            let log = bounded_text(&artifact("log"), LOG_LIMIT)?;
            if !log.contains("Rerun") && !log.contains("rerun") {
                break;
            }
        }
    }
    for snapshot in &snapshots {
        snapshot.validate(cancel)?;
    }
    let (document, info) = Document::open(&artifact("pdf"), cancel)?;
    // Recheck after native PDF validation as well; it can take time.
    for snapshot in &snapshots {
        snapshot.validate(cancel)?;
    }
    Ok(Compiled { document, info })
}

fn quote(path: &Path) -> String {
    serde_json::to_string(&path.to_string_lossy()).unwrap()
}
fn sandbox_profile(source: &Path, output: &Path, engine: &Engine) -> String {
    let fonts = engine
        .font_cache
        .as_ref()
        .map(|path| format!("(allow file-read* (subpath {}))", quote(path)))
        .unwrap_or_default();
    let bibtex = engine
        .executable
        .parent()
        .unwrap()
        .join("bibtex")
        .canonicalize()
        .ok()
        .filter(|path| path.starts_with(&engine.installation))
        .unwrap_or_else(|| engine.executable.clone());
    format!(
        r#"(version 1)
(deny default)
(import "system.sb")
(deny network*)
(deny process-fork)
(deny file-write-create (subpath "/cores"))
(allow file-read-metadata)
(allow sysctl-read)
(allow process-exec (literal {exe}) (literal {bibtex}))
(allow file-map-executable (literal {exe}) (literal {bibtex}) (subpath "/System/Library") (subpath "/usr/lib"))
(allow file-read* (literal {exe}) (subpath {source}) (subpath {output}) (subpath {install})
    (subpath "/System/Library") (subpath "/usr/lib") (literal "/dev/null") (literal "/dev/urandom"))
(allow file-write* (subpath {output}) (literal "/dev/null"))
{fonts}
(allow signal (target self))
"#,
        exe = quote(&engine.executable),
        bibtex = quote(&bibtex),
        source = quote(source),
        output = quote(output),
        install = quote(&engine.installation)
    )
}

fn command(
    engine: &Engine,
    profile: &Path,
    source: &Path,
    output: &Path,
    executable: &Path,
) -> Command {
    let mut command = Command::new("/usr/bin/sandbox-exec");
    command
        .arg("-f")
        .arg(profile)
        .arg(executable)
        .current_dir(source)
        .env_clear()
        .env("PATH", engine.executable.parent().unwrap())
        .env("HOME", output)
        .env("TMPDIR", output)
        .env("LC_ALL", "C")
        .env("TEXMFCNF", engine.installation.join("texmf-dist/web2c"))
        .env("TEXMFHOME", output.join("empty"))
        .env("TEXMFCONFIG", output.join("empty"))
        .env("TEXMFVAR", output.join("cache"))
        .env("TEXMFOUTPUT", output)
        .env("TEXINPUTS", format!(".:{}//:", source.display()))
        .env("BIBINPUTS", format!("{}//:", source.display()))
        .env("BSTINPUTS", format!("{}//:", source.display()))
        .env("shell_escape", "f")
        .env("openout_any", "p")
        .env("MKTEXFMT", "0")
        .env("MKTEXTFM", "0")
        .env("MKTEXPK", "0")
        .env("MKTEXMF", "0");
    if let Some(fonts) = &engine.font_cache {
        command.env("VARTEXFONTS", fonts);
    }
    command
}

#[cfg(unix)]
fn run(
    mut command: Command,
    output: &Path,
    cancel: &Cancel,
    deadline: Instant,
) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    check(cancel)?;
    let log_path = output.join(".plastext-console.log");
    let log = File::create(&log_path).map_err(|e| e.to_string())?;
    command
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log);
    // Only async-signal-safe operations run between fork and exec.
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            libc::setpriority(libc::PRIO_PROCESS, 0, 10);
            libc::alarm(65);
            for (resource, limit) in [
                (libc::RLIMIT_CORE, 0),
                (libc::RLIMIT_CPU, 60),
                (libc::RLIMIT_FSIZE, 64 * 1024 * 1024),
            ] {
                let value = libc::rlimit {
                    rlim_cur: limit,
                    rlim_max: limit,
                };
                if libc::setrlimit(resource, &value) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            // A process group includes every descendant; no obsolete child is
            // left rendering after cancellation, timeout, errors or shutdown.
            unsafe {
                libc::kill(-(self.0.id() as i32), libc::SIGTERM);
            }
            let until = Instant::now() + Duration::from_millis(100);
            while Instant::now() < until {
                if self.0.try_wait().ok().flatten().is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            unsafe {
                libc::kill(-(self.0.id() as i32), libc::SIGKILL);
            }
            let _ = self.0.wait();
        }
    }
    let mut child = Child(command.spawn().map_err(|e| e.to_string())?);
    loop {
        check(cancel)?;
        if Instant::now() >= deadline {
            return Err(
                "TeX build exceeded 60 seconds; the last successful preview is retained".into(),
            );
        }
        if fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0) > LOG_LIMIT
            || directory_bytes(output)? > OUTPUT_LIMIT
        {
            return Err("TeX build exceeded its output or log limit".into());
        }
        if let Some(status) = child.0.try_wait().map_err(|e| e.to_string())? {
            if status.success() {
                return Ok(());
            }
            return Err(format!("TeX build failed ({status}):\n{}", tail(&log_path)));
        }
        #[cfg(target_os = "macos")]
        if crate::pdf::worker_resident_bytes(child.0.id())
            .is_some_and(|rss| rss > 1024 * 1024 * 1024)
        {
            return Err("TeX compiler exceeded 1 GiB of resident memory".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
#[cfg(not(unix))]
fn run(_: Command, _: &Path, _: &Cancel, _: Instant) -> Result<(), String> {
    Err("TeX preview is currently available on macOS".into())
}

fn directory_bytes(path: &Path) -> Result<u64, String> {
    count_output(path, &mut 0, 0)
}
fn count_output(path: &Path, files: &mut usize, depth: usize) -> Result<u64, String> {
    if depth > 32 {
        return Err("TeX output is nested too deeply".into());
    }
    let mut bytes = 0;
    for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        *files += 1;
        if *files > 10_000 {
            return Err("TeX build exceeded 10,000 output entries".into());
        }
        if entry.file_type().map_err(|e| e.to_string())?.is_symlink() {
            return Err("TeX output contains a symlink".into());
        }
        let meta = entry.metadata().map_err(|e| e.to_string())?;
        bytes += if meta.is_dir() {
            count_output(&entry.path(), files, depth + 1)?
        } else {
            meta.len()
        };
        if bytes > OUTPUT_LIMIT {
            break;
        }
    }
    Ok(bytes)
}
fn bounded_text(path: &Path, limit: u64) -> Result<String, String> {
    let mut text = String::new();
    File::open(path)
        .map_err(|e| e.to_string())?
        .take(limit)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    Ok(text)
}
fn tail(path: &Path) -> String {
    let read = || -> std::io::Result<String> {
        let mut file = File::open(path)?;
        let len = file.metadata()?.len();
        file.seek(SeekFrom::Start(len.saturating_sub(4096)))?;
        let mut bytes = Vec::new();
        file.take(4096).read_to_end(&mut bytes)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    };
    read().unwrap_or_else(|e| e.to_string())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn sandbox_child() {
        let Some(root) = std::env::var_os("PLASTEXT_TEX_SANDBOX_TEST") else {
            return;
        };
        let root = std::path::PathBuf::from(root);
        assert_eq!(fs::read(root.join("source/input.txt")).unwrap(), b"allowed");
        fs::write(root.join("output/allowed.txt"), "allowed").unwrap();
        assert!(fs::read(root.join("private.txt")).is_err());
        assert!(fs::write(root.join("outside.txt"), "blocked").is_err());
        assert_eq!(fs::read(root.join("fonts/cached.pk")).unwrap(), b"font");
        assert!(fs::write(root.join("fonts/new.pk"), "blocked").is_err());
        let address = std::env::var("PLASTEXT_TEX_SANDBOX_ADDRESS").unwrap();
        let result = std::net::TcpStream::connect_timeout(
            &address.parse().unwrap(),
            Duration::from_millis(200),
        );
        assert!(result.is_err(), "networking must be denied");
        assert!(
            Command::new("/bin/sh")
                .arg("-c")
                .arg("true")
                .status()
                .is_err(),
            "child execution must be denied"
        );
    }
    #[test]
    fn sandbox_enforces_read_write_network_and_subprocess_boundaries() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let source = root.join("source");
        let output = root.join("output");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&output).unwrap();
        fs::write(source.join("input.txt"), "allowed").unwrap();
        fs::write(root.join("private.txt"), "not authorized").unwrap();
        let fonts = root.join("fonts");
        fs::create_dir(&fonts).unwrap();
        fs::write(fonts.join("cached.pk"), "font").unwrap();
        let exe = std::env::current_exe().unwrap();
        let engine = Engine {
            executable: exe.clone(),
            installation: source.clone(),
            font_cache: Some(fonts),
        };
        let profile = root.join("sandbox.sb");
        fs::write(&profile, sandbox_profile(&source, &output, &engine)).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut command = command(&engine, &profile, &source, &output, &exe);
        command
            .args([
                "--exact",
                "tex::compiler::tests::sandbox_child",
                "--nocapture",
            ])
            .env("PLASTEXT_TEX_SANDBOX_TEST", &root)
            .env(
                "PLASTEXT_TEX_SANDBOX_ADDRESS",
                listener.local_addr().unwrap().to_string(),
            );
        run(
            command,
            &output,
            &Arc::new(AtomicBool::new(false)),
            Instant::now() + Duration::from_secs(10),
        )
        .unwrap();
        assert!(!root.join("outside.txt").exists());
    }
}
