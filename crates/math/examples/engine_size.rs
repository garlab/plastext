//! Lays out one formula with whichever engine feature is enabled, so the
//! release binary's size shows what that engine adds. Without an engine
//! feature it's the baseline.
//!
//! cargo build --release -p plastext-math --example engine_size [--features katex-rs|latex-rust]

fn main() {
    let tex = std::env::args()
        .nth(1)
        .unwrap_or_else(|| r"\int_0^1 x^2\,dx = \frac{1}{3}".into());
    #[cfg(feature = "katex-rs")]
    {
        let ctx = katex::KatexContext::default();
        let settings = katex::Settings::builder().display_mode(true).build();
        let tree = katex::render_to_dom_tree(&ctx, &tex, &settings).expect("katex-rs");
        println!("katex-rs: height {} depth {}", tree.height, tree.depth);
    }
    #[cfg(feature = "latex-rust")]
    {
        let font = latex_rust::MathFont::stix_two_math().expect("STIX Two Math");
        let ast = latex_rust::parse(&tex).expect("parse");
        let laid_out =
            latex_rust::layout(&ast, &font, latex_rust::MathStyle::Display).expect("layout");
        println!("latex-rust: width {:?}", laid_out.width);
    }
    #[cfg(not(any(feature = "katex-rs", feature = "latex-rust")))]
    println!("no engine: {}", tex.len());
}
