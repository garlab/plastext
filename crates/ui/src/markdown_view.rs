use gpui::{
    AnyElement, App, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable,
    FontStyle, FontWeight, InteractiveText, KeyBinding, ListAlignment, ListOffset, ListState,
    SharedString, StyledText, Subscription, Task, TextRun, Window, actions, div, font, list,
    prelude::*, px, relative,
};
use plastext_buffer::{ByteSource as _, FileSource, LineIndex};
use plastext_editor::{TextStyle, TextView, Theme};
use plastext_markdown::{
    Block, BlockKind, Document, DocumentIndex, LinkTarget, PageCache, parse, resolve_link,
};
use plastext_syntax::{Highlight, Symbol};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::tab_view::format_size;
mod editing;
mod find;
mod highlights;
mod math;
mod mermaid;
mod metadata;
mod profile;
mod selection;
mod visibility;
type CodeHighlights = Arc<Vec<(std::ops::Range<usize>, Highlight)>>;

actions!(
    markdown,
    [
        ToggleSource,
        Save,
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
    selection::init(cx);
    cx.bind_keys([
        KeyBinding::new("cmd-s", Save, Some("MarkdownPreview")),
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
    Initial(Document, usize),
    Finished(Arc<DocumentIndex>, Arc<Vec<Symbol>>, Vec<Arc<Block>>),
    Error(String),
}

/// Body text size, in logical pixels, and line height in multiples of it.
const BODY_SIZE: f32 = 16.;
const LINE_HEIGHT: f32 = 1.55;

pub struct MarkdownView {
    path: PathBuf,
    source: Arc<FileSource>,
    preview_source: Arc<dyn plastext_markdown::DocumentSource>,
    editing_document: Option<Entity<plastext_editor::EditingDocument>>,
    _document_observer: Option<Subscription>,
    preview_generation: Option<u64>,
    refresh_generation: Option<u64>,
    refresh_task: Option<Task<()>>,
    index: Arc<LineIndex>,
    text_style: TextStyle,
    settings: Arc<plastext_settings::Markdown>,
    prose_width: Option<gpui::Pixels>,
    zoom: f32,
    document: Arc<DocumentIndex>,
    pages: PageCache,
    pending_pages: HashSet<usize>,
    failed_pages: HashSet<usize>,
    symbols: Arc<Vec<Symbol>>,
    list: ListState,
    visible: visibility::VisibleBlocks,
    focus: FocusHandle,
    rendered: bool,
    source_view: Option<Entity<TextView>>,
    _source_observer: Option<Subscription>,
    find: Option<find::FindState>,
    reading: selection::State,
    parsing: bool,
    preview_complete: bool,
    notice: Option<SharedString>,
    pending_offset: Option<usize>,
    pending_anchor: Option<String>,
    expanded: HashSet<usize>,
    images: HashMap<String, Option<Result<Arc<gpui::RenderImage>, String>>>,
    image_cancels: HashMap<String, Arc<AtomicBool>>,
    image_sizes: HashMap<String, (u32, u32)>,
    image_order: VecDeque<String>,
    image_rows: HashMap<String, Vec<usize>>,
    formulas: math::Formulas,
    diagrams: mermaid::Diagrams,
    code_highlights: HashMap<usize, Option<CodeHighlights>>,
    cancel: Arc<AtomicBool>,
    _parse: Task<()>,
    profile: Option<profile::ReadingProfile>,
}

impl MarkdownView {
    pub fn new(
        path: PathBuf,
        source: Arc<FileSource>,
        index: Arc<LineIndex>,
        text_style: TextStyle,
        settings: Arc<plastext_settings::Markdown>,
        cx: &mut Context<Self>,
    ) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let (send, receive) = async_channel::bounded(2);
        cx.background_executor()
            .spawn({
                let source = source.clone();
                let cancel = cancel.clone();
                async move {
                    let result = (|| -> std::io::Result<()> {
                        let head = source.head(64 * 1024)?;
                        let initial = parse(&String::from_utf8_lossy(&head));
                        if send
                            .send_blocking(Update::Initial(initial, head.len()))
                            .is_err()
                        {
                            return Ok(());
                        }
                        let document = Arc::new(DocumentIndex::build(
                            &source,
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
                                .map(|h| Symbol {
                                    name: h.text.clone(),
                                    kind: "heading".into(),
                                    range: h.source.clone(),
                                    depth: h.level.saturating_sub(1) as usize,
                                })
                                .collect(),
                        );
                        let first = if document.pages.is_empty() {
                            Vec::new()
                        } else {
                            document.parse_page(&source, 0, || cancel.load(Ordering::Relaxed))?
                        };
                        send.send_blocking(Update::Finished(document, symbols, first))
                            .ok();
                        Ok(())
                    })();
                    if let Err(error) = result
                        && error.kind() != std::io::ErrorKind::Interrupted
                    {
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
            }
        });
        let list = ListState::new(0, ListAlignment::Top, px(300.));
        let weak = cx.entity().downgrade();
        list.set_scroll_handler(move |_, _, cx| {
            weak.update(cx, |_, cx| cx.notify()).ok();
        });
        let owner = cx.entity_id();
        cx.on_release(move |this, cx| {
            this.cancel.store(true, Ordering::Relaxed);
            // Let cancelled refreshes retire their snapshots/results on a worker.
            if let Some(task) = this.refresh_task.take() {
                task.detach();
            }
            this.diagrams.clear(owner, cx);
            this.clear_images(cx);
            if let Some(document) = this.editing_document.take() {
                document.update(cx, |doc, _| doc.release_view());
            }
            let source = std::mem::replace(
                &mut this.preview_source,
                Arc::new(FileSource::from_bytes(Vec::new())),
            );
            let document = std::mem::take(&mut this.document);
            cx.background_executor()
                .spawn(async move {
                    drop((source, document));
                })
                .detach();
        })
        .detach();
        Self {
            path,
            preview_source: source.clone(),
            editing_document: None,
            _document_observer: None,
            preview_generation: None,
            refresh_generation: None,
            refresh_task: None,
            source,
            index,
            text_style,
            settings,
            prose_width: None,
            zoom: 1.,
            document: Arc::default(),
            pages: PageCache::default(),
            pending_pages: HashSet::new(),
            failed_pages: HashSet::new(),
            symbols: Arc::default(),
            list,
            visible: visibility::VisibleBlocks::default(),
            focus: cx.focus_handle(),
            rendered: true,
            source_view: None,
            _source_observer: None,
            find: None,
            reading: selection::State::default(),
            parsing: true,
            preview_complete: false,
            notice: None,
            pending_offset: None,
            pending_anchor: None,
            expanded: HashSet::new(),
            images: HashMap::new(),
            image_cancels: HashMap::new(),
            image_sizes: HashMap::new(),
            image_order: VecDeque::new(),
            image_rows: HashMap::new(),
            formulas: math::Formulas::default(),
            diagrams: mermaid::Diagrams::default(),
            code_highlights: HashMap::new(),
            cancel,
            _parse: parsing,
            profile: None,
        }
    }

    fn receive(&mut self, update: Update, cx: &mut Context<Self>) {
        match update {
            Update::Initial(document, bytes) => {
                self.document = Arc::new(DocumentIndex::initial(&document, bytes));
                self.pages.insert(0, document.blocks);
                self.visible.reset(&self.document.blocks, &self.expanded);
                self.list.reset(self.visible.len());
                self.update_symbols();
            }
            Update::Finished(document, symbols, first) => {
                let offset = self.pending_offset.unwrap_or(self.position(cx) as usize);
                let within = self.list.logical_scroll_top().offset_in_item;
                if let Some(find) = &mut self.find {
                    find.reset();
                }
                self.reading = selection::State::default();
                let old_document = std::mem::replace(&mut self.document, document);
                let old_pages = std::mem::take(&mut self.pages);
                self.pending_pages.clear();
                self.failed_pages.clear();
                self.pages.insert(0, first);
                self.image_rows.clear();
                self.formulas.clear_rows();
                self.code_highlights.clear();
                self.visible.reset(&self.document.blocks, &self.expanded);
                self.list.reset(self.visible.len());
                self.parsing = false;
                self.preview_complete = true;
                let old_symbols = std::mem::replace(&mut self.symbols, symbols);
                cx.background_executor()
                    .spawn(async move {
                        drop((old_document, old_pages, old_symbols));
                    })
                    .detach();
                self.list.scroll_to(ListOffset {
                    item_ix: self.visible.row(self.document.block_at(offset)),
                    offset_in_item: within,
                });
                self.search(cx);
                self.number_equations(cx);
                if !self.document.warnings.is_empty() {
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
        self.visible
            .block(self.list.logical_scroll_top().item_ix)
            .and_then(|ix| self.document.blocks.get(ix))
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
            let block = self.document.block_at(offset as usize);
            self.reveal_metadata(block);
            self.list.scroll_to(ListOffset {
                item_ix: self.visible.row(block),
                offset_in_item: px(0.),
            });
        }
        cx.notify();
    }
    pub fn scroll_to_line(&mut self, line: u64, cx: &mut Context<Self>) {
        if self.scroll_edited_line(line, cx) {
            return;
        }
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
    pub fn summary(&self, cx: &App) -> String {
        format!(
            "{}    {}    Markdown",
            format_size(
                self.editing_document
                    .as_ref()
                    .and_then(|doc| doc.read(cx).version())
                    .map_or_else(|| self.source.len(), |version| version.text().len())
            ),
            if self.parsing {
                "Rendering…"
            } else if self.rendered {
                "Rendered"
            } else {
                "Source"
            }
        )
    }
    pub fn notice(&self, cx: &App) -> Option<SharedString> {
        self.editing_document
            .as_ref()
            .and_then(|doc| doc.read(cx).notice())
            .or_else(|| self.notice.clone())
    }
    pub fn set_zoom(&mut self, factor: f32, font_size: gpui::Pixels, cx: &mut Context<Self>) {
        self.text_style.font_size = font_size;
        if let Some(source) = &self.source_view {
            source.update(cx, |view, cx| view.set_font_size(font_size, cx));
        }
        if self.zoom == factor {
            return;
        }
        let mut anchor = self.list.logical_scroll_top();
        anchor.offset_in_item *= factor / self.zoom;
        self.zoom = factor;
        self.prose_width = None;
        self.formulas.zoom_changed();
        self.list.reset(self.visible.len());
        self.list.scroll_to(anchor);
        cx.notify();
    }

    pub fn duplicate(&self, cx: &mut Context<Self>) -> Entity<Self> {
        let path = self.path.clone();
        let source = self.source.clone();
        let index = self.index.clone();
        let style = self.text_style.clone();
        let settings = self.settings.clone();
        let offset = self.position(cx);
        let rendered = self.rendered;
        let expanded = self.expanded.clone();
        let document = self.editing_document.clone();
        let source_view = self
            .source_view
            .as_ref()
            .map(|view| view.update(cx, |view, cx| view.duplicate(cx)));
        cx.new(|cx| {
            let mut view = Self::new(path, source, index, style, settings, cx);
            view.pending_offset = Some(offset as usize);
            view.rendered = rendered;
            view.expanded = expanded;
            view.source_view = source_view;
            if let Some(document) = document {
                view.set_editing_document(document, cx);
            }
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
                    let mut view = TextView::new(
                        self.source.clone(),
                        self.index.clone(),
                        self.text_style.clone(),
                        language,
                        cx,
                    );
                    view.set_file_path(&self.path);
                    if let Some(document) = &self.editing_document {
                        view.set_embedded_editing_document(document.clone(), cx);
                    }
                    view
                })
            });
            if self._source_observer.is_none() {
                self._source_observer = Some(cx.observe(view, |_, _, cx| cx.notify()));
            }
            view.update(cx, |view, cx| {
                view.scroll_to_offset(offset, cx);
                view.edit_by_default(cx);
            });
            view.focus_handle(cx).focus(window);
        } else {
            if let Some(view) = &self.source_view {
                let offset = view.read(cx).position();
                self.scroll_to_offset(offset, cx);
                if self
                    .editing_document
                    .as_ref()
                    .and_then(|doc| doc.read(cx).version())
                    .is_some_and(|version| self.preview_generation != Some(version.generation()))
                {
                    self.pending_offset = Some(offset as usize);
                }
            }
            self.focus.focus(window);
        }
        self.reading.stop();
        self.rendered = !self.rendered;
        self.refresh_preview(true, cx);
        if !self.rendered {
            self.diagrams.clear(cx.entity_id(), cx);
            self.clear_images(cx);
        }
        cx.notify();
    }
    fn reveal_metadata(&mut self, block: usize) {
        let mut changed = false;
        for section in plastext_markdown::BlockEntry::ancestors(&self.document.blocks, block) {
            if !plastext_markdown::BlockEntry::section_open(
                &self.document.blocks,
                section,
                &self.expanded,
            ) {
                if !self.expanded.remove(&section) {
                    self.expanded.insert(section);
                }
                changed = true;
            }
        }
        if changed {
            self.visible.reset(&self.document.blocks, &self.expanded);
            self.list.reset(self.visible.len());
        }
    }

    fn toggle_metadata(&mut self, section: usize, cx: &mut Context<Self>) {
        let top = self.list.logical_scroll_top();
        let anchor = self.visible.block(top.item_ix).unwrap_or(0);
        if !self.expanded.remove(&section) {
            self.expanded.insert(section);
        }
        self.visible.reset(&self.document.blocks, &self.expanded);
        self.list.reset(self.visible.len());
        self.list.scroll_to(ListOffset {
            item_ix: self.visible.row(anchor),
            offset_in_item: if self.visible.block(self.visible.row(anchor)) == Some(anchor) {
                top.offset_in_item
            } else {
                px(0.)
            },
        });
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
    #[allow(clippy::too_many_arguments)]
    fn rich_text(
        &self,
        block: &Block,
        range: std::ops::Range<usize>,
        id: (usize, usize),
        mono: bool,
        theme: Theme,
        window: &Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        // The displayed text: the source, except that rendered inline
        // formulas replace their TeX with placeholders.
        let mut text = String::with_capacity(range.len());
        let mut runs = Vec::new();
        let mut links = Vec::new();
        let mut targets = Vec::new();
        let mut slots = Vec::new();
        let mut replacements = Vec::new();
        let mut marks = Vec::new();
        let mut at = range.start;
        let bold = matches!(
            block.kind,
            BlockKind::Heading { .. } | BlockKind::TableRow { header: true, .. }
        );
        let inline_math = !mono && math::inline_math_shown(block);
        for span in &block.spans {
            let start = span.range.start.max(range.start);
            let end = span.range.end.min(range.end);
            if start >= end {
                continue;
            }
            if start > at {
                text.push_str(&block.text[at..start]);
                runs.push(text_run(start - at, mono, bold, false, theme.text, self));
            }
            let style = &span.style;
            if style.math
                && inline_math
                && span.range == (start..end)
                && let Some((count, formula)) = self.inline_slot(
                    block,
                    &block.text[start..end],
                    span.source.start,
                    math::inline_color(style, theme),
                )
            {
                let slot = math::Slot::new(text.len(), count, formula);
                replacements.push(selection::Replacement {
                    source: start - range.start..end - range.start,
                    display: slot.range(),
                });
                if let Some(target) = &style.link {
                    links.push(slot.range());
                    targets.push(target.clone());
                }
                if style.mark {
                    marks.push(slot.range());
                }
                text.push_str(&math::SLOT_CHAR.to_string().repeat(count));
                slots.push(slot);
                let transparent = gpui::Rgba {
                    a: 0.,
                    ..theme.text
                };
                runs.push(text_run(count, false, false, false, transparent, self));
                at = end;
                continue;
            }
            let color = if style.dim {
                theme.text_muted
            } else if style.math {
                theme.syntax(Highlight::Number)
            } else if style.link.is_some() {
                theme.syntax(Highlight::Link)
            } else {
                theme.text
            };
            if !mono
                && style.script != 0
                && let Some(slot) = self.script_slot(
                    block,
                    text.len(),
                    &block.text[start..end],
                    style,
                    color,
                    window,
                )
            {
                let display = slot.range();
                replacements.push(selection::Replacement {
                    source: start - range.start..end - range.start,
                    display: display.clone(),
                });
                if let Some(target) = &style.link {
                    links.push(display.clone());
                    targets.push(target.clone());
                }
                if style.mark {
                    marks.push(display.clone());
                }
                text.push_str(&math::SLOT_CHAR.to_string().repeat(display.len()));
                runs.push(text_run(
                    display.len(),
                    false,
                    false,
                    false,
                    gpui::Rgba {
                        a: 0.,
                        ..theme.text
                    },
                    self,
                ));
                slots.push(slot);
                at = end;
                continue;
            }
            if style.mark {
                marks.push(text.len()..text.len() + end - start);
            }
            let mut run = text_run(
                end - start,
                mono || style.code,
                style.strong || bold,
                style.emphasis,
                color,
                self,
            );
            if style.strike {
                run.strikethrough = Some(gpui::StrikethroughStyle {
                    thickness: px(1.),
                    color: None,
                });
            }
            if let Some(target) = &style.link {
                links.push(text.len()..text.len() + end - start);
                targets.push(target.clone());
            }
            text.push_str(&block.text[start..end]);
            runs.push(run);
            at = end;
        }
        if at < range.end {
            text.push_str(&block.text[at..range.end]);
            runs.push(text_run(
                range.end - at,
                mono,
                false,
                false,
                theme.text,
                self,
            ));
        }
        let weak = cx.entity().downgrade();
        let tooltip_targets = targets.clone();
        let tooltip_ranges = links.clone();
        math::Slot::set_line_starts(&mut slots, &text);
        let styled = StyledText::new(text).with_runs(runs);
        let layout = styled.layout().clone();
        let text = InteractiveText::new(("md-text", id.1), styled)
            .on_click(links, move |ix, _, cx| {
                weak.update(cx, |view, cx| {
                    if !view.reading.suppress_link
                        && !view.reading.selected.range().is_some_and(|r| !r.is_empty())
                    {
                        view.open_link(&targets[ix], cx);
                    }
                })
                .ok();
            })
            .tooltip(move |ix, _, cx| {
                let link = tooltip_ranges
                    .iter()
                    .position(|range| range.contains(&ix))?;
                let text: SharedString = tooltip_targets[link].to_string().into();
                Some(cx.new(|_| LinkTooltip(text)).into())
            });
        let content = self.with_inline_math(block, text, layout.clone(), slots);
        self.selectable(
            id.0,
            range.start,
            block.text[range].to_owned().into(),
            Some(layout),
            replacements,
            marks,
            content,
            cx,
        )
    }
    fn image(
        &mut self,
        target: &str,
        alt: &str,
        attributes: Option<&plastext_markdown::ImageAttributes>,
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
                let token = Arc::new(AtomicBool::new(false));
                self.image_cancels.insert(target.clone(), token.clone());
                let loading = cx.background_executor().spawn({
                    let target = target.clone();
                    let token = token.clone();
                    async move {
                        let image = if target.starts_with("data:image/") {
                            plastext_platform::images::preview_data_uri(&target, &token)
                        } else if let LinkTarget::Local { path, .. } = local {
                            plastext_platform::images::preview(&path, &token)
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
                        if this
                            .image_cancels
                            .get(&target)
                            .is_none_or(|current| !Arc::ptr_eq(current, &token))
                        {
                            return;
                        }
                        this.image_cancels.remove(&target);
                        if let Ok(image) = &image {
                            if this.image_sizes.len() >= 128
                                && !this.image_sizes.contains_key(&target)
                                && let Some(key) = this.image_sizes.keys().next().cloned()
                            {
                                this.image_sizes.remove(&key);
                            }
                            let size = image.size(0);
                            this.image_sizes.insert(
                                target.clone(),
                                (size.width.0 as u32, size.height.0 as u32),
                            );
                        }
                        this.images.insert(target.clone(), Some(image));
                        while this.image_order.len() > 128
                            || this
                                .images
                                .values()
                                .filter_map(|entry| entry.as_ref().and_then(|v| v.as_ref().ok()))
                                .map(|image| image.as_bytes(0).map_or(0, <[u8]>::len) * 2)
                                .sum::<usize>()
                                > 64 * 1024 * 1024
                        {
                            if let Some(key) = this.image_order.pop_front() {
                                if let Some(Some(Ok(image))) = this.images.remove(&key) {
                                    cx.defer(move |cx| {
                                        cx.drop_image(image, None);
                                    });
                                }
                                if let Some(token) = this.image_cancels.remove(&key) {
                                    token.store(true, Ordering::Relaxed);
                                }
                                this.image_sizes.remove(&key);
                                this.image_rows.remove(&key);
                            }
                        }
                        if let Some(rows) = this.image_rows.get(&target) {
                            let anchor = this.list.logical_scroll_top();
                            for &row in rows {
                                if row < this.document.blocks.len() {
                                    let visible = this.visible.row(row);
                                    this.list.splice(visible..visible + 1, 1);
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
                let (width, height) = image_box(width, height, attributes);
                div()
                    .w(px(width * self.zoom))
                    .max_w_full()
                    .map(|mut d| {
                        d.style().aspect_ratio = Some(width / height);
                        d
                    })
                    .child(
                        gpui::img(image.clone())
                            .size_full()
                            .object_fit(gpui::ObjectFit::Contain),
                    )
                    .into_any_element()
            }
            Some(Err(error)) => div()
                .p_3()
                .bg(theme.surface)
                .text_color(theme.text_muted)
                .child(format!("{alt}: {error}"))
                .into_any_element(),
            None if self.image_sizes.contains_key(target) => {
                let (w, h) = self.image_sizes[target];
                let (width, height) = image_box(w as f32, h as f32, attributes);
                div()
                    .w(px(width * self.zoom))
                    .max_w_full()
                    .map(|mut d| {
                        d.style().aspect_ratio = Some(width / height);
                        d
                    })
                    .into_any_element()
            }
            None => div()
                .p_3()
                .text_color(theme.text_muted)
                .child(format!("Loading image · {alt}"))
                .into_any_element(),
        }
    }

    fn clear_images(&mut self, cx: &mut App) {
        for (_, token) in self.image_cancels.drain() {
            token.store(true, Ordering::Relaxed);
        }
        for (_, entry) in self.images.drain() {
            if let Some(Ok(image)) = entry {
                cx.defer(move |cx| {
                    cx.drop_image(image, None);
                });
            }
        }
        self.image_order.clear();
        self.image_rows.clear();
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
            let original = self.document.clone();
            cx.spawn(async move |this, cx| {
                let highlights = parsing.await;
                this.update(cx, |this, cx| {
                    if Arc::ptr_eq(&this.document, &original) {
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
        let text = StyledText::new(block.text.clone()).with_highlights(highlights);
        let layout = text.layout().clone();
        self.selectable(
            ix,
            0,
            block.text.clone().into(),
            Some(layout),
            Vec::new(),
            Vec::new(),
            div()
                .font_family(self.text_style.font_family.clone())
                .child(text),
            cx,
        )
    }

    fn copy_fence(&mut self, ix: usize, cx: &mut Context<Self>) {
        let document = self.document.clone();
        let source = self.preview_source.clone();
        let cancel = self.cancel.clone();
        let copying = cx.background_executor().spawn(async move {
            let end = ix
                + document
                    .blocks
                    .iter()
                    .skip(ix)
                    .enumerate()
                    .take_while(|(i, b)| {
                        *i == 0 || (b.kind == plastext_markdown::BlockType::Code && !b.code_first)
                    })
                    .count();
            let mut text = String::new();
            let mut at = ix;
            while at < end {
                let page = document.blocks[at].page as usize;
                let blocks =
                    document.parse_page(&source, page, || cancel.load(Ordering::Relaxed))?;
                let range = &document.pages[page].blocks;
                let until = range.end.min(end);
                for block in &blocks[at - range.start..until - range.start] {
                    if text.len() + block.text.len() > 32 * 1024 * 1024 {
                        return Err(std::io::Error::other(
                            "Code fence exceeds the 32 MiB copy limit; use Source view",
                        ));
                    }
                    text.push_str(&block.text);
                }
                at = until;
            }
            Ok(text)
        });
        cx.spawn(async move |this, cx| {
            let result = copying.await;
            this.update(cx, |this, cx| match result {
                Ok(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
                Err(error) => {
                    this.notice = Some(error.to_string().into());
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn request_page(&mut self, page: usize, cx: &mut Context<Self>) {
        if page >= self.document.pages.len()
            || self.pages.contains(page)
            || self.pending_pages.contains(&page)
            || self.failed_pages.contains(&page)
            || self.pending_pages.len() >= 2
        {
            return;
        }
        self.pending_pages.insert(page);
        let document = self.document.clone();
        let source = self.preview_source.clone();
        let cancel = self.cancel.clone();
        let work = cx.background_executor().spawn({
            let document = document.clone();
            async move { document.parse_page(&source, page, || cancel.load(Ordering::Relaxed)) }
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                if !Arc::ptr_eq(&document, &this.document) {
                    return;
                }
                this.pending_pages.remove(&page);
                match result {
                    Ok(blocks) => {
                        if !this.pages.insert(page, blocks) {
                            this.failed_pages.insert(page);
                            this.notice = Some(
                                "A Markdown page exceeds the parsed-cache budget; use Source view."
                                    .into(),
                            );
                        } else if let Some(profile) = &mut this.profile {
                            profile.loaded_page(page);
                        }
                    }
                    Err(error) => {
                        this.failed_pages.insert(page);
                        this.notice = Some(error.to_string().into());
                    }
                }
                let blocks = &document.pages[page].blocks;
                if !blocks.is_empty() {
                    let first = this.visible.row(blocks.start);
                    let end = this.visible.row(blocks.end - 1) + 1;
                    let anchor = this.list.logical_scroll_top();
                    this.list.splice(first..end, end - first);
                    this.list.scroll_to(anchor);
                }
                cx.notify();
            })
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
        let Some(entry) = self.document.blocks.get(ix).cloned() else {
            return div().into_any_element();
        };
        let page = entry.page as usize;
        let block = self.pages.get(page, entry.local as usize);
        if !self.parsing {
            self.request_page(page, cx);
            // A small margin in source order; no whole-document prefetch.
            self.request_page(page + 1, cx);
            if page > 0 {
                self.request_page(page - 1, cx);
            }
        }
        let Some(block) = block else {
            return div()
                .w_full()
                .flex()
                .justify_center()
                .child(
                    div()
                        .id(("md-block", ix))
                        .w_full()
                        .max_w(self.prose_width.unwrap_or(px(820.)))
                        .h(px(40.))
                        .px_6()
                        .py_2()
                        .text_color(Theme::get(window, cx).text_muted)
                        .child(if self.failed_pages.contains(&page) {
                            "Preview unavailable · use Source"
                        } else {
                            "Loading…"
                        }),
                )
                .into_any_element();
        };
        if let Some(profile) = &mut self.profile {
            let row = self.visible.row(ix);
            profile.rendered_target(row);
        }
        let theme = Theme::get(window, cx);
        self.prepare_math(&block, ix, theme, window, cx);
        let base = div()
            .id(("md-block", ix))
            .w_full()
            .max_w(self.prose_width.unwrap_or(px(820.)))
            .px_6()
            .py_2()
            .when(
                matches!(block.kind, BlockKind::FrontMatterRow { .. }),
                |d| d.py_0(),
            )
            .when(matches!(block.kind, BlockKind::TableRow { .. }), |d| {
                // Rows remain independently virtualized, but spacing belongs
                // around the table, not between every pair of rows. Consult
                // the compact index so this also works across cache pages.
                d.py_0()
                    .when(
                        matches!(block.kind, BlockKind::TableRow { header: true, .. }),
                        |d| d.pt_2(),
                    )
                    .when(
                        self.document
                            .blocks
                            .get(ix + 1)
                            .is_none_or(|next| next.kind != plastext_markdown::BlockType::TableRow),
                        |d| d.pb_2(),
                    )
            })
            .text_size(px(BODY_SIZE * self.zoom))
            .line_height(relative(LINE_HEIGHT))
            .text_color(theme.text)
            .map(|d| match block.alignment {
                plastext_markdown::Alignment::Center => d.text_center(),
                plastext_markdown::Alignment::Right => d.text_right(),
                _ => d,
            });
        let content = match &block.kind {
            BlockKind::Details { open } => {
                let section = block.source.start;
                let expanded = *open ^ self.expanded.contains(&section);
                div()
                    .id(("md-details", ix))
                    .flex()
                    .items_start()
                    .gap_2()
                    .cursor_pointer()
                    .child(
                        div()
                            .flex_none()
                            .text_color(theme.text_muted)
                            .child(if expanded { "▾" } else { "▸" }),
                    )
                    .child(div().flex_1().min_w_0().child(self.rich_text(
                        &block,
                        0..block.text.len(),
                        (ix, 0),
                        false,
                        theme,
                        window,
                        cx,
                    )))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.reading.selected.range().is_some_and(|r| !r.is_empty()) {
                            this.toggle_metadata(section, cx);
                        }
                    }))
                    .into_any_element()
            }
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
                    .gap(px(8. * self.zoom))
                    .mt(px(12. * self.zoom))
                    .pb(px(8. * self.zoom))
                    .when(*level <= 2, |d| d.border_b_1().border_color(theme.border))
                    .text_size(px(math::text_size(&block.kind) * self.zoom))
                    .font_weight(FontWeight::BOLD)
                    .child(self.rich_text(
                        &block,
                        0..block.text.len(),
                        (ix, 0),
                        false,
                        theme,
                        window,
                        cx,
                    ))
                    .child(
                        div()
                            .id(("md-anchor", ix))
                            .flex_none()
                            .text_size(px(14. * self.zoom))
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
            BlockKind::Code { .. } => self.code_block(block.clone(), ix, theme, window, cx),
            BlockKind::FrontMatter { .. }
            | BlockKind::FrontMatterRow { .. }
            | BlockKind::FrontMatterSource { .. } => {
                self.metadata_block(&block, ix, theme, window, cx)
            }
            BlockKind::TableRow {
                header,
                cells,
                alignments,
                ..
            } => div()
                .flex()
                .w_full()
                .border_b_1()
                .border_r_1()
                .border_color(theme.border)
                .when(*header, |d| {
                    d.border_t_1()
                        .bg(theme.surface)
                        .font_weight(FontWeight::BOLD)
                })
                .children(cells.iter().enumerate().map(|(cell, range)| {
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .justify_center()
                        .px_3()
                        .py(px(6. * self.zoom))
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
                            (ix, cell),
                            false,
                            theme,
                            window,
                            cx,
                        ))
                }))
                .into_any_element(),
            BlockKind::Image {
                destination,
                title,
                attributes,
            } => {
                let dark = matches!(
                    window.appearance(),
                    gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark
                );
                let target = attributes
                    .as_ref()
                    .map_or(destination.as_str(), |a| a.destination(destination, dark));
                let align = attributes.as_ref().map_or(block.alignment, |a| a.align);
                let image = self.image(target, &block.text, attributes.as_deref(), ix, theme, cx);
                let target_link = attributes
                    .as_ref()
                    .and_then(|a| a.link.clone())
                    .or_else(|| block.spans.iter().find_map(|span| span.style.link.clone()));
                let title = title.clone();
                div()
                    .id(("md-image", ix))
                    .w_full()
                    .flex()
                    .when(!title.is_empty(), |d| {
                        d.tooltip(move |_, cx| cx.new(|_| LinkTooltip(title.clone().into())).into())
                    })
                    .when_some(target_link, |d, link| {
                        d.cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| this.open_link(&link, cx)))
                    })
                    .map(|d| match align {
                        plastext_markdown::Alignment::Center => d.justify_center(),
                        plastext_markdown::Alignment::Right => d.justify_end(),
                        _ => d,
                    })
                    .child(image)
                    .into_any_element()
            }
            BlockKind::Math => self.display_math(&block, ix, theme, window, cx),
            _ => self.rich_text(
                &block,
                0..block.text.len(),
                (ix, 0),
                false,
                theme,
                window,
                cx,
            ),
        };
        // List items are laid out as roots, so their auto margins do not
        // center them. Give each row a full-width flex parent instead.
        div()
            .w_full()
            .flex()
            .justify_center()
            .child(
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
                                .text_size(px(14. * self.zoom))
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme.syntax(Highlight::Link))
                                .child(q.clone())
                        }))
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .when(
                                    matches!(block.kind, BlockKind::TableRow { in_list: true, .. }),
                                    |d| {
                                        // Reserve the same marker column on every
                                        // row, even though only the header has a bullet.
                                        d.child(
                                            div()
                                                .w(px(18.))
                                                .flex_none()
                                                .child(block.marker.clone().unwrap_or_default()),
                                        )
                                    },
                                )
                                .when(
                                    !matches!(
                                        block.kind,
                                        BlockKind::TableRow { in_list: true, .. }
                                    ),
                                    |d| {
                                        d.children(
                                            block
                                                .marker
                                                .clone()
                                                .map(|m| div().flex_none().child(m)),
                                        )
                                    },
                                )
                                .children(block.task.map(|checked| {
                                    div()
                                        .flex_none()
                                        .text_color(theme.text_muted)
                                        .child(if checked { "☑" } else { "☐" })
                                }))
                                .child(div().flex_1().min_w_0().child(content)),
                        ),
                ),
            )
            .into_any_element()
    }
}

fn image_box(
    width: f32,
    height: f32,
    attrs: Option<&plastext_markdown::ImageAttributes>,
) -> (f32, f32) {
    let aspect = width.max(1.) / height.max(1.);
    match (attrs.and_then(|a| a.width), attrs.and_then(|a| a.height)) {
        (Some(w), Some(h)) => (w as f32, h as f32),
        (Some(w), None) => (w as f32, w as f32 / aspect),
        (None, Some(h)) => (h as f32 * aspect, h as f32),
        _ => (width.min(720.), width.min(720.) / aspect),
    }
}

fn text_run(
    len: usize,
    mono: bool,
    bold: bool,
    italic: bool,
    color: gpui::Rgba,
    view: &MarkdownView,
) -> TextRun {
    let family = if mono {
        view.text_style.font_family.clone()
    } else if view.settings.prose_font == "system" {
        ".SystemUIFont".into()
    } else {
        view.settings.prose_font.clone().into()
    };
    let mut face = font(family);
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
        self.reading.hits = Default::default();
        if self.diagrams.begin_frame(cx.entity_id(), cx) {
            let view = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                let _ = view.update(cx, |this, cx| {
                    if this.diagrams.finish_frame() {
                        cx.notify();
                    }
                });
            });
        }
        if self.prose_width.is_none() {
            let width = match self.settings.max_width {
                plastext_settings::ProseWidth::Pixels(width) => px(width * self.zoom),
                plastext_settings::ProseWidth::Chars(count) => {
                    let family = if self.settings.prose_font == "system" {
                        ".SystemUIFont"
                    } else {
                        &self.settings.prose_font
                    };
                    let system = window.text_system();
                    let id = system.resolve_font(&font(family.to_owned()));
                    system
                        .advance(id, px(BODY_SIZE * self.zoom), '0')
                        .map_or(px(8. * self.zoom), |s| s.width)
                        * count
                }
            };
            self.prose_width = Some(width + px(48.));
        }
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
                weak.update(cx, |view, cx| {
                    view.visible
                        .block(ix)
                        .map(|block| view.render_block(block, window, cx))
                        .unwrap_or_else(|| div().into_any_element())
                })
                .unwrap_or_else(|_| div().into_any_element())
            })
            .size_full()
            .into_any_element()
        };
        div()
            .key_context("Markdown")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .on_action(cx.listener(Self::toggle_source))
            .on_action(cx.listener(Self::save))
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
                    .scroll_to_reveal_item(this.visible.len().saturating_sub(1));
                cx.notify();
            }))
            .when(self.profile.is_some(), |d| {
                let weak = cx.entity().downgrade();
                d.child(
                    gpui::canvas(
                        |_, _, _| (),
                        move |_, _, window, cx| {
                            weak.update(cx, |this, cx| this.profile_painted(window, cx))
                                .ok();
                        },
                    )
                    .absolute()
                    .size_full(),
                )
            })
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
                    } else if !self.rendered
                        && self.source_status(cx).is_some_and(|status| !status.editing)
                    {
                        "Markdown · press e or Enter to edit"
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
                        d.relative()
                            .key_context("MarkdownPreview")
                            .on_action(cx.listener(Self::copy_selection))
                            .on_action(cx.listener(Self::select_all))
                            .on_action(cx.listener(Self::clear_selection))
                            .track_focus(&self.focus)
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(|this, _, window, cx| {
                                    this.focus.focus(window);
                                    this.clear_selection(&selection::Clear, window, cx);
                                }),
                            )
                    })
                    .child(body)
                    .when(self.rendered, |d| d.child(self.selection_layer(cx))),
            )
    }
}

#[cfg(test)]
#[path = "markdown_view/editing_tests.rs"]
mod editing_tests;
