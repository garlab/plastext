//! Runs the actual compiler and the app-equivalent PDF worker without a window.
#[cfg(target_os = "macos")]
fn main() {
    use plastext_platform::{
        pdf,
        tex::{self, Config, Engine, Session},
    };
    use std::{
        fs,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::{Duration, Instant},
    };
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--pdf-worker")) {
        pdf::worker_main().unwrap();
        return;
    }
    let Ok(engine) = Engine::discover() else {
        println!("TeX integration: skipped (TeX Live is not installed)");
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("main.tex");
    let chapter = root.path().join("chapter.tex");
    fs::write(
        &input,
        "\\documentclass{article}\\ifnum\\pdfstrcmp{\\jobname}{main}=0\\else\\errmessage{Wrong job name}\\fi\\begin{document}\\input{chapter}\\end{document}",
    )
    .unwrap();
    fs::write(&chapter, "First revision").unwrap();
    let original = fs::read(&input).unwrap();
    let config = Config::new(&input, engine).unwrap();
    let (session, _events) = Session::start(config.clone()).unwrap();
    let wait = |predicate: &dyn Fn(&tex::State) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let state = session.state();
            if predicate(&state) {
                return state;
            }
            assert!(
                Instant::now() < deadline,
                "Timed out: {} {:?}",
                state.status,
                state.error
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    };
    let first = wait(&|state| state.output.is_some());
    assert_eq!(first.output.as_ref().unwrap().info.pages, 1);
    assert_eq!(fs::read(&input).unwrap(), original);
    assert_eq!(
        fs::read_dir(root.path()).unwrap().count(),
        2,
        "all generated files stay outside the input folder"
    );
    session.set_paused(true);
    session.set_paused(false);
    std::thread::sleep(Duration::from_millis(900));
    assert_eq!(
        session.state().output.as_ref().unwrap().document.id,
        first.output.as_ref().unwrap().document.id,
        "switching modes without an edit must reuse the completed build"
    );

    // Atomic editor saves and included-file edits are observed by the OS watcher.
    fs::write(
        root.path().join("replacement.tex"),
        "\\undefinedcontrolsequence",
    )
    .unwrap();
    fs::rename(root.path().join("replacement.tex"), &chapter).unwrap();
    let failed = wait(&|state| state.error.is_some());
    assert_eq!(
        failed.output.as_ref().unwrap().document.id,
        first.output.as_ref().unwrap().document.id
    );
    for revision in 0..20 {
        fs::write(&chapter, format!("Revision {revision}")).unwrap();
        std::thread::sleep(Duration::from_millis(10));
    }
    fs::write(&chapter, "Latest revision\\newpage Second page").unwrap();
    let latest = wait(&|state| state.output.as_ref().is_some_and(|out| out.info.pages == 2));
    assert!(latest.error.is_none());
    session.set_paused(true);
    fs::write(&chapter, "One\\newpage Two\\newpage Three").unwrap();
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(
        session.state().output.as_ref().unwrap().document.id,
        latest.output.as_ref().unwrap().document.id
    );
    session.set_paused(false);
    wait(&|state| state.output.as_ref().is_some_and(|out| out.info.pages == 3));
    drop(session);

    fs::write(&input, "\\documentclass{article}\\begin{document}A citation \\cite{a}.\\bibliographystyle{plain}\\bibliography{refs}\\end{document}").unwrap();
    fs::write(
        root.path().join("refs.bib"),
        "@book{a,author={Ada Author},title={A book},publisher={Press},year={2026}}",
    )
    .unwrap();
    let bibliography = tex::compile(&config, &Arc::new(AtomicBool::new(false))).unwrap();
    assert!(bibliography.info.pages > 0);
    fs::write(&input, &original).unwrap();

    // A stuck native TeX loop must not pin the compiler slot or leave a process.
    fs::write(&chapter, "\\loop\\iftrue\\repeat").unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let active = std::thread::spawn(move || tex::compile(&config, &flag));
    std::thread::sleep(Duration::from_millis(300));
    let start = Instant::now();
    cancel.store(true, Ordering::Relaxed);
    assert_eq!(active.join().unwrap().err().unwrap(), tex::CANCELLED);
    assert!(start.elapsed() < Duration::from_secs(2));

    let project = tempfile::tempdir().unwrap();
    for name in ["paper", "shared", "private"] {
        fs::create_dir(project.path().join(name)).unwrap();
    }
    let input = project.path().join("paper/main.tex");
    fs::write(
        &input,
        "\\documentclass{article}\\begin{document}\\input{../shared/part}\\end{document}",
    )
    .unwrap();
    fs::write(
        project.path().join("shared/part.tex"),
        "Explicitly authorized",
    )
    .unwrap();
    fs::write(project.path().join("private/secret.tex"), "Not authorized").unwrap();
    let config = Config::new(&input, Engine::discover().unwrap()).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    assert!(tex::compile(&config, &cancel).is_err());
    let config = Config::with_inputs(
        &input,
        Engine::discover().unwrap(),
        &[project.path().join("shared")],
    )
    .unwrap();
    assert_eq!(tex::compile(&config, &cancel).unwrap().info.pages, 1);
    fs::write(
        &input,
        "\\documentclass{article}\\begin{document}\\input{../private/secret}\\end{document}",
    )
    .unwrap();
    assert!(
        tex::compile(&config, &cancel).is_err(),
        "adding a sibling must not grant the whole parent folder"
    );
    println!(
        "TeX integration: compile, job name, BibTeX, atomic saves, included inputs, latest revision, retained preview, pause/resume, cancellation, additional input confinement passed"
    );
}
#[cfg(not(target_os = "macos"))]
fn main() {
    println!("TeX integration: macOS only");
}
