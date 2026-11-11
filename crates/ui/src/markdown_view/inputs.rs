//! Include invalidation uses the workspace's existing filesystem watches.
use super::*;
use plastext_platform::watch::Batch;

impl MarkdownView {
    pub(crate) fn input_paths(&self) -> impl Iterator<Item = &std::path::Path> {
        self.input_versions.iter().flat_map(|d| d.paths())
    }

    pub(crate) fn check_inputs(&mut self, batch: &Batch, cx: &mut Context<Self>) {
        if self.checking_inputs || self.inputs_dirty || self.input_versions.is_empty() {
            return;
        }
        let inputs: Vec<_> = self
            .input_versions
            .iter()
            .enumerate()
            .filter(|(_, d)| d.paths().any(|path| batch.affects(path)))
            .map(|(i, d)| (i, d.clone()))
            .collect();
        if inputs.is_empty() {
            return;
        }
        self.checking_inputs = true;
        let document = self.document.clone();
        let work = cx.background_executor().spawn(async move {
            inputs
                .into_iter()
                .filter_map(|(i, input)| input.changed_version().map(|new| (i, new)))
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let changed = work.await;
            this.update(cx, |this, cx| {
                this.checking_inputs = false;
                if !changed.is_empty() && Arc::ptr_eq(&document, &this.document) {
                    for (i, version) in changed {
                        this.input_versions[i] = version;
                    }
                    this.inputs_dirty = true;
                    this.cancel.store(true, Ordering::Relaxed);
                    this.parsing = false;
                    this.refresh_preview(false, cx);
                }
            })
            .ok();
            cx.background_executor()
                .spawn(async move {
                    drop(document);
                })
                .detach();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use std::time::Duration;

    #[gpui::test]
    fn saved_includes_refresh_visible_views_and_defer_hidden_views(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.tex");
        let child = dir.path().join("child.tex");
        let text = "\\input{child}\n\\section{Tail}\nRoot text.\n";
        std::fs::write(&path, text).unwrap();
        std::fs::write(&child, "\\section{Original}\nIncluded text.\n").unwrap();
        let (source, index) = plastext_buffer::open(&path, Default::default()).unwrap();
        cx.update(crate::init);
        let window = cx.add_window(|_, cx| {
            let doc =
                plastext_editor::EditingDocument::shared(path.clone(), Default::default(), cx);
            let mut view = MarkdownView::new_latex(
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
                cx,
            );
            view.set_editing_document(doc, cx);
            view
        });
        cx.run_until_parked();
        let before = window
            .update(cx, |view, _, _| {
                assert!(!view.parsing);
                assert!(view.symbols.iter().any(|h| h.name.contains("Original")));
                assert_eq!(view.input_paths().count(), 1);
                assert_eq!(
                    view.symbols.last().unwrap().range.start,
                    text.find("\\section{Tail}").unwrap()
                );
                view.document.clone()
            })
            .unwrap();
        std::fs::write(&child, "\\section{Updated}\nNew included text.\n").unwrap();
        window
            .update(cx, |view, _, cx| {
                view.check_inputs(
                    &Batch {
                        rescan: true,
                        ..Batch::default()
                    },
                    cx,
                )
            })
            .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(300));
        cx.run_until_parked();
        let updated = window
            .update(cx, |view, _, cx| {
                assert!(!view.parsing);
                assert!(!Arc::ptr_eq(&before, &view.document));
                assert!(view.symbols.iter().any(|h| h.name.contains("Updated")));
                // Repeated events for unchanged files must not reparse.
                view.check_inputs(
                    &Batch {
                        rescan: true,
                        ..Batch::default()
                    },
                    cx,
                );
                view.document.clone()
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| {
                assert!(Arc::ptr_eq(&updated, &view.document));
                view.set_visible(false, cx);
            })
            .unwrap();
        std::fs::write(&child, "\\section{Deferred}\nLast revision.\n").unwrap();
        window
            .update(cx, |view, _, cx| {
                view.check_inputs(
                    &Batch {
                        rescan: true,
                        ..Batch::default()
                    },
                    cx,
                )
            })
            .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(300));
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| {
                assert!(Arc::ptr_eq(&updated, &view.document));
                assert!(view.inputs_dirty);
                view.set_visible(true, cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, _, _| {
                assert!(view.symbols.iter().any(|h| h.name.contains("Deferred")));
                assert!(!view.inputs_dirty);
            })
            .unwrap();

        // A revision over the page budget is tried once. A later saved fix
        // must still refresh, even though the failed revision was not published.
        std::fs::write(&child, "x\n\n".repeat(11_000)).unwrap();
        window
            .update(cx, |view, _, cx| {
                view.check_inputs(
                    &Batch {
                        rescan: true,
                        ..Batch::default()
                    },
                    cx,
                )
            })
            .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(300));
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| {
                assert!(
                    view.notice
                        .as_ref()
                        .is_some_and(|n| n.contains("Couldn't refresh"))
                );
                assert!(!view.inputs_dirty);
                view.check_inputs(
                    &Batch {
                        rescan: true,
                        ..Batch::default()
                    },
                    cx,
                );
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, _, _| assert!(!view.parsing))
            .unwrap();
        std::fs::write(&child, "\\section{Recovered}\nValid text.\n").unwrap();
        window
            .update(cx, |view, _, cx| {
                view.check_inputs(
                    &Batch {
                        rescan: true,
                        ..Batch::default()
                    },
                    cx,
                )
            })
            .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(300));
        cx.run_until_parked();
        window
            .update(cx, |view, _, _| {
                assert!(view.symbols.iter().any(|h| h.name.contains("Recovered")));
            })
            .unwrap();
    }
}
