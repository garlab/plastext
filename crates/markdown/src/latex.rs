//! Native LaTeX events adapted to the reading view's blocks, without an
//! intermediate Markdown document. Only checkpoints and block locations are
//! retained; parsed pages use the same bounded cache as Markdown.
mod inputs;
#[cfg(test)]
mod tests;
use crate::{
    Alignment, Block, BlockKind, DocumentIndex, DocumentSource, Heading, InlineStyle, Span,
};
pub use inputs::Dependency;
use latex_reader::{self as lr, Event, Tag, TagKind};
use std::{
    io,
    ops::Range,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// The reader currently requires a contiguous UTF-8 snapshot. Larger sources
/// remain available in Source and PDF Preview rather than allocating unboundedly.
pub const MAX_SOURCE_BYTES: usize = 32 * 1024 * 1024;
const MAX_PAGE_BYTES: usize = 8 * 1024 * 1024;
const MAX_PAGE_BLOCKS: usize = 4096;

pub(crate) struct LatexDocument {
    source: String,
    pub(crate) index: lr::Index,
    states: Vec<State>,
    pub(crate) source_map: lr::InputSourceMap,
    pub(crate) dependencies: Vec<Dependency>,
}
impl std::fmt::Debug for LatexDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LatexDocument")
            .field("bytes", &self.source.len())
            .field("checkpoints", &self.states.len())
            .finish()
    }
}

impl LatexDocument {
    pub(crate) fn retained_bytes(&self) -> usize {
        // Reader internals are opaque. Charge a conservative allowance for its
        // bounded definitions/labels and each depth-limited checkpoint as well
        // as the snapshot and our adapter state.
        self.source.capacity()
            + self.source_map.retained_bytes()
            + self.dependencies.len() * 16 * 1024
            + 4 * 1024 * 1024
            + self.states.len() * 32 * 1024
    }

    pub(crate) fn parse_page(
        &self,
        page: usize,
        cancelled: &impl Fn() -> bool,
    ) -> io::Result<Vec<Arc<Block>>> {
        let mut adapter = Adapter::new(self.states[page].clone());
        for event in self.index.window(&self.source, page) {
            if cancelled() {
                return Err(cancel_error());
            }
            adapter.event(event)?;
        }
        adapter.finish();
        Ok(adapter.blocks)
    }
}

pub fn build(
    source: &(impl DocumentSource + ?Sized),
    cancel: &AtomicBool,
) -> io::Result<DocumentIndex> {
    build_impl(source, None, cancel)
}

/// Expand local inputs inside the document's repository (or its directory
/// when no repository is found), then build the native reading index.
pub fn build_with_path(
    source: &(impl DocumentSource + ?Sized),
    path: &Path,
    cancel: &AtomicBool,
) -> io::Result<DocumentIndex> {
    build_impl(source, Some(path), cancel)
}

fn build_impl(
    source: &(impl DocumentSource + ?Sized),
    path: Option<&Path>,
    cancel: &AtomicBool,
) -> io::Result<DocumentIndex> {
    let len = usize::try_from(source.byte_len()).unwrap_or(usize::MAX);
    if len > MAX_SOURCE_BYTES {
        return Err(io::Error::other(
            "Native LaTeX rendering is limited to 32 MiB; use Source or PDF Preview",
        ));
    }
    let mut bytes = Vec::with_capacity(len);
    for start in (0..len).step_by(64 * 1024) {
        if cancel.load(Ordering::Relaxed) {
            return Err(cancel_error());
        }
        source.read_range(
            start as u64..(start + 64 * 1024).min(len) as u64,
            &mut bytes,
        )?;
    }
    if bytes.len() != len {
        return Err(io::Error::other("LaTeX source changed while being read"));
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| io::Error::other("Native LaTeX rendering requires UTF-8"))?;
    let mut inputs = path
        .map(|path| inputs::Inputs::new(path, cancel))
        .transpose()?;
    let expanded = lr::expand_cancellable(
        &text,
        path.and_then(|p| p.file_stem()).and_then(|s| s.to_str()),
        &lr::InputLimits {
            max_bytes: MAX_SOURCE_BYTES,
            ..Default::default()
        },
        cancel,
        &mut |name, kind| inputs.as_mut()?.read(name, kind),
    )
    .map_err(|_| cancel_error())?;
    let source_map = expanded.source_map;
    let dependencies = inputs.map_or_else(Vec::new, |i| i.dependencies);
    let missing = expanded.missing;
    let text = expanded.text.into_owned();
    let options = lr::Options {
        max_checkpoints: 2048,
        limits: lr::Limits {
            max_nesting: 64,
            max_assignments: 2048,
            max_definition_bytes: 256 * 1024,
            max_labels: 8192,
            ..Default::default()
        },
        ..Default::default()
    };
    let index = lr::index(&text, &options, cancel).map_err(|_| cancel_error())?;
    let mut result = DocumentIndex::default();
    if !missing.is_empty() {
        result.warnings.push(format!(
            "{} included file(s) unavailable or outside the local reading limits; their commands remain visible.",
            missing.len()
        ));
    }
    let mut states = Vec::new();
    let mut state = State::default();
    for page in 0..index.checkpoints().len() {
        if cancel.load(Ordering::Relaxed) {
            return Err(cancel_error());
        }
        states.push(state.clone());
        let mut adapter = Adapter::new(state);
        let mut events = index.window(&text, page).with_cancel(cancel);
        for event in &mut events {
            if matches!(event.event, Event::Include(_)) && result.warnings.is_empty() {
                result.warnings.push(
                    "An included file could not be resolved; its command remains visible.".into(),
                );
            }
            adapter.event(event)?;
        }
        if events.cancelled() {
            return Err(cancel_error());
        }
        adapter.finish();
        state = adapter.state;
        let start = index.checkpoints()[page].offset();
        let end = index
            .checkpoints()
            .get(page + 1)
            .map_or(text.len(), |c| c.offset());
        for block in &adapter.blocks {
            if let BlockKind::Heading { level, id } = &block.kind {
                result.headings.push(Heading {
                    text: block.text.clone(),
                    id: id.clone(),
                    level: *level,
                    source: block.source.clone(),
                });
            }
        }
        result.push_latex_page(start..end, &adapter.blocks);
        if result.retained_bytes()
            + text.len()
            + states.len() * 32 * 1024
            + 4 * 1024 * 1024
            + source_map.retained_bytes()
            + dependencies.len() * 16 * 1024
            > crate::INDEX_BUDGET
        {
            return Err(io::Error::other(
                "Native LaTeX index exceeds its 64 MiB budget; use Source or PDF Preview",
            ));
        }
    }
    for diagnostic in index.diagnostics().first().iter().take(8) {
        result.warnings.push(format!(
            "LaTeX {:?}{} at byte {}",
            diagnostic.kind,
            diagnostic
                .name
                .as_ref()
                .map_or(String::new(), |name| format!(": {name}")),
            diagnostic.range.start
        ));
    }
    result.latex = Some(Arc::new(LatexDocument {
        source: text,
        index,
        states,
        source_map,
        dependencies,
    }));
    Ok(result)
}

fn cancel_error() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "LaTeX rendering cancelled")
}
fn alignment(align: lr::Align) -> Alignment {
    match align {
        lr::Align::Left | lr::Align::Justify => Alignment::Left,
        lr::Align::Center => Alignment::Center,
        lr::Align::Right => Alignment::Right,
    }
}

#[derive(Clone, Default)]
struct State {
    style: InlineStyle,
    stack: Vec<(TagKind, InlineStyle, Alignment)>,
    alignment: Alignment,
    lists: usize,
    quote: usize,
    marker: Option<String>,
    code: Option<String>,
    columns: Arc<[Alignment]>,
    table_rows: usize,
}
struct Adapter {
    state: State,
    blocks: Vec<Arc<Block>>,
    block: Option<Block>,
    cell: usize,
    cell_span: u16,
    bytes: usize,
}
impl Adapter {
    fn new(state: State) -> Self {
        Self {
            state,
            blocks: Vec::new(),
            block: None,
            cell: 0,
            cell_span: 1,
            bytes: 0,
        }
    }
    fn finish(&mut self) {
        self.flush();
    }
    fn flush(&mut self) {
        if let Some(mut block) = self.block.take() {
            if block.text.is_empty()
                && !matches!(block.kind, BlockKind::Image { .. } | BlockKind::Rule)
            {
                return;
            }
            block.text.shrink_to_fit();
            block.spans.shrink_to_fit();
            let block = Arc::new(block);
            self.bytes += crate::parsed_bytes(std::slice::from_ref(&block));
            self.blocks.push(block);
        }
    }
    fn begin(&mut self, kind: BlockKind, range: Range<usize>) {
        self.flush();
        let mut block = Block::new(kind, range);
        block.indent = self.state.lists.saturating_sub(1);
        block.marker = self.state.marker.take();
        block.quote = (self.state.quote > 0).then(String::new);
        block.alignment = self.state.alignment;
        self.block = Some(block);
    }
    fn append(&mut self, text: &str, range: Range<usize>, style: InlineStyle) {
        if text.is_empty() {
            return;
        }
        if self.block.is_none() {
            let kind = self
                .state
                .code
                .as_ref()
                .map_or(BlockKind::Paragraph, |language| BlockKind::Code {
                    language: language.clone(),
                    attributes: String::new(),
                    first: false,
                });
            self.begin(kind, range.start..range.start);
        }
        let mut remaining = text;
        let mut consumed = 0;
        while !remaining.is_empty() {
            let block = self.block.as_mut().unwrap();
            let room = crate::BLOCK_TEXT_LIMIT.saturating_sub(block.text.len());
            // Keep formulas and table rows intact within their own bounded limits.
            let indivisible =
                style.math || matches!(block.kind, BlockKind::TableRow { .. } | BlockKind::Math);
            let mut count = if indivisible {
                remaining.len()
            } else {
                remaining.len().min(room)
            };
            while !remaining.is_char_boundary(count) {
                count -= 1;
            }
            if count == 0 {
                let mut kind = block.kind.clone();
                if let BlockKind::Code { first, .. } = &mut kind {
                    *first = false;
                }
                let offset = if range.len() == text.len() {
                    range.start + consumed
                } else {
                    range.start
                };
                self.begin(kind, offset..offset);
                continue;
            }
            let start = block.text.len();
            block.text.push_str(&remaining[..count]);
            let source = if range.len() == text.len() {
                range.start + consumed..range.start + consumed + count
            } else {
                range.clone()
            };
            block.source.end = block.source.end.max(source.end);
            block.spans.push(Span {
                range: start..start + count,
                source,
                style: style.clone(),
            });
            consumed += count;
            remaining = &remaining[count..];
        }
    }
    fn event(&mut self, spanned: lr::Spanned<'_>) -> io::Result<()> {
        let range = spanned.range;
        match spanned.event {
            Event::Start(tag) => {
                self.state
                    .stack
                    .push((tag.kind(), self.state.style.clone(), self.state.alignment));
                match tag {
                    Tag::Title => {
                        self.state.alignment = Alignment::Center;
                        self.heading(1, None, range);
                    }
                    Tag::Heading { level, number } => {
                        self.heading(level.saturating_sub(1).clamp(1, 6), number, range)
                    }
                    Tag::Author | Tag::Date => {
                        self.state.alignment = Alignment::Center;
                        self.begin(BlockKind::Paragraph, range);
                    }
                    Tag::Paragraph { .. } => self.begin(BlockKind::Paragraph, range),
                    Tag::List(_) => {
                        self.flush();
                        self.state.lists += 1;
                    }
                    Tag::Item { label } => {
                        self.flush();
                        self.state.marker = Some(label.unwrap_or_else(|| "•".into()));
                    }
                    Tag::ItemLabel => self.state.style.strong = true,
                    Tag::Quote(_) | Tag::Abstract => {
                        self.flush();
                        self.state.quote += 1;
                    }
                    Tag::Align(align) => self.state.alignment = alignment(align),
                    Tag::Code { language } => {
                        let language = language.unwrap_or_default().into_owned();
                        self.state.code = Some(language.clone());
                        self.begin(
                            BlockKind::Code {
                                language,
                                attributes: String::new(),
                                first: true,
                            },
                            range,
                        );
                    }
                    Tag::InlineCode => self.state.style.code = true,
                    Tag::Style(style) => match style {
                        lr::Style::Bold => self.state.style.strong = true,
                        lr::Style::Emphasis => {
                            self.state.style.emphasis = !self.state.style.emphasis
                        }
                        lr::Style::Italic | lr::Style::Slanted => self.state.style.emphasis = true,
                        lr::Style::Monospace => self.state.style.code = true,
                        lr::Style::Strike => self.state.style.strike = true,
                        lr::Style::Superscript => self.state.style.script = 1,
                        lr::Style::Subscript => self.state.style.script = -1,
                        _ => {}
                    },
                    Tag::Link { url } => self.state.style.link = Some(Arc::from(url.as_ref())),
                    Tag::Theorem {
                        name,
                        number,
                        style,
                    } => {
                        self.begin(BlockKind::Paragraph, range.clone());
                        self.append(
                            &format!(
                                "{name}{}.",
                                number.map_or(String::new(), |n| format!(" {n}"))
                            ),
                            range,
                            InlineStyle {
                                strong: true,
                                ..Default::default()
                            },
                        );
                        self.flush();
                        self.state.style.emphasis = style == lr::TheoremStyle::Plain;
                    }
                    Tag::Proof => {
                        self.begin(BlockKind::Paragraph, range.clone());
                        self.append(
                            "Proof.",
                            range,
                            InlineStyle {
                                emphasis: true,
                                ..Default::default()
                            },
                        );
                        self.flush();
                    }
                    Tag::Caption { number, kind } => {
                        self.begin(BlockKind::Paragraph, range.clone());
                        self.append(
                            &format!(
                                "{kind:?}{}: ",
                                number.map_or(String::new(), |n| format!(" {n}"))
                            ),
                            range,
                            InlineStyle {
                                strong: true,
                                ..Default::default()
                            },
                        );
                    }
                    Tag::Table { columns } => {
                        self.flush();
                        self.state.table_rows = 0;
                        self.state.columns = columns.iter().map(|c| alignment(c.align)).collect();
                    }
                    Tag::Row => self.begin(
                        BlockKind::TableRow {
                            header: false,
                            in_list: self.state.lists > 0,
                            cells: Vec::new(),
                            alignments: self.state.columns.clone(),
                        },
                        range,
                    ),
                    Tag::Cell { span, .. } => {
                        self.cell = self.block.as_ref().map_or(0, |b| b.text.len());
                        self.cell_span = span;
                    }
                    Tag::Footnote { number } => {
                        self.append(
                            &format!(" [{}: ", number.as_deref().unwrap_or("*")),
                            range,
                            self.state.style.clone(),
                        );
                    }
                    Tag::Bibliography => {
                        self.heading(2, None, range.clone());
                        self.append("References", range, InlineStyle::default());
                        self.flush();
                    }
                    Tag::BibItem { label } => {
                        self.flush();
                        self.state.marker = Some(label);
                    }
                    Tag::Unsupported { block: true, .. } | Tag::Figure(_) | Tag::TitleBlock => {
                        self.flush()
                    }
                    _ => {}
                }
            }
            Event::End(kind) => {
                match kind {
                    TagKind::Cell => {
                        if let Some(block) = &mut self.block
                            && let BlockKind::TableRow { cells, .. } = &mut block.kind
                        {
                            cells.push(self.cell..block.text.len());
                            for _ in 1..self.cell_span.min(64) {
                                cells.push(block.text.len()..block.text.len());
                            }
                        }
                    }
                    TagKind::Footnote => self.append("] ", range.clone(), self.state.style.clone()),
                    TagKind::List => {
                        self.flush();
                        self.state.lists = self.state.lists.saturating_sub(1);
                    }
                    TagKind::Quote | TagKind::Abstract => {
                        self.flush();
                        self.state.quote = self.state.quote.saturating_sub(1);
                    }
                    TagKind::Code => {
                        self.flush();
                        self.state.code = None;
                    }
                    TagKind::Row => {
                        self.flush();
                        self.state.table_rows += 1;
                    }
                    TagKind::Paragraph
                    | TagKind::Heading
                    | TagKind::Title
                    | TagKind::Author
                    | TagKind::Date
                    | TagKind::Table
                    | TagKind::Caption
                    | TagKind::Item
                    | TagKind::Theorem
                    | TagKind::Proof
                    | TagKind::Figure
                    | TagKind::BibItem => self.flush(),
                    _ => {}
                }
                if let Some(at) = self
                    .state
                    .stack
                    .iter()
                    .rposition(|(tag, _, _)| *tag == kind)
                {
                    let (_, style, align) = self.state.stack[at].clone();
                    self.state.stack.truncate(at);
                    self.state.style = style;
                    self.state.alignment = align;
                }
            }
            Event::Text(text) => self.append(&text, range, self.state.style.clone()),
            Event::Raw(text) => self.append(
                &text,
                range,
                InlineStyle {
                    code: true,
                    ..self.state.style.clone()
                },
            ),
            Event::Math(math) => {
                if math.display {
                    self.begin(BlockKind::Math, range.clone());
                }
                self.append(
                    &math.prepared,
                    range,
                    InlineStyle {
                        math: !math.display,
                        ..self.state.style.clone()
                    },
                );
                if math.display {
                    self.flush();
                }
            }
            Event::Reference(reference) => {
                self.append(&reference.display(), range, self.state.style.clone())
            }
            Event::Citation(citation) => {
                self.append(&citation.display(), range, self.state.style.clone())
            }
            Event::Image(image) => {
                self.begin(
                    BlockKind::Image {
                        destination: image.path.into_owned(),
                        title: String::new(),
                        attributes: None,
                    },
                    range,
                );
                self.flush();
            }
            Event::Include(include) => self.append(
                &format!(
                    "\\{}{{{}}}",
                    match include.kind {
                        lr::IncludeKind::Input => "input",
                        lr::IncludeKind::Include => "include",
                        lr::IncludeKind::Bibliography => "bibliography",
                        lr::IncludeKind::Listing => "lstinputlisting",
                    },
                    include.path
                ),
                range,
                InlineStyle {
                    code: true,
                    ..Default::default()
                },
            ),
            Event::Hint(lr::Hint::Align(align)) => {
                self.state.alignment = alignment(align);
                if let Some(block) = &mut self.block {
                    block.alignment = self.state.alignment;
                }
            }
            Event::LineBreak => self.append("\n", range, self.state.style.clone()),
            Event::TableRule if self.state.table_rows == 1 => {
                if let Some(block) = self.blocks.last_mut()
                    && let BlockKind::TableRow { header, .. } = &mut Arc::make_mut(block).kind
                {
                    *header = true;
                }
            }
            Event::TableRule => {}
            Event::Hint(_) => self.flush(),
        }
        if self.blocks.len() > MAX_PAGE_BLOCKS
            || self.bytes
                + self
                    .block
                    .as_ref()
                    .map_or(0, |b| b.text.len() + b.spans.len() * size_of::<Span>())
                > MAX_PAGE_BYTES
        {
            return Err(io::Error::other(
                "LaTeX page exceeds rendering limits; use Source or PDF Preview",
            ));
        }
        Ok(())
    }
    fn heading(&mut self, level: u8, number: Option<String>, range: Range<usize>) {
        self.begin(
            BlockKind::Heading {
                level,
                id: format!("tex-{}", range.start),
            },
            range.clone(),
        );
        if let Some(number) = number {
            self.append(&format!("{number}  "), range, self.state.style.clone());
        }
    }
}
