use super::*;
use plastext_buffer::{SessionError, TextEdit, UndoGroup};

impl TextView {
    pub(super) fn open_replace(
        &mut self,
        _: &crate::actions::Replace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_editing() {
            return;
        }
        if self.find.is_none() {
            self.find(&Find, window, cx);
        } else if let Some(find) = &self.find {
            find.input.focus_handle(cx).focus(window);
        }
        if self
            .find
            .as_ref()
            .is_some_and(|find| find.replacement.is_none())
        {
            let input = cx.new(|cx| TextInput::new("Replace", cx));
            self.find.as_mut().unwrap().replacement = Some(input);
        }
        cx.notify();
    }

    pub(super) fn replace_matches(&mut self, all: bool, cx: &mut Context<Self>) {
        let Some(find) = &self.find else { return };
        if find.searching
            || find.replacing
            || find.input.read(cx).text().is_empty()
            || !self.is_editing()
            || self.has_pending_input()
        {
            return;
        }
        let Some(replacement) = find
            .replacement
            .as_ref()
            .map(|input| input.read(cx).text().to_owned())
        else {
            return;
        };
        let target = if all {
            None
        } else {
            find.current.and_then(|i| find.matches.get(i)).cloned()
        };
        if !all && target.is_none() {
            return;
        }
        let Ok(query) = DocumentQuery::new(find.input.read(cx).text(), find.options) else {
            return;
        };
        let Some(editing) = &self.editing else { return };
        let (document, pane) = (editing.document.clone(), editing.pane);
        let Some(base) = document
            .read(cx)
            .file
            .as_ref()
            .and_then(|file| file.session().pane_snapshot(pane?).ok())
        else {
            return;
        };
        let snapshot = base.document().text().clone();
        let scope = find.scope.clone().unwrap_or(0..snapshot.len());
        let cancel = Arc::new(AtomicBool::new(false));
        self.find.as_mut().unwrap().replace_cancel = cancel.clone();
        self.find.as_mut().unwrap().replacing = true;
        document.update(cx, |doc, cx| {
            doc.pending_inputs += 1;
            cx.notify();
        });
        let current = cancel.clone();
        let work = cx.background_executor().spawn(async move {
            let mut edits = Vec::new();
            let mut bytes = 0;
            let mut limit = false;
            query
                .scan(
                    snapshot.len(),
                    |range, out| snapshot.read_uncached(range, out),
                    scope,
                    Some(&replacement),
                    &cancel,
                    |range, text| {
                        if target.as_ref().is_some_and(|target| *target != range) {
                            return true;
                        }
                        let text = text.unwrap();
                        bytes += text.len();
                        if bytes > MAX_COPY as usize || edits.len() >= 100_000 {
                            limit = true;
                            return false;
                        }
                        edits.push(TextEdit { range, text });
                        all
                    },
                )
                .map_err(|e| e.to_string())?;
            if limit {
                return Err(
                    "Replacement exceeds 100,000 matches or 64 MB; use a smaller selection".into(),
                );
            }
            if cancel.load(Ordering::Relaxed) {
                return Err("Replacement cancelled".into());
            }
            let prepared = snapshot
                .prepare_edits_loading(&edits, &cancel)
                .map_err(|e| format!("Cannot prepare replacement: {e:?}"))?;
            let next = if all {
                None
            } else {
                edits
                    .first()
                    .map(|edit| edit.range.start + edit.text.len().max(1) as u64)
            };
            Ok((prepared, edits.len(), next))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let installed = document.update(cx, |doc, cx| {
                doc.pending_inputs = doc.pending_inputs.saturating_sub(1);
                let result = result.and_then(|(prepared, count, next)| {
                    if current.load(Ordering::Relaxed) {
                        cx.background_executor()
                            .spawn(async move { drop(prepared) })
                            .detach();
                        return Err("Replacement cancelled".into());
                    }
                    let session = doc.file.as_mut().ok_or("Document is closed")?.session_mut();
                    session
                        .apply_prepared(&base, prepared, UndoGroup::New)
                        .map_err(|error| match error {
                            SessionError::Stale => {
                                "Document or selection changed; repeat Replace".to_owned()
                            }
                            other => format!("Couldn't replace: {other:?}"),
                        })?;
                    Ok((count, next))
                });
                doc.changed(cx);
                result
            });
            this.update(cx, |this, cx| {
                if let Some(find) = &mut this.find {
                    find.replacing = false;
                }
                let mut advance = None;
                this.notice = Some(
                    match installed {
                        Ok(Ok((count, next))) => {
                            advance = next;
                            format!("Replaced {count} matches")
                        }
                        Ok(Err(error)) => error,
                        Err(_) => "Document is closed".into(),
                    }
                    .into(),
                );
                this.sync_editing(cx);
                if let Some(next) = advance
                    && let Some(find) = &mut this.find
                {
                    find.next_from = Some(next);
                    this.select_pending_match(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}
