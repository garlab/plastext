//! Runs katex-rs and latex-rust over the pinned KaTeX screenshot cases, as
//! typst_cost does for MiTeX + Typst. Prints one JSON object.
//!
//! cargo run --release -p plastext-math --features katex-rs,latex-rust --example engine_compare

use std::time::Instant;

use katex::{KatexContext, Settings, render_to_dom_tree};
use latex_rust::{MathFont, MathStyle};
use serde_json::{Value, json};

fn main() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../spike/katex/cases.json")).expect("KaTeX cases");
    // Same preprocessing as scripts/math-corpus.py.
    let formulas: Vec<(&str, String, bool)> = cases
        .iter()
        .map(|case| {
            let mut tex = case["tex"].as_str().expect("tex").to_owned();
            if let Some(macros) = case["options"]["macros"].as_object() {
                for (name, value) in macros {
                    tex = format!("\\newcommand{{{name}}}{{{}}}{tex}", value.as_str().unwrap());
                }
            }
            (
                case["name"].as_str().expect("name"),
                tex,
                case["display"].as_bool().unwrap_or(false),
            )
        })
        .collect();

    let started = Instant::now();
    let katex = KatexContext::default();
    let katex_setup = ms(started);
    let inline = Settings::builder().display_mode(false).build();
    let display = Settings::builder().display_mode(true).build();
    let katex_report = run(&formulas, |tex, is_display| {
        let settings = if is_display { &display } else { &inline };
        render_to_dom_tree(&katex, tex, settings)
            .map(|tree| tree.height + tree.depth > 0.)
            .map_err(|err| err.to_string())
    });

    let started = Instant::now();
    let font = MathFont::stix_two_math().expect("STIX Two Math");
    let latex_rust_setup = ms(started);
    let latex_rust_report = run(&formulas, |tex, is_display| {
        let style = if is_display {
            MathStyle::Display
        } else {
            MathStyle::Text
        };
        latex_rust::parse(tex)
            .map_err(|err| err.to_string())
            .and_then(|ast| latex_rust::layout(&ast, &font, style).map_err(|err| err.to_string()))
            .map(|laid_out| !laid_out.width.is_zero())
    });

    let mut report = json!({
        "cases": formulas.len(),
        "katex_rs": katex_report,
        "latex_rust": latex_rust_report,
    });
    report["katex_rs"]["setup_ms"] = json!(katex_setup);
    report["latex_rust"]["setup_ms"] = json!(latex_rust_setup);
    println!("{report}");
}

/// Times `render` on every case; a case passes when it returns `Ok(true)`.
fn run(
    formulas: &[(&str, String, bool)],
    render: impl Fn(&str, bool) -> Result<bool, String>,
) -> Value {
    let mut times = Vec::new();
    let mut failed = Vec::new();
    let mut first_ms = None;
    for (name, tex, display) in formulas {
        let started = Instant::now();
        let result = render(tex, *display);
        let elapsed = ms(started);
        first_ms.get_or_insert(elapsed);
        match result {
            Ok(true) => times.push(elapsed),
            Ok(false) => failed.push(json!({ "name": name, "reason": "empty layout" })),
            Err(err) => failed
                .push(json!({ "name": name, "reason": err.chars().take(120).collect::<String>() })),
        }
    }
    let compiled = times.len();
    let total: f64 = times.iter().sum();
    times.sort_by(f64::total_cmp);
    let at = |q: f64| times[((times.len() - 1) as f64 * q).round() as usize];
    json!({
        "compiled": compiled,
        "first_case_ms": first_ms,
        "per_formula_ms": { "p50": at(0.5), "p90": at(0.9), "max": at(1.) },
        "total_ms": total,
        "failed": failed,
    })
}

fn ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.
}
