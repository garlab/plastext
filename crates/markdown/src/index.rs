//! Source-backed Markdown pages. No parsed page is retained by the index.
use crate::DocumentSource;
use crate::parser::{HeadingIds, References, options, parse_context};
use crate::{Block, BlockKind, Document, Heading, InlineStyle, Span};
use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag};
use std::{
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
    io,
    ops::Range,
    sync::Arc,
};
use unicase::UniCase;

pub const PAGE_BYTES: usize = 64 * 1024;
pub const PARSE_WINDOW_BYTES: usize = 256 * 1024;
pub const INDEX_BUDGET: usize = 64 * 1024 * 1024;
const GLOBAL_BUDGET: usize = 16 * 1024 * 1024;
const NO_SECTION: u32 = u32::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum BlockType {
    Paragraph,
    Heading,
    Code,
    Math,
    Metadata,
    MetadataRow,
    Details,
    TableHead,
    TableRow,
    Other,
}

/// 32 bytes on a 64-bit target, including source mapping and page lookup.
#[derive(Clone, Debug)]
pub struct BlockEntry {
    pub source: Range<usize>,
    pub page: u32,
    pub local: u32,
    pub kind: BlockType,
    pub code_first: bool,
    pub default_open: bool,
    section: u32,
}
impl BlockEntry {
    /// Block index of the enclosing collapsible section. Using an index keeps
    /// records compact even when a details header occurs past 4 GiB of source.
    pub fn section(&self) -> Option<usize> {
        (self.section != NO_SECTION).then_some(self.section as usize)
    }
    fn parents(blocks: &[Self], block: usize) -> impl Iterator<Item = usize> + '_ {
        std::iter::successors(blocks.get(block).and_then(Self::section), |index| {
            blocks
                .get(*index)?
                .section()
                .filter(|parent| parent < index)
        })
        .take(128)
    }
    /// Absolute source offsets of the enclosing headers, innermost first.
    pub fn ancestors(blocks: &[Self], block: usize) -> impl Iterator<Item = usize> + '_ {
        Self::parents(blocks, block)
            .filter_map(|index| blocks.get(index).map(|block| block.source.start))
    }
    pub fn section_open(blocks: &[Self], section: usize, toggled: &HashSet<usize>) -> bool {
        let header = blocks
            .partition_point(|b| b.source.start <= section)
            .saturating_sub(1);
        blocks.get(header).is_some_and(|b| b.default_open) ^ toggled.contains(&section)
    }
    pub fn visible(blocks: &[Self], block: usize, toggled: &HashSet<usize>) -> bool {
        Self::parents(blocks, block).all(|index| {
            blocks
                .get(index)
                .is_some_and(|header| header.default_open ^ toggled.contains(&header.source.start))
        })
    }
    fn new(block: &Block, page: usize, local: usize, previous: &[Self]) -> Self {
        Self {
            source: block.source.clone(),
            page: page as u32,
            local: local as u32,
            kind: match block.kind {
                BlockKind::Paragraph => BlockType::Paragraph,
                BlockKind::Details { .. } => BlockType::Details,
                BlockKind::Heading { .. } => BlockType::Heading,
                BlockKind::Code { .. } => BlockType::Code,
                BlockKind::Math => BlockType::Math,
                BlockKind::TableRow { header: true, .. } => BlockType::TableHead,
                BlockKind::TableRow { header: false, .. } => BlockType::TableRow,
                BlockKind::FrontMatter { .. } => BlockType::Metadata,
                BlockKind::FrontMatterRow { .. } | BlockKind::FrontMatterSource { .. } => {
                    BlockType::MetadataRow
                }
                _ => BlockType::Other,
            },
            code_first: matches!(block.kind, BlockKind::Code { first: true, .. }),
            default_open: matches!(block.kind, BlockKind::Details { open: true }),
            section: block
                .details
                .or_else(|| block.kind.metadata_section())
                .and_then(|offset| {
                    previous
                        .partition_point(|b| b.source.start <= offset)
                        .checked_sub(1)
                })
                .map_or(NO_SECTION, |n| n as u32),
        }
    }
}

#[derive(Clone, Debug)]
enum Mode {
    Normal,
    Metadata,
    Table { header: Arc<str>, continued: bool },
    Code { opening: Arc<str>, continued: bool },
    Source,
}
impl Mode {
    fn prefix(&self) -> &str {
        match self {
            Self::Table {
                header,
                continued: true,
            } => header,
            Self::Code {
                opening,
                continued: true,
            } => opening,
            _ => "",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Page {
    pub source: Range<usize>,
    pub blocks: Range<usize>,
    mode: Mode,
    fingerprint: u64,
    html: Option<Arc<crate::parser::html::HtmlState>>,
}

#[derive(Default, Debug)]
pub struct DocumentIndex {
    pub blocks: Vec<BlockEntry>,
    pub headings: Vec<Heading>,
    pub warnings: Vec<String>,
    pub pages: Vec<Page>,
    pub equation_starts: HashMap<usize, u32>,
    pub macros: crate::MacroHistory,
    references: References,
    footnotes: HashSet<UniCase<String>>,
    html_context_bytes: usize,
}

impl DocumentIndex {
    /// Used only for the bounded first-screen preview while indexing runs.
    pub fn initial(document: &Document, bytes: usize) -> Self {
        let mut macros = crate::MacroHistory::default();
        for block in &document.blocks {
            macros.collect(block);
        }
        let mut blocks = Vec::with_capacity(document.blocks.len());
        for (i, block) in document.blocks.iter().enumerate() {
            blocks.push(BlockEntry::new(block, 0, i, &blocks));
        }
        Self {
            macros,
            blocks,
            headings: document.headings.clone(),
            warnings: document.warnings.clone(),
            pages: vec![Page {
                source: 0..bytes,
                blocks: 0..document.blocks.len(),
                mode: Mode::Normal,
                fingerprint: 0,
                html: None,
            }],
            ..Self::default()
        }
    }

    /// Two bounded passes: collect source pages/global definitions, then index
    /// rendered blocks. A page's temporary text and spans die before the next.
    pub fn build(
        source: &(impl DocumentSource + ?Sized),
        cancelled: impl Fn() -> bool,
        equations: impl Fn(&str, &crate::MacroContext) -> u32,
    ) -> io::Result<Self> {
        let mut index = Self::default();
        let mut at = 0;
        let mut carry = None;
        let mut global_bytes = 0;
        while at < source.byte_len() as usize {
            check_cancel(&cancelled)?;
            let (page, next) = next_page(source, at, carry.take())?;
            if matches!(page.mode, Mode::Source) && index.warnings.is_empty() {
                index.warnings.push("Markdown constructs exceeding parsing limits are shown as source; large tables and code fences with bounded headers remain rendered.".into());
            }
            let raw = read(source, page.source.clone())?;
            let text = format!("{}{}", page.mode.prefix(), raw);
            if !matches!(page.mode, Mode::Source) {
                let prepared = crate::math_blocks::prepare(&text, page.source.start == 0, false);
                let parser = Parser::new_ext(&prepared, options(page.source.start == 0));
                let mut definitions: Vec<_> = parser.reference_definitions().iter().collect();
                definitions.sort_unstable_by_key(|(_, def)| def.span.start);
                for (label, def) in definitions {
                    let key = UniCase::new(label.to_owned());
                    if let std::collections::hash_map::Entry::Vacant(entry) =
                        index.references.entry(key)
                    {
                        global_bytes += label.len()
                            + def.dest.len()
                            + def.title.as_ref().map_or(0, |s| s.len())
                            + 128;
                        check_global(global_bytes)?;
                        entry.insert((
                            Arc::from(def.dest.as_ref()),
                            Arc::from(def.title.as_deref().unwrap_or("")),
                        ));
                    }
                }
                let footnotes_present = text.contains("[^");
                for event in parser.take_while(|_| footnotes_present) {
                    if let Event::Start(Tag::FootnoteDefinition(id)) = event {
                        let key = UniCase::new(id.into_string());
                        if index.footnotes.insert(key.clone()) {
                            global_bytes += key.as_ref().len() + 64;
                            check_global(global_bytes)?;
                        }
                    }
                }
            }
            at = page.source.end;
            index.pages.push(Page {
                fingerprint: fingerprint(raw.as_bytes()),
                ..page
            });
            carry = next;
            if index.retained_bytes() > INDEX_BUDGET {
                return Err(limit_error());
            }
        }
        let mut ids = HeadingIds::default();
        let mut equation = 1u32;
        let mut html = crate::parser::html::HtmlState::default();
        for page_id in 0..index.pages.len() {
            check_cancel(&cancelled)?;
            index.pages[page_id].html = (!html.is_empty()).then(|| {
                index.html_context_bytes += size_of::<crate::parser::html::HtmlState>()
                    + 2 * size_of::<usize>()
                    + html.retained_bytes();
                Arc::new(html.clone())
            });
            let (document, next_html) = index.read_page(source, page_id, &cancelled, &mut ids)?;
            html = next_html;
            let first = index.blocks.len();
            for (local, block) in document.blocks.iter().enumerate() {
                if matches!(block.kind, BlockKind::Math) {
                    let count = equations(&block.text, &index.macros.at(block.source.start));
                    if count > 0 {
                        index.equation_starts.insert(index.blocks.len(), equation);
                        equation = equation.saturating_add(count);
                    }
                }
                index.macros.collect(block);
                index
                    .blocks
                    .push(BlockEntry::new(block, page_id, local, &index.blocks));
            }
            index.headings.extend(document.headings);
            for warning in document.warnings {
                if !index.warnings.contains(&warning) {
                    index.warnings.push(warning);
                }
            }
            index.pages[page_id].blocks = first..index.blocks.len();
            if index.retained_bytes() > INDEX_BUDGET {
                return Err(limit_error());
            }
        }
        // A stack makes heading section ends linear even for deeply nested outlines.
        let mut stack: Vec<usize> = Vec::new();
        for i in 0..index.headings.len() {
            while stack
                .last()
                .is_some_and(|&previous| index.headings[previous].level >= index.headings[i].level)
            {
                let previous = stack.pop().unwrap();
                index.headings[previous].source.end = index.headings[i].source.start;
            }
            stack.push(i);
        }
        for i in stack {
            index.headings[i].source.end = source.byte_len() as usize;
        }
        index.blocks.shrink_to_fit();
        index.pages.shrink_to_fit();
        index.headings.shrink_to_fit();
        Ok(index)
    }

    pub fn block_at(&self, byte: usize) -> usize {
        self.blocks
            .partition_point(|b| b.source.start <= byte)
            .saturating_sub(1)
    }
    pub fn anchor(&self, id: &str) -> Option<usize> {
        self.headings
            .iter()
            .find(|h| h.id == id)
            .map(|h| h.source.start)
    }
    pub fn retained_bytes(&self) -> usize {
        self.blocks.capacity() * size_of::<BlockEntry>()
            + self.pages.capacity() * size_of::<Page>()
            + self.html_context_bytes
            + self.headings.capacity() * size_of::<Heading>()
            + self
                .headings
                .iter()
                .map(|h| h.text.capacity() + h.id.capacity())
                .sum::<usize>()
            + self
                .references
                .iter()
                .map(|(key, (url, title))| key.as_ref().len() + url.len() + title.len() + 128)
                .sum::<usize>()
            + self
                .footnotes
                .iter()
                .map(|key| key.as_ref().len() + 64)
                .sum::<usize>()
            + self.equation_starts.capacity() * 24
            + self.macros.retained_bytes()
    }
    pub fn parse_page(
        &self,
        source: &(impl DocumentSource + ?Sized),
        page: usize,
        cancelled: impl Fn() -> bool,
    ) -> io::Result<Vec<Arc<Block>>> {
        let (mut document, _) =
            self.read_page(source, page, &cancelled, &mut HeadingIds::default())?;
        let expected = &self.pages[page];
        if document.blocks.len() != expected.blocks.len() {
            return Err(io::Error::other(
                "Markdown changed while it was being read; reopen the file",
            ));
        }
        for block in &mut document.blocks {
            let at = block.source.start;
            if let BlockKind::Heading { id, .. } = &mut Arc::make_mut(block).kind {
                let h = self.headings.partition_point(|h| h.source.start < at);
                if let Some(heading) = self.headings.get(h).filter(|h| h.source.start == at) {
                    *id = heading.id.clone();
                }
            }
        }
        Ok(document.blocks)
    }
    fn read_page(
        &self,
        source: &(impl DocumentSource + ?Sized),
        page: usize,
        cancelled: &impl Fn() -> bool,
        ids: &mut HeadingIds,
    ) -> io::Result<(Document, crate::parser::html::HtmlState)> {
        check_cancel(cancelled)?;
        let page = &self.pages[page];
        let raw = read(source, page.source.clone())?;
        if fingerprint(raw.as_bytes()) != page.fingerprint {
            return Err(io::Error::other(
                "Markdown changed while it was being read; reopen the file",
            ));
        }
        if matches!(page.mode, Mode::Source) {
            let mut document = source_document(&raw, page.source.start);
            let html = page.html.as_deref().cloned().unwrap_or_default();
            for block in &mut document.blocks {
                Arc::make_mut(block).details = html.section();
            }
            return Ok((document, html));
        }
        let prefix = page.mode.prefix();
        let mut text = format!("{prefix}{raw}");
        let mut prefix_len = prefix.len();
        // Use document-wide first-definition-wins semantics, including when a
        // later duplicate definition occurs in this page.
        let prepared = crate::math_blocks::prepare(&text, page.source.start == 0, false);
        let ranges: Vec<_> = Parser::new_ext(&prepared, options(page.source.start == 0))
            .reference_definitions()
            .iter()
            .map(|(_, def)| def.span.clone())
            .collect();
        for range in ranges {
            // Replacing complete UTF-8 characters with ASCII spaces preserves byte offsets.
            let blank: String = text[range.clone()]
                .bytes()
                .map(|b| {
                    if b == b'\n' || b == b'\r' {
                        b as char
                    } else {
                        ' '
                    }
                })
                .collect();
            text.replace_range(range, &blank);
        }
        if !matches!(page.mode, Mode::Code { .. } | Mode::Metadata) && !self.footnotes.is_empty() {
            let mut added = HashSet::new();
            let mut definitions = String::new();
            let prepared = crate::math_blocks::prepare(&text, page.source.start == 0, false);
            let local: HashSet<_> = Parser::new_ext(&prepared, options(page.source.start == 0))
                .filter_map(|event| match event {
                    Event::Start(Tag::FootnoteDefinition(id)) => {
                        Some(UniCase::new(id.into_string()))
                    }
                    _ => None,
                })
                .collect();
            for (at, _) in raw.match_indices("[^") {
                let tail = &raw[at + 2..];
                if let Some(end) = tail.find(']').filter(|&end| end < 1000) {
                    let label = &tail[..end];
                    let key = UniCase::new(label.to_owned());
                    if !label.contains(['\n', '\r'])
                        && self.footnotes.contains(&key)
                        && !local.contains(&key)
                        && added.insert(key)
                    {
                        definitions.push_str(&format!("[^{label}]: placeholder\n\n"));
                    }
                }
            }
            prefix_len += definitions.len();
            text.insert_str(0, &definitions);
        }
        let real_end = text.len();
        let mut blocks = Vec::new();
        let mut summary = parse_context(
            &text,
            cancelled,
            &mut |batch| {
                blocks.extend(batch);
                true
            },
            ids,
            Some(&self.references),
            page.source.start == 0,
            prefix_len..real_end,
            Some((
                page.html
                    .as_deref()
                    .unwrap_or(&crate::parser::html::HtmlState::default()),
                page.source.start as isize - prefix_len as isize,
            )),
        );
        check_cancel(cancelled)?;
        blocks.retain(|b| b.source.start < real_end && b.source.end > prefix_len);
        summary
            .headings
            .retain(|h| h.source.start < real_end && h.source.start >= prefix_len);
        for block in &mut blocks {
            let block = Arc::make_mut(block);
            shift(
                &mut block.source,
                prefix_len,
                page.source.start,
                page.source.end,
            );
            for span in &mut block.spans {
                shift(
                    &mut span.source,
                    prefix_len,
                    page.source.start,
                    page.source.end,
                );
            }
            match &mut block.kind {
                BlockKind::FrontMatterRow { section, .. }
                | BlockKind::FrontMatterSource { section } => {
                    *section += page.source.start;
                }
                BlockKind::Code { first, .. }
                    if matches!(
                        page.mode,
                        Mode::Code {
                            continued: true,
                            ..
                        }
                    ) =>
                {
                    *first = false;
                }
                _ => {}
            }
        }
        for heading in &mut summary.headings {
            shift(
                &mut heading.source,
                prefix_len,
                page.source.start,
                page.source.end,
            );
        }
        Ok((
            Document {
                blocks,
                headings: summary.headings,
                warnings: summary.warnings,
            },
            summary.html,
        ))
    }

    /// Streams rendered-text matches without touching the viewport cache.
    pub fn search(
        &self,
        source: &(impl DocumentSource + ?Sized),
        query: &str,
        limit: usize,
        cancelled: impl Fn() -> bool,
        mut emit: impl FnMut(Vec<crate::RenderedMatch>) -> bool,
    ) -> io::Result<()> {
        if query.is_empty() || limit == 0 {
            return Ok(());
        }
        let mut remaining = limit;
        for (id, page) in self.pages.iter().enumerate() {
            check_cancel(&cancelled)?;
            let blocks = self.parse_page(source, id, &cancelled)?;
            let hits = crate::find_rendered(&blocks, page.blocks.start, query, remaining);
            remaining -= hits.len();
            if !hits.is_empty() && !emit(hits) {
                break;
            }
            if remaining == 0 {
                break;
            }
        }
        Ok(())
    }
}

fn shift(range: &mut Range<usize>, prefix: usize, start: usize, end: usize) {
    range.start = (range.start.saturating_sub(prefix) + start).min(end);
    range.end = (range.end.saturating_sub(prefix) + start).min(end);
}
fn fingerprint(bytes: &[u8]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}
fn check_cancel(cancelled: &impl Fn() -> bool) -> io::Result<()> {
    if cancelled() {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Markdown work cancelled",
        ))
    } else {
        Ok(())
    }
}
fn limit_error() -> io::Error {
    io::Error::other("Markdown index exceeds its 64 MiB budget; use Source view")
}
fn check_global(bytes: usize) -> io::Result<()> {
    if bytes > GLOBAL_BUDGET {
        Err(io::Error::other(
            "Markdown reference definitions exceed their 16 MiB budget; use Source view",
        ))
    } else {
        Ok(())
    }
}
fn read(source: &(impl DocumentSource + ?Sized), range: Range<usize>) -> io::Result<String> {
    let mut bytes = Vec::with_capacity(range.len());
    source.read_range(range.start as u64..range.end as u64, &mut bytes)?;
    if bytes.len() != range.len() {
        return Err(io::Error::other(
            "Markdown changed while it was being read; reopen the file",
        ));
    }
    String::from_utf8(bytes)
        .map_err(|_| io::Error::other("Markdown preview requires UTF-8; use Source view"))
}

fn source_document(text: &str, offset: usize) -> Document {
    let mut document = Document::default();
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + crate::BLOCK_TEXT_LIMIT).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        if let Some((at, _)) = text[start..end].match_indices('\n').nth(31) {
            end = start + at + 1;
        }
        let mut block = Block::new(
            BlockKind::Code {
                language: String::new(),
                first: start == 0,
            },
            offset + start..offset + end,
        );
        block.text = text[start..end].into();
        block.spans.push(Span {
            range: 0..end - start,
            source: block.source.clone(),
            style: InlineStyle::default(),
        });
        document.blocks.push(Arc::new(block));
        start = end;
    }
    document
}

#[derive(Clone)]
enum UnitKind {
    Normal,
    Metadata,
    Table,
    Code(bool),
}
struct Unit {
    source: Range<usize>,
    kind: UnitKind,
}
fn units(text: &str, metadata: bool) -> Vec<Unit> {
    let prepared = crate::math_blocks::prepare(text, metadata, true);
    let parser = Parser::new_ext(&prepared, options(metadata));
    let mut units: Vec<_> = parser
        .reference_definitions()
        .iter()
        .map(|(_, def)| Unit {
            source: def.span.clone(),
            kind: UnitKind::Normal,
        })
        .collect();
    let mut depth = 0usize;
    for (event, range) in parser.into_offset_iter() {
        match event {
            Event::Start(tag) => {
                if depth == 0 {
                    units.push(Unit {
                        source: range,
                        kind: match tag {
                            Tag::MetadataBlock(_) => UnitKind::Metadata,
                            Tag::Table(_) => UnitKind::Table,
                            Tag::CodeBlock(CodeBlockKind::Fenced(_)) => UnitKind::Code(true),
                            Tag::CodeBlock(CodeBlockKind::Indented) => UnitKind::Code(false),
                            _ => UnitKind::Normal,
                        },
                    });
                }
                depth += 1;
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            _ if depth == 0 => units.push(Unit {
                source: range,
                kind: UnitKind::Normal,
            }),
            _ => {}
        }
    }
    units.sort_unstable_by_key(|unit| unit.source.start);
    units
}

fn next_page(
    source: &(impl DocumentSource + ?Sized),
    at: usize,
    carry: Option<Mode>,
) -> io::Result<(Page, Option<Mode>)> {
    next_page_window(source, at, carry, PAGE_BYTES + 4096)
}

fn next_page_window(
    source: &(impl DocumentSource + ?Sized),
    at: usize,
    carry: Option<Mode>,
    window: usize,
) -> io::Result<(Page, Option<Mode>)> {
    let end = (at + window).min(source.byte_len() as usize);
    // Read a few extra bytes so a window never splits a UTF-8 scalar.
    let mut bytes = Vec::new();
    source.read_range(at as u64..end as u64, &mut bytes)?;
    let valid = match std::str::from_utf8(&bytes) {
        Ok(text) => text.len(),
        Err(error) if error.error_len().is_none() && end < source.byte_len() as usize => {
            error.valid_up_to()
        }
        Err(_) => {
            return Err(io::Error::other(
                "Markdown preview requires UTF-8; use Source view",
            ));
        }
    };
    let mut raw = std::str::from_utf8(&bytes[..valid]).unwrap();
    let eof = at + raw.len() == source.byte_len() as usize;
    if !eof && let Some(newline) = raw.rfind('\n') {
        raw = &raw[..newline + 1];
    }
    let make = |len: usize, mode: Mode, next| {
        (
            Page {
                source: at..at + len,
                blocks: 0..0,
                mode,
                fingerprint: 0,
                html: None,
            },
            next,
        )
    };
    if matches!(carry, Some(Mode::Source)) {
        let blank = raw
            .find("\n\n")
            .map(|n| n + 2)
            .or_else(|| raw.find("\r\n\r\n").map(|n| n + 4));
        let cut = line_cut(raw, blank.unwrap_or(PAGE_BYTES).min(PAGE_BYTES));
        return Ok(make(
            cut,
            Mode::Source,
            (blank.is_none_or(|n| n > cut) && !eof).then_some(Mode::Source),
        ));
    }
    let prefix = carry.as_ref().map_or("", Mode::prefix);
    let text = format!("{prefix}{raw}");
    let all = units(&text, at == 0);
    if let Some(mode) = carry {
        let Some(first) = all.first() else {
            return Ok(make(raw.len(), Mode::Normal, None));
        };
        let matches = matches!(
            (&mode, &first.kind),
            (Mode::Table { .. }, UnitKind::Table) | (Mode::Code { .. }, UnitKind::Code(_))
        );
        let length = first
            .source
            .end
            .saturating_sub(mode.prefix().len())
            .min(raw.len());
        if !matches || length == 0 {
            return next_page(source, at, None);
        }
        let complete = eof
            || first.source.end < text.trim_end_matches(['\r', '\n']).len()
            || matches!(&mode, Mode::Code { opening, .. } if fence_closed(opening, &raw[..length]));
        let cut = if length <= PAGE_BYTES && complete {
            length
        } else {
            line_cut(raw, length.min(PAGE_BYTES))
        };
        let next = (!(complete && cut >= length)).then_some(mode.clone());
        return Ok(make(cut, mode, next));
    }
    let Some(first) = all.first() else {
        return Ok(make(raw.len(), Mode::Normal, None));
    };
    if matches!(first.kind, UnitKind::Metadata) {
        if first.source.end < raw.len() || eof {
            return Ok(make(first.source.end, Mode::Metadata, None));
        }
        if window < PARSE_WINDOW_BYTES {
            return next_page_window(source, at, None, PARSE_WINDOW_BYTES);
        }
        return Ok(make(
            line_cut(raw, PAGE_BYTES),
            Mode::Source,
            Some(Mode::Source),
        ));
    }
    // Leave the last construct for the next window: a later line may extend it,
    // turn a paragraph into a setext heading/table, or add a link title.
    let stable = if eof {
        all.len()
    } else {
        all.len().saturating_sub(1)
    };
    let mut cut = 0;
    for unit in all.iter().take(stable) {
        if unit.source.end > PAGE_BYTES {
            break;
        }
        cut = unit.source.end;
    }
    if cut > 0 {
        return Ok(make(cut, Mode::Normal, None));
    }
    if first.source.start > 0 && !raw[..first.source.start].trim().is_empty() {
        return Ok(make(first.source.start, Mode::Normal, None));
    }
    match first.kind {
        UnitKind::Table | UnitKind::Code(_) => {
            let body = &raw[first.source.start..];
            let mode = match first.kind {
                UnitKind::Table => {
                    let header_end = body
                        .match_indices('\n')
                        .nth(1)
                        .map_or(body.len(), |(n, _)| n + 1);
                    if header_end > PAGE_BYTES {
                        return Ok(make(
                            line_cut(raw, PAGE_BYTES),
                            Mode::Source,
                            Some(Mode::Source),
                        ));
                    }
                    Mode::Table {
                        header: Arc::from(&body[..header_end]),
                        continued: false,
                    }
                }
                UnitKind::Code(fenced) => {
                    let end = body.find('\n').map_or(body.len(), |n| n + 1);
                    if fenced && end > PAGE_BYTES {
                        return Ok(make(
                            line_cut(raw, PAGE_BYTES),
                            Mode::Source,
                            Some(Mode::Source),
                        ));
                    }
                    Mode::Code {
                        opening: if fenced {
                            Arc::from(&body[..end])
                        } else {
                            Arc::from("")
                        },
                        continued: false,
                    }
                }
                _ => unreachable!(),
            };
            let complete = eof
                || stable > 0
                || matches!(&mode, Mode::Code { opening, .. } if body.lines().count() > 1 && fence_closed(opening, body));
            let cut = if complete && first.source.end <= PAGE_BYTES {
                first.source.end
            } else {
                line_cut(raw, first.source.end.min(PAGE_BYTES))
            };
            let next = if complete && cut >= first.source.end {
                None
            } else {
                Some(match &mode {
                    Mode::Table { header, .. } => Mode::Table {
                        header: header.clone(),
                        continued: true,
                    },
                    Mode::Code { opening, .. } => Mode::Code {
                        opening: opening.clone(),
                        continued: true,
                    },
                    _ => unreachable!(),
                })
            };
            Ok(make(cut, mode, next))
        }
        UnitKind::Normal if stable > 0 || eof => Ok(make(first.source.end, Mode::Normal, None)),
        _ if window < PARSE_WINDOW_BYTES => next_page_window(source, at, None, PARSE_WINDOW_BYTES),
        _ => {
            let cut = line_cut(raw, PAGE_BYTES);
            Ok(make(cut, Mode::Source, Some(Mode::Source)))
        }
    }
}

fn line_cut(text: &str, limit: usize) -> usize {
    let mut end = limit.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].rfind('\n').map_or(end, |n| n + 1)
}
fn fence_closed(opening: &str, text: &str) -> bool {
    let opening = opening.trim_start();
    let Some(marker @ (b'`' | b'~')) = opening.as_bytes().first().copied() else {
        return false;
    };
    let count = opening.bytes().take_while(|&b| b == marker).count();
    let last = text
        .trim_end_matches(['\r', '\n'])
        .rsplit('\n')
        .next()
        .unwrap_or("");
    let stripped = last.trim_start_matches(' ');
    let n = stripped.bytes().take_while(|&b| b == marker).count();
    last.len() - stripped.len() <= 3 && n >= count && stripped[n..].trim().is_empty()
}
