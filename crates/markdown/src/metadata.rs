//! Bounded front-matter parsing. YAML aliases are displayed, never expanded;
//! tags are inert. Rows carry source ranges for find and Source navigation.
use crate::{BLOCK_TEXT_LIMIT, Block, BlockKind, InlineStyle, Span};
use std::ops::Range;

const INPUT_LIMIT: usize = 64 * 1024;
const ROW_LIMIT: usize = 512;
const DEPTH_LIMIT: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetadataFormat {
    Yaml,
    Toml,
}
impl MetadataFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Yaml => "YAML",
            Self::Toml => "TOML",
        }
    }
}

struct Cell {
    text: String,
    source: Range<usize>,
}
struct Entry {
    key: Cell,
    value: Cell,
}

/// `body` excludes the opening/closing delimiter lines.
pub(super) fn rows(
    body: &str,
    offset: usize,
    section: usize,
    format: MetadataFormat,
) -> Result<Vec<Block>, String> {
    if body.len() > INPUT_LIMIT {
        return Err("Front matter exceeds the 64 KiB table limit.".into());
    }
    let entries = match format {
        MetadataFormat::Yaml => yaml(body)?,
        MetadataFormat::Toml => toml(body)?,
    };
    let mut blocks = Vec::new();
    for entry in entries {
        if entry.key.text.len() > 2048 {
            return Err("Front-matter key exceeds the 2 KiB table limit.".into());
        }
        let mut consumed = 0;
        loop {
            let key = if consumed == 0 { &entry.key.text } else { "" };
            let remaining = &entry.value.text[consumed..];
            let mut len = remaining.len().min(BLOCK_TEXT_LIMIT - key.len() - 1);
            while !remaining.is_char_boundary(len) {
                len -= 1;
            }
            // Multiline metadata is bounded by height as well as byte count.
            if let Some((end, _)) = remaining[..len].match_indices('\n').nth(31) {
                len = end + 1;
            }
            let value = &remaining[..len];
            let key_source = offset + entry.key.source.start..offset + entry.key.source.end;
            let value_source = if entry.value.source.len() == entry.value.text.len() {
                offset + entry.value.source.start + consumed
                    ..offset + entry.value.source.start + consumed + len
            } else {
                offset + entry.value.source.start..offset + entry.value.source.end
            };
            let mut block = Block::new(
                BlockKind::FrontMatterRow {
                    section,
                    key: 0..key.len(),
                    value: key.len() + 1..key.len() + 1 + value.len(),
                },
                if consumed == 0 {
                    key_source.start.min(value_source.start)..value_source.end.max(key_source.end)
                } else {
                    value_source.clone()
                },
            );
            block.text = format!("{key}\t{value}");
            block.spans = vec![
                Span {
                    range: 0..key.len(),
                    source: key_source,
                    style: InlineStyle {
                        strong: true,
                        ..Default::default()
                    },
                },
                Span {
                    range: key.len() + 1..block.text.len(),
                    source: value_source,
                    style: InlineStyle::default(),
                },
            ];
            blocks.push(block);
            if blocks.len() > ROW_LIMIT {
                return Err("Front matter exceeds the 512-row table limit.".into());
            }
            consumed += len;
            if consumed == entry.value.text.len() {
                break;
            }
        }
    }
    Ok(blocks)
}

fn raw(body: &str, source: Range<usize>) -> Cell {
    Cell {
        text: body[source.clone()].to_owned(),
        source,
    }
}

fn yaml(body: &str) -> Result<Vec<Entry>, String> {
    use saphyr_parser::{Event, Parser};
    let mut entries = Vec::new();
    let mut depth = 0;
    let mut documents = 0;
    let mut root_mapping = false;
    let mut key: Option<Cell> = None;
    let mut collection_start = 0;
    // The iterator input uses scalar offsets. Avoid the str input's mixed
    // byte/scalar offsets after non-ASCII quoted text in saphyr-parser 0.2.2.
    let offsets: Vec<_> = body
        .char_indices()
        .map(|(at, _)| at)
        .chain([body.len()])
        .collect();
    for (count, result) in Parser::new_from_iter(body.chars()).enumerate() {
        if count >= 8192 {
            return Err("Front matter exceeds the 8,192-event YAML limit.".into());
        }
        let (event, span) = result.map_err(|e| e.to_string())?;
        let start = offsets
            .get(span.start.index())
            .copied()
            .ok_or("Invalid YAML source offset")?;
        let end = offsets
            .get(span.end.index())
            .copied()
            .ok_or("Invalid YAML source offset")?;
        let source = start..end;
        match event {
            Event::DocumentStart(_) => {
                documents += 1;
                if documents > 1 {
                    return Err("Front matter must contain one YAML document.".into());
                }
            }
            Event::MappingStart(..) | Event::SequenceStart(..) => {
                if depth == 0 {
                    root_mapping = matches!(event, Event::MappingStart(..));
                    if !root_mapping {
                        return Err("Front matter must be a key/value mapping.".into());
                    }
                } else if depth == 1 {
                    if key.is_none() {
                        return Err("Complex YAML keys are shown as source.".into());
                    }
                    collection_start = source.start;
                }
                depth += 1;
                if depth > DEPTH_LIMIT {
                    return Err("Front matter exceeds the 64-level nesting limit.".into());
                }
            }
            Event::MappingEnd | Event::SequenceEnd => {
                depth -= 1;
                if depth == 1 {
                    entries.push(Entry {
                        key: key.take().ok_or("Missing YAML key")?,
                        value: raw(body, collection_start..source.end),
                    });
                }
            }
            Event::Scalar(text, _, _, tag) => {
                if depth == 0 && !text.is_empty() {
                    return Err("Front matter must be a key/value mapping.".into());
                }
                if depth == 1 {
                    let cell = if tag.is_some() {
                        raw(body, source)
                    } else {
                        Cell {
                            text: text.into_owned(),
                            source,
                        }
                    };
                    if let Some(key) = key.take() {
                        entries.push(Entry { key, value: cell });
                    } else {
                        key = Some(cell);
                    }
                }
            }
            Event::Alias(_) if depth == 1 => {
                let Some(key) = key.take() else {
                    return Err("YAML alias keys are shown as source.".into());
                };
                entries.push(Entry {
                    key,
                    value: raw(body, source),
                });
            }
            Event::Alias(_) if depth == 0 => {
                return Err("Front matter must be a key/value mapping.".into());
            }
            _ => {}
        }
        if entries.len() > ROW_LIMIT {
            return Err("Front matter exceeds the 512-row table limit.".into());
        }
    }
    if !root_mapping && !body.trim().is_empty() && entries.is_empty() {
        // Comments/empty YAML are a valid empty metadata table.
        if !body
            .lines()
            .all(|line| line.trim().is_empty() || line.trim_start().starts_with('#'))
        {
            return Err("Front matter must be a key/value mapping.".into());
        }
    }
    Ok(entries)
}

fn toml(body: &str) -> Result<Vec<Entry>, String> {
    check_toml_depth(body)?;
    let document =
        toml_edit::Document::parse(body).map_err(|e| format!("Invalid TOML: {}", e.message()))?;
    let mut entries = Vec::new();
    toml_table(body, document.as_table(), "", 0, &mut entries)?;
    // TOML's logical table order can differ from source order (dotted keys and
    // later table extensions). Navigation requires monotonically ordered rows.
    entries.sort_by_key(|entry| entry.key.source.start);
    Ok(entries)
}

/// The TOML tree parser bounds array/inline-table recursion, but dotted keys
/// can also create deeply nested tables. Check the flat event stream before
/// allocating that tree (including before its recursive drop).
fn check_toml_depth(body: &str) -> Result<(), String> {
    use toml_parser::parser::{Event, EventKind, RecursionGuard, parse_document};
    let source = toml_parser::Source::new(body);
    let tokens = source.lex().into_vec();
    let mut keys = 0;
    let mut table = 0;
    let mut pending = 0;
    let mut stack = Vec::new();
    let mut over_limit = false;
    let mut receiver = |event: Event| {
        match event.kind() {
            EventKind::StdTableOpen | EventKind::ArrayTableOpen => {
                keys = 0;
                table = 0;
            }
            EventKind::SimpleKey => {
                keys += 1;
                over_limit |= stack.last().copied().unwrap_or(table) + keys > DEPTH_LIMIT;
            }
            EventKind::StdTableClose | EventKind::ArrayTableClose => {
                // Each named table might be an array-of-tables ancestor.
                table = keys * 2;
                over_limit |= table > DEPTH_LIMIT;
                keys = 0;
            }
            EventKind::KeyValSep => {
                pending = keys;
                keys = 0;
            }
            EventKind::InlineTableOpen | EventKind::ArrayOpen => {
                let depth = stack.last().copied().unwrap_or(table) + pending + 1;
                stack.push(depth);
                over_limit |= depth > DEPTH_LIMIT;
                pending = 0;
            }
            EventKind::InlineTableClose | EventKind::ArrayClose => {
                stack.pop();
                pending = 0;
            }
            EventKind::Scalar | EventKind::ValueSep => pending = 0,
            _ => {}
        }
    };
    let mut errors: Option<toml_parser::ParseError> = None;
    parse_document(
        &tokens,
        &mut RecursionGuard::new(&mut receiver, DEPTH_LIMIT as u32),
        &mut errors,
    );
    if over_limit || errors.is_some() {
        return Err("Invalid TOML or front matter exceeds the 64-level nesting limit.".into());
    }
    Ok(())
}

fn toml_table(
    body: &str,
    table: &toml_edit::Table,
    prefix: &str,
    depth: usize,
    entries: &mut Vec<Entry>,
) -> Result<(), String> {
    if depth > DEPTH_LIMIT {
        return Err("Front matter exceeds the 64-level nesting limit.".into());
    }
    for (name, item) in table.iter() {
        let key_source = table.key(name).and_then(|key| key.span()).unwrap_or(0..0);
        let key_text = &body[key_source.clone()];
        let key = if prefix.is_empty() {
            key_text.to_owned()
        } else {
            format!("{prefix}.{key_text}")
        };
        if key.len() > 2048 {
            return Err("Front-matter key exceeds the 2 KiB table limit.".into());
        }
        match item {
            toml_edit::Item::Table(table) if !table.is_empty() => {
                toml_table(body, table, &key, depth + 1, entries)?;
            }
            toml_edit::Item::ArrayOfTables(tables) => {
                for (index, table) in tables.iter().enumerate() {
                    let key = format!("{key}[{index}]");
                    if table.is_empty() {
                        let source = table.span().unwrap_or(key_source.clone());
                        entries.push(Entry {
                            key: Cell {
                                text: key,
                                source: source.clone(),
                            },
                            value: Cell {
                                text: "{}".into(),
                                source,
                            },
                        });
                    } else {
                        toml_table(body, table, &key, depth + 1, entries)?;
                    }
                }
            }
            _ => {
                let source = item.span().unwrap_or(key_source.clone());
                let value = match item.as_value() {
                    Some(toml_edit::Value::String(text)) => Cell {
                        text: text.value().clone(),
                        source,
                    },
                    _ if item.is_table() => Cell {
                        text: "{}".into(),
                        source,
                    },
                    _ => raw(body, source),
                };
                entries.push(Entry {
                    key: Cell {
                        text: key,
                        source: key_source,
                    },
                    value,
                });
                if entries.len() > ROW_LIMIT {
                    return Err("Front matter exceeds the 512-row table limit.".into());
                }
            }
        }
        if entries.len() > ROW_LIMIT {
            return Err("Front matter exceeds the 512-row table limit.".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Document, find_rendered, parse};

    fn cells(doc: &Document) -> Vec<(&str, &str)> {
        doc.blocks
            .iter()
            .filter_map(|b| {
                if let BlockKind::FrontMatterRow { key, value, .. } = &b.kind {
                    Some((&b.text[key.clone()], &b.text[value.clone()]))
                } else {
                    None
                }
            })
            .collect()
    }

    fn check_sources(doc: &Document, source: &str) {
        assert!(
            doc.blocks
                .windows(2)
                .all(|pair| pair[0].source.start <= pair[1].source.start)
        );
        for block in &doc.blocks {
            assert!(block.text.len() <= BLOCK_TEXT_LIMIT);
            assert!(
                source.get(block.source.clone()).is_some(),
                "{:?}",
                block.source
            );
            for span in &block.spans {
                assert!(
                    source.get(span.source.clone()).is_some(),
                    "{:?}",
                    span.source
                );
                assert!(block.text.get(span.range.clone()).is_some());
            }
        }
    }

    #[test]
    fn yaml_decodes_scalars_preserves_collections_and_maps_unicode_crlf() {
        let source = "---\r\ntitle: \"café 🚀\"\r\ndescription: >-\r\n  first line\r\n  second line\r\ntags: [rust, native]\r\nauthor:\r\n  name: Ada\r\n  active: true\r\n---\r\n# Body\r\n";
        let doc = parse(source);
        assert!(matches!(
            doc.blocks[0].kind,
            BlockKind::FrontMatter { error: None, .. }
        ));
        let cells = cells(&doc);
        assert_eq!(cells[0], ("title", "café 🚀"));
        assert_eq!(cells[1], ("description", "first line second line"));
        assert_eq!(cells[2], ("tags", "[rust, native]"));
        assert_eq!(cells[3].0, "author");
        assert!(cells[3].1.contains("name: Ada"));
        assert!(cells[3].1.contains("active: true"));
        let hit = &find_rendered(&doc.blocks, 0, "café", 10)[0];
        assert_eq!(hit.source, source.find('"').unwrap());
        check_sources(&doc, source);
        assert_eq!(doc.headings[0].text, "Body");
    }

    #[test]
    fn yaml_aliases_and_tags_are_inert_without_expansion() {
        let source = "---\na: &a [one, two]\nb: &b [*a, *a]\nc: *b\nd: !custom tagged\n---\n";
        let doc = parse(source);
        assert!(matches!(
            doc.blocks[0].kind,
            BlockKind::FrontMatter { error: None, .. }
        ));
        let cells = cells(&doc);
        assert!(cells[1].1.contains("*a"));
        assert_eq!(cells[2], ("c", "*b"));
        assert!(cells[3].1.contains("tagged"));
        assert!(cells.iter().map(|(_, value)| value.len()).sum::<usize>() < source.len());
        check_sources(&doc, source);
    }

    #[test]
    fn empty_values_collections_and_quoted_keys_remain_visible() {
        let source =
            "+++\n\"a.b\" = \"unicode: 🦀\"\n[empty]\n[[empty_array]]\n[[empty_array]]\n+++\n";
        let doc = parse(source);
        assert_eq!(
            cells(&doc),
            [
                ("\"a.b\"", "unicode: 🦀"),
                ("empty", "{}"),
                ("empty_array[0]", "{}"),
                ("empty_array[1]", "{}")
            ]
        );
        check_sources(&doc, source);
        let source = "---\nempty:\nmap: {}\nlist: []\n'🚀.clé': café\nnext: [one, 🦀]\n---\n";
        let doc = parse(source);
        assert_eq!(
            cells(&doc),
            [
                ("empty", ""),
                ("map", "{}"),
                ("list", "[]"),
                ("🚀.clé", "café"),
                ("next", "[one, 🦀]")
            ]
        );
        check_sources(&doc, source);
    }

    #[test]
    fn toml_tables_dotted_keys_and_array_tables_follow_source_order() {
        let source = "+++\ntitle = \"café\"\na.b = 1\nother = true\na.c = 2\n[author]\nname = \"Ada\"\ntags = [\"rust\", \"native\"]\n[[links]]\nurl = \"first.md\"\n[[links]]\nurl = \"second.md\"\n+++\n# Body\n";
        let doc = parse(source);
        assert!(
            matches!(
                doc.blocks[0].kind,
                BlockKind::FrontMatter { error: None, .. }
            ),
            "{:?}",
            doc.blocks[0].kind
        );
        assert_eq!(
            cells(&doc),
            [
                ("title", "café"),
                ("a.b", "1"),
                ("other", "true"),
                ("a.c", "2"),
                ("author.name", "Ada"),
                ("author.tags", "[\"rust\", \"native\"]"),
                ("links[0].url", "first.md"),
                ("links[1].url", "second.md"),
            ]
        );
        let hit = &find_rendered(&doc.blocks, 0, "second.md", 10)[0];
        assert_eq!(hit.source, source.find("\"second.md\"").unwrap());
        check_sources(&doc, source);
    }

    #[test]
    fn invalid_non_mapping_and_over_limit_metadata_remains_complete_source() {
        for (delimiter, body) in [
            ("---", "bad: [unclosed\n".to_owned()),
            ("---", "- sequence\n- root\n".to_owned()),
            ("+++", "bad = [unclosed\n".to_owned()),
            ("---", format!("value: {}\n", "é".repeat(INPUT_LIMIT))),
            (
                "---",
                (0..600).map(|i| format!("key{i}: value\n")).collect(),
            ),
            ("+++", format!("{}last = 1\n", "x.".repeat(2000))),
            (
                "+++",
                format!("value = {}1{}\n", "[".repeat(2000), "]".repeat(2000)),
            ),
            (
                "---",
                format!("value: {}1{}\n", "[".repeat(2000), "]".repeat(2000)),
            ),
        ] {
            let source = format!("{delimiter}\n{body}{delimiter}\n\n# After\n");
            let doc = parse(&source);
            assert!(
                matches!(
                    doc.blocks[0].kind,
                    BlockKind::FrontMatter { error: Some(_), .. }
                ),
                "{:?}",
                doc.blocks[0].kind
            );
            let rendered: String = doc
                .blocks
                .iter()
                .filter(|b| matches!(b.kind, BlockKind::FrontMatterSource { .. }))
                .map(|b| b.text.as_str())
                .collect();
            assert_eq!(rendered, body);
            assert_eq!(doc.headings[0].text, "After");
            check_sources(&doc, &source);
        }
    }

    #[test]
    fn long_values_are_split_without_dropping_bytes_or_losing_section() {
        let value = "é".repeat(20_000);
        let source = format!("---\nkey: {value}\n---\n");
        let doc = parse(&source);
        let cells = cells(&doc);
        assert_eq!(cells[0].0, "key");
        assert!(cells.iter().skip(1).all(|(key, _)| key.is_empty()));
        assert_eq!(
            cells.iter().map(|(_, value)| *value).collect::<String>(),
            value
        );
        assert!(
            doc.blocks
                .iter()
                .skip(1)
                .all(|b| b.kind.metadata_section() == Some(0))
        );
        check_sources(&doc, &source);
    }
}
