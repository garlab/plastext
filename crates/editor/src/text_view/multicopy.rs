use super::*;
use plastext_buffer::{TextEdit, UndoGroup};

impl TextView {
    pub(super) fn copy_selections(&mut self, cut: bool, cx: &mut Context<Self>) -> bool {
        let selections = self.all_selections();
        if selections.len() <= 1 {
            return false;
        }
        if self.has_pending_input() {
            self.notice = Some("Wait for pending input before copying.".into());
            return true;
        }
        let mut ranges: Vec<_> = selections
            .iter()
            .map(Selection::range)
            .filter(|r| !r.is_empty())
            .collect();
        ranges.sort_by_key(|r| r.start);
        if ranges.is_empty() {
            return true;
        }
        if ranges.iter().map(|r| r.end - r.start).sum::<u64>() + ranges.len() as u64 > MAX_COPY {
            self.notice = Some("Selection exceeds the 64 MB clipboard limit.".into());
            return true;
        }
        let editing = self.editing.as_ref().unwrap();
        let document = editing.document.clone();
        let Some(base) = document
            .read(cx)
            .file
            .as_ref()
            .and_then(|f| f.session().pane_snapshot(editing.pane?).ok())
        else {
            return true;
        };
        let snapshot = base.document().text().clone();
        self.copy_task = None;
        let cancel = Arc::new(AtomicBool::new(false));
        let current = cancel.clone();
        let work = cx.background_executor().spawn(async move {
            let mut bytes = Vec::new();
            for (i, r) in ranges.iter().enumerate() {
                if current.load(Ordering::Relaxed) {
                    return Err("Copy cancelled".into());
                }
                if i > 0 {
                    bytes.push(b'\n');
                }
                snapshot
                    .read_uncached(r.clone(), &mut bytes)
                    .map_err(|e| e.to_string())?;
            }
            let prepared = if cut {
                let edits: Vec<_> = ranges
                    .into_iter()
                    .map(|range| TextEdit {
                        range,
                        text: String::new(),
                    })
                    .collect();
                Some(
                    snapshot
                        .prepare_edits_loading(&edits, &current)
                        .map_err(|e| format!("{e:?}"))?,
                )
            } else {
                None
            };
            Ok::<_, String>((String::from_utf8_lossy(&bytes).into_owned(), prepared))
        });
        cx.spawn(async move |this,cx| {
            let result=work.await;
            this.update(cx, |this,cx| {
                match result {
                    Ok((text,prepared)) => {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                        if let Some(prepared) = prepared {
                            document.update(cx, |doc,cx| {
                                if let Some(file) = &mut doc.file && let Err(error) = file.session_mut().apply_prepared(&base,prepared,UndoGroup::New) {
                                    this.notice=Some(format!("Copied; couldn't cut because the selection changed: {error:?}").into());
                                }
                                doc.changed(cx);
                            });
                            this.sync_editing(cx);
                        }
                    }
                    Err(error) => this.notice=Some(format!("Couldn't copy: {error}").into()),
                }
                this.copy_task = None;
                cx.notify();
            }).ok();
        }).detach();
        true
    }
}
