use gpui::{
    AnyElement, App, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable,
    FontStyle, FontWeight, InteractiveText, KeyBinding, ListAlignment, ListOffset, ListState,
    SharedString, StyledText, Subscription, Task, TextRun, Window, actions, div, font, list,
    prelude::*, px, relative,
};
use plastext_buffer::{ByteSource as _, FileSource, LineIndex};
use plastext_editor::{TextStyle, TextView, Theme};
use plastext_markdown::{
    Block, BlockKind, Document, LinkTarget, ParseSummary, parse, parse_stream, resolve_link,
};
use plastext_syntax::{Highlight, Symbol};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    ops::ControlFlow,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::tab_view::format_size;
mod find;
type CodeHighlights = Arc<Vec<(std::ops::Range<usize>, Highlight)>>;

actions!(
    markdown,
    [
        ToggleSource,
        ScrollDown,
        ScrollUp,
        PageDown,
        PageUp,
        ToStart,
        ToEnd
    ]
);

pub fn init(cx: &mut App) {
    find::init(cx);
    cx.bind_keys([
        KeyBinding::new("cmd-shift-v", ToggleSource, Some("Markdown")),
        KeyBinding::new("j", ScrollDown, Some("MarkdownPreview")),
        KeyBinding::new("k", ScrollUp, Some("MarkdownPreview")),
        KeyBinding::new("space", PageDown, Some("MarkdownPreview")),
        KeyBinding::new("shift-space", PageUp, Some("MarkdownPreview")),
        KeyBinding::new("g", ToStart, Some("MarkdownPreview")),
        KeyBinding::new("shift-g", ToEnd, Some("MarkdownPreview")),
    ]);
}

pub enum MarkdownEvent {
    Open {
        path: PathBuf,
        fragment: Option<String>,
    },
}
impl EventEmitter<MarkdownEvent> for MarkdownView {}

enum Update {
    Initial(Document),
    Batch {
        first: bool,
        blocks: Vec<Arc<Block>>,
    },
    Finished(ParseSummary, bool),
    Error(String),
}

pub struct MarkdownView {
    path: PathBuf,
    source: Arc<FileSource>,
    index: Arc<LineIndex>,
    text_style: TextStyle,
    document: Document,
    symbols: Arc<Vec<Symbol>>,
    list: ListState,
    focus: FocusHandle,
    rendered: bool,
    source_view: Option<Entity<TextView>>,
    _source_observer: Option<Subscription>,
    find: Option<find::FindState>,
    parsing: bool,
    notice: Option<SharedString>,
    pending_offset: Option<usize>,
    pending_anchor: Option<String>,
    expanded: HashSet<usize>,
    images: HashMap<String, Option<Result<Arc<gpui::RenderImage>, String>>>,
    image_order: VecDeque<String>,
    image_rows: HashMap<String, Vec<usize>>,
    code_highlights: HashMap<usize, Option<CodeHighlights>>,
    cancel: Arc<AtomicBool>,
    _parse: Task<()>,
}

impl MarkdownView {
    pub fn new(
        path: PathBuf,
        source: Arc<FileSource>,
        index: Arc<LineIndex>,
        text_style: TextStyle,
        cx: &mut Context<Self>,
    ) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let (send, receive) = async_channel::bounded(4);
        cx.background_executor()
            .spawn({
                let source = source.clone();
                let cancel = cancel.clone();
                async move {
                    let result = (|| -> std::io::Result<()> {
                        // Show the first screen before the parser's full-document
                        // reference-definition pass. Full batches replace it later.
                        let head = source.head(64 * 1024)?;
                        let initial = parse(&String::from_utf8_lossy(&head));
                        if send.send_blocking(Update::Initial(initial)).is_err() {
                            return Ok(());
                        }
                        if cancel.load(Ordering::Relaxed) {
                            return Ok(());
                        }
                        // Keep even accidentally huge .md files bounded; Source
                        // remains available through the chunked text reader.
                        const PREVIEW_LIMIT: u64 = 128 * 1024 * 1024;
                        let len = source.len().min(PREVIEW_LIMIT);
                        let mut bytes = Vec::with_capacity(len as usize);
                        source.scan(0..len, |chunk| {
                            if cancel.load(Ordering::Relaxed) {
                                return ControlFlow::Break(());
                            }
                            bytes.extend_from_slice(chunk);
                            ControlFlow::Continue(())
                        })?;
                        if cancel.load(Ordering::Relaxed) {
                            return Ok(());
                        }
                        let utf8 = match std::str::from_utf8(&bytes) {
                            Err(error) if source.len() > len && error.error_len().is_none() => {
                                // A preview boundary can cut a valid multibyte scalar.
                                std::str::from_utf8(&bytes[..error.valid_up_to()])
                            }
                            result => result,
                        };
                        let text = utf8.map_err(|_| {
                            std::io::Error::other(
                                "Markdown preview requires UTF-8; use Source view for this file.",
                            )
                        })?;
                        let mut first = true;
                        let summary = parse_stream(
                            text,
                            || cancel.load(Ordering::Relaxed),
                            |blocks| {
                                let result =
                                    send.send_blocking(Update::Batch { first, blocks }).is_ok();
                                first = false;
                                result
                            },
                        );
                        send.send_blocking(Update::Finished(summary, source.len() > PREVIEW_LIMIT))
                            .ok();
                        Ok(())
                    })();
                    if let Err(error) = result {
                        send.send_blocking(Update::Error(error.to_string())).ok();
                    }
                }
            })
            .detach();
        let parsing = cx.spawn(async move |this, cx| {
            while let Ok(update) = receive.recv().await {
                if this
                    .update(cx, |this, cx| this.receive(update, cx))
                    .is_err()
                {
                    break;
                }
                // A continuously ready channel must not starve input/paint.
                // This yield exists only while parse results are arriving.
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(1))
                    .await;
            }
        });
        let list = ListState::new(0, ListAlignment::Top, px(300.));
        let weak = cx.entity().downgrade();
        list.set_scroll_handler(move |_, _, cx| {
            weak.update(cx, |_, cx| cx.notify()).ok();
        });
        Self {
            path,
            source,
            index,
            text_style,
            document: Document::default(),
            symbols: Arc::default(),
            list,
            focus: cx.focus_handle(),
            rendered: true,
            source_view: None,
            _source_observer: None,
            find: None,
            parsing: true,
            notice: None,
            pending_offset: None,
            pending_anchor: None,
            expanded: HashSet::new(),
            images: HashMap::new(),
            image_order: VecDeque::new(),
            image_rows: HashMap::new(),
            code_highlights: HashMap::new(),
            cancel,
            _parse: parsing,
        }
    }

    fn receive(&mut self, update: Update, cx: &mut Context<Self>) {
        match update {
            Update::Initial(document) => {
                self.document = document;
                self.list.reset(self.document.blocks.len());
                self.update_symbols();
            }
            Update::Batch { first, blocks } => {
                if first {
                    if let Some(find) = &mut self.find {
                        find.reset();
                    }
                    self.pending_offset = self.pending_offset.or(Some(self.position(cx) as usize));
                    self.document.blocks.clear();
                    self.image_rows.clear();
                    self.code_highlights.clear();
                    self.list.reset(0);
                }
                let old = self.document.blocks.len();
                self.document.blocks.extend(blocks);
                self.list.splice(old..old, self.document.blocks.len() - old);
            }
            Update::Finished(summary, limited) => {
                self.parsing = false;
                self.document.headings = summary.headings;
                self.document.warnings = summary.warnings;
                self.update_symbols();
                self.search(cx);
                if limited {
                    self.notice = Some(
                        "Preview covers the first 128 MiB. Source view can read the whole file."
                            .into(),
                    );
                } else if !self.document.warnings.is_empty() {
                    self.notice = Some(self.document.warnings.join(" ").into());
                }
            }
            Update::Error(error) => {
                self.parsing = false;
                self.notice = Some(format!("Couldn't render Markdown: {error}").into());
            }
        }
        if let Some(offset) = self.pending_offset
            && (!self.parsing
                || self
                    .document
                    .blocks
                    .last()
                    .is_some_and(|b| b.source.end >= offset))
        {
            self.pending_offset = None;
            self.scroll_to_offset(offset as u64, cx);
        }
        if let Some(anchor) = self.pending_anchor.clone()
            && let Some(offset) = self.document.anchor(&anchor)
        {
            self.pending_anchor = None;
            self.scroll_to_offset(offset as u64, cx);
        }
        cx.notify();
    }
    fn update_symbols(&mut self) {
        self.symbols = Arc::new(
            self.document
                .headings
                .iter()
                .map(|h| Symbol {
                    name: h.text.clone(),
                    kind: "heading".into(),
                    range: h.source.clone(),
                    depth: h.level.saturating_sub(1) as usize,
                })
                .collect(),
        );
    }
    pub fn symbols(&self) -> Arc<Vec<Symbol>> {
        self.symbols.clone()
    }
    pub fn position(&self, cx: &App) -> u64 {
        if !self.rendered
            && let Some(view) = &self.source_view
        {
            return view.read(cx).position();
        }
        self.document
            .blocks
            .get(self.list.logical_scroll_top().item_ix)
            .map_or(0, |b| b.source.start as u64)
    }
    pub fn scroll_to_offset(&mut self, offset: u64, cx: &mut Context<Self>) {
        if let Some(view) = &self.source_view {
            view.update(cx, |v, cx| v.scroll_to_offset(offset, cx));
        }
        if self.parsing
            && self
                .document
                .blocks
                .last()
                .is_none_or(|b| b.source.end < offset as usize)
        {
            self.pending_offset = Some(offset as usize);
        } else {
            self.list.scroll_to(ListOffset {
                item_ix: self.document.block_at(offset as usize),
                offset_in_item: px(0.),
            });
        }
        cx.notify();
    }
    pub fn scroll_to_line(&mut self, line: u64, cx: &mut Context<Self>) {
        if let Ok(Some(offset)) = self.index.line_start(line.saturating_sub(1), &*self.source) {
            self.scroll_to_offset(offset, cx);
        }
    }
    pub fn scroll_to_anchor(&mut self, anchor: String, cx: &mut Context<Self>) {
        if let Some(line) = anchor.strip_prefix('L').and_then(|s| s.parse::<u64>().ok()) {
            self.scroll_to_line(line, cx);
        } else if let Some(offset) = self.document.anchor(&anchor) {
            self.scroll_to_offset(offset as u64, cx);
        } else if self.parsing {
            self.pending_anchor = Some(anchor);
        } else {
            self.notice = Some(format!("Heading #{anchor} was not found.").into());
            cx.notify();
        }
    }
    pub fn summary(&self) -> String {
        format!(
            "{}    {}    Markdown",
            format_size(self.source.len()),
            if self.parsing {
                "Rendering…"
            } else if self.rendered {
                "Rendered"
            } else {
                "Source"
            }
        )
    }
    pub fn notice(&self) -> Option<SharedString> {
        self.notice.clone()
    }
    pub fn duplicate(&self, cx: &mut Context<Self>) -> Entity<Self> {
        let path = self.path.clone();
        let source = self.source.clone();
        let index = self.index.clone();
        let style = self.text_style.clone();
        let offset = self.position(cx);
        let rendered = self.rendered;
        let source_view = self
            .source_view
            .as_ref()
            .map(|view| view.update(cx, |view, cx| view.duplicate(cx)));
        cx.new(|cx| {
            let mut view = Self::new(path, source, index, style, cx);
            view.pending_offset = Some(offset as usize);
            view.rendered = rendered;
            view.source_view = source_view;
            if let Some(source) = &view.source_view {
                view._source_observer = Some(cx.observe(source, |_, _, cx| cx.notify()));
            }
            view
        })
    }
    fn toggle_source(&mut self, _: &ToggleSource, window: &mut Window, cx: &mut Context<Self>) {
        if self.rendered {
            let offset = self.position(cx);
            let view = self.source_view.get_or_insert_with(|| {
                let language = plastext_syntax::detect(&self.path, b"");
                cx.new(|cx| {
                    TextView::new(
                        self.source.clone(),
                        self.index.clone(),
                        self.text_style.clone(),
                        language,
                        cx,
                    )
                })
            });
            if self._source_observer.is_none() {
                self._source_observer = Some(cx.observe(view, |_, _, cx| cx.notify()));
            }
            view.update(cx, |v, cx| v.scroll_to_offset(offset, cx));
            view.focus_handle(cx).focus(window);
        } else {
            if let Some(view) = &self.source_view {
                let offset = view.read(cx).position();
                self.scroll_to_offset(offset, cx);
            }
            self.focus.focus(window);
        }
        self.rendered = !self.rendered;
        cx.notify();
    }
    fn open_link(&mut self, target: &str, cx: &mut Context<Self>) {
        match resolve_link(&self.path, target) {
            LinkTarget::Anchor(anchor) => self.scroll_to_anchor(anchor, cx),
            LinkTarget::Local { path, fragment } => cx.emit(MarkdownEvent::Open { path, fragment }),
            LinkTarget::External(url) => cx.open_url(&url),
            LinkTarget::Blocked => {
                self.notice = Some("This link's URL scheme is not supported.".into());
                cx.notify();
            }
        }
    }
    fn rich_text(
        &self,
        block: &Block,
        range: std::ops::Range<usize>,
        id: usize,
        mono: bool,
        theme: Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let text = block.text[range.clone()].to_owned();
        let mut runs = Vec::new();
        let mut links = Vec::new();
        let mut targets = Vec::new();
        let mut at = range.start;
        let bold = matches!(
            block.kind,
            BlockKind::Heading { .. } | BlockKind::TableRow { header: true, .. }
        );
        for span in &block.spans {
            let start = span.range.start.max(range.start);
            let end = span.range.end.min(range.end);
            if start >= end {
                continue;
            }
            if start > at {
                runs.push(text_run(start - at, mono, false, false, theme.text, theme));
            }
            let style = &span.style;
            let color = if style.dim {
                theme.text_muted
            } else if style.math {
                theme.syntax(Highlight::Number)
            } else if style.link.is_some() {
                theme.syntax(Highlight::Link)
            } else {
                theme.text
            };
            let mut run = text_run(
                end - start,
                mono || style.code,
                style.strong || bold,
                style.emphasis,
                color,
                theme,
            );
            if style.strike {
                run.strikethrough = Some(gpui::StrikethroughStyle {
                    thickness: px(1.),
                    color: None,
                });
            }
            if let Some(target) = &style.link {
                links.push(start - range.start..end - range.start);
                targets.push(target.clone());
            }
            runs.push(run);
            at = end;
        }
        if at < range.end {
            runs.push(text_run(
                range.end - at,
                mono,
                false,
                false,
                theme.text,
                theme,
            ));
        }
        let weak = cx.entity().downgrade();
        let tooltip_targets = targets.clone();
        let tooltip_ranges = links.clone();
        let runs = self.highlight_matches(&text, runs, theme);
        InteractiveText::new(("md-text", id), StyledText::new(text).with_runs(runs))
            .on_click(links, move |ix, _, cx| {
                weak.update(cx, |view, cx| view.open_link(&targets[ix], cx))
                    .ok();
            })
            .tooltip(move |ix, _, cx| {
                let link = tooltip_ranges
                    .iter()
                    .position(|range| range.contains(&ix))?;
                let text: SharedString = tooltip_targets[link].to_string().into();
                Some(cx.new(|_| LinkTooltip(text)).into())
            })
            .into_any_element()
    }
    fn image(
        &mut self,
        target: &str,
        alt: &str,
        ix: usize,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !self.images.contains_key(target) {
            let local = resolve_link(&self.path, target);
            if !target.starts_with("data:image/") && !matches!(local, LinkTarget::Local { .. }) {
                return div()
                    .p_3()
                    .rounded_md()
                    .bg(theme.surface)
                    .text_color(theme.text_muted)
                    .child(format!("Remote image blocked · {alt}"))
                    .into_any_element();
            }
            if self.images.values().filter(|entry| entry.is_none()).count() < 4 {
                let target = target.to_owned();
                self.images.insert(target.clone(), None);
                self.image_order.retain(|key| key != &target);
                self.image_order.push_back(target.clone());
                let loading = cx.background_executor().spawn({
                    let target = target.clone();
                    async move {
                        let image = if target.starts_with("data:image/") {
                            plastext_assets::load_data_uri(&target)
                        } else if let LinkTarget::Local { path, .. } = local {
                            plastext_assets::load_local(&path)
                        } else {
                            Err("Unsupported image URL".into())
                        };
                        image
                            .map(crate::image_view::render_image)
                            .map(|(image, _, _)| image)
                    }
                });
                cx.spawn(async move |this, cx| {
                    let image = loading.await;
                    this.update(cx, |this, cx| {
                        this.images.insert(target.clone(), Some(image));
                        while this.image_order.len() > 128
                            || this
                                .images
                                .values()
                                .filter_map(|entry| entry.as_ref().and_then(|v| v.as_ref().ok()))
                                .map(|image| image.as_bytes(0).map_or(0, <[u8]>::len))
                                .sum::<usize>()
                                > 64 * 1024 * 1024
                        {
                            if let Some(key) = this.image_order.pop_front() {
                                this.images.remove(&key);
                                this.image_rows.remove(&key);
                            }
                        }
                        if let Some(rows) = this.image_rows.get(&target) {
                            let anchor = this.list.logical_scroll_top();
                            for &row in rows {
                                if row < this.document.blocks.len() {
                                    this.list.splice(row..row + 1, 1);
                                }
                            }
                            this.list.scroll_to(anchor);
                        }
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
        }
        let rows = self.image_rows.entry(target.to_owned()).or_default();
        if !rows.contains(&ix) && rows.len() < 128 {
            rows.push(ix);
        }
        match self.images.get(target).and_then(Option::as_ref) {
            Some(Ok(image)) => {
                let dimensions = image.size(0);
                let width = dimensions.width.0 as f32;
                let height = dimensions.height.0 as f32;
                let scale = (720. / width).min(1.);
                gpui::img(image.clone())
                    .w(px(width * scale))
                    .h(px(height * scale))
                    .max_w_full()
                    .into_any_element()
            }
            Some(Err(error)) => div()
                .p_3()
                .bg(theme.surface)
                .text_color(theme.text_muted)
                .child(format!("{alt}: {error}"))
                .into_any_element(),
            None => div()
                .p_3()
                .text_color(theme.text_muted)
                .child(format!("Loading image · {alt}"))
                .into_any_element(),
        }
    }

    fn code_text(
        &mut self,
        block: Arc<Block>,
        ix: usize,
        language: &str,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !self.code_highlights.contains_key(&ix)
            && self
                .code_highlights
                .values()
                .filter(|value| value.is_none())
                .count()
                < 4
        {
            self.code_highlights.insert(ix, None);
            let language = plastext_syntax::languages()
                .iter()
                .find(|l| l.name.eq_ignore_ascii_case(language))
                .or_else(|| {
                    plastext_syntax::detect(&PathBuf::from(format!("code.{language}")), b"")
                });
            let parsing = cx.background_executor().spawn({
                let block = block.clone();
                async move {
                    language
                        .map(|language| {
                            plastext_syntax::SyntaxTree::parse(
                                language,
                                block.text.as_bytes(),
                                || false,
                            )
                            .map(|tree| tree.highlights(block.text.as_bytes(), 0..block.text.len()))
                            .unwrap_or_else(|| language.lexical.highlights(block.text.as_bytes()))
                        })
                        .unwrap_or_default()
                }
            });
            let original = block.clone();
            cx.spawn(async move |this, cx| {
                let highlights = parsing.await;
                this.update(cx, |this, cx| {
                    if this
                        .document
                        .blocks
                        .get(ix)
                        .is_some_and(|b| Arc::ptr_eq(b, &original))
                    {
                        if this.code_highlights.len() > 128 {
                            this.code_highlights.retain(|&key, _| key.abs_diff(ix) < 64);
                        }
                        this.code_highlights.insert(ix, Some(Arc::new(highlights)));
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
        let highlights = self
            .code_highlights
            .get(&ix)
            .and_then(Option::as_ref)
            .map(|spans| {
                spans
                    .iter()
                    .map(|(range, kind)| {
                        (
                            range.clone(),
                            gpui::HighlightStyle {
                                color: Some(theme.syntax(*kind).into()),
                                ..Default::default()
                            },
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        div()
            .font_family(self.text_style.font_family.clone())
            .child(StyledText::new(block.text.clone()).with_highlights(highlights))
            .into_any_element()
    }

    fn copy_fence(&mut self, ix: usize, cx: &mut Context<Self>) {
        let blocks: Vec<_> = self
            .document
            .blocks
            .iter()
            .skip(ix)
            .enumerate()
            .take_while(|(i, block)| {
                *i == 0 || matches!(block.kind, BlockKind::Code { first: false, .. })
            })
            .map(|(_, block)| block.clone())
            .collect();
        let copying = cx
            .background_executor()
            .spawn(async move { blocks.iter().map(|b| b.text.as_str()).collect::<String>() });
        cx.spawn(async move |_, cx| {
            let text = copying.await;
            cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string(text)))
                .ok();
        })
        .detach();
    }

    fn render_block(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(block) = self.document.blocks.get(ix).cloned() else {
            return div().into_any_element();
        };
        let theme = Theme::get(window, cx);
        let base = div()
            .id(("md-block", ix))
            .w_full()
            .max_w(px(820.))
            .mx_auto()
            .px_6()
            .py_2()
            .text_size(px(16.))
            .line_height(relative(1.55))
            .text_color(theme.text);
        let content = match &block.kind {
            BlockKind::Rule => div()
                .w_full()
                .h(px(1.))
                .my_3()
                .bg(theme.border)
                .into_any_element(),
            BlockKind::Heading { level, id } => {
                let anchor = id.clone();
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .mt_3()
                    .pb_2()
                    .when(*level <= 2, |d| d.border_b_1().border_color(theme.border))
                    .text_size(px(match level {
                        1 => 30.,
                        2 => 24.,
                        3 => 20.,
                        _ => 17.,
                    }))
                    .font_weight(FontWeight::BOLD)
                    .child(self.rich_text(&block, 0..block.text.len(), ix, false, theme, cx))
                    .child(
                        div()
                            .id(("md-anchor", ix))
                            .text_sm()
                            .text_color(theme.text_muted)
                            .cursor_pointer()
                            .child("#")
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(format!(
                                    "#{anchor}"
                                )))
                            }),
                    )
                    .into_any_element()
            }
            BlockKind::Code { language, first } => {
                let text = self.code_text(block.clone(), ix, language, theme, cx);
                div()
                    .p_3()
                    .rounded_md()
                    .bg(theme.surface)
                    .text_size(px(13.))
                    .when(*first, |d| {
                        d.child(
                            div()
                                .flex()
                                .justify_between()
                                .mb_2()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .child(language.clone())
                                .child(
                                    div()
                                        .id(("md-copy", ix))
                                        .cursor_pointer()
                                        .child(if self.parsing { "Loading…" } else { "Copy" })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if !this.parsing {
                                                this.copy_fence(ix, cx);
                                            }
                                        })),
                                ),
                        )
                    })
                    .child(text)
                    .into_any_element()
            }
            BlockKind::FrontMatter => {
                let expanded = self.expanded.contains(&ix);
                div()
                    .p_3()
                    .rounded_md()
                    .bg(theme.surface)
                    .text_sm()
                    .child(
                        div()
                            .id(("md-metadata", ix))
                            .cursor_pointer()
                            .text_color(theme.text_muted)
                            .child(if expanded {
                                "▾ Front matter"
                            } else {
                                "▸ Front matter"
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.expanded.remove(&ix) {
                                    this.expanded.insert(ix);
                                }
                                this.list.splice(ix..ix + 1, 1);
                                cx.notify();
                            })),
                    )
                    .when(expanded, |d| {
                        d.child(self.rich_text(&block, 0..block.text.len(), ix, true, theme, cx))
                    })
                    .into_any_element()
            }
            BlockKind::TableRow {
                header,
                cells,
                alignments,
            } => div()
                .flex()
                .w_full()
                .border_b_1()
                .border_color(theme.border)
                .when(*header, |d| {
                    d.bg(theme.surface).font_weight(FontWeight::BOLD)
                })
                .children(cells.iter().enumerate().map(|(cell, range)| {
                    div()
                        .flex_1()
                        .min_w_0()
                        .px_2()
                        .py_1()
                        .border_l_1()
                        .border_color(theme.border)
                        .map(|d| match alignments.get(cell) {
                            Some(plastext_markdown::Alignment::Center) => d.text_center(),
                            Some(plastext_markdown::Alignment::Right) => d.text_right(),
                            _ => d,
                        })
                        .child(self.rich_text(
                            &block,
                            range.clone(),
                            ix.wrapping_mul(4096) + cell,
                            false,
                            theme,
                            cx,
                        ))
                }))
                .into_any_element(),
            BlockKind::Image { destination, .. } => {
                self.image(destination, &block.text, ix, theme, cx)
            }
            BlockKind::Math => div()
                .p_3()
                .bg(theme.surface)
                .child(self.rich_text(&block, 0..block.text.len(), ix, true, theme, cx))
                .into_any_element(),
            _ => self.rich_text(&block, 0..block.text.len(), ix, false, theme, cx),
        };
        base.child(
            div()
                .pl(px(block.indent as f32 * 18.))
                .when(block.quote.is_some(), |d| {
                    d.border_l_2()
                        .border_color(theme.syntax(Highlight::Link))
                        .pl_3()
                })
                .children(block.quote.as_ref().filter(|s| !s.is_empty()).map(|q| {
                    div()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.syntax(Highlight::Link))
                        .child(q.clone())
                }))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .children(block.marker.clone().map(|m| div().flex_none().child(m)))
                        .children(block.task.map(|checked| {
                            div()
                                .flex_none()
                                .text_color(theme.text_muted)
                                .child(if checked { "☑" } else { "☐" })
                        }))
                        .child(div().flex_1().min_w_0().child(content)),
                ),
        )
        .into_any_element()
    }
}

fn text_run(
    len: usize,
    mono: bool,
    bold: bool,
    italic: bool,
    color: gpui::Rgba,
    _theme: Theme,
) -> TextRun {
    let mut face = font(if mono { "Menlo" } else { ".SystemUIFont" });
    if bold {
        face.weight = FontWeight::BOLD;
    }
    if italic {
        face.style = FontStyle::Italic;
    }
    TextRun {
        len,
        font: face,
        color: color.into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}
struct LinkTooltip(SharedString);
impl Render for LinkTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .p_2()
            .bg(gpui::rgb(0x303038))
            .text_color(gpui::rgb(0xffffff))
            .text_xs()
            .child(self.0.clone())
    }
}
impl Drop for MarkdownView {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl Focusable for MarkdownView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        if !self.rendered
            && let Some(view) = &self.source_view
        {
            view.focus_handle(cx)
        } else {
            self.focus.clone()
        }
    }
}
impl Render for MarkdownView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(window, cx);
        let find_bar = self.find_bar(theme, cx);
        let weak = cx.entity().downgrade();
        let body = if !self.rendered {
            self.source_view
                .as_ref()
                .unwrap()
                .clone()
                .into_any_element()
        } else if self.document.blocks.is_empty() {
            div()
                .p_6()
                .text_color(theme.text_muted)
                .child(if self.parsing {
                    "Rendering Markdown…"
                } else {
                    "Empty document"
                })
                .into_any_element()
        } else {
            list(self.list.clone(), move |ix, window, cx| {
                weak.update(cx, |view, cx| view.render_block(ix, window, cx))
                    .unwrap_or_else(|_| div().into_any_element())
            })
            .size_full()
            .into_any_element()
        };
        div()
            .key_context("Markdown")
            .size_full()
            .flex()
            .flex_col()
            .on_action(cx.listener(Self::toggle_source))
            .on_action(cx.listener(Self::find))
            .on_action(cx.listener(Self::next_match))
            .on_action(cx.listener(Self::previous_match))
            .on_action(cx.listener(|this, _: &ScrollDown, _, cx| {
                this.list.scroll_by(px(28.));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ScrollUp, _, cx| {
                this.list.scroll_by(px(-28.));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &PageDown, _, cx| {
                this.list.scroll_by(px(500.));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &PageUp, _, cx| {
                this.list.scroll_by(px(-500.));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToStart, _, cx| {
                this.list.scroll_to(ListOffset {
                    item_ix: 0,
                    offset_in_item: px(0.),
                });
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToEnd, _, cx| {
                this.list
                    .scroll_to_reveal_item(this.document.blocks.len().saturating_sub(1));
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(30.))
                    .px_3()
                    .text_xs()
                    .bg(theme.surface)
                    .text_color(theme.text_muted)
                    .border_b_1()
                    .border_color(theme.border)
                    .child(if self.parsing {
                        "Markdown · rendering…"
                    } else {
                        "Markdown"
                    })
                    .child(
                        div()
                            .id("md-toggle-source")
                            .cursor_pointer()
                            .child(if self.rendered {
                                "Source  ⇧⌘V"
                            } else {
                                "Rendered  ⇧⌘V"
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_source(&ToggleSource, window, cx)
                            })),
                    ),
            )
            .children(find_bar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .when(self.rendered, |d| {
                        d.key_context("MarkdownPreview")
                            .track_focus(&self.focus)
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(|this, _, window, _| this.focus.focus(window)),
                            )
                    })
                    .child(body),
            )
    }
}
