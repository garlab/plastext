//! Keep tree-backed colors across edit-mode entry and refresh immutable versions.
use super::*;

const EDIT_PARSE_DELAY: Duration = Duration::from_millis(40);
const COPY_CHUNK: u64 = 256 * 1024;

impl TextView {
    pub(super) fn refresh_edit_syntax(&mut self, immediate: bool, cx: &mut Context<Self>) {
        self.cancel_parse.store(true, Ordering::Relaxed);
        if let Some(task) = self._edit_parse.take() {
            task.detach();
        }
        let Some(snapshot) = self.source.snapshot.clone() else {
            return;
        };
        let Some(language) = self
            .language
            .filter(|_| snapshot.len() <= self.style.max_parse_size)
        else {
            let tree = self.syntax.take();
            let text = self.syntax_text.take();
            cx.background_executor()
                .spawn(async move {
                    drop((tree, text, snapshot));
                })
                .detach();
            return;
        };
        let old_tree = self.syntax.clone();
        let old_text = self.syntax_text.clone();
        let revision = snapshot.revision();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel_parse = cancel.clone();
        self._edit_parse = Some(cx.spawn(async move |this, cx| {
            if !immediate {
                cx.background_executor().timer(EDIT_PARSE_DELAY).await;
            }
            let work = cx.background_executor().spawn({
                let cancel = cancel.clone();
                async move {
                    if cancel.load(Ordering::Relaxed) {
                        return None;
                    }
                    let mut text = Vec::with_capacity(snapshot.len() as usize);
                    let mut at = 0;
                    while at < snapshot.len() {
                        if cancel.load(Ordering::Relaxed) {
                            return None;
                        }
                        let end = (at + COPY_CHUNK).min(snapshot.len());
                        snapshot.read_uncached(at..end, &mut text).ok()?;
                        at = end;
                    }
                    let (tree, text) = match old_tree.zip(old_text) {
                        Some((tree, before))
                            if before.as_ref() == text && tree.parsed_len() == text.len() =>
                        {
                            (tree, before)
                        }
                        Some((tree, before)) => (
                            Arc::new(
                                tree.reparse(&before, &text, || cancel.load(Ordering::Relaxed))?,
                            ),
                            Bytes::from(text),
                        ),
                        None => (
                            Arc::new(SyntaxTree::parse(language, &text, || {
                                cancel.load(Ordering::Relaxed)
                            })?),
                            Bytes::from(text),
                        ),
                    };
                    (!cancel.load(Ordering::Relaxed)).then_some((tree, text))
                }
            });
            let mut ready = work.await;
            this.update(cx, |this, cx| {
                if cancel.load(Ordering::Relaxed)
                    || !Arc::ptr_eq(&this.cancel_parse, &cancel)
                    || !this
                        .source
                        .snapshot
                        .as_ref()
                        .is_some_and(|text| text.revision() == revision)
                {
                    return;
                }
                if let Some((tree, text)) = ready.take() {
                    let old = (this.syntax.replace(tree), this.syntax_text.replace(text));
                    cx.background_executor()
                        .spawn(async move {
                            drop(old);
                        })
                        .detach();
                    cx.notify();
                }
                this._edit_parse = None;
            })
            .ok();
            cx.background_executor()
                .spawn(async move {
                    drop(ready);
                })
                .detach();
        }));
    }
}
