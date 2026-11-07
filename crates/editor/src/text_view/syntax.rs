//! Keep tree-backed colors across edit-mode entry and refresh immutable versions.
use super::*;

const EDIT_PARSE_DELAY: Duration = Duration::from_millis(40);
const COPY_CHUNK: u64 = 256 * 1024;

impl TextView {
    pub(super) fn map_syntax_edits(&mut self, edits: Option<&[plastext_buffer::EditDelta]>) {
        let Some(text) = &self.syntax_text else {
            return;
        };
        let map = self
            .syntax_map
            .get_or_insert_with(|| vec![(0..text.len() as u64, 0)]);
        let Some(edits) = edits else {
            map.clear();
            return;
        };
        for edit in edits {
            let mut next = Vec::new();
            for (range, original) in map.drain(..) {
                let before = range.end.min(edit.range.start);
                if range.start < before {
                    next.push((range.start..before, original));
                }
                let after = range.start.max(edit.range.end);
                if after < range.end {
                    let shifted = edit.range.start + edit.inserted + (after - edit.range.end);
                    next.push((
                        shifted..shifted + range.end - after,
                        original + after - range.start,
                    ));
                }
            }
            *map = next;
            if map.len() > 4096 {
                map.clear();
                break;
            }
        }
    }

    /// Keep colors on surviving bytes, including the unchanged suffix of an
    /// edited line. Only newly inserted text uses lexical colors until parsing.
    pub(super) fn mapped_highlights(
        &self,
        visible: Range<u64>,
        line: &[u8],
    ) -> Option<Vec<(Range<u64>, Highlight)>> {
        let map = self.syntax_map.as_ref()?;
        let tree = self.syntax.as_ref()?;
        let text = self.syntax_text.as_ref()?;
        let mut spans = Vec::new();
        let mut gaps = vec![visible.clone()];
        for (range, original) in map
            .iter()
            .skip_while(|(range, _)| range.end <= visible.start)
            .take_while(|(range, _)| range.start < visible.end)
        {
            let a = range.start.max(visible.start);
            let b = range.end.min(visible.end);
            let old_a = original + a - range.start;
            let old_b = old_a + b - a;
            if old_b > tree.parsed_len() as u64 {
                continue;
            }
            spans.extend(
                tree.highlights(text, old_a as usize..old_b as usize)
                    .into_iter()
                    .filter_map(|(r, kind)| {
                        let start = (r.start as u64).max(old_a);
                        let end = (r.end as u64).min(old_b);
                        (start < end).then_some((a + start - old_a..a + end - old_a, kind))
                    }),
            );
            gaps = gaps
                .into_iter()
                .flat_map(|gap| {
                    let mut left = Vec::new();
                    if gap.start < a {
                        left.push(gap.start..gap.end.min(a));
                    }
                    if gap.end > b {
                        left.push(gap.start.max(b)..gap.end);
                    }
                    left
                })
                .collect();
        }
        for (r, kind) in self.language?.lexical.highlights(line) {
            let r = visible.start + r.start as u64..visible.start + r.end as u64;
            for gap in &gaps {
                let start = r.start.max(gap.start);
                let end = r.end.min(gap.end);
                if start < end {
                    spans.push((start..end, kind));
                }
            }
        }
        Some(spans)
    }

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
                    this.syntax_map = None;
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
