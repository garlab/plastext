use super::*;
use gpui::{TestAppContext, VisualTestContext, size};

#[gpui::test]
fn long_titles_wrap_inside_the_column_at_different_widths_and_zoom(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let title = "A long document title about integer representations and their distribution across several families";
    cx.update(crate::init);
    for format in [DocumentFormat::Latex, DocumentFormat::Markdown] {
        let path = dir.path().join(if format == DocumentFormat::Latex {
            "main.tex"
        } else {
            "main.md"
        });
        let text = if format == DocumentFormat::Latex {
            format!(
                "\\title{{{title}}}\\date{{}}\\begin{{document}}\\maketitle\n\\section{{Short}}\nBody.\\end{{document}}"
            )
        } else {
            format!("# {title}\n\n## Short\n\nBody.\n")
        };
        std::fs::write(&path, text).unwrap();
        let (source, index) = plastext_buffer::open(&path, Default::default()).unwrap();
        let window = cx.add_window(|_, cx| {
            MarkdownView::new_format(
                path,
                source,
                index,
                TextStyle {
                    font_family: "Menlo".into(),
                    font_size: px(14.),
                    line_height: 1.5,
                    tab_size: 4,
                    sticky_scroll: false,
                    max_parse_size: 0,
                },
                Arc::default(),
                format,
                cx,
            )
        });
        cx.run_until_parked();
        for width in [900., 480.] {
            VisualTestContext::from_window(window.into(), cx)
                .simulate_resize(size(px(width), px(1000.)));
            for zoom in [0.8, 1., 1.5] {
                window
                    .update(cx, |view, _, cx| view.set_zoom(zoom, px(14. * zoom), cx))
                    .unwrap();
                cx.run_until_parked();
                let mut visual = VisualTestContext::from_window(window.into(), cx);
                let heading = visual.debug_bounds("md-heading-0").unwrap();
                let text = visual.debug_bounds("md-heading-text-0").unwrap();
                let anchor = visual.debug_bounds("md-anchor-0").unwrap();
                assert!(heading.right() <= px(width));
                assert!(
                    text.right() <= heading.right(),
                    "title overflows at width {width}, zoom {zoom}"
                );
                assert!(
                    anchor.right() <= heading.right() + px(1.),
                    "anchor overflows at width {width}, zoom {zoom}"
                );
                assert!(
                    text.size.height > px(30. * zoom * LINE_HEIGHT * 1.5),
                    "title should wrap at width {width}, zoom {zoom}"
                );
                let short_text = visual.debug_bounds("md-heading-text-1").unwrap();
                let short_anchor = visual.debug_bounds("md-anchor-1").unwrap();
                assert!((short_anchor.left() - short_text.right() - px(8. * zoom)).abs() < px(1.));
                assert!(
                    short_anchor.right() < heading.right() - px(32.),
                    "a short heading's anchor should not move to the column edge"
                );
            }
        }
    }
}
