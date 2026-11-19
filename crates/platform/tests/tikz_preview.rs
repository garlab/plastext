//! Exercise the native figure worker and check the placement of missing children.
use document_graphics::Raster;
use plastext_platform::tikz;
use std::sync::atomic::AtomicBool;

fn main() {
    #[cfg(target_os = "macos")]
    if std::env::args().nth(1).as_deref() == Some("--tikz-worker") {
        tikz::worker_main().unwrap();
        return;
    }
    let cancel = AtomicBool::new(false);
    if !cfg!(target_os = "macos") {
        assert_eq!(
            tikz::render("", &cancel).unwrap_err().kind(),
            std::io::ErrorKind::Unsupported
        );
        return;
    }
    for missing_first in [false, true] {
        let children = if missing_first {
            r"child[missing] child {node {\valueof{16}}}"
        } else {
            r"child {node {\valueof{16}}} child[missing]"
        };
        let source = format!(
            r"\newcommand{{\valueof}}[1]{{$\mathbf{{#1}}$}}
            \begin{{tikzpicture}}[level distance=12mm, level 1/.style={{sibling distance=40mm}}]
            \node {{$8$}} {children};
            \end{{tikzpicture}}"
        );
        let image = tikz::render(&source, &cancel).unwrap();
        assert!(image.original_width > 60 && image.original_height > 40);
        let top = ink_center(&image, 0..image.height / 4);
        let bottom = ink_center(&image, 3 * image.height / 4..image.height);
        if missing_first {
            assert!(top < 0.3 && bottom > 0.7, "{top}, {bottom}");
        } else {
            assert!(top > 0.7 && bottom < 0.3, "{top}, {bottom}");
        }
    }
    assert!(tikz::render("no picture", &cancel).is_err());
    assert_eq!(
        tikz::render("cancelled", &AtomicBool::new(true))
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::Interrupted
    );
}

fn ink_center(image: &Raster, rows: std::ops::Range<u32>) -> f64 {
    let mut sum = 0;
    let mut count = 0;
    for y in rows {
        for x in 0..image.width {
            if image.rgba[((y * image.width + x) * 4) as usize] < 128 {
                sum += u64::from(x);
                count += 1;
            }
        }
    }
    assert!(count > 0);
    sum as f64 / (f64::from(count) * f64::from(image.width))
}
