//! What MiTeX + Typst costs per formula with one reused engine, over the
//! pinned KaTeX screenshot cases. Prints one JSON object; run it in a fresh
//! process for each sample, since the first formula pays for a cold engine.
//!
//! cargo run --release -p plastext-math --features typst-engine --example typst_cost -- [bundled|bundled-math|system] [threads]

use std::{
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};

use plastext_math::{Fonts, TypstMath};
use serde_json::{Value, json};

fn main() {
    let mut args = std::env::args().skip(1);
    let fonts = match args.next().as_deref().unwrap_or("bundled") {
        "bundled" => Fonts::Bundled,
        "bundled-math" => Fonts::BundledMath,
        "system" => Fonts::System,
        other => panic!("unknown font set {other}"),
    };
    let threads: usize = args.next().map_or(4, |n| n.parse().expect("thread count"));
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../spike/katex/cases.json")).expect("KaTeX cases");
    // Same preprocessing as scripts/math-corpus.py.
    let formulas: Vec<(String, bool)> = cases
        .iter()
        .map(|case| {
            let mut tex = case["tex"].as_str().expect("tex").to_owned();
            if let Some(macros) = case["options"]["macros"].as_object() {
                for (name, value) in macros {
                    tex = format!("\\newcommand{{{name}}}{{{}}}{tex}", value.as_str().unwrap());
                }
            }
            (tex, case["display"].as_bool().unwrap_or(false))
        })
        .collect();
    let mut report = json!({ "fonts": format!("{fonts:?}"), "cases": formulas.len() });
    let rss_start = rss_mb();

    let started = Instant::now();
    let math = TypstMath::new(fonts).expect("engine");
    report["engine_ms"] = json!(ms(started));
    report["rss_engine_mb"] = json!(rss_mb() - rss_start);

    let started = Instant::now();
    math.layout(r"\frac{a+b}{c}", true).expect("first formula");
    report["first_formula_ms"] = json!(ms(started));

    let conversions: Vec<f64> = formulas
        .iter()
        .map(|(tex, _)| {
            let started = Instant::now();
            TypstMath::convert(tex).ok();
            ms(started)
        })
        .collect();
    report["convert_ms"] = stats(conversions);

    // Every case once, as when opening a document full of new formulas.
    let mut ok = Vec::new();
    let mut failed = Vec::new();
    let mut times = Vec::new();
    let mut glyphs = 0;
    for ((tex, display), case) in formulas.iter().zip(&cases) {
        let started = Instant::now();
        let result = math.layout(tex, *display);
        let elapsed = ms(started);
        if let Ok(layout) = result {
            assert!(layout.glyphs + layout.shapes > 0 && layout.width > 0.);
            glyphs += layout.glyphs;
            ok.push((tex.as_str(), *display));
            times.push(elapsed);
        } else {
            failed.push(case["name"].clone());
        }
    }
    report["compiled"] = json!(ok.len());
    report["failed"] = json!(failed);
    report["glyphs"] = json!(glyphs);
    report["distinct_ms"] = stats(times.clone());
    report["distinct_total_ms"] = json!(times.iter().sum::<f64>());
    report["rss_after_distinct_mb"] = json!(rss_mb() - rss_start);

    // The same formulas again, with Typst's memoized results available.
    let repeat: Vec<f64> = ok
        .iter()
        .map(|&(tex, display)| {
            let started = Instant::now();
            math.layout(tex, display).expect("compiled before");
            ms(started)
        })
        .collect();
    report["repeat_ms"] = stats(repeat);

    TypstMath::evict(0);
    report["rss_after_evict_mb"] = json!(rss_mb() - rss_start);

    // All compiled formulas as one document, starting from an empty cache.
    let started = Instant::now();
    let batch = math.layout_all(&ok);
    report["batch_total_ms"] = json!(ms(started));
    report["batch_ok"] = json!(batch.is_ok());
    TypstMath::evict(0);

    // The same formulas one by one again from an empty cache, then spread
    // across threads, to compare wall times.
    let started = Instant::now();
    for &(tex, display) in &ok {
        math.layout(tex, display).expect("compiled before");
    }
    report["sequential_total_ms"] = json!(ms(started));
    TypstMath::evict(0);
    let next = AtomicUsize::new(0);
    let started = Instant::now();
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                while let Some(&(tex, display)) = ok.get(next.fetch_add(1, Ordering::Relaxed)) {
                    math.layout(tex, display).expect("compiled before");
                }
            });
        }
    });
    report["threads"] = json!(threads);
    report["parallel_total_ms"] = json!(ms(started));
    println!("{report}");
}

fn ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.
}

fn stats(mut values: Vec<f64>) -> Value {
    values.sort_by(f64::total_cmp);
    let at = |q: f64| values[((values.len() - 1) as f64 * q).round() as usize];
    json!({ "n": values.len(), "p50": at(0.5), "p90": at(0.9), "max": at(1.) })
}

/// Resident memory of this process, in MB.
fn rss_mb() -> f64 {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<f64>()
        .expect("rss")
        / 1024.
}
