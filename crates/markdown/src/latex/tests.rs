use super::*;
use plastext_buffer::FileSource;

fn build_text(text: &str) -> (FileSource, DocumentIndex) {
    let source = FileSource::from_bytes(text.as_bytes().to_vec());
    let document = build(&source, &AtomicBool::new(false)).unwrap();
    (source, document)
}

#[test]
fn local_inputs_listings_and_bibliography_keep_root_positions() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join(".git")).unwrap();
    std::fs::create_dir(root.join("paper")).unwrap();
    std::fs::create_dir(root.join("code")).unwrap();
    std::fs::write(
        root.join("code/check.py"),
        "# omitted\nprint(42)\n# omitted too\n",
    )
    .unwrap();
    std::fs::write(
        root.join("paper/table.tex"),
        r"\begin{tabular}{lr}Name&Value\\ Alpha&42\\\end{tabular}",
    )
    .unwrap();
    let text = r"\newcommand{\codepath}{../code}
\IfFileExists{../missing/check.py}{\renewcommand{\codepath}{../missing}}{}
\begin{document}
\section{Before}
\input{table}
\VerbatimInput[firstline=2,lastline=2]{\codepath/check.py}
\lstinputlisting[language=Python,firstline=2,lastline=2]{\codepath/check.py}
\section{After}\label{after}Tail $x$.
\begin{thebibliography}{9}\bibitem{book}A book.\bibitem[Author, 2024]{named}Another book.\end{thebibliography}
\end{document}";
    let source = FileSource::from_bytes(text.as_bytes().to_vec());
    let index = build_with_path(
        &source,
        &root.join("paper/main.tex"),
        &AtomicBool::new(false),
    )
    .unwrap();
    let blocks: Vec<_> = (0..index.pages.len())
        .flat_map(|p| index.parse_page(&source, p, || false).unwrap())
        .collect();
    let body = blocks
        .iter()
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!body.contains(r"\input"), "{body}");
    assert!(!body.contains("Input["), "{body}");
    assert!(!body.contains("omitted"), "{body}");
    assert!(body.contains("Alpha") && body.contains("42"), "{body}");
    assert_eq!(
        blocks
            .iter()
            .filter(|b| matches!(b.kind, BlockKind::Code { .. }) && b.text.contains("print(42)"))
            .count(),
        2
    );
    let markers: Vec<_> = blocks.iter().filter_map(|b| b.marker.as_deref()).collect();
    assert!(markers.contains(&"[1]"), "{markers:?}");
    assert!(markers.contains(&"[Author, 2024]"), "{markers:?}");
    assert!(!markers.iter().any(|m| m.starts_with("[[")));
    let after = index
        .headings
        .iter()
        .find(|h| h.text.contains("After"))
        .unwrap();
    let original = text.find(r"\section{After}").unwrap();
    assert_eq!(index.source_offset(after.source.start), original);
    assert_eq!(index.rendered_offset(original), after.source.start);
    let listing = blocks
        .iter()
        .find(|b| matches!(b.kind, BlockKind::Code { .. }))
        .unwrap();
    assert_eq!(
        index.source_offset(listing.source.start),
        text.find(r"\VerbatimInput").unwrap()
    );
    assert_eq!(index.dependencies().len(), 3);
    assert!(index.dependencies().iter().all(|d| !d.changed()));
    std::fs::write(root.join("code/check.py"), "print('new')\n").unwrap();
    assert!(index.dependencies().iter().any(Dependency::changed));
}

#[test]
fn missing_oversized_and_outside_inputs_remain_visible() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("paper");
    std::fs::create_dir(&base).unwrap();
    std::fs::write(dir.path().join("outside.tex"), "private outside text").unwrap();
    let huge = std::fs::File::create(base.join("huge.tex")).unwrap();
    huge.set_len(MAX_SOURCE_BYTES as u64 + 1).unwrap();
    let text = r"\input{missing}\input{huge}\input{../outside}";
    let source = FileSource::from_bytes(text.as_bytes().to_vec());
    let index = build_with_path(&source, &base.join("main.tex"), &AtomicBool::new(false)).unwrap();
    let blocks = index.parse_page(&source, 0, || false).unwrap();
    let body = blocks
        .iter()
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(body.contains("missing") && body.contains("huge") && body.contains("../outside"));
    assert!(!body.contains("private outside text"));
    assert!(!index.warnings.is_empty());
    assert!(
        index
            .dependencies()
            .iter()
            .all(|d| d.path.starts_with(std::fs::canonicalize(&base).unwrap()))
    );
    std::fs::write(base.join("missing.tex"), "now available").unwrap();
    assert!(index.dependencies().iter().any(Dependency::changed));
}

#[cfg(unix)]
#[test]
fn input_symlinks_cannot_escape_the_project() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("paper");
    std::fs::create_dir(&base).unwrap();
    std::fs::write(dir.path().join("outside.tex"), "outside content").unwrap();
    std::os::unix::fs::symlink("../outside.tex", base.join("link.tex")).unwrap();
    let source = FileSource::from_bytes(br"\input{link}".to_vec());
    let index = build_with_path(&source, &base.join("main.tex"), &AtomicBool::new(false)).unwrap();
    let blocks = index.parse_page(&source, 0, || false).unwrap();
    assert!(!blocks.iter().any(|b| b.text.contains("outside content")));
}

#[test]
fn native_events_preserve_structure_math_references_and_unsupported_source() {
    let text = r"\documentclass{article}
\newcommand{\R}{\mathbb{R}}
\title{A native document}\author{A. Writer}\date{}
\begin{document}\maketitle
\section{Introduction}\label{intro}
Text with \textbf{bold}, \emph{emphasis}, and $x\in\R$.
\begin{equation}\label{eq}x=2\end{equation}
See section \ref{intro} and equation \eqref{eq}.
\begin{itemize}\item First\item Second\end{itemize}
\begin{tabular}{lr}Name&Value\\\hline Alpha&42\\\end{tabular}
\begin{verbatim}
raw $x$ \unknown
\end{verbatim}
\unknown{visible}\input{chapter}
\end{document}";
    let (source, index) = build_text(text);
    let blocks = index.parse_page(&source, 0, || false).unwrap();
    let all = blocks
        .iter()
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(all.contains("A native document"));
    assert!(all.contains("See section 1 and equation (1)"), "{all}");
    assert!(all.contains(r"\unknown"));
    assert!(all.contains("chapter"));
    assert!(blocks.iter().any(|b| {
        b.spans
            .iter()
            .any(|s| s.style.strong && &b.text[s.range.clone()] == "bold")
    }));
    assert!(
        blocks
            .iter()
            .any(|b| b.marker.as_deref() == Some("•") && b.text == "First")
    );
    assert!(
        blocks
            .iter()
            .any(|b| matches!(&b.kind, BlockKind::TableRow { cells, .. } if cells.len() == 2))
    );
    let equation = blocks.iter().find(|b| b.kind == BlockKind::Math).unwrap();
    assert!(equation.text.contains(r"\tag*{(1)}"), "{}", equation.text);
    let context = index.macro_context(text.find("$x").unwrap());
    assert!(context.latex().unwrap().bindings().any(|b| b.name == r"\R"));
}

#[test]
fn pages_restore_nested_styles_and_macros_when_visited_backwards() {
    let text = format!(
        r"\def\value{{1}}\begin{{quote}}\begin{{itemize}}{}\end{{itemize}}\end{{quote}}\def\value{{2}}$\value$",
        "\\item \\textbf{bold} and $\\value$\n\n".repeat(2500)
    );
    let (source, index) = build_text(&text);
    assert!(index.pages.len() > 2);
    let forward: Vec<_> = (0..index.pages.len())
        .map(|p| index.parse_page(&source, p, || false).unwrap())
        .collect();
    for page in (0..index.pages.len()).rev() {
        assert_eq!(
            index.parse_page(&source, page, || false).unwrap(),
            forward[page]
        );
    }
    assert_eq!(
        forward
            .iter()
            .flatten()
            .filter(|b| b.marker.is_some())
            .count(),
        2500
    );
    let first = text.find("$\\value$").unwrap();
    let last = text.rfind("$\\value$").unwrap();
    let body = |offset| {
        index
            .macro_context(offset)
            .latex()
            .unwrap()
            .bindings()
            .find(|b| b.name == r"\value")
            .unwrap()
            .body
            .clone()
    };
    assert_eq!(body(first), "1");
    assert_eq!(body(last), "2");
    let mut hits = Vec::new();
    index
        .search(
            &source,
            "bold",
            3000,
            || false,
            |batch| {
                hits.extend(batch);
                true
            },
        )
        .unwrap();
    assert_eq!(hits.len(), 2500);
}

#[test]
fn cancellation_and_snapshot_limits_leave_source_available() {
    let source = FileSource::from_bytes(b"\\section{Test}".to_vec());
    assert_eq!(
        build(&source, &AtomicBool::new(true)).unwrap_err().kind(),
        io::ErrorKind::Interrupted
    );
    struct Large;
    impl DocumentSource for Large {
        fn byte_len(&self) -> u64 {
            (MAX_SOURCE_BYTES + 1) as u64
        }
        fn read_range(&self, _: Range<u64>, _: &mut Vec<u8>) -> io::Result<()> {
            panic!("must reject before reading")
        }
    }
    assert!(
        build(&Large, &AtomicBool::new(false))
            .unwrap_err()
            .to_string()
            .contains("32 MiB")
    );
    let (_, index) = build_text("Hello");
    assert_eq!(
        index.parse_page(&source, 0, || true).unwrap_err().kind(),
        io::ErrorKind::Interrupted
    );
}

#[test]
fn huge_plain_paragraph_is_split_into_bounded_blocks() {
    let text = "some words ".repeat(9000);
    let (source, index) = build_text(&text);
    let blocks = index.parse_page(&source, 0, || false).unwrap();
    assert!(blocks.len() > 1);
    for pair in blocks.windows(2) {
        assert_eq!(pair[0].source.end, pair[1].source.start);
        assert!(pair[1].source.start > pair[0].source.start);
    }
    assert!(
        blocks
            .iter()
            .all(|b| b.text.len() <= crate::BLOCK_TEXT_LIMIT)
    );
    assert_eq!(
        blocks
            .iter()
            .map(|b| b.text.as_str())
            .collect::<String>()
            .trim(),
        text.trim()
    );
}

#[test]
#[ignore = "manual release-mode timing; prints summaries only"]
fn native_latex_timing() {
    let paragraph =
        "\\section{Reading}\nText with \\textbf{emphasis}, \\emph{italics}, and $x^2+y^2$.\n\n";
    for repetitions in [1000, 15000] {
        let text = paragraph.repeat(repetitions);
        let start = std::time::Instant::now();
        let (source, index) = build_text(&text);
        let indexing = start.elapsed();
        let start = std::time::Instant::now();
        let last = index
            .parse_page(&source, index.pages.len() - 1, || false)
            .unwrap();
        println!(
            "bytes={} pages={} blocks={} index_ms={:.2} last_page_ms={:.2} retained_budget_bytes={}",
            text.len(),
            index.pages.len(),
            index.blocks.len(),
            indexing.as_secs_f64() * 1000.,
            start.elapsed().as_secs_f64() * 1000.,
            index.retained_bytes()
        );
        assert!(!last.is_empty());
    }
}
