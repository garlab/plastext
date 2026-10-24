use crate::{Alignment, BLOCK_TEXT_LIMIT, Block, BlockKind, Document, Heading, InlineStyle, Span};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    sync::Arc,
};

const BATCH: usize = 256;
const MAX_DEPTH: usize = 128;

#[derive(Default, Debug)]
pub struct ParseSummary {
    pub headings: Vec<Heading>,
    pub warnings: Vec<String>,
    pub cancelled: bool,
}

pub fn parse(text: &str) -> Document {
    let mut blocks = Vec::new();
    let result = parse_stream(
        text,
        || false,
        |batch| {
            blocks.extend(batch);
            true
        },
    );
    Document {
        blocks,
        headings: result.headings,
        warnings: result.warnings,
    }
}

/// Emits bounded batches in source order. No recursive model or UI objects;
/// table rows and long code blocks become independent virtualized items.
/// The parser's reference-definition pass stays on the worker thread.
pub fn parse_stream(
    text: &str,
    cancelled: impl Fn() -> bool,
    mut emit: impl FnMut(Vec<Arc<Block>>) -> bool,
) -> ParseSummary {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_HEADING_ATTRIBUTES
        | Options::ENABLE_GFM
        | Options::ENABLE_DEFINITION_LIST
        | Options::ENABLE_MATH
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | Options::ENABLE_PLUSES_DELIMITED_METADATA_BLOCKS;
    let mut builder = Builder::new(&mut emit);
    let mut depth = 0usize;
    let mut inline = Vec::new();
    let mut style = InlineStyle::default();
    let mut alignments: Arc<[Alignment]> = Arc::from([]);
    let mut cell_start = 0;
    let mut image_depth = None;
    let mut in_comment = false;
    for (event, range) in Parser::new_ext(text, options).into_offset_iter() {
        if cancelled() || builder.stopped {
            builder.summary.cancelled = true;
            break;
        }
        match &event {
            Event::Start(_) => {
                depth += 1;
                if depth > MAX_DEPTH {
                    if builder.summary.warnings.is_empty() {
                        builder
                            .summary
                            .warnings
                            .push("Deeply nested content is flattened after 128 levels.".into());
                    }
                    continue;
                }
            }
            Event::End(_) if depth > MAX_DEPTH => {
                depth -= 1;
                continue;
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
        match event {
            Event::Start(Tag::Paragraph) => builder.start(BlockKind::Paragraph, range),
            Event::Start(Tag::Heading { level, id, .. }) => builder.start(
                BlockKind::Heading {
                    level: level as u8,
                    id: id.map(|s| s.into_string()).unwrap_or_default(),
                },
                range,
            ),
            Event::Start(Tag::CodeBlock(kind)) => {
                let language = match kind {
                    CodeBlockKind::Fenced(s) => {
                        s.split_whitespace().next().unwrap_or("").to_owned()
                    }
                    _ => String::new(),
                };
                builder.start(
                    if language == "math" {
                        BlockKind::Math
                    } else {
                        BlockKind::Code {
                            language,
                            first: true,
                        }
                    },
                    range,
                );
            }
            Event::Start(Tag::MetadataBlock(_)) => builder.start(BlockKind::FrontMatter, range),
            Event::Start(Tag::BlockQuote(kind)) => {
                builder.flush();
                builder.quotes.push(
                    kind.map(|s| format!("{s:?}").to_uppercase())
                        .unwrap_or_default(),
                );
            }
            Event::Start(Tag::List(first)) => {
                builder.flush();
                builder.lists.push(first);
            }
            Event::Start(Tag::Item) => {
                builder.flush();
                builder.marker = Some(match builder.lists.last_mut() {
                    Some(Some(number)) => {
                        let marker = format!("{number}.");
                        *number = number.saturating_add(1);
                        marker
                    }
                    _ => "•".into(),
                });
            }
            Event::Start(Tag::FootnoteDefinition(id)) => {
                builder.start(
                    BlockKind::Heading {
                        level: 6,
                        id: format!("fn-{id}"),
                    },
                    range.clone(),
                );
                builder.append(&format!("Footnote {id}"), range, &style);
                builder.flush();
            }
            Event::Start(Tag::DefinitionListTitle) => {
                builder.start(BlockKind::Paragraph, range);
                style.strong = true;
            }
            Event::Start(Tag::DefinitionListDefinition) => {
                builder.flush();
                builder.marker = Some("—".into());
            }
            Event::Start(Tag::Table(alignment)) => {
                builder.flush();
                alignments = alignment
                    .into_iter()
                    .map(|a| match a {
                        pulldown_cmark::Alignment::Center => Alignment::Center,
                        pulldown_cmark::Alignment::Right => Alignment::Right,
                        _ => Alignment::Left,
                    })
                    .collect();
            }
            Event::Start(Tag::TableHead) | Event::Start(Tag::TableRow) => {
                let header = matches!(event, Event::Start(Tag::TableHead));
                builder.start(
                    BlockKind::TableRow {
                        header,
                        cells: Vec::new(),
                        alignments: alignments.clone(),
                    },
                    range,
                );
            }
            Event::Start(Tag::TableCell) => {
                cell_start = builder.current.as_ref().map_or(0, |b| b.text.len());
            }
            Event::End(TagEnd::TableCell) => {
                if let Some(block) = &mut builder.current
                    && let BlockKind::TableRow { cells, .. } = &mut block.kind
                {
                    cells.push(cell_start..block.text.len());
                    if block.text.len() < BLOCK_TEXT_LIMIT {
                        block.text.push('\t');
                    }
                }
            }
            Event::Start(Tag::Image {
                dest_url, title, ..
            }) => {
                builder.start(
                    BlockKind::Image {
                        destination: dest_url.into_string(),
                        title: title.into_string(),
                    },
                    range,
                );
                image_depth = Some(depth);
            }
            Event::Start(Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link { .. }) => {
                inline.push(style.clone());
                match event {
                    Event::Start(Tag::Emphasis) => style.emphasis = true,
                    Event::Start(Tag::Strong) => style.strong = true,
                    Event::Start(Tag::Strikethrough) => style.strike = true,
                    Event::Start(Tag::Link { dest_url, .. }) => {
                        style.link = Some(dest_url.into_string().into())
                    }
                    _ => {}
                }
            }
            Event::End(
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link,
            ) => style = inline.pop().unwrap_or_default(),
            Event::End(TagEnd::Image) if image_depth == Some(depth + 1) => {
                builder.flush();
                image_depth = None;
            }
            Event::End(TagEnd::DefinitionListTitle) => {
                builder.flush();
                style.strong = false;
            }
            Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::CodeBlock
                | TagEnd::MetadataBlock(_)
                | TagEnd::TableHead
                | TagEnd::TableRow
                | TagEnd::Item
                | TagEnd::FootnoteDefinition
                | TagEnd::DefinitionListDefinition,
            ) => builder.flush(),
            Event::End(TagEnd::List(_)) => {
                builder.flush();
                builder.lists.pop();
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                builder.flush();
                builder.quotes.pop();
            }
            Event::Text(value) => builder.append(&value, range, &style),
            Event::Code(value) => {
                let mut code = style.clone();
                code.code = true;
                builder.append(&value, range, &code);
            }
            Event::InlineMath(value) => {
                let mut math = style.clone();
                math.math = true;
                math.code = true;
                builder.append(&value, range, &math);
            }
            Event::DisplayMath(value) => {
                builder.start(BlockKind::Math, range.clone());
                builder.append(&value, range, &style);
                builder.flush();
            }
            Event::SoftBreak => builder.append(" ", range, &style),
            Event::HardBreak => builder.append("\n", range, &style),
            Event::Rule => {
                builder.start(BlockKind::Rule, range);
                builder.flush();
            }
            Event::TaskListMarker(checked) => {
                builder.ensure(range);
                if let Some(block) = &mut builder.current {
                    block.task = Some(checked);
                    block.marker = None;
                }
            }
            Event::FootnoteReference(id) => {
                let mut footnote = style.clone();
                footnote.link = Some(format!("#fn-{id}").into());
                builder.append(&format!("[{id}]"), range, &footnote);
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                let html = strip_comments(&html, &mut in_comment);
                if html.is_empty() {
                    continue;
                }
                // Declarative subset; all unrecognized HTML remains visible source.
                match html.trim().to_ascii_lowercase().as_str() {
                    "<br>" | "<br/>" | "<br />" => builder.append("\n", range, &style),
                    "<b>" | "<strong>" => style.strong = true,
                    "</b>" | "</strong>" => style.strong = false,
                    "<i>" | "<em>" => style.emphasis = true,
                    "</i>" | "</em>" => style.emphasis = false,
                    "<code>" | "<kbd>" => style.code = true,
                    "</code>" | "</kbd>" => style.code = false,
                    "<hr>" | "<hr/>" | "<hr />" => {
                        builder.start(BlockKind::Rule, range);
                        builder.flush();
                    }
                    _ => {
                        let mut raw = style.clone();
                        raw.dim = true;
                        raw.code = true;
                        builder.append(&html, range, &raw);
                    }
                }
            }
            _ => {}
        }
    }
    builder.flush();
    if !builder.batch.is_empty() && !(builder.emit)(std::mem::take(&mut builder.batch)) {
        builder.summary.cancelled = true;
    }
    // Headings describe sections for the existing outline's scroll-spy.
    for i in 0..builder.summary.headings.len() {
        let level = builder.summary.headings[i].level;
        let end = builder.summary.headings[i + 1..]
            .iter()
            .find(|h| h.level <= level)
            .map_or(text.len(), |h| h.source.start);
        builder.summary.headings[i].source.end = end;
    }
    builder.summary
}

struct Builder<'a> {
    current: Option<Block>,
    batch: Vec<Arc<Block>>,
    emit: &'a mut dyn FnMut(Vec<Arc<Block>>) -> bool,
    stopped: bool,
    lists: Vec<Option<u64>>,
    quotes: Vec<String>,
    marker: Option<String>,
    ids: HashMap<String, usize>,
    used_ids: HashSet<String>,
    summary: ParseSummary,
}
impl<'a> Builder<'a> {
    fn new(emit: &'a mut dyn FnMut(Vec<Arc<Block>>) -> bool) -> Self {
        Self {
            current: None,
            batch: Vec::new(),
            emit,
            stopped: false,
            lists: Vec::new(),
            quotes: Vec::new(),
            marker: None,
            ids: HashMap::new(),
            used_ids: HashSet::new(),
            summary: ParseSummary::default(),
        }
    }
    fn ensure(&mut self, range: Range<usize>) {
        if self.current.is_none() {
            self.start(BlockKind::Paragraph, range);
        }
    }
    fn start(&mut self, kind: BlockKind, range: Range<usize>) {
        self.flush();
        let mut block = Block::new(kind, range);
        block.indent = self.lists.len().saturating_sub(1).min(16);
        block.marker = self.marker.take();
        block.quote = self.quotes.last().cloned();
        self.current = Some(block);
    }
    fn append(&mut self, mut value: &str, source: Range<usize>, style: &InlineStyle) {
        let mut consumed = 0;
        let exact = source.len() == value.len();
        while !value.is_empty() && !self.stopped {
            self.ensure(source.clone());
            let block = self.current.as_mut().unwrap();
            let available = BLOCK_TEXT_LIMIT.saturating_sub(block.text.len());
            if available < 4 {
                let kind = match &block.kind {
                    BlockKind::Code { language, .. } => BlockKind::Code {
                        language: language.clone(),
                        first: false,
                    },
                    BlockKind::Math => BlockKind::Math,
                    _ => BlockKind::Paragraph,
                };
                self.flatten_wide_table();
                self.start(kind, source.start + consumed..source.end);
                continue;
            }
            let mut take = available.min(value.len());
            while !value.is_char_boundary(take) {
                take -= 1;
            }
            // Keep individual code items short even when a fence is huge.
            if matches!(block.kind, BlockKind::Code { .. })
                && let Some((end, _)) = value[..take].match_indices('\n').nth(31)
            {
                take = end + 1;
            }
            let start = block.text.len();
            block.text.push_str(&value[..take]);
            let origin = if exact {
                source.start + consumed..source.start + consumed + take
            } else {
                source.clone()
            };
            block.source.end = block.source.end.max(origin.end);
            block.spans.push(Span {
                range: start..block.text.len(),
                source: origin,
                style: style.clone(),
            });
            value = &value[take..];
            consumed += take;
            if !value.is_empty() {
                let kind = match &block.kind {
                    BlockKind::Code { language, .. } => BlockKind::Code {
                        language: language.clone(),
                        first: false,
                    },
                    BlockKind::Math => BlockKind::Math,
                    _ => BlockKind::Paragraph,
                };
                self.flatten_wide_table();
                self.start(kind, source.start + consumed..source.end);
            }
        }
    }
    fn flatten_wide_table(&mut self) {
        if let Some(block) = &mut self.current
            && matches!(block.kind, BlockKind::TableRow { .. })
        {
            // Preserve all cell text when a row is too wide for one layout item.
            // Otherwise flushing mid-cell would leave that text outside cells[].
            block.kind = BlockKind::Paragraph;
            const WARNING: &str = "Oversized table rows are displayed as text.";
            if !self.summary.warnings.iter().any(|s| s == WARNING) {
                self.summary.warnings.push(WARNING.into());
            }
        }
    }
    fn flush(&mut self) {
        let Some(mut block) = self.current.take() else {
            return;
        };
        if block.text.is_empty()
            && !matches!(
                block.kind,
                BlockKind::Rule | BlockKind::Image { .. } | BlockKind::TableRow { .. }
            )
        {
            return;
        }
        if let BlockKind::Heading { level, id } = &mut block.kind {
            if id.is_empty() {
                *id = block
                    .text
                    .to_lowercase()
                    .chars()
                    .filter_map(|ch| {
                        if ch.is_alphanumeric() || ch == '_' || ch == '-' {
                            Some(ch)
                        } else if ch.is_whitespace() {
                            Some('-')
                        } else {
                            None
                        }
                    })
                    .collect();
            }
            let base = if id.is_empty() {
                "section".to_owned()
            } else {
                id.clone()
            };
            let count = self.ids.entry(base.clone()).or_default();
            loop {
                *id = if *count == 0 {
                    base.clone()
                } else {
                    format!("{base}-{count}")
                };
                *count += 1;
                if self.used_ids.insert(id.clone()) {
                    break;
                }
            }
            self.summary.headings.push(Heading {
                text: block.text.clone(),
                id: id.clone(),
                level: *level,
                source: block.source.clone(),
            });
        }
        self.batch.push(Arc::new(block));
        if self.batch.len() >= BATCH {
            self.stopped = !(self.emit)(std::mem::take(&mut self.batch));
        }
    }
}

fn strip_comments(mut html: &str, in_comment: &mut bool) -> String {
    let mut output = String::new();
    while !html.is_empty() {
        if *in_comment {
            let Some(end) = html.find("-->") else { break };
            html = &html[end + 3..];
            *in_comment = false;
        } else if let Some(start) = html.find("<!--") {
            output.push_str(&html[..start]);
            html = &html[start + 4..];
            *in_comment = true;
        } else {
            output.push_str(html);
            break;
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gfm_blocks_styles_source_maps_and_anchors() {
        let text = "---\ntitle: Demo\n---\n# Hello *world*\n\n[link](other.md#anchor) &amp; **bold** ~~gone~~\n\n- [x] done\n- next\n\n> [!WARNING]\n> Careful\n\n| Left | Right |\n| :--- | ---: |\n| one | two |\n\n# Hello world\n";
        let doc = parse(text);
        assert!(matches!(doc.blocks[0].kind, BlockKind::FrontMatter));
        assert_eq!(
            doc.headings
                .iter()
                .map(|h| h.id.as_str())
                .collect::<Vec<_>>(),
            ["hello-world", "hello-world-1"]
        );
        assert!(doc.blocks.iter().any(|b| b.task == Some(true)));
        assert!(
            doc.blocks
                .iter()
                .any(|b| b.quote.as_deref() == Some("WARNING"))
        );
        let rows: Vec<_> = doc
            .blocks
            .iter()
            .filter(|b| matches!(b.kind, BlockKind::TableRow { .. }))
            .collect();
        assert_eq!(rows.len(), 2);
        for block in &doc.blocks {
            assert!(block.source.end <= text.len());
            for span in &block.spans {
                assert!(text.is_char_boundary(span.source.start));
                assert!(span.source.end <= text.len());
                assert!(block.text.get(span.range.clone()).is_some());
            }
        }
        let paragraph = doc
            .blocks
            .iter()
            .find(|b| b.text.starts_with("link"))
            .unwrap();
        assert!(paragraph.text.contains("& bold gone"));
        assert_eq!(
            paragraph.source_at(paragraph.text.find('&').unwrap()),
            text.find("&amp;").unwrap()
        );
        assert_eq!(
            paragraph.spans[0].style.link.as_deref(),
            Some("other.md#anchor")
        );
    }
    #[test]
    fn large_fences_tables_and_unicode_have_bounded_layout_items() {
        let doc = parse(&format!(
            "```rust\n{}\n```\n",
            "let café = 42;\n".repeat(10000)
        ));
        assert!(doc.blocks.len() > 100);
        assert!(doc.blocks.iter().all(|b| b.text.len() <= BLOCK_TEXT_LIMIT));
        assert!(
            doc.blocks
                .iter()
                .all(|b| matches!(b.kind, BlockKind::Code { .. }))
        );
        let doc = parse(&format!(
            "| A | B |\n|---|---|\n{}",
            "| a | b |\n".repeat(10000)
        ));
        assert_eq!(doc.blocks.len(), 10001);
    }
    #[test]
    fn references_resolve_and_cancellation_stops_publication() {
        let doc = parse("[later][ref]\n\n[ref]: next.md\n");
        assert_eq!(
            doc.blocks[0].spans[0].style.link.as_deref(),
            Some("next.md")
        );
        let mut batches = 0;
        let result = parse_stream(
            &"text\n\n".repeat(1000),
            || false,
            |_| {
                batches += 1;
                false
            },
        );
        assert!(result.cancelled);
        assert_eq!(batches, 1);
        let result = parse_stream("hi", || true, |_| panic!("cancelled"));
        assert!(result.cancelled);
    }
    #[test]
    fn unsupported_html_stays_inert_and_comments_disappear() {
        let doc = parse(
            "<!-- hidden\nhidden again\n-->\n\n<script>alert(1)</script>\n\nText <strong>bold</strong><br>next",
        );
        assert!(!doc.blocks.iter().any(|b| b.text.contains("hidden")));
        assert!(doc.blocks.iter().any(|b| b.text.contains("<script>")));
        assert!(
            doc.blocks
                .iter()
                .flat_map(|b| &b.spans)
                .any(|s| s.style.strong)
        );
    }
    #[test]
    fn pathological_tables_nesting_and_slug_collisions() {
        let cell = "é".repeat(BLOCK_TEXT_LIMIT);
        let doc = parse(&format!("| A | B |\n|---|---|\n| {cell} | tail |\n"));
        assert!(doc.blocks.iter().all(|b| b.text.len() <= BLOCK_TEXT_LIMIT));
        let rendered: String = doc.blocks.iter().map(|b| b.text.as_str()).collect();
        assert_eq!(rendered.matches('é').count(), BLOCK_TEXT_LIMIT);
        assert!(rendered.contains("tail"));
        for b in &doc.blocks {
            if let BlockKind::TableRow { cells, .. } = &b.kind {
                assert!(cells.iter().all(|r| b.text.get(r.clone()).is_some()));
            }
        }
        let nested = parse(&format!("{}text", "> ".repeat(2000)));
        assert!(!nested.warnings.is_empty());
        assert!(nested.blocks.iter().any(|b| b.text.contains("text")));
        let doc = parse("# x\n# x\n# x-1\n# x {#x}\n");
        let ids: HashSet<_> = doc.headings.iter().map(|h| &h.id).collect();
        assert_eq!(ids.len(), 4);
    }
}
