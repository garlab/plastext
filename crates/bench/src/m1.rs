use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use plastext_buffer::{FileSource, OpenOptions, rows};
use plastext_project::{TreeOptions, walk_files};
use plastext_search::{FileMatcher, ProjectQuery, SearchOptions, search_project};
use plastext_syntax::SyntaxTree;
use serde_json::{Value, json};

fn ms(time: Duration) -> f64 {
    time.as_secs_f64() * 1000.
}

fn settle(matcher: &mut FileMatcher) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    while matcher.tick().running {
        if Instant::now() > deadline {
            return Err(io::Error::other("matching timed out"));
        }
        thread::sleep(Duration::from_micros(100));
    }
    Ok(())
}

fn metric(report: &mut Vec<Value>, name: &str, samples: &mut [f64], budget: Option<f64>) -> bool {
    samples.sort_by(f64::total_cmp);
    let p99 = samples[(samples.len() * 99).div_ceil(100).saturating_sub(1)];
    let p50 = samples[samples.len() / 2];
    let max = samples[samples.len() - 1];
    let pass = budget.is_none_or(|budget| p99 <= budget);
    println!(
        "{} {name}: p50 {p50:.2} ms, p99 {p99:.2} ms, max {max:.2} ms (n={}){}",
        if budget.is_none() {
            "INFO"
        } else if pass {
            "PASS"
        } else {
            "FAIL"
        },
        samples.len(),
        budget.map_or(String::new(), |value| format!(", budget {value:.0} ms"))
    );
    report.push(json!({"name": name, "unit": "ms", "p50": p50, "p99": p99, "max": max, "samples": samples, "budget": budget, "pass": pass}));
    pass
}

/// Each query is typed, then erased, so broad prefixes and backspacing are
/// measured too. Record both the first fresh nonempty snapshot (or completed
/// empty result), and final ranking of the entire index. Cached answers count.
fn queries(matcher: &mut FileMatcher, runs: usize) -> io::Result<(Vec<f64>, Vec<f64>)> {
    let mut first_results = Vec::new();
    let mut completed = Vec::new();
    for _ in 0..runs {
        let mut previous = String::new();
        for query in ["sched", "net", "drivers", "include", "core.c"] {
            for len in (1..=query.len()).chain((0..query.len()).rev()) {
                let pattern = &query[..len];
                let start = Instant::now();
                matcher.reparse(pattern, pattern.starts_with(&previous));
                let mut first = None;
                loop {
                    let status = matcher.tick();
                    if status.changed && (matcher.matched_count() > 0 || !status.running) {
                        first.get_or_insert(ms(start.elapsed()));
                    }
                    if !status.running {
                        break;
                    }
                    if start.elapsed() > Duration::from_secs(30) {
                        return Err(io::Error::other("query timed out"));
                    }
                    thread::sleep(Duration::from_micros(100));
                }
                std::hint::black_box(matcher.item(0));
                let finished = ms(start.elapsed());
                first_results.push(first.unwrap_or(finished));
                completed.push(finished);
                if std::env::var_os("PLASTEXT_TRACE_QUERIES").is_some() {
                    eprintln!(
                        "{pattern:?}: first {:.3} ms; complete {finished:.3} ms",
                        first.unwrap_or(finished)
                    );
                }
                previous = pattern.to_owned();
            }
        }
    }
    Ok((first_results, completed))
}

pub fn run(root: &Path, runs: usize, million: bool, output: Option<&Path>) -> io::Result<()> {
    if !root.is_dir() {
        return Err(io::Error::other("M1 requires an existing source directory"));
    }
    let runs = runs.max(1);
    let options = TreeOptions::default();
    let mut report = Vec::new();
    let mut pass = true;
    let mut first_results = Vec::new();
    let mut walk_times = Vec::new();
    let mut paths = Vec::new();
    let mut matcher = FileMatcher::new(&[]);
    for _ in 0..runs {
        let start = Instant::now();
        matcher = FileMatcher::new(&["kernel/sched/core.c".into(), "net/core/dev.c".into()]);
        let injector = matcher.injector();
        let done = Arc::new(AtomicBool::new(false));
        let collected = Mutex::new(Vec::new());
        let mut first = None;
        thread::scope(|scope| {
            let walk_done = done.clone();
            let collected = &collected;
            let options = &options;
            scope.spawn(move || {
                walk_files(root, options, &AtomicBool::new(false), |path| {
                    let path = path.to_string_lossy().into_owned();
                    injector.push(path.clone());
                    collected.lock().unwrap().push(path);
                });
                walk_done.store(true, Ordering::Release);
            });
            loop {
                let status = matcher.tick();
                if matcher.matched_count() > 0 {
                    first.get_or_insert(ms(start.elapsed()));
                }
                if done.load(Ordering::Acquire) && !status.running {
                    break;
                }
                thread::sleep(Duration::from_micros(100));
            }
        });
        // The final injection might race the last tick; settle once more.
        settle(&mut matcher)?;
        first_results
            .push(first.ok_or_else(|| io::Error::other("source tree had no searchable files"))?);
        walk_times.push(ms(start.elapsed()));
        paths = collected.into_inner().unwrap();
    }
    println!(
        "Source: {} ({} visible files); {runs} runs",
        root.display(),
        paths.len()
    );
    let (mut first, mut completed) = queries(&mut matcher, runs)?;
    pass &= metric(
        &mut report,
        "Quick Open first snapshot from walk start",
        &mut first_results,
        Some(100.),
    );
    metric(
        &mut report,
        "complete file walk and matching",
        &mut walk_times,
        None,
    );
    pass &= metric(
        &mut report,
        "Quick Open per keystroke (core)",
        &mut first,
        Some(16.),
    );
    pass &= metric(
        &mut report,
        "Quick Open completed ranking per keystroke (core)",
        &mut completed,
        Some(16.),
    );
    if million {
        let mut large = FileMatcher::new(&[]);
        let injector = large.injector();
        for i in 0..1_000_000 {
            injector.push(format!(
                "copy-{}/{path}",
                i / paths.len(),
                path = paths[i % paths.len()]
            ));
        }
        settle(&mut large)?;
        let (mut first, mut completed) = queries(&mut large, runs)?;
        pass &= metric(
            &mut report,
            "Quick Open 1M synthetic paths per keystroke (core)",
            &mut first,
            Some(16.),
        );
        pass &= metric(
            &mut report,
            "Quick Open 1M completed ranking per keystroke (core)",
            &mut completed,
            Some(16.),
        );
    }
    let mut search_times = Vec::new();
    let query = ProjectQuery::new("printk", SearchOptions::default()).map_err(io::Error::other)?;
    // Warm the filesystem/search path once; this budget explicitly uses a warm cache.
    for run in 0..=runs {
        let start = Instant::now();
        let first = Mutex::new(None);
        let cancel = AtomicBool::new(false);
        search_project(root, &options, &query, 100, &cancel, |_| {
            first.lock().unwrap().get_or_insert(ms(start.elapsed()));
            cancel.store(true, Ordering::Relaxed);
        });
        let elapsed = first
            .into_inner()
            .unwrap()
            .ok_or_else(|| io::Error::other("search probe 'printk' found no results"))?;
        if run > 0 {
            search_times.push(elapsed);
        }
    }
    pass &= metric(
        &mut report,
        "project search first result (warm)",
        &mut search_times,
        Some(50.),
    );
    let file = root.join("kernel/sched/core.c");
    let mut open_times = Vec::new();
    let mut syntax_times = Vec::new();
    let mut fold_times = Vec::new();
    for _ in 0..runs {
        let start = Instant::now();
        let source = FileSource::open(&file, OpenOptions::default())?;
        let head = source.head(8192)?;
        let (page, error) = rows::rows(&source, 0, 60);
        if error.is_some() || page.is_empty() {
            return Err(io::Error::other("viewport could not be read"));
        }
        open_times.push(ms(start.elapsed()));
        let text = source
            .in_memory()
            .ok_or_else(|| io::Error::other("source probe too large"))?;
        let language =
            plastext_syntax::detect(&file, &head).ok_or_else(|| io::Error::other("no language"))?;
        let tree = SyntaxTree::parse(language, text, || false)
            .ok_or_else(|| io::Error::other("parse failed"))?;
        std::hint::black_box(tree.highlights(text, 0..page.last().unwrap().end as usize));
        syntax_times.push(ms(start.elapsed()));
        let fold_start = Instant::now();
        std::hint::black_box(tree.folds(text));
        fold_times.push(ms(fold_start.elapsed()));
    }
    metric(
        &mut report,
        "kernel/sched/core.c bytes ready (excludes paint)",
        &mut open_times,
        None,
    );
    metric(
        &mut report,
        "kernel/sched/core.c open + tree + viewport highlights",
        &mut syntax_times,
        None,
    );
    metric(
        &mut report,
        "kernel/sched/core.c syntax folds (background)",
        &mut fold_times,
        None,
    );
    if let Some(output) = output {
        let result = json!({"schema": 1, "source": root, "files": paths.len(), "runs": runs, "million_synthetic_paths": million,
            "scope": "core only; native rendering via read/launch; hardware is not normalized", "passed": pass, "metrics": report});
        fs::write(output, serde_json::to_vec_pretty(&result)?)?;
    }
    if pass {
        Ok(())
    } else {
        Err(io::Error::other("one or more measured M1 budgets failed"))
    }
}

pub fn read(
    file: &Path,
    root: Option<&Path>,
    frames: usize,
    app: Option<PathBuf>,
    foreground: bool,
    keep_visible: bool,
    output: Option<&Path>,
) -> io::Result<()> {
    if !file.is_file() {
        return Err(io::Error::other(
            "native reading profile requires an existing source file",
        ));
    }
    let app = app.unwrap_or(
        std::env::current_exe()?
            .with_file_name(format!("plastext-app{}", std::env::consts::EXE_SUFFIX)),
    );
    let mut command = Command::new(&app);
    if let Some(root) = root {
        command.arg(root);
    }
    command.env_remove("PLASTEXT_PROFILE_FOREGROUND");
    command.env_remove("PLASTEXT_PROFILE_KEEP_VISIBLE");
    if foreground {
        command.env("PLASTEXT_PROFILE_FOREGROUND", "1");
    }
    if keep_visible {
        command.env("PLASTEXT_PROFILE_KEEP_VISIBLE", "1");
    }
    let log = std::env::temp_dir().join(format!("plastext-read-{}.log", std::process::id()));
    println!("Native profile log: {}", log.display());
    let mut child = command
        .arg(file)
        .env("PLASTEXT_PROFILE_READING", frames.max(1).to_string())
        .env_remove("PLASTEXT_PROFILE_STARTUP")
        .env("ZED_MEASUREMENTS", "1")
        .env(
            "XDG_CONFIG_HOME",
            std::env::temp_dir().join(format!("plastext-bench-config-{}", std::process::id())),
        )
        .env(
            "PLASTEXT_SOCKET",
            std::env::temp_dir().join(format!("plastext-read-{}.sock", std::process::id())),
        )
        .stdout(Stdio::null())
        .stderr(fs::File::create(&log)?)
        .spawn()?;
    // A file without parser support must fail, not leave the app running
    // indefinitely. Allow for a slow display plus a minute of setup.
    let deadline = Instant::now() + Duration::from_secs(60 + frames as u64 / 20);
    let mut timed_out = false;
    let mut stalled = false;
    let mut progress = (0, Instant::now());
    let mut last_probe = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if last_probe.elapsed() >= Duration::from_millis(250) {
            let bytes = fs::metadata(&log).map_or(0, |m| m.len());
            if bytes != progress.0 {
                progress = (bytes, Instant::now());
            }
            last_probe = Instant::now();
        }
        if Instant::now() >= deadline || progress.1.elapsed() >= Duration::from_secs(30) {
            timed_out = true;
            stalled = progress.1.elapsed() >= Duration::from_secs(30);
            child.kill().ok();
            break child.wait()?;
        }
        thread::sleep(Duration::from_millis(20));
    };
    let stderr = fs::read_to_string(&log)?;
    let markdown = plastext_markdown::is_markdown(file);
    let parsed = crate::native_log::NativeLog::parse(&stderr, markdown);
    let complete = parsed.complete(status.success(), timed_out, frames);
    let mut report = Vec::new();
    let mut passed = true;
    for &(name, value, budget) in &parsed.paints {
        passed &= metric(&mut report, name, &mut [value], budget);
    }
    let mut times = parsed.frames;
    if !parsed.intervals.is_empty() {
        metric(
            &mut report,
            "native frame intervals (includes display pacing)",
            &mut parsed.intervals.clone(),
            None,
        );
    }
    if !times.is_empty() {
        passed &= metric(
            &mut report,
            "native scrolling frame wall time (draw + submit)",
            &mut times,
            Some(8.),
        );
    }
    passed &= complete;
    if let Some(output) = output {
        fs::write(
            output,
            serde_json::to_vec_pretty(&json!({
                "schema": 1, "file": file, "view": if markdown { "markdown" } else { "code" },
                "requested_frames": frames.max(1), "observed_frames": times.len(), "foreground": foreground,
                "complete": complete, "timed_out": timed_out, "stalled": stalled,
                "keep_visible": keep_visible, "passed": passed, "log": log,
                "scope": "native GPUI draw/submit wall time, not GPU completion; hardware is not normalized",
                "metrics": report,
            }))?,
        )?;
    }
    if !complete {
        return Err(io::Error::other(format!(
            "incomplete native profile ({} frames, status {}, timed out {}):\n{}",
            times.len(),
            status,
            timed_out,
            stderr.lines().rev().take(30).collect::<Vec<_>>().join("\n")
        )));
    }
    if passed {
        Ok(())
    } else {
        Err(io::Error::other("native reading budget failed"))
    }
}
