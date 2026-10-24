//! Development-only native math comparison. Not linked into the application.
use std::{fs, path::PathBuf, time::Instant};
use typst_layout::PagedDocument;

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--case") {
        let args: Vec<_> = std::env::args().collect();
        let tex = fs::read_to_string(&args[2]).expect("TeX fixture");
        let display = args.get(4).is_none_or(|v| v != "inline");
        match render(&tex, display) {
            Ok(svg) => fs::write(&args[3], svg).expect("SVG output"),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        return;
    }
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("plastext-math-spike"));
    fs::create_dir_all(&output).unwrap();
    let samples = [
        ("fraction", r"\frac{a+b}{c}"),
        ("root", r"\sqrt[3]{x^2+y^2}"),
        ("sum", r"\sum_{i=1}^n i^2"),
        (
            "integral",
            r"\int_{-\infty}^{\infty}e^{-x^2}\,dx=\sqrt{\pi}",
        ),
        ("matrix", r"\begin{pmatrix}a&b\\c&d\end{pmatrix}"),
        ("cases", r"f(x)=\begin{cases}x^2&x\geq0\\-x&x<0\end{cases}"),
        ("aligned", r"\begin{aligned}a&=b+c\\d&=e+f\end{aligned}"),
        ("gather", r"\begin{gather}a=b+c\\d=e+f\end{gather}"),
        ("array", r"\begin{array}{cc}a&b\\c&d\end{array}"),
        (
            "fonts",
            r"\mathbb{R}\quad\mathcal{F}\quad\mathfrak{g}\quad\mathrm{d}x",
        ),
        ("text", r"x\in\mathbb{R}\text{ for all }x"),
        ("operator", r"\operatorname{argmax}_{x} f(x)"),
        (
            "accents",
            r"\hat{x}+\bar{y}+\vec{v}+\overline{AB}+\underbrace{a+b}_{n}",
        ),
        (
            "ams",
            r"\therefore A\subseteq B\iff A\cap B=A\quad\nexists x\in\varnothing",
        ),
        ("tag", r"E=mc^2\tag{1}"),
        ("macro", r"\newcommand{\sq}[1]{#1^2}\sq{x}+\sq{y}"),
        ("def", r"\def\sq#1{#1^2}\sq{x}"),
        ("color", r"\color{red}{x}+\color{blue}{y}"),
        ("binomial", r"\binom{n}{k}=\frac{n!}{k!(n-k)!}"),
        ("limits", r"\lim_{x\to0}\frac{\sin x}{x}=1"),
    ];
    let start = Instant::now();
    let mut passed = 0;
    for (name, tex) in samples {
        let begin = Instant::now();
        let result = render(tex, true);
        match result {
            Ok(svg) => {
                passed += 1;
                fs::write(output.join(format!("{name}.svg")), svg).unwrap();
                println!(
                    "PASS {name}: {:.2} ms",
                    begin.elapsed().as_secs_f64() * 1000.
                );
            }
            Err(error) => println!("FAIL {name}: {error}"),
        }
    }
    println!(
        "{passed}/{} representative expressions compiled in {:.2} ms. This is not the KaTeX corpus or a visual-equivalence score. Output: {}",
        samples.len(),
        start.elapsed().as_secs_f64() * 1000.,
        output.display()
    );
}

fn render(tex: &str, display: bool) -> Result<String, String> {
    let converted = mitex::convert_math(tex, None).map_err(|e| e.to_string())?;
    let source = format!(
        "#import \"standard.typ\": package\n#set page(width: auto, height: auto, margin: 8pt)\n#set text(size: 18pt)\n#math.equation(block: {display}, eval({:?}, scope: package.scope))",
        if display {
            format!("$ {converted} $")
        } else {
            format!("${converted}$")
        }
    );
    let engine = typst_as_lib::TypstEngine::builder()
        .main_file(source)
        .with_static_source_file_resolver([
            ("standard.typ", include_str!("../spike/mitex/standard.typ")),
            ("prelude.typ", include_str!("../spike/mitex/prelude.typ")),
        ])
        .fonts(typst_assets::fonts())
        .build();
    let document: PagedDocument = engine.compile().output.map_err(|e| e.to_string())?;
    let page = document.pages().first().ok_or("no page")?;
    Ok(typst_svg::svg(page, &Default::default()))
}
