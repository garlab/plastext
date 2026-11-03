//! Declarative HTML adapter. Tokens become ordinary native blocks/spans. There
//! is no DOM, CSS evaluator, script engine, or resource loader in this module.
use super::*;
use crate::ImageAttributes;
use html5gum::{DefaultEmitter, StartTag, Token, Tokenizer};
use std::borrow::Cow;

const MAX_HTML_BYTES: usize = 256 * 1024;
const MAX_TOKENS: usize = 32_768;
const MAX_ATTRIBUTE: usize = 16 * 1024;
const MAX_CONTEXT_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct HtmlState {
    frames: Vec<Frame>,
    raw: Option<String>,
    raw_depth: usize,
    picture: Option<ImageAttributes>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Frame {
    name: String,
    style: InlineStyle,
    align: Alignment,
    section: Option<usize>,
}
impl HtmlState {
    pub(crate) fn retained_bytes(&self) -> usize {
        self.frames.capacity() * size_of::<Frame>()
            + self
                .frames
                .iter()
                .map(|f| f.name.capacity() + f.style.link.as_ref().map_or(0, |l| l.len()))
                .sum::<usize>()
            + self.raw.as_ref().map_or(0, String::capacity)
            + self
                .picture
                .as_ref()
                .map_or(0, ImageAttributes::retained_bytes)
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.frames.is_empty() && self.raw.is_none() && self.picture.is_none()
    }
    pub(crate) fn section(&self) -> Option<usize> {
        self.frames.last().and_then(|f| f.section)
    }
    pub(super) fn alignment(&self) -> Alignment {
        self.frames.last().map_or(Alignment::Left, |f| f.align)
    }
    pub(super) fn style<'a>(&self, markdown: &'a InlineStyle) -> Cow<'a, InlineStyle> {
        let Some(frame) = self.frames.last() else {
            return Cow::Borrowed(markdown);
        };
        let mut style = markdown.clone();
        style.strong |= frame.style.strong;
        style.emphasis |= frame.style.emphasis;
        style.code |= frame.style.code;
        style.mark |= frame.style.mark;
        if frame.style.script != 0 {
            style.script = frame.style.script;
        }
        if frame.style.link.is_some() {
            style.link = frame.style.link.clone();
        }
        Cow::Owned(style)
    }
    fn push(&mut self, name: &str) {
        self.frames.push(Frame {
            name: name.into(),
            style: self.style(&InlineStyle::default()).into_owned(),
            align: self.alignment(),
            section: self.section(),
        });
    }
    fn close(&mut self, name: &str) {
        if let Some(index) = self.frames.iter().rposition(|f| f.name == name) {
            self.frames.truncate(index);
        }
    }
}

impl Builder<'_> {
    pub(super) fn html_fragment(
        &mut self,
        source: &str,
        base: usize,
        style: &InlineStyle,
        cancelled: &impl Fn() -> bool,
    ) {
        if source.len() > MAX_HTML_BYTES {
            self.html_limit();
            self.raw_html(source, base..base + source.len(), style);
            return;
        }
        let mut emitter = DefaultEmitter::<usize>::new_with_span();
        emitter.naively_switch_states(true);
        let tokenizer = Tokenizer::new_with_emitter(source, emitter);
        let mut end = 0;
        for (count, token) in tokenizer.enumerate() {
            if cancelled() || self.stopped {
                self.summary.cancelled = true;
                break;
            }
            if count >= MAX_TOKENS {
                self.html_limit();
                self.raw_html(&source[end..], base + end..base + source.len(), style);
                break;
            }
            let Ok(token) = token;
            let span = match &token {
                Token::StartTag(t) => t.span,
                Token::EndTag(t) => t.span,
                Token::String(t) | Token::Comment(t) => t.span,
                Token::Doctype(t) => t.span,
                Token::Error(_) => continue,
            };
            // Tokenizer spans include entity escapes and are UTF-8 byte offsets.
            let local = span.start.min(source.len())..span.end.min(source.len());
            let Some(raw) = source.get(local.clone()) else {
                continue;
            };
            end = local.end;
            let range = base + local.start..base + local.end;
            if let Some(name) = &self.html.raw {
                match &token {
                    Token::StartTag(tag) if tag.name.as_ref() == name.as_bytes() => {
                        self.html.raw_depth = self.html.raw_depth.saturating_add(1)
                    }
                    Token::EndTag(tag) if tag.name.as_ref() == name.as_bytes() => {
                        self.html.raw_depth = self.html.raw_depth.saturating_sub(1);
                        if self.html.raw_depth == 0 {
                            self.html.raw = None;
                        }
                    }
                    _ => {}
                }
                self.raw_html(raw, range, style);
                continue;
            }
            match token {
                Token::StartTag(tag) => self.html_start(tag, raw, range, style),
                Token::EndTag(tag) => {
                    let name = String::from_utf8_lossy(&tag.name);
                    if matches!(name.as_ref(), "p" | "div" | "summary" | "details") {
                        self.flush();
                    }
                    if name == "picture" {
                        self.html.picture = None;
                    }
                    if supported(&name) {
                        self.html.close(&name);
                    } else {
                        self.raw_html(raw, range, style);
                    }
                }
                Token::String(text) => {
                    let decoded = String::from_utf8_lossy(&text.value);
                    // HTML collapses ASCII whitespace; Markdown outside HTML is
                    // still parsed by pulldown, including inside details/divs.
                    let mut value = String::new();
                    let mut space = self
                        .current
                        .as_ref()
                        .is_none_or(|b| b.text.is_empty() || b.text.ends_with(char::is_whitespace));
                    for ch in decoded.chars() {
                        if matches!(ch, ' ' | '\t' | '\r' | '\n' | '\x0c') {
                            if !space {
                                value.push(' ');
                            }
                            space = true;
                        } else {
                            value.push(ch);
                            space = false;
                        }
                    }
                    if !value.is_empty() {
                        self.append(&value, range, style);
                    }
                }
                Token::Comment(_) => {}
                _ => self.raw_html(raw, range, style),
            }
        }
    }

    fn html_limit(&mut self) {
        const MESSAGE: &str = "HTML exceeding parsing limits is shown as source.";
        if !self.summary.warnings.iter().any(|s| s == MESSAGE) {
            self.summary.warnings.push(MESSAGE.into());
        }
    }
    fn raw_html(&mut self, raw: &str, range: Range<usize>, style: &InlineStyle) {
        let mut style = style.clone();
        style.dim = true;
        style.code = true;
        self.append(raw, range, &style);
    }
    fn html_start(
        &mut self,
        tag: StartTag<usize>,
        raw: &str,
        range: Range<usize>,
        style: &InlineStyle,
    ) {
        let name = String::from_utf8_lossy(&tag.name);
        let next_bytes = self.html.retained_bytes()
            + size_of::<Frame>()
            + self
                .html
                .frames
                .last()
                .and_then(|f| f.style.link.as_ref())
                .map_or(0, |link| link.len())
            + tag
                .attributes
                .values()
                .map(|value| value.len())
                .sum::<usize>()
            + name.len();
        if !supported(&name)
            || self.html.frames.len() >= MAX_DEPTH
            || next_bytes > MAX_CONTEXT_BYTES
        {
            if supported(&name) {
                self.html_limit();
            }
            if !void(&name) {
                self.html.raw = Some(name.chars().take(256).collect());
                self.html.raw_depth = 1;
            }
            self.raw_html(raw, range, style);
            return;
        }
        match name.as_ref() {
            "br" => self.append("\n", range, style),
            "hr" => {
                self.start(BlockKind::Rule, range);
                self.flush();
            }
            "img" => {
                let mut attributes = self.html.picture.clone().unwrap_or_default();
                attributes.link = self.html.style(style).link.clone();
                attributes.width = dimension(attr(&tag, "width"));
                attributes.height = dimension(attr(&tag, "height"));
                attributes.align = attr(&tag, "align")
                    .as_deref()
                    .map(alignment)
                    .unwrap_or(self.html.alignment());
                self.start(
                    BlockKind::Image {
                        destination: attr(&tag, "src").unwrap_or_default(),
                        title: attr(&tag, "title").unwrap_or_default(),
                        attributes: Some(Box::new(attributes)),
                    },
                    range.clone(),
                );
                if let Some(alt) = attr(&tag, "alt") {
                    self.append(&alt, range, style);
                }
                self.flush();
            }
            "picture" => {
                self.html.picture = Some(ImageAttributes::default());
            }
            "source" => {
                let target = attr(&tag, "srcset").and_then(|set| {
                    let first = set.split_whitespace().next()?.trim_end_matches(',');
                    Some(
                        if first.starts_with("data:") {
                            first
                        } else {
                            first.split(',').next()?
                        }
                        .to_owned(),
                    )
                });
                let media = attr(&tag, "media")
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .replace(char::is_whitespace, "");
                if let Some(picture) = &mut self.html.picture {
                    match media.as_str() {
                        "(prefers-color-scheme:dark)" if picture.dark.is_none() => {
                            picture.dark = target
                        }
                        "(prefers-color-scheme:light)" if picture.light.is_none() => {
                            picture.light = target
                        }
                        "" => {
                            if picture.dark.is_none() {
                                picture.dark = target.clone();
                            }
                            if picture.light.is_none() {
                                picture.light = target;
                            }
                        }
                        _ => {}
                    }
                }
            }
            "details" => {
                let open = tag.attributes.contains_key(b"open".as_slice());
                self.start(BlockKind::Details { open }, range.clone());
                self.html.push(&name);
                self.html.frames.last_mut().unwrap().section =
                    Some(range.start.saturating_add_signed(self.html_offset));
            }
            "summary" => {
                // A details header is held until its summary (or first body block).
                if !matches!(
                    self.current.as_ref().map(|b| &b.kind),
                    Some(BlockKind::Details { .. })
                ) {
                    self.raw_html(raw, range, style);
                    return;
                }
                self.html.push(&name);
            }
            _ => {
                self.html.push(&name);
                let frame = self.html.frames.last_mut().unwrap();
                match name.as_ref() {
                    "b" | "strong" => frame.style.strong = true,
                    "i" | "em" => frame.style.emphasis = true,
                    "code" | "kbd" => frame.style.code = true,
                    "mark" => frame.style.mark = true,
                    "sub" => frame.style.script = -1,
                    "sup" => frame.style.script = 1,
                    "a" => frame.style.link = attr(&tag, "href").map(Arc::from),
                    "p" | "div" => {
                        if let Some(align) = attr(&tag, "align") {
                            frame.align = alignment(&align);
                        }
                        self.start(BlockKind::Paragraph, range);
                    }
                    _ => {}
                }
            }
        }
    }
}

fn supported(name: &str) -> bool {
    matches!(
        name,
        "details"
            | "summary"
            | "img"
            | "picture"
            | "source"
            | "br"
            | "sub"
            | "sup"
            | "kbd"
            | "mark"
            | "a"
            | "b"
            | "strong"
            | "i"
            | "em"
            | "code"
            | "p"
            | "div"
            | "hr"
    )
}
fn void(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}
fn attr(tag: &StartTag<usize>, name: &str) -> Option<String> {
    let value = &tag.attributes.get(name.as_bytes())?.value;
    (value.len() <= MAX_ATTRIBUTE).then(|| String::from_utf8_lossy(value).into_owned())
}
fn alignment(value: &str) -> Alignment {
    match value.trim().to_ascii_lowercase().as_str() {
        "center" => Alignment::Center,
        "right" => Alignment::Right,
        _ => Alignment::Left,
    }
}
fn dimension(value: Option<String>) -> Option<u16> {
    value?
        .trim()
        .parse::<u16>()
        .ok()
        .filter(|&n| n > 0 && n <= 16_384)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlockEntry, BlockType, DocumentIndex};
    use plastext_buffer::FileSource;

    #[test]
    fn inline_tags_restore_nested_styles_and_preserve_entities() {
        let source = "A <b>bold <b>nested</b> still</b> plain <i>x</i> H<sub>2</sub>O <sup>é</sup> <kbd>⌘C</kbd> <mark>marked &amp; safe</mark> <a href='other.md#h' onclick='run()'>link</a>.";
        let doc = parse(source);
        assert_eq!(doc.blocks.len(), 1);
        let b = &doc.blocks[0];
        assert_eq!(
            b.text,
            "A bold nested still plain x H2O é ⌘C marked & safe link."
        );
        for (value, strong, script, mark) in [
            ("nested", true, 0, false),
            (" still", true, 0, false),
            (" plain ", false, 0, false),
            ("2", false, -1, false),
            ("é", false, 1, false),
            ("marked ", false, 0, true),
        ] {
            let span = b
                .spans
                .iter()
                .find(|s| b.text[s.range.clone()].contains(value))
                .unwrap();
            assert_eq!(
                (span.style.strong, span.style.script, span.style.mark),
                (strong, script, mark),
                "{value}"
            );
        }
        let link = b.spans.iter().find(|s| s.style.link.is_some()).unwrap();
        assert_eq!(link.style.link.as_deref(), Some("other.md#h"));
        assert_eq!(&source[link.source.clone()], "link");
    }

    #[test]
    fn block_html_decodes_attributes_and_chooses_picture_variants() {
        let doc = parse(
            "<div align='center'>\n<p>A &amp; <strong>B</strong><br>next</p>\n<picture>\n<source media='(prefers-color-scheme: dark)' srcset='dark.svg'>\n<source media='(prefers-color-scheme: light)' srcset='light.svg 1x, big.svg 2x'>\n<img SRC='fallback.svg' alt='A &amp; B' width='320' height='180' align='right' onerror='fetch(1)'>\n</picture>\n<hr>\n<!-- hidden -->\n</div>\n\nEnd",
        );
        assert_eq!(doc.blocks[0].text, "A & B\nnext");
        assert_eq!(doc.blocks[0].alignment, Alignment::Center);
        let image = doc
            .blocks
            .iter()
            .find(|b| matches!(b.kind, BlockKind::Image { .. }))
            .unwrap();
        let BlockKind::Image {
            destination,
            attributes: Some(attributes),
            ..
        } = &image.kind
        else {
            panic!("missing image")
        };
        assert_eq!(image.text, "A & B");
        assert_eq!(
            (attributes.width, attributes.height, attributes.align),
            (Some(320), Some(180), Alignment::Right)
        );
        assert_eq!(attributes.destination(destination, true), "dark.svg");
        assert_eq!(attributes.destination(destination, false), "light.svg");
        assert!(doc.blocks.iter().any(|b| b.kind == BlockKind::Rule));
        assert_eq!(doc.blocks.last().unwrap().alignment, Alignment::Left);
        assert!(!doc.blocks.iter().any(|b| b.text.contains("hidden")));
    }

    #[test]
    fn nested_details_keep_headers_and_visibility_across_pages_and_reverse_visits() {
        let source = format!(
            "<details>\n<summary>Outer <b>title</b></summary>\n\n{}\n<details open>\n<summary>Inner</summary>\n\nsecret\n\n</details>\n\nouter end\n\n</details>\n\nvisible\n",
            "body **text**\n\n".repeat(12000)
        );
        let source = FileSource::from_bytes(source.into_bytes());
        let index = DocumentIndex::build(&source, || false, |_, _| 0).unwrap();
        assert!(index.pages.len() > 2);
        assert_eq!(size_of::<BlockEntry>(), 32);
        let headers: Vec<_> = index
            .blocks
            .iter()
            .enumerate()
            .filter(|(_, b)| b.kind == BlockType::Details)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(headers.len(), 2);
        let toggles = HashSet::new();
        let visible: Vec<_> = (0..index.blocks.len())
            .filter(|&i| BlockEntry::visible(&index.blocks, i, &toggles))
            .collect();
        assert_eq!(visible, [0, index.blocks.len() - 1]);
        let mut toggles = HashSet::from([0]);
        assert!((0..index.blocks.len()).all(|i| BlockEntry::visible(&index.blocks, i, &toggles)));
        let inner = index.blocks[headers[1]].source.start;
        toggles.insert(inner);
        assert!(!BlockEntry::visible(
            &index.blocks,
            headers[1] + 1,
            &toggles
        ));
        assert!(BlockEntry::visible(
            &index.blocks,
            index.blocks.len() - 2,
            &toggles
        ));
        for p in (0..index.pages.len()).rev() {
            let page = index.parse_page(&source, p, || false).unwrap();
            for (local, block) in page.iter().enumerate() {
                let ix = index.pages[p].blocks.start + local;
                assert_eq!(
                    block.details,
                    index.blocks[ix]
                        .section()
                        .map(|parent| index.blocks[parent].source.start)
                );
            }
        }
    }

    #[test]
    fn copied_text_omits_collapsed_sections_and_find_reaches_their_source() {
        let text = "<details>\r\n<summary>Café &amp; title</summary>\r\n\r\nH<sub>2</sub>O hidden **needle**\r\n\r\n</details>\r\n\r\nvisible";
        let source = FileSource::from_bytes(text.as_bytes().to_vec());
        let index = DocumentIndex::build(&source, || false, |_, _| 0).unwrap();
        let copy = |toggled| {
            crate::selection::copy_selection(
                &index,
                &source,
                &crate::selection::Selection::All,
                &toggled,
                || false,
                10000,
            )
            .unwrap()
        };
        assert_eq!(copy(HashSet::new()), "Café & title\n\nvisible");
        assert_eq!(
            copy(HashSet::from([0])),
            "Café & title\n\nH2O hidden needle\n\nvisible"
        );
        let mut hits = Vec::new();
        index
            .search(
                &source,
                "needle",
                10,
                || false,
                |found| {
                    hits.extend(found);
                    true
                },
            )
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(
            BlockEntry::ancestors(&index.blocks, hits[0].block).collect::<Vec<_>>(),
            [0]
        );
    }

    #[test]
    fn active_content_unknown_tags_and_malformed_attributes_stay_inert() {
        let text = "<script src='https://example.invalid/x'><img src='local.png'></script>\n<style>@import 'https://example.invalid/a';</style>\n<iframe src='http://localhost:1'></iframe>\n<form><img src='secret.png'></form>\n<meta http-equiv='refresh' content='0;url=https://example.invalid'>\n<custom>unknown</custom>\n\n[normal](javascript:alert(1)) <a href='java&#x09;script:alert(1)'>bad</a>\n\n<img src='//example.invalid/a' width='9999999999' height='-1'/>";
        let doc = parse(text);
        let images: Vec<_> = doc
            .blocks
            .iter()
            .filter(|b| matches!(b.kind, BlockKind::Image { .. }))
            .collect();
        assert_eq!(
            images.len(),
            1,
            "images inside unsupported containers must stay source"
        );
        let BlockKind::Image {
            destination,
            attributes: Some(a),
            ..
        } = &images[0].kind
        else {
            unreachable!()
        };
        assert_eq!((a.width, a.height), (None, None));
        assert!(matches!(
            crate::resolve_link(std::path::Path::new("doc.md"), destination),
            crate::LinkTarget::External(_)
        ));
        for b in &doc.blocks {
            for span in &b.spans {
                if let Some(link) = &span.style.link {
                    assert_eq!(
                        crate::resolve_link(std::path::Path::new("doc.md"), link),
                        crate::LinkTarget::Blocked
                    );
                }
            }
        }
        assert!(
            doc.blocks
                .iter()
                .flat_map(|b| &b.spans)
                .any(|s| s.style.dim)
        );
    }

    #[test]
    fn deep_and_large_html_remains_bounded_and_cancellable() {
        let source = format!("{}text{}", "<b>".repeat(10000), "</b>".repeat(10000));
        let parsed = parse(&source);
        assert!(
            parsed
                .blocks
                .iter()
                .all(|b| b.text.len() <= BLOCK_TEXT_LIMIT)
        );
        assert!(!parsed.warnings.is_empty());
        let big = format!("<div>{}</div>", "x".repeat(MAX_HTML_BYTES + 1));
        let parsed = parse(&big);
        assert!(!parsed.warnings.is_empty());
        let mut calls = 0;
        let result = parse_stream(
            &source,
            || true,
            |_| {
                calls += 1;
                true
            },
        );
        assert!(result.cancelled);
        assert_eq!(calls, 0);
    }
}
