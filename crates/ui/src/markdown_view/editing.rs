//! Share the source editor and refresh previews from immutable unsaved versions.
use super::*;
use plastext_buffer::DocumentSnapshot;
use plastext_editor::{EditingDocument, TextViewStatus};
use plastext_markdown::DocumentSource;
use std::time::Duration;

impl MarkdownView {
    pub fn editing_document(&self) -> Option<Entity<EditingDocument>> {
        self.editing_document.clone()
    }
    pub fn set_editing_document(
        &mut self,
        document: Entity<EditingDocument>,
        cx: &mut Context<Self>,
    ) {
        if self.editing_document.is_some() {
            return;
        }
        document.update(cx, |doc, _| doc.retain_view());
        self._document_observer = Some(cx.observe(&document, |this, _, cx| {
            this.refresh_preview(false, cx);
            cx.notify();
        }));
        if let Some(source) = &self.source_view {
            source.update(cx, |view, cx| {
                view.set_embedded_editing_document(document.clone(), cx)
            });
        }
        self.editing_document = Some(document);
        self.refresh_preview(false, cx);
    }
    pub fn source_status(&self, cx: &App) -> Option<TextViewStatus> {
        (!self.rendered)
            .then(|| self.source_view.as_ref().map(|view| view.read(cx).status()))
            .flatten()
    }
    pub(super) fn save(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(document) = &self.editing_document {
            if document.read(cx).has_pending_input() {
                self.notice = Some("Wait for pending input to finish before saving.".into());
                cx.notify();
            } else {
                document.update(cx, |document, cx| document.save(cx));
            }
        }
    }
    pub(super) fn scroll_edited_line(&mut self, line: u64, cx: &mut Context<Self>) -> bool {
        let Some(version) = self
            .editing_document
            .as_ref()
            .and_then(|doc| doc.read(cx).version())
        else {
            return false;
        };
        if !self.rendered {
            if let Some(view) = &self.source_view {
                view.update(cx, |view, cx| view.scroll_to_line(line, cx));
            }
            return true;
        }
        let cancel = self.cancel.clone();
        let work = cx.background_executor().spawn({
            let text = version.text().clone();
            async move {
                while !cancel.load(Ordering::Relaxed) {
                    match text.line_start(line.saturating_sub(1)) {
                        Ok(offset) => return offset,
                        Err(plastext_buffer::ReadError::Missing(range)) => text.load(range).ok()?,
                        Err(_) => return None,
                    }
                }
                None
            }
        });
        cx.spawn(async move |this, cx| {
            let offset = work.await;
            this.update(cx, |this, cx| {
                if this.current_version(&version, cx)
                    && let Some(offset) = offset
                {
                    this.scroll_to_offset(offset, cx);
                }
            })
            .ok();
            cx.background_executor()
                .spawn(async move {
                    drop(version);
                })
                .detach();
        })
        .detach();
        true
    }
    /// Only visible previews consume parsing work. Source-only typing merely
    /// invalidates the previous generation; opening Preview starts immediately.
    pub(super) fn refresh_preview(&mut self, immediate: bool, cx: &mut Context<Self>) {
        let Some(document) = self.editing_document.clone() else {
            return;
        };
        let Some(version) = document.read(cx).version() else {
            return;
        };
        let generation = version.generation();
        if self.preview_generation == Some(generation) {
            return;
        }
        if !self.rendered || self.diagrams.hidden {
            self.cancel.store(true, Ordering::Relaxed);
            self._parse = Task::ready(());
            if let Some(task) = self.refresh_task.take() {
                task.detach();
            }
            self.refresh_generation = None;
            self.parsing = false;
            return;
        }
        if self.refresh_generation == Some(generation) && !immediate {
            return;
        }
        self.cancel.store(true, Ordering::Relaxed);
        self._parse = Task::ready(());
        if let Some(task) = self.refresh_task.take() {
            task.detach();
        }
        self.cancel = Arc::new(AtomicBool::new(false));
        self.refresh_generation = Some(generation);
        self.parsing = true;
        self.notice = None;
        self.preview_complete = false;
        let cancel = self.cancel.clone();
        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            if !immediate {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
            }
            if cancel.load(Ordering::Relaxed) {
                cx.background_executor()
                    .spawn(async move {
                        drop(version);
                    })
                    .detach();
                return;
            }
            let source: Arc<dyn DocumentSource> = Arc::new(version.text().clone());
            let work = cx.background_executor().spawn({
                let source = source.clone();
                let cancel = cancel.clone();
                async move { build_preview(&source, &cancel) }
            });
            let result = work.await;
            // Keep rejected indexes and snapshots off the UI thread on retirement.
            let mut ready = Some((source, result));
            this.update(cx, |this, cx| {
                if cancel.load(Ordering::Relaxed)
                    || !Arc::ptr_eq(&cancel, &this.cancel)
                    || !this.current_version(&version, cx)
                {
                    return;
                }
                let (source, result) = ready.take().unwrap();
                this.refresh_task = None;
                match result {
                    Ok(update) => {
                        let old_source = std::mem::replace(&mut this.preview_source, source);
                        cx.background_executor()
                            .spawn(async move {
                                drop(old_source);
                            })
                            .detach();
                        this.diagrams.clear(cx.entity_id(), cx);
                        this.diagrams.reset_source_choices();
                        this.clear_images(cx);
                        this.expanded.clear();
                        this.preview_generation = Some(generation);
                        this.receive(update, cx);
                    }
                    Err(error) => {
                        this.parsing = false;
                        this.notice = Some(
                            format!(
                                "Couldn't refresh Markdown: {error}. Showing the previous preview."
                            )
                            .into(),
                        );
                        cx.notify();
                    }
                }
            })
            .ok();
            cx.background_executor()
                .spawn(async move {
                    drop((ready, version));
                })
                .detach();
        }));
        cx.notify();
    }
    fn current_version(&self, version: &DocumentSnapshot, cx: &App) -> bool {
        self.editing_document
            .as_ref()
            .is_some_and(|doc| doc.read(cx).is_current(version))
    }
}

fn build_preview(source: &Arc<dyn DocumentSource>, cancel: &AtomicBool) -> std::io::Result<Update> {
    let document = Arc::new(DocumentIndex::build(
        source,
        || cancel.load(Ordering::Relaxed),
        |tex, macros| {
            macros.sources().map_or(0, |sources| {
                plastext_math::equation_numbers_with_macros(tex, sources)
            })
        },
    )?);
    let symbols = Arc::new(
        document
            .headings
            .iter()
            .map(|heading| Symbol {
                name: heading.text.clone(),
                kind: "heading".into(),
                range: heading.source.clone(),
                depth: heading.level.saturating_sub(1) as usize,
            })
            .collect(),
    );
    let first = if document.pages.is_empty() {
        Vec::new()
    } else {
        document.parse_page(source, 0, || cancel.load(Ordering::Relaxed))?
    };
    Ok(Update::Finished(document, symbols, first))
}
